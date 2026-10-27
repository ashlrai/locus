#!/usr/bin/env python3
"""Integration tests use real Git worktrees and a temporary HOME, never live jobs."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

import bridge_worktree_reaper as reaper


class ReaperTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name).resolve()
        self.home = self.base / "home"
        self.home.mkdir()
        self.environment = patch.dict(
            os.environ,
            {
                "HOME": str(self.home),
                "GIT_CONFIG_NOSYSTEM": "1",
                "GIT_CONFIG_GLOBAL": os.devnull,
            },
        )
        self.environment.start()
        self.addCleanup(self.environment.stop)
        self.repo = self.base / "repo"
        self.root = self.base / "worktrees"
        self.root.mkdir()
        self.repo.mkdir()
        self.git(self.repo, "init", "-q")
        self.git(self.repo, "config", "user.name", "Reaper Fixture")
        self.git(self.repo, "config", "user.email", "fixture@example.invalid")
        (self.repo / "tracked").write_text("original\n")
        (self.repo / ".gitignore").write_text("target/\n")
        self.git(self.repo, "add", ".")
        self.git(self.repo, "commit", "-qm", "fixture", "--no-verify")
        self.git(
            self.repo, "remote", "add", "origin", "https://github.com/ashlrai/locus.git"
        )
        self.job = "RQ-CLI-1"
        self.worktree = self.root / self.job
        self.git(self.repo, "worktree", "add", "-q", "--detach", str(self.worktree))
        self.sha = self.git(self.worktree, "rev-parse", "HEAD").strip()
        self.state = self.home / ".ashlr-runs"
        self.manifest = self.base / "published.json"
        self.entry = {
            "job": self.job,
            "pr": {
                "url": "https://github.com/ashlrai/locus/pull/183",
                "head_oid": self.sha,
                "observed_at": time.time(),
            },
        }

    def git(self, path, *args):
        result = subprocess.run(
            ["git", "-C", str(path), *args], capture_output=True, text=True
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout

    def run_reaper(self, entries=None):
        self.manifest.write_text(
            json.dumps([self.entry] if entries is None else entries)
        )
        return reaper.run_cycle(self.manifest, self.root, self.state)

    def assert_retained(self, reason):
        result = self.run_reaper()
        self.assertEqual(result["results"][0]["reason"], reason)
        self.assertTrue(self.worktree.exists())

    def test_quarantines_untracked_ignored_and_symlink_then_plain_remove_and_prune(
        self,
    ):
        (self.worktree / "notes with spaces.txt").write_text("preserve\n")
        (self.worktree / "target").mkdir()
        (self.worktree / "target" / "build.bin").write_bytes(b"artifact")
        (self.worktree / "outside-link").symlink_to(self.base / "absent")
        calls = []
        original_git = reaper.git

        def capture(path, *args):
            calls.append(args)
            return original_git(path, *args)

        with patch.object(reaper, "git", capture):
            record = self.run_reaper()
        self.assertFalse(self.worktree.exists())
        quarantine = self.state / "quarantine" / self.job
        self.assertEqual(
            (quarantine / "notes with spaces.txt").read_text(), "preserve\n"
        )
        self.assertEqual(
            (quarantine / "target" / "build.bin").read_bytes(), b"artifact"
        )
        self.assertTrue((quarantine / "outside-link").is_symlink())
        self.assertIn(("worktree", "remove", str(self.worktree)), calls)
        self.assertIn(("worktree", "prune"), calls)
        self.assertTrue(all("--force" not in call for call in calls))
        self.assertEqual(record["results"][0]["status"], "removed")
        self.assertGreater(record["before"]["used_bytes"], 0)
        self.assertIn("free_bytes", record["after"])

    def test_rejects_different_pr_head(self):
        self.entry["pr"]["head_oid"] = "0" * 40
        self.assert_retained("pr-head-mismatch")

    def test_prune_failure_does_not_report_removed_worktree_as_retained(self):
        original_git = reaper.git

        def fail_prune(path, *args):
            if args == ("worktree", "prune"):
                raise reaper.Refused("git-command-timeout")
            return original_git(path, *args)

        with patch.object(reaper, "git", fail_prune):
            record = self.run_reaper()
        self.assertFalse(self.worktree.exists())
        self.assertEqual(record["results"][0]["status"], "removed")
        self.assertEqual(record["results"][0]["prune_status"], "failed")

    def test_preserves_uncommitted_tracked_edits_before_quarantine(self):
        (self.worktree / "tracked").write_text("changed\n")
        (self.worktree / "notes").write_text("keep here")
        self.assert_retained("tracked-changes")
        self.assertTrue((self.worktree / "notes").exists())

    def test_preserves_staged_edits(self):
        (self.worktree / "tracked").write_text("changed\n")
        self.git(self.worktree, "add", "tracked")
        self.assert_retained("tracked-changes")

    def test_assume_unchanged_cannot_hide_tracked_edits(self):
        self.git(self.worktree, "update-index", "--assume-unchanged", "tracked")
        (self.worktree / "tracked").write_text("hidden work\n")
        self.assert_retained("hidden-index-entries")
        self.assertEqual((self.worktree / "tracked").read_text(), "hidden work\n")

    def test_skip_worktree_cannot_hide_tracked_edits(self):
        self.git(self.worktree, "update-index", "--skip-worktree", "tracked")
        (self.worktree / "tracked").write_text("hidden work\n")
        self.assert_retained("hidden-index-entries")

    def test_git_timeout_is_redacted(self):
        with patch.object(
            reaper.subprocess,
            "run",
            side_effect=subprocess.TimeoutExpired("sensitive command", 60),
        ):
            self.assert_retained("git-command-timeout")

    def test_no_pr_link_means_no_removal(self):
        self.entry.pop("pr")
        self.assert_retained("published-pr-required")

    def test_expired_metadata_means_no_removal(self):
        self.entry["pr"]["observed_at"] -= 301
        self.assert_retained("stale-pr-metadata")

    def test_repository_must_match_pr(self):
        self.entry["pr"]["url"] = "https://github.com/ashlrai/phantom/pull/183"
        self.assert_retained("pr-repository-mismatch")

    def test_locked_worktree_retained(self):
        self.git(self.repo, "worktree", "lock", str(self.worktree))
        self.assert_retained("main-or-locked-worktree")

    def test_quarantine_never_overwrites_previous_files(self):
        destination = self.state / "quarantine" / self.job
        destination.mkdir(parents=True)
        (destination / "notes").write_text("old")
        (self.worktree / "notes").write_text("new")
        self.assert_retained("quarantine-collision")
        self.assertEqual((destination / "notes").read_text(), "old")
        self.assertEqual((self.worktree / "notes").read_text(), "new")

    def test_explicitly_excluded_job_and_path_traversal_retained(self):
        record = self.run_reaper(
            [{"job": "RQ-TESTDIET-phantom-secrets"}, {"job": "../RQ-CLI-1"}]
        )
        self.assertEqual(
            [r["reason"] for r in record["results"]], ["job-not-allowlisted"] * 2
        )
        self.assertTrue(self.worktree.exists())

    def test_logs_idle_and_invalid_cycles(self):
        self.run_reaper([])
        self.manifest.write_text("invalid")
        reaper.run_cycle(self.manifest, self.root, self.state)
        records = [
            json.loads(line)
            for line in (self.state / "worktree-reaper.jsonl").read_text().splitlines()
        ]
        self.assertEqual(len(records), 2)
        self.assertEqual(records[0]["results"], [])
        self.assertEqual(
            records[1]["results"][0]["reason"], "manifest-unavailable-or-invalid"
        )

    def test_cycle_wrapper_reaps_on_failure_and_preserves_exit_status(self):
        self.manifest.write_text(json.dumps([self.entry]))
        result = subprocess.run(
            [
                sys.executable,
                str(Path(reaper.__file__)),
                "--manifest",
                str(self.manifest),
                "--worktree-root",
                str(self.root),
                "--state-dir",
                str(self.state),
                "--",
                sys.executable,
                "-c",
                "raise SystemExit(7)",
            ],
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 7, result.stderr)
        self.assertFalse(self.worktree.exists())
        self.assertEqual(
            len((self.state / "worktree-reaper.jsonl").read_text().splitlines()), 1
        )

    def test_symlink_quarantine_rejected_without_removing_worktree(self):
        self.state.mkdir()
        (self.state / "quarantine").symlink_to(self.repo, target_is_directory=True)
        (self.worktree / "notes").write_text("preserve")
        self.assert_retained("symlinked-state-directory")

    def test_malformed_metadata_fails_closed_and_logs(self):
        record = self.run_reaper(
            [{"job": []}, {"job": self.job, "pr": {"url": 123}}, None]
        )
        self.assertEqual(len(record["results"]), 3)
        self.assertTrue(
            all(result["status"] == "retained" for result in record["results"])
        )
        self.assertTrue(self.worktree.exists())

    def test_new_untracked_file_during_quarantine_prevents_removal(self):
        (self.worktree / "notes").write_text("preserve")
        original_rename = os.rename

        def concurrent_writer(source, destination):
            original_rename(source, destination)
            (self.worktree / "new-notes").write_text("concurrent writer")

        with patch.object(reaper.os, "rename", concurrent_writer):
            self.assert_retained("untracked-files-changed")
        self.assertEqual((self.worktree / "new-notes").read_text(), "concurrent writer")
        self.assertEqual(
            (self.state / "quarantine" / self.job / "notes").read_text(), "preserve"
        )

    def test_symlink_log_prevents_any_removal(self):
        self.state.mkdir()
        (self.state / "worktree-reaper.jsonl").symlink_to(self.base / "outside")
        with self.assertRaises(OSError):
            self.run_reaper()
        self.assertTrue(self.worktree.exists())
        self.assertFalse((self.base / "outside").exists())


if __name__ == "__main__":
    unittest.main()

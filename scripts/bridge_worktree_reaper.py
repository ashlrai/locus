#!/usr/bin/env python3
"""Reap explicitly published bridge worktrees without force or data loss.

The bridge supplies fresh PR metadata obtained through its scoped tool access.
This module never reads credentials or calls a hosting API. See docs/bridge-reaper.md.
"""

import argparse
import fcntl
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import time

TARGETS = frozenset(
    ("RQ-CLI-1", "RQ-DEADCODE-locus", "RQ-PLUG-SELFTEST", "RQ-PS-AGENT-CREATE")
)
MAX_METADATA_AGE_SECONDS = 300


class Refused(Exception):
    """A safety precondition failed; keep the worktree."""


def git(path, *args):
    result = subprocess.run(["git", "-C", str(path), *args], capture_output=True)
    if result.returncode:
        # Git stderr can include remote URLs or filenames; do not write it to logs.
        raise Refused("git-command-failed")
    return result.stdout


def private_dir(path):
    """Refuse symlinked quarantine/log ancestors instead of following them."""
    path = Path(os.path.abspath(path))
    for parent in reversed((path, *path.parents)):
        if parent.is_symlink():
            raise Refused("symlinked-state-directory")
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    return path


def disk_snapshot(path):
    usage = shutil.disk_usage(path)
    return {
        "total_bytes": usage.total,
        "used_bytes": usage.used,
        "free_bytes": usage.free,
    }


def validate_entry(entry, root, now):
    job = entry.get("job")
    if not isinstance(job, str) or job not in TARGETS:
        raise Refused("job-not-allowlisted")
    pr = entry.get("pr", {})
    if (
        not isinstance(pr, dict)
        or not isinstance(pr.get("url"), str)
        or not re.fullmatch(
            r"https://github\.com/ashlrai/[A-Za-z0-9_.-]+/pull/[1-9][0-9]*",
            pr.get("url", ""),
        )
    ):
        raise Refused("published-pr-required")
    sha = pr.get("head_oid", "")
    if not isinstance(sha, str) or not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise Refused("invalid-pr-head")
    observed = pr.get("observed_at", 0)
    if (
        not isinstance(observed, (int, float))
        or not 0 <= now - observed <= MAX_METADATA_AGE_SECONDS
    ):
        raise Refused("stale-pr-metadata")
    path = root / job
    if path.is_symlink() or path.resolve() != path:
        raise Refused("symlinked-worktree")
    if not path.is_dir():
        raise Refused("worktree-absent")
    top = Path(os.fsdecode(git(path, "rev-parse", "--show-toplevel")).strip()).resolve()
    if (
        top != path
        or path == Path.cwd().resolve()
        or path in Path.cwd().resolve().parents
    ):
        raise Refused("unsafe-worktree-path")
    common = Path(
        os.fsdecode(
            git(path, "rev-parse", "--path-format=absolute", "--git-common-dir")
        ).strip()
    )
    gitdir = Path(os.fsdecode(git(path, "rev-parse", "--absolute-git-dir")).strip())
    if gitdir == common or (gitdir / "locked").exists():
        raise Refused("main-or-locked-worktree")
    expected_repo = pr["url"].split("/pull/")[0].removeprefix("https://github.com/")
    remote = (
        os.fsdecode(git(path, "remote", "get-url", "origin"))
        .strip()
        .removesuffix(".git")
    )
    if remote not in (
        "https://github.com/" + expected_repo,
        "git@github.com:" + expected_repo,
    ):
        raise Refused("pr-repository-mismatch")
    verify_clean_head(path, sha)
    return job, path, common, sha


def verify_clean_head(path, sha):
    if git(path, "rev-parse", "HEAD").decode().strip() != sha:
        raise Refused("pr-head-mismatch")
    if git(path, "status", "--porcelain=v1", "-z", "--untracked-files=no"):
        raise Refused("tracked-changes")
    index = git(path, "ls-files", "--stage")
    if index.startswith(b"160000 ") or b"\n160000 " in index:
        raise Refused("submodule-worktree")


def reap(entry, root, state, now):
    job, path, common, sha = validate_entry(entry, root, now)
    # Include ignored build output: git removal can otherwise discard it silently.
    others = git(
        path, "ls-files", "--others", "--directory", "--no-empty-directory", "-z"
    )
    relative_paths = [
        Path(os.fsdecode(name).rstrip("/")) for name in others.split(b"\0") if name
    ]
    quarantine = private_dir(state / "quarantine" / job)
    moves = []
    for relative in relative_paths:
        if relative.is_absolute() or ".." in relative.parts or ".git" in relative.parts:
            raise Refused("unsafe-untracked-path")
        source, destination = path / relative, quarantine / relative
        if destination.exists() or destination.is_symlink():
            raise Refused("quarantine-collision")
        if source.parent.resolve() != source.parent:
            raise Refused("symlinked-untracked-parent")
        # Preflight all destinations before moving anything; never overwrite a prior run.
        private_dir(destination.parent)
        moves.append((source, destination))
    for source, destination in moves:
        # os.rename is atomic on the local volume and fails safely across filesystems.
        os.rename(source, destination)
    verify_clean_head(path, sha)
    if git(path, "ls-files", "--others", "--directory", "--no-empty-directory", "-z"):
        raise Refused("untracked-files-changed")
    # A concurrent writer after quarantine makes this fail closed. Never force removal.
    git(common, "worktree", "remove", str(path))
    git(common, "worktree", "prune")
    return {"job": job, "status": "removed", "quarantined_entries": len(moves)}


def run_cycle(manifest, root, state):
    """Call once in every bridge cycle, including idle and failed dispatch cycles."""
    root = Path(root).resolve(strict=True)
    state = private_dir(state)
    # Serialize bridge cycles to avoid quarantine/removal races between reapers.
    lock_fd = os.open(
        state / "reaper.lock", os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600
    )
    with os.fdopen(lock_fd, "a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        # Verify the audit destination is writable before any destructive action.
        log_fd = os.open(
            state / "worktree-reaper.jsonl",
            os.O_WRONLY | os.O_CREAT | os.O_APPEND | os.O_NOFOLLOW,
            0o600,
        )
        with os.fdopen(log_fd, "a") as log:
            before = disk_snapshot(root)
            results = []
            try:
                entries = json.loads(Path(manifest).read_text())
                if not isinstance(entries, list):
                    raise Refused("manifest-must-be-list")
                for entry in entries:
                    job = entry.get("job") if isinstance(entry, dict) else None
                    try:
                        if not isinstance(entry, dict):
                            raise Refused("invalid-entry")
                        results.append(reap(entry, root, state, time.time()))
                    except (Refused, OSError) as error:
                        results.append(
                            {
                                "job": job
                                if isinstance(job, str) and job in TARGETS
                                else "not-allowlisted",
                                "status": "retained",
                                "reason": str(error)
                                if isinstance(error, Refused)
                                else "filesystem-error",
                            }
                        )
            except (OSError, ValueError, Refused):
                results.append(
                    {"status": "retained", "reason": "manifest-unavailable-or-invalid"}
                )
            record = {
                "timestamp": time.time(),
                "before": before,
                "after": disk_snapshot(root),
                "results": results,
            }
            log.write(json.dumps(record, sort_keys=True) + "\n")
            log.flush()
            os.fsync(log.fileno())
            return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", required=True)
    parser.add_argument("--worktree-root", required=True)
    parser.add_argument("--state-dir", type=Path, default=Path.home() / ".ashlr-runs")
    parser.add_argument(
        "command",
        nargs=argparse.REMAINDER,
        help="Optional single bridge cycle command after --",
    )
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    code = 0
    try:
        if command:
            code = subprocess.run(command).returncode
    finally:
        record = run_cycle(args.manifest, args.worktree_root, args.state_dir)
        print(json.dumps(record, sort_keys=True))
    return code


if __name__ == "__main__":
    raise SystemExit(main())

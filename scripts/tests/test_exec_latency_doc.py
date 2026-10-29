"""Exercise the measurement code published in docs/exec-latency.md."""

import ast
import os
import pathlib
import shlex
import subprocess
import sys
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
DOC = ROOT / "docs" / "exec-latency.md"


def measurement_code():
    text = DOC.read_text()
    return text.split("python3 - \"$locus\" <<'PY'\n", 1)[1].split("\nPY\n", 1)[0]


def peer_measurement_code():
    text = DOC.read_text()
    marker = 'python3 - "$locus" "$work/direnv" "$work/encrypted.env" <<\'PY\'\n'
    return text.split(marker, 1)[1].split("\nPY\n", 1)[0]


class MeasurementInstructionsTest(unittest.TestCase):
    def run_measurement(self, exit_code):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            fake = root / "locus"
            fake.write_text(
                "#!/bin/sh\n"
                "[ \"$*\" = 'exec --no-resolve -- /usr/bin/true' ] || exit 2\n"
                "printf x >> calls\n"
                f"exit {exit_code}\n"
            )
            fake.chmod(0o755)
            result = subprocess.run(
                [sys.executable, "-", str(fake)],
                input=measurement_code(), text=True, capture_output=True, cwd=root,
            )
            return result, len((root / "calls").read_text())

    def test_warmup_is_dropped_and_all_runs_succeed(self):
        result, calls = self.run_measurement(0)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls, 21)
        self.assertIn("direct: n=20 ", result.stdout)
        self.assertIn("locus: n=20 ", result.stdout)
        self.assertIn("difference of medians:", result.stdout)

    def test_failed_command_never_reports_results(self):
        result, calls = self.run_measurement(7)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(calls, 1)
        self.assertIn("locus run 1 failed: exit 7; no result", result.stderr)
        self.assertEqual(result.stdout, "")


class PeerMeasurementInstructionsTest(unittest.TestCase):
    def test_changelog_attributes_the_report_and_links_the_comparison(self):
        changelog = (ROOT / "CHANGELOG.md").read_text(encoding="utf-8")
        note = changelog.split("**Execution latency report", 1)[1].split("\n\n", 1)[0]
        for expected in ("issues/170", "`locus exec -- true`", "3–5x",
                         "`direnv exec <dir> true`", "`sops exec-env <file> true`",
                         "issue's report, not a verified benchmark",
                         "docs/BENCHMARKS.md#core-operation-latency",
                         "docs/exec-latency.md#comparing-launchers-for-issue-170"):
            self.assertIn(expected, note)

    def test_readme_commands_match_the_timed_launchers(self):
        readme = (ROOT / "README.md").read_text()
        section = readme.split("### Comparing exec launch latency\n", 1)[1]
        block = section.split("```bash\n", 1)[1].split("\n```", 1)[0]
        substitutions = {
            "$locus": "LOCUS_BINARY",
            "$work/direnv": "DIRENV_DIR",
            "$work/encrypted.env": "ENCRYPTED_ENV",
        }
        documented = []
        for line in block.splitlines():
            for source, target in substitutions.items():
                line = line.replace(source, target)
            documented.append(shlex.split(line))

        tree = ast.parse(peer_measurement_code())
        assignment = next(node for node in tree.body if isinstance(node, ast.Assign)
                          and any(isinstance(target, ast.Name) and target.id == "commands"
                                  for target in node.targets))
        values = {"locus": "LOCUS_BINARY", "directory": "DIRENV_DIR",
                  "encrypted": "ENCRYPTED_ENV"}
        timed = [[values[item.id] if isinstance(item, ast.Name) else ast.literal_eval(item)
                  for item in command.elts] for command in assignment.value.values]
        self.assertEqual(documented, timed)

    def test_setup_allows_envrc_at_the_physical_path(self):
        text = DOC.read_text().split("## Comparing launchers for issue #170", 1)[1]
        self.assertIn('work="$(cd "$work" && pwd -P)"', text)
        self.assertIn('direnv allow "$work/direnv/.envrc"', text)

    def run_measurement(self, failing=None):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            for name in ("locus", "direnv", "sops"):
                fake = root / name
                fake.write_text(
                    "#!/bin/sh\n"
                    f"printf '%s:%s\\n' '{name}' \"$*\" >> calls\n"
                    f"exit {7 if name == failing else 0}\n"
                )
                fake.chmod(0o755)
            # Only PATH is extended; the fake commands write arguments, never env.
            env = os.environ.copy()
            env["PATH"] = str(root) + os.pathsep + env.get("PATH", "")
            result = subprocess.run(
                [sys.executable, "-", str(root / "locus"), "fixture-dir", "fixture.env"],
                input=peer_measurement_code(), text=True, capture_output=True,
                cwd=root, env=env,
            )
            calls = (root / "calls").read_text().splitlines()
            return result, calls

    def test_all_launchers_use_the_same_child_and_discard_warmups(self):
        result, calls = self.run_measurement()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(calls), 36 * 3)
        self.assertEqual(calls.count("locus:exec --no-resolve -- /usr/bin/true"), 36)
        self.assertEqual(calls.count("direnv:exec fixture-dir /usr/bin/true"), 36)
        self.assertEqual(calls.count("sops:exec-env fixture.env /usr/bin/true"), 36)
        for name in ("direct", "locus", "direnv", "sops"):
            self.assertIn(f"{name}: n=32 ", result.stdout)
        # The first four rounds have different starting positions.
        self.assertEqual([line.split(":", 1)[0] for line in calls[:12]], [
            "locus", "direnv", "sops", "locus", "direnv", "sops",
            "direnv", "sops", "locus", "sops", "locus", "direnv",
        ])

    def test_each_launcher_failure_suppresses_all_results(self):
        for name in ("locus", "direnv", "sops"):
            with self.subTest(name=name):
                result, _ = self.run_measurement(failing=name)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(f"{name} run 1 failed: exit 7; no results", result.stderr)
                self.assertEqual(result.stdout, "")


if __name__ == "__main__":
    unittest.main()

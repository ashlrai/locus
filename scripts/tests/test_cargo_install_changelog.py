"""Exercise the issue #168 changelog recipe without compiling the workspace."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]


def recipe():
    changelog = (ROOT / "CHANGELOG.md").read_text(encoding="utf-8")
    section = changelog.split("**Reproduce cargo-install timings", 1)[1]
    return section.split("```bash\n", 1)[1].split("\n  ```", 1)[0]


class CargoInstallRecipeTest(unittest.TestCase):
    def test_note_records_reported_observation_without_claiming_a_fix(self):
        changelog = (ROOT / "CHANGELOG.md").read_text(encoding="utf-8")
        unreleased = changelog.split("## [Unreleased]", 1)[1].split("## [0.5.2]", 1)[0]
        entry = unreleased.split("**Reproduce cargo-install timings", 1)[1]
        note = entry.split("```bash", 1)[0]
        self.assertIn("issues/168", note)
        self.assertIn("`cargo install --git`", note)
        self.assertIn("`locus-core` compiling twice", note)
        self.assertIn("`locus-cli` and `locus-mcp`", note)
        self.assertIn("during the `locus-mcp` install after `locus-cli` finished", note)
        self.assertIn("unresolved issue", note)
        self.assertIn("no fix is claimed", note)

    def run_recipe(self, fail_at=None):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fake = root / "cargo"
            fake.write_text(
                f"#!{sys.executable}\n"
                "import json, os, sys\n"
                "with open(os.environ['CALL_LOG'], 'a') as f:\n"
                "    f.write(json.dumps(sys.argv[1:]) + '\\n')\n"
                "if os.environ.get('FAIL_CALL') == str(sum(1 for _ in open(os.environ['CALL_LOG']))):\n"
                "    sys.exit(7)\n"
                "print(json.dumps({'reason': 'compiler-artifact'}))\n"
                "print('Compiling locus-core', file=sys.stderr)\n",
                encoding="utf-8",
            )
            fake.chmod(0o755)
            env = os.environ.copy()
            env.update(PATH=str(root) + os.pathsep + env.get("PATH", ""),
                       CALL_LOG=str(root / "calls"), TMPDIR=str(root))
            env.pop("FAIL_CALL", None)
            if fail_at:
                env["FAIL_CALL"] = str(fail_at)
            result = subprocess.run(["bash", "-c", recipe()], cwd=ROOT, env=env,
                                    text=True, capture_output=True)
            calls = [json.loads(line) for line in (root / "calls").read_text().splitlines()]
            return result, calls

    def test_compares_clean_independent_targets_and_shared_separate_target(self):
        result, calls = self.run_recipe()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(calls), 3)
        for args in calls:
            self.assertEqual(args[0], "install")
            self.assertIn("--locked", args)
            self.assertIn("--message-format=json", args)
        def value(args, option):
            return args[args.index(option) + 1]
        self.assertEqual(len({value(args, "--rev") for args in calls}), 1)
        self.assertEqual(len({value(args, "--git") for args in calls}), 1)
        self.assertNotEqual(value(calls[0], "--target-dir"), value(calls[1], "--target-dir"))
        self.assertEqual(value(calls[1], "--target-dir"), value(calls[2], "--target-dir"))
        self.assertNotEqual(value(calls[0], "--root"), value(calls[1], "--root"))
        self.assertEqual(value(calls[1], "--root"), value(calls[2], "--root"))
        self.assertIn("locus-cli", calls[0])
        self.assertIn("locus-mcp", calls[0])
        self.assertIn("locus-cli", calls[1])
        self.assertNotIn("locus-mcp", calls[1])
        self.assertIn("locus-mcp", calls[2])
        self.assertNotIn("locus-cli", calls[2])
        self.assertEqual(result.stdout.count("Compiling locus-core"), 3)
        self.assertEqual(result.stdout.count("real "), 3)

    def test_failure_stops_comparison(self):
        for fail_at in (1, 2):
            with self.subTest(fail_at=fail_at):
                result, calls = self.run_recipe(fail_at)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(len(calls), fail_at)
                self.assertEqual(result.stdout, "")


if __name__ == "__main__":
    unittest.main()

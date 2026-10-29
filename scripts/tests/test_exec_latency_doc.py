"""Exercise the measurement code published in docs/exec-latency.md."""

import pathlib
import subprocess
import sys
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
DOC = ROOT / "docs" / "exec-latency.md"


def measurement_code():
    text = DOC.read_text()
    return text.split("python3 - \"$locus\" <<'PY'\n", 1)[1].split("\nPY\n", 1)[0]


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


if __name__ == "__main__":
    unittest.main()

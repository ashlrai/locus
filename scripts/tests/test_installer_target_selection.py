"""Exercise archive selection independently of the host and network."""

import subprocess
import unittest
from pathlib import Path


INSTALLER = Path(__file__).resolve().parents[1] / "install.sh"


def select(os_name, architecture):
    return subprocess.run(
        ["bash", "-c", 'source "$1"; platform_target "$2" "$3"',
         "installer-test", str(INSTALLER), os_name, architecture],
        text=True, capture_output=True, check=False,
    )


class InstallerTargetSelectionTest(unittest.TestCase):
    def test_supported_targets(self):
        cases = {
            ("Darwin", "arm64"): "aarch64-apple-darwin",
            ("Darwin", "aarch64"): "aarch64-apple-darwin",
            ("Darwin", "x86_64"): "x86_64-apple-darwin",
            ("Darwin", "amd64"): "x86_64-apple-darwin",
            ("Linux", "x86_64"): "x86_64-unknown-linux-gnu",
            ("Linux", "amd64"): "x86_64-unknown-linux-gnu",
        }
        for (os_name, architecture), target in cases.items():
            with self.subTest(os=os_name, architecture=architecture):
                result = select(os_name, architecture)
                self.assertEqual((result.returncode, result.stdout, result.stderr),
                                 (0, target + "\n", ""))

    def test_unsupported_targets_fail_without_output(self):
        for os_name, architecture in [("Linux", "aarch64"), ("Linux", "arm64"),
                                      ("Windows", "x86_64"), ("Darwin", "i386"),
                                      ("", ""), ("Linux/x86_64", "")]:
            with self.subTest(os=os_name, architecture=architecture):
                result = select(os_name, architecture)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual((result.stdout, result.stderr), ("", ""))


if __name__ == "__main__":
    unittest.main()

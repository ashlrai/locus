"""Check that the installer journey stays reachable and executable."""

import re
import subprocess
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
DOC = ROOT / "docs/installer-user-journey.md"


class InstallerUserJourneyTest(unittest.TestCase):
    def test_links_and_index(self):
        text = DOC.read_text(encoding="utf-8")
        self.assertIn(DOC.name, (ROOT / "docs/README.md").read_text(encoding="utf-8"))
        self.assertIn("https://github.com/ashlrai/locus/issues/169", text)
        links = re.findall(r"\]\(([^)]+)\)", text)
        local = [link.split("#", 1)[0] for link in links if not link.startswith("https://")]
        self.assertTrue(local)
        for link in local:
            with self.subTest(link=link):
                self.assertTrue((DOC.parent / link).is_file())

    def test_documented_help_command_runs(self):
        text = DOC.read_text(encoding="utf-8")
        command = "bash scripts/install.sh --help"
        self.assertIn(command, text)
        result = subprocess.run(command.split(), cwd=ROOT, text=True, capture_output=True, check=True)
        self.assertIn("--version VERSION", result.stdout)
        self.assertIn("--bin-dir DIRECTORY", result.stdout)


if __name__ == "__main__":
    unittest.main()

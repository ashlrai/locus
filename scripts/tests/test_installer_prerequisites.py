"""Check prerequisite documentation against the installer and its index."""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DOC = ROOT / "docs/installer-prerequisites.md"


class InstallerPrerequisitesTest(unittest.TestCase):
    def test_checked_commands_are_documented(self):
        installer = (ROOT / "scripts/install.sh").read_text(encoding="utf-8")
        text = DOC.read_text(encoding="utf-8")
        commands = re.search(r"for tool in ([^;]+); do", installer)
        self.assertIsNotNone(commands)
        for command in commands.group(1).split():
            with self.subTest(command=command):
                self.assertIn(f"`{command}`", text)
        for command in ("sha256sum", "shasum"):
            self.assertIn(f"`{command}`", text)

    def test_document_is_discoverable_and_links_resolve(self):
        text = DOC.read_text(encoding="utf-8")
        index = (ROOT / "docs/README.md").read_text(encoding="utf-8")
        self.assertIn("](installer-prerequisites.md)", index)
        self.assertIn("https://github.com/ashlrai/locus/issues/169", text)
        links = re.findall(r"\]\(([^)]+)\)", text)
        local_links = [link for link in links if not re.match(r"\w+://", link)]
        self.assertTrue(local_links)
        for link in local_links:
            with self.subTest(link=link):
                self.assertTrue((DOC.parent / link.split("#", 1)[0]).is_file())


if __name__ == "__main__":
    unittest.main()

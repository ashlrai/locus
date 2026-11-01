"""Keep the archive acceptance contract discoverable and its references valid."""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DOC = ROOT / "docs/installer-archive-acceptance.md"


class InstallerArchiveAcceptanceTest(unittest.TestCase):
    def test_index_and_issue_link(self):
        index = (ROOT / "docs/README.md").read_text(encoding="utf-8")
        text = DOC.read_text(encoding="utf-8")
        self.assertIn("](installer-archive-acceptance.md)", index)
        self.assertIn("https://github.com/ashlrai/locus/issues/169", text)

    def test_local_references_resolve(self):
        text = DOC.read_text(encoding="utf-8")
        links = re.findall(r"\]\(([^)]+)\)", text)
        local = [link.split("#", 1)[0] for link in links if not re.match(r"\w+://", link)]
        self.assertTrue(local)
        for link in local:
            with self.subTest(link=link):
                self.assertTrue((DOC.parent / link).is_file(), link)


if __name__ == "__main__":
    unittest.main()

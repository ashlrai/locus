"""Check that the installer requirements are discoverable and traceable."""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DOC = ROOT / "docs/installer-requirements.md"


class InstallerRequirementsTest(unittest.TestCase):
    def test_index_and_issue_link(self):
        index = (ROOT / "docs/README.md").read_text(encoding="utf-8")
        text = DOC.read_text(encoding="utf-8")
        self.assertIn("](installer-requirements.md)", index)
        self.assertIn("https://github.com/ashlrai/locus/issues/169", text)
        self.assertLess(text.index("## Stated outcome and constraints"),
                        text.index("## Current implementation context"))
        self.assertLess(text.index("## Current implementation context"),
                        text.index("## Open questions"))

    def test_relative_links_resolve(self):
        text = DOC.read_text(encoding="utf-8")
        links = re.findall(r"\]\(([^)]+)\)", text)
        local_links = [link for link in links if not re.match(r"\w+://", link)]
        self.assertTrue(local_links)
        for link in local_links:
            with self.subTest(link=link):
                self.assertTrue((DOC.parent / link.split("#", 1)[0]).is_file())

    def test_issue_scope_and_release_evidence_are_distinct(self):
        text = DOC.read_text(encoding="utf-8")
        outcome = text.split("## Stated outcome and constraints", 1)[1].split("##", 1)[0]
        evidence = text.split("## Evidence needed for the proposed path", 1)[1].split("##", 1)[0]
        self.assertRegex(outcome, r"one-command shell installation path")
        self.assertRegex(outcome, r"release archive")
        self.assertRegex(outcome, r"neither npm nor Rust")
        self.assertIn("not claims", evidence)
        self.assertIn("supported native hosts", evidence)
        self.assertIn("missing or invalid verification material", evidence)


if __name__ == "__main__":
    unittest.main()

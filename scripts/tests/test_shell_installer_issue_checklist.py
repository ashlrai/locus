"""Keep the issue checklist's proposal and evidence boundaries explicit."""

import re
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
CHECKLIST = ROOT / "docs/shell-installer-issue-checklist.md"


class ShellInstallerIssueChecklistTest(unittest.TestCase):
    def test_goal_and_evidence_boundaries(self):
        text = CHECKLIST.read_text(encoding="utf-8")
        self.assertIn("https://github.com/ashlrai/locus/issues/169", text)
        self.assertRegex(text, r"one-command shell installation\s+that downloads and verifies a release archive")
        self.assertRegex(text, r"without npm, Node\.js,\s+Cargo, or Rust")
        self.assertIn("not a claim that a published installation command", text)
        self.assertRegex(text, r"mocked downloads and platform detection do not prove native compatibility")
        self.assertNotRegex(text, r"(?m)^- \[x\]")

    def test_local_links_resolve_and_index_points_here(self):
        text = CHECKLIST.read_text(encoding="utf-8")
        links = re.findall(r"\]\(([^)]+)\)", text)
        local = [link.split("#", 1)[0] for link in links if not link.startswith("https://")]
        self.assertTrue(local)
        for link in local:
            with self.subTest(link=link):
                self.assertTrue((CHECKLIST.parent / link).is_file())
        self.assertIn(CHECKLIST.name, (ROOT / "docs/README.md").read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()

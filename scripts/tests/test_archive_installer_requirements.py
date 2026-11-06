"""Keep the archive installer requirements traceable and its decisions explicit."""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DOC = ROOT / "docs/archive-installer-requirements.md"


class ArchiveInstallerRequirementsTest(unittest.TestCase):
    def test_issue_index_and_section_order(self):
        text = DOC.read_text(encoding="utf-8")
        index = (ROOT / "docs/README.md").read_text(encoding="utf-8")
        self.assertIn("https://github.com/ashlrai/locus/issues/169", text)
        self.assertIn("](archive-installer-requirements.md)", index)
        self.assertLess(text.index("## Goal from the issue"),
                        text.index("## Prerequisite exclusions"))
        self.assertLess(text.index("## Prerequisite exclusions"),
                        text.index("## Unresolved decisions"))

    def test_goal_and_excluded_prerequisites_are_stated(self):
        text = DOC.read_text(encoding="utf-8")
        self.assertRegex(text, r"one-command archive installation")
        self.assertIn("direnv", text)
        self.assertIn("neither npm nor Rust", text)
        self.assertRegex(text, r"npm and Node\.js cannot be prerequisites")
        self.assertRegex(text, r"Rust toolchain cannot be a prerequisite")

    def test_unresolved_platform_and_location_decisions(self):
        text = DOC.read_text(encoding="utf-8")
        decisions = re.search(
            r"(?ms)^## Unresolved decisions\n(.*?)(?=^## |\Z)", text
        )
        self.assertIsNotNone(decisions)
        body = decisions.group(1)
        for heading in ("### Platform coverage", "### Installation location"):
            with self.subTest(heading=heading):
                self.assertIn(heading, body)
        questions = re.findall(r"(?m)^- ", body)
        self.assertGreaterEqual(len(questions), 6)

    def test_local_links_resolve(self):
        text = DOC.read_text(encoding="utf-8")
        links = re.findall(r"\]\(([^)]+)\)", text)
        local = [link.split("#", 1)[0] for link in links if not re.match(r"\w+://", link)]
        self.assertTrue(local)
        for link in local:
            with self.subTest(link=link):
                self.assertTrue((DOC.parent / link).is_file())


if __name__ == "__main__":
    unittest.main()

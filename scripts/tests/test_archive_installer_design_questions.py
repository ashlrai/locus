"""Keep the issue's unanswered installer decisions visible and traceable."""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DOC = ROOT / "docs/archive-installer-design-questions.md"


class ArchiveInstallerDesignQuestionsTest(unittest.TestCase):
    def test_scope_and_questions(self):
        text = DOC.read_text(encoding="utf-8")
        self.assertIn("https://github.com/ashlrai/locus/issues/169", text)
        self.assertIn("open questions, not claims", text)
        self.assertIn("without npm or Rust", text)
        for heading in (
            "Platform detection", "Archive selection", "Installation location",
            "Failure behavior",
        ):
            with self.subTest(heading=heading):
                section = re.search(
                    rf"(?ms)^## {re.escape(heading)}\n(.*?)(?=^## |\Z)", text
                )
                self.assertIsNotNone(section)
                questions = re.findall(r"(?ms)^- (.*?)(?=^- |\Z)", section.group(1))
                self.assertGreaterEqual(len(questions), 3)
                for question in questions:
                    self.assertIn("?", question)

    def test_index_and_local_links(self):
        text = DOC.read_text(encoding="utf-8")
        index = (ROOT / "docs/README.md").read_text(encoding="utf-8")
        self.assertIn("](archive-installer-design-questions.md)", index)
        links = re.findall(r"\]\(([^)]+)\)", text)
        local = [link.split("#", 1)[0] for link in links if not re.match(r"\w+://", link)]
        self.assertTrue(local)
        for link in local:
            with self.subTest(link=link):
                self.assertTrue((DOC.parent / link).is_file())


if __name__ == "__main__":
    unittest.main()

"""Keep the installer error questions discoverable and scoped as questions."""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DOC = ROOT / "docs/installer-error-questions.md"


class InstallerErrorQuestionsTest(unittest.TestCase):
    def test_issue_scope_and_question_sections(self):
        text = DOC.read_text(encoding="utf-8")
        self.assertIn("https://github.com/ashlrai/locus/issues/169", text)
        self.assertIn("do not assert", text)
        self.assertIn("Mocked results do not establish", text)
        for heading in ("Unsupported platforms", "Download failure", "Installation failure"):
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
        self.assertIn("](installer-error-questions.md)", index)
        local_links = [
            link.split("#", 1)[0]
            for link in re.findall(r"\]\(([^)]+)\)", text)
            if not re.match(r"\w+://", link)
        ]
        self.assertTrue(local_links)
        for link in local_links:
            with self.subTest(link=link):
                self.assertTrue((DOC.parent / link).is_file())


if __name__ == "__main__":
    unittest.main()

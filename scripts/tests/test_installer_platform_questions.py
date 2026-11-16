"""Keep the platform decision record discoverable and its sources usable."""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DOC = ROOT / "docs/installer-platform-questions.md"


class InstallerPlatformQuestionsTest(unittest.TestCase):
    def test_discoverable_and_traces_issue(self):
        index = (ROOT / "docs/README.md").read_text(encoding="utf-8")
        text = DOC.read_text(encoding="utf-8")
        self.assertIn("](installer-platform-questions.md)", index)
        self.assertIn("https://github.com/ashlrai/locus/issues/169", text)
        self.assertIn("not a promise", text)
        self.assertIn("Mocked selection tests alone cannot establish", text)

    def test_local_evidence_links_exist(self):
        text = DOC.read_text(encoding="utf-8")
        links = re.findall(r"\]\(([^)]+)\)", text)
        local = [link.split("#", 1)[0] for link in links if not re.match(r"\w+://", link)]
        self.assertTrue(local)
        for link in local:
            with self.subTest(link=link):
                self.assertTrue((DOC.parent / link).is_file())


if __name__ == "__main__":
    unittest.main()

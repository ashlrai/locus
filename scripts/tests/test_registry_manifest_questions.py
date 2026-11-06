"""Check the release-manifest questions remain linked and explicitly open."""

from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[2]
DOC = ROOT / "docs/registry-manifest-verification-questions.md"


class RegistryManifestQuestionsTest(unittest.TestCase):
    def test_local_sources_exist(self):
        text = DOC.read_text(encoding="utf-8")
        links = re.findall(r"\]\(([^)#]+)(?:#[^)]*)?\)", text)
        for target in ("./registry-trust.md", "../CHANGELOG.md", "./registry-manifest-verification.md"):
            with self.subTest(target=target):
                self.assertIn(target, links)
                self.assertTrue((DOC.parent / target).is_file())

    def test_current_status_and_open_decisions_are_explicit(self):
        text = DOC.read_text(encoding="utf-8")
        self.assertIn("without `--sign`", text)
        self.assertIn("no\nconfigured CI signing setup or published registry root", text)
        self.assertIn("`signature unsigned`", text)
        self.assertIn("does not authenticate", text)
        self.assertIn("questions below are unresolved", text)
        questions = text.split("## Questions to resolve before relying on release verification", 1)[1]
        self.assertGreaterEqual(questions.count("?"), 10)
        for section in ("Trust and key lifecycle", "What a verified release should bind", "Release and consumer operations"):
            self.assertIn(f"### {section}", questions)


if __name__ == "__main__":
    unittest.main()

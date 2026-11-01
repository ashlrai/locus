"""Keep the archive requirements traceable and its local references usable."""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DOC = ROOT / "docs/installer-archive-requirements.md"


class InstallerArchiveRequirementsTest(unittest.TestCase):
    def test_issue_and_index_link_to_requirements(self):
        text = DOC.read_text(encoding="utf-8")
        index = (ROOT / "docs/README.md").read_text(encoding="utf-8")
        self.assertIn("https://github.com/ashlrai/locus/issues/169", text)
        self.assertIn("](installer-archive-requirements.md)", index)
        self.assertLess(text.index("## Goal and constraints from the issue"),
                        text.index("## Archive questions to resolve"))
        self.assertLess(text.index("## Archive questions to resolve"),
                        text.index("## Existing implementation and evidence"))

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

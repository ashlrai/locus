"""Check that release acceptance remains discoverable and its links work."""

import re
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
DOC = ROOT / "docs/archive-installer-acceptance.md"


class ArchiveInstallerAcceptanceTest(unittest.TestCase):
    def test_traceability_and_local_links(self):
        text = DOC.read_text(encoding="utf-8")
        index = (ROOT / "docs/README.md").read_text(encoding="utf-8")
        self.assertIn("https://github.com/ashlrai/locus/issues/169", text)
        self.assertIn("](archive-installer-acceptance.md)", index)
        links = re.findall(r"\]\(([^)]+)\)", text)
        local = [link.split("#", 1)[0] for link in links if not re.match(r"\w+://", link)]
        self.assertTrue(local)
        for link in local:
            with self.subTest(link=link):
                self.assertTrue((DOC.parent / link).is_file())

    def test_candidate_targets_track_script(self):
        text = DOC.read_text(encoding="utf-8")
        installer = (ROOT / "scripts/install.sh").read_text(encoding="utf-8")
        documented = set(re.findall(r"`([\w-]+-(?:darwin|gnu))`", text))
        selected = set(re.findall(r"\btarget=([\w-]+)\s*;;", installer))
        self.assertTrue(selected)
        self.assertEqual(documented, selected)


if __name__ == "__main__":
    unittest.main()

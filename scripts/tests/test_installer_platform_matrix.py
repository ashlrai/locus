"""Keep the documented shell targets and local source links in sync."""

import re
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
MATRIX = ROOT / "docs/installer-platform-matrix.md"


class InstallerPlatformMatrixTest(unittest.TestCase):
    def test_documented_targets_match_installer_selection(self):
        matrix = MATRIX.read_text()
        installer = (ROOT / "scripts/install.sh").read_text()
        documented = set(re.findall(r"\| `(\S+-(?:darwin|gnu))` \|", matrix))
        selected = set(re.findall(r"\btarget=([\w-]+)\s*;;", installer))
        self.assertTrue(selected)
        self.assertEqual(documented, selected)

    def test_local_sources_exist(self):
        links = re.findall(r"\]\(([^)]+)\)", MATRIX.read_text())
        local = [link.split("#", 1)[0] for link in links if not link.startswith("https://")]
        self.assertTrue(local)
        for link in local:
            with self.subTest(link=link):
                self.assertTrue((MATRIX.parent / link).is_file())


if __name__ == "__main__":
    unittest.main()

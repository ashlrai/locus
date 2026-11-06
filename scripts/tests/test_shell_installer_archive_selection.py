"""Check that the selection guide tracks the installer's actual host pairs."""

import re
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
GUIDE = ROOT / "docs/shell-installer-archive-selection.md"


class ShellInstallerArchiveSelectionTest(unittest.TestCase):
    def test_documented_host_pairs_match_installer(self):
        guide = GUIDE.read_text()
        installer = (ROOT / "scripts/install.sh").read_text()
        rows = re.findall(r"^\| `([^`]+)` \| `([^`]+)` \| `([^`]+)` \|$", guide, re.M)
        documented = {(os_name, arch): target for os_name, arch, target in rows}
        selected = {}
        for alternatives, target in re.findall(
            r"^\s+([\w/|]+)\) target=([\w-]+)\s*;;", installer, re.M
        ):
            for pair in alternatives.split("|"):
                selected[tuple(pair.split("/"))] = target
        self.assertTrue(selected)
        self.assertEqual(len(rows), len(documented), "duplicate host pair in guide")
        self.assertEqual(documented, selected)

    def test_local_links_resolve(self):
        links = re.findall(r"\]\(([^)]+)\)", GUIDE.read_text())
        local = [link.split("#", 1)[0] for link in links if not link.startswith("https://")]
        self.assertTrue(local)
        for link in local:
            with self.subTest(link=link):
                self.assertTrue((GUIDE.parent / link).is_file())


if __name__ == "__main__":
    unittest.main()

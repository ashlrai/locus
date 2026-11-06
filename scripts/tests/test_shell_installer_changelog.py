"""Keep the issue #169 changelog entry scoped to the proposal."""

from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]


class ShellInstallerChangelogTest(unittest.TestCase):
    def test_unreleased_documentation_describes_the_proposal(self):
        changelog = (ROOT / "CHANGELOG.md").read_text(encoding="utf-8")
        unreleased = changelog.split("## [Unreleased]", 1)[1].split("\n## [", 1)[0]
        documentation = unreleased.split("### Documentation", 1)[1].split("\n### ", 1)[0]
        note = documentation.split("**Shell installer proposal", 1)[1].split("\n\n", 1)[0]

        self.assertIn("https://github.com/ashlrai/locus/issues/169", note)
        self.assertIn("proposes a one-command shell installer", note)
        self.assertIn("downloads and verifies", note)
        self.assertIn("release archive", note)
        self.assertIn("without npm or Rust", note)
        self.assertIn("does not assert release availability", note)


if __name__ == "__main__":
    unittest.main()

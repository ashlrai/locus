# Locus — Homebrew formula
#
# This formula is mirrored for review in the main repo. Ship it by copying
# into ashlrai/homebrew-ashlr (or a dedicated ashlrai/homebrew-locus tap).
# See integrations/homebrew/README.md.
#
# Default: build from source (Rust). After the first tagged release with
# prebuilt assets (locus-<target>.tar.gz from release.yml), you can switch
# the body to the prebuilt URL block commented at the bottom.

class Locus < Formula
  desc "Identity plane for coding agents — wrong account, impossible"
  homepage "https://github.com/ashlrai/locus"
  version "0.1.0"
  license "MIT"
  head "https://github.com/ashlrai/locus.git", branch: "main"

  # Stable source tarball — update sha256 when tagging vX.Y.Z:
  #   curl -sL "https://github.com/ashlrai/locus/archive/refs/tags/v#{version}.tar.gz" | shasum -a 256
  url "https://github.com/ashlrai/locus/archive/refs/tags/v#{version}.tar.gz"
  sha256 "0000000000000000000000000000000000000000000000000000000000000000"

  depends_on "rust" => :build

  def install
    system "cargo", "install", "--locked", *std_cargo_args(path: "crates/locus-cli")
    system "cargo", "install", "--locked", *std_cargo_args(path: "crates/locus-mcp")
  end

  def caveats
    <<~EOS
      Pin a binding, then prove isolation:

        locus init --with-samples
        locus pin personal
        locus whoami

      MCP (Claude Code / Cursor):

        locus pin acme
        locus setup --client claude

      Docs: #{homepage}
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/locus --version")
    assert_predicate bin/"locus-mcp", :executable?
  end
end

# --- Optional: prebuilt binary formula (after release assets exist) ---------
# Replace the class body above with the following once release.yml publishes
# locus-<triple>.tar.gz assets and you have real sha256 digests:
#
# class Locus < Formula
#   desc "Identity plane for coding agents — wrong account, impossible"
#   homepage "https://github.com/ashlrai/locus"
#   version "0.1.0"
#   license "MIT"
#
#   on_macos do
#     on_arm do
#       url "https://github.com/ashlrai/locus/releases/download/v#{version}/locus-aarch64-apple-darwin.tar.gz"
#       sha256 "REPLACE"
#     end
#     on_intel do
#       url "https://github.com/ashlrai/locus/releases/download/v#{version}/locus-x86_64-apple-darwin.tar.gz"
#       sha256 "REPLACE"
#     end
#   end
#
#   on_linux do
#     on_intel do
#       url "https://github.com/ashlrai/locus/releases/download/v#{version}/locus-x86_64-unknown-linux-gnu.tar.gz"
#       sha256 "REPLACE"
#     end
#   end
#
#   def install
#     # release.yml packs locus-<target>/{locus,locus-mcp}
#     nested = Dir["locus-*"].find { |p| File.directory?(p) }
#     prefix = nested || "."
#     bin.install "#{prefix}/locus"
#     bin.install "#{prefix}/locus-mcp"
#   end
#
#   test do
#     assert_match version.to_s, shell_output("#{bin}/locus --version")
#     assert_predicate bin/"locus-mcp", :executable?
#   end
# end

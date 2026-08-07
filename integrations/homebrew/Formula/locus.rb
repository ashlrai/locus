# Locus — Homebrew formula
#
# Mirrored for review in the main repo. Ship by copying into
# ashlrai/homebrew-ashlr (or ashlrai/homebrew-locus). See
# integrations/homebrew/README.md and docs/RELEASE.md.
#
# === First release (v0.1.0) ===
# 1. Tag:  git tag -a v0.1.0 -m "Locus v0.1.0" && git push origin v0.1.0
# 2. Wait for .github/workflows/release.yml to attach locus-*.tar.gz assets.
# 3. Source formula sha256 (this file's default install path):
#      curl -sL "https://github.com/ashlrai/locus/archive/refs/tags/v0.1.0.tar.gz" | shasum -a 256
#    Replace the placeholder sha256 below, then PR into the live tap.
# 4. Optional: switch to prebuilt binary formula (commented block at bottom)
#    once you have per-target digests of the release assets.
#
# Default body: build from source (Rust). Prebuilt assets from release.yml:
#   locus-aarch64-apple-darwin.tar.gz
#   locus-x86_64-apple-darwin.tar.gz
#   locus-x86_64-unknown-linux-gnu.tar.gz

class Locus < Formula
  desc "Identity plane for coding agents — wrong account, impossible"
  homepage "https://github.com/ashlrai/locus"
  version "0.1.0"
  license "MIT"
  head "https://github.com/ashlrai/locus.git", branch: "main"

  # Stable source tarball — update sha256 after tagging v0.1.0 (first release):
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

# --- Optional: prebuilt binary formula (after first release assets exist) ---
# After v0.1.0 (or later) is tagged and release.yml has published
# locus-<triple>.tar.gz, replace the class body above with the following and
# fill real sha256 digests:
#
#   curl -sL "https://github.com/ashlrai/locus/releases/download/v0.1.0/locus-aarch64-apple-darwin.tar.gz" | shasum -a 256
#   curl -sL "https://github.com/ashlrai/locus/releases/download/v0.1.0/locus-x86_64-apple-darwin.tar.gz" | shasum -a 256
#   curl -sL "https://github.com/ashlrai/locus/releases/download/v0.1.0/locus-x86_64-unknown-linux-gnu.tar.gz" | shasum -a 256
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

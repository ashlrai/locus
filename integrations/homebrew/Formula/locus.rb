# Locus — Homebrew formula
#
# Mirrored for review in the main repo. Ship by copying into
# ashlrai/homebrew-ashlr (or ashlrai/homebrew-locus). See
# integrations/homebrew/README.md and docs/RELEASE.md.
#
# === v0.4.0 (this formula version) ===
# 1. Tag:  git tag -a v0.4.0 -m "Locus v0.4.0" && git push origin v0.4.0
#    (do not force-push tags)
# 2. Wait for .github/workflows/release.yml to attach locus-*.tar.gz assets.
# 3. Source formula sha256 (this file's default install path):
#      curl -sL "https://github.com/ashlrai/locus/archive/refs/tags/v0.4.0.tar.gz" | shasum -a 256
#    Replace the REPLACE placeholder sha256 below, then PR into the live tap.
# 4. Optional: switch to prebuilt binary formula (commented block at bottom)
#    once you have per-target digests of the release assets.
#
# === v0.3.0 reference digests (published 2026-08-15) ===
# Source archive (GitHub tag tarball):
#   https://github.com/ashlrai/locus/archive/refs/tags/v0.3.0.tar.gz
#   sha256: e53e99479fa1fd88e92d0603aff0fe6da407b96cc51e8b761dafd6e254d61b33
# Release assets (gh release download v0.3.0 -R ashlrai/locus):
#   locus-aarch64-apple-darwin.tar.gz
#     sha256: 356aa29b91fdc8e433fcbdb269aed5108c349ac12a4c3501cabd95f2f75b319d
#   locus-x86_64-apple-darwin.tar.gz
#     sha256: cbc2e322d71154d0a9e18a1db8bf6d4319fd5c0737472c62f7911df62122ffee
#   locus-x86_64-unknown-linux-gnu.tar.gz
#     sha256: 41c93be717847e4cc5e930d1a5eb050ec51e2bf03ceee260445b45a433f26313
#
# === v0.2.0 reference digests (published 2026-08-09) ===
# Source archive (GitHub tag tarball):
#   https://github.com/ashlrai/locus/archive/refs/tags/v0.2.0.tar.gz
#   sha256: 257c9a5427e5cbd106c6fae5a324f6a921fdb15e7e49441bae2106ec6ed8826f
# Release assets (gh release download v0.2.0 -R ashlrai/locus):
#   locus-aarch64-apple-darwin.tar.gz
#     sha256: 25f30770368ce460105c3accbe24bb0dafa9d709be4017e4e419b61b1db1eaea
#   locus-x86_64-apple-darwin.tar.gz
#     sha256: cb4c89a36302dd23e916f54b80c8522d830bb61e385f9e76d1ee68257ac8367a
#   locus-x86_64-unknown-linux-gnu.tar.gz
#     sha256: d275f31f0d344c4b25ef20b4365a5a8e1cde7fdc4f50fc7c76c8dc3ca1b20a2e
#
# === v0.1.0 reference digests (published 2026-08-07) ===
# Source archive (GitHub tag tarball):
#   https://github.com/ashlrai/locus/archive/refs/tags/v0.1.0.tar.gz
#   sha256: a0a8e9e14bd9b3322faca27d2efe42a2dcc473d84a40aab3497a4296b8d68cce
# Release assets (gh release download v0.1.0 -R ashlrai/locus):
#   locus-aarch64-apple-darwin.tar.gz
#     sha256: 0f184f8f38257ee6b9a623543400ab9ac5b8bc1eb11a105aaf4dc8fe582c3f83
#   locus-x86_64-apple-darwin.tar.gz
#     sha256: 426695db3469c8fe71798268cde6abb3d2dd30b1ad05212682451063b56b08d3
#   locus-x86_64-unknown-linux-gnu.tar.gz
#     sha256: 16632ae69e1881830644ee411c1cb889986d25e1e24e9d8e8b01728bbf0da7c6
#
# Default body: build from source (Rust). Prebuilt assets from release.yml:
#   locus-aarch64-apple-darwin.tar.gz
#   locus-x86_64-apple-darwin.tar.gz
#   locus-x86_64-unknown-linux-gnu.tar.gz

class Locus < Formula
  desc "Identity plane for coding agents — wrong account, impossible"
  homepage "https://github.com/ashlrai/locus"
  url "https://github.com/ashlrai/locus/archive/refs/tags/v0.4.0.tar.gz"
  version "0.4.0"
  sha256 "046ccb3bf0d873793051ce2b170851bb44f3c6bf458178e2486cda960d04a358"
  license "MIT"
  head "https://github.com/ashlrai/locus.git", branch: "main"

  # Stable source tarball (verify after publishing v0.4.0):
  #   curl -sL "https://github.com/ashlrai/locus/archive/refs/tags/v#{version}.tar.gz" | shasum -a 256
  # v0.3.0 source sha256 (reference): e53e99479fa1fd88e92d0603aff0fe6da407b96cc51e8b761dafd6e254d61b33
  # v0.2.0 source sha256 (reference): 257c9a5427e5cbd106c6fae5a324f6a921fdb15e7e49441bae2106ec6ed8826f
  # v0.1.0 source sha256 (reference): a0a8e9e14bd9b3322faca27d2efe42a2dcc473d84a40aab3497a4296b8d68cce

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

      Firm workflow:

        locus enter acme
        locus run -b personal -- npm test
        locus doctor   # SAFE | WARN | UNSAFE
        locus notify status   # OFF by default

      Docs: #{homepage}
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/locus --version")
    assert_predicate bin/"locus-mcp", :executable?
  end
end

# --- Optional: prebuilt binary formula (after release assets exist) ---
# After a release is tagged and release.yml has published locus-<triple>.tar.gz,
# replace the class body above with the following and fill real sha256 digests:
#
#   curl -sL "https://github.com/ashlrai/locus/releases/download/v0.4.0/locus-aarch64-apple-darwin.tar.gz" | shasum -a 256
#   curl -sL "https://github.com/ashlrai/locus/releases/download/v0.4.0/locus-x86_64-apple-darwin.tar.gz" | shasum -a 256
#   curl -sL "https://github.com/ashlrai/locus/releases/download/v0.4.0/locus-x86_64-unknown-linux-gnu.tar.gz" | shasum -a 256
#
# v0.3.0 asset digests (reference only):
#   aarch64-apple-darwin: 356aa29b91fdc8e433fcbdb269aed5108c349ac12a4c3501cabd95f2f75b319d
#   x86_64-apple-darwin:  cbc2e322d71154d0a9e18a1db8bf6d4319fd5c0737472c62f7911df62122ffee
#   x86_64-unknown-linux-gnu: 41c93be717847e4cc5e930d1a5eb050ec51e2bf03ceee260445b45a433f26313
#
# v0.2.0 asset digests (reference only):
#   aarch64-apple-darwin: 25f30770368ce460105c3accbe24bb0dafa9d709be4017e4e419b61b1db1eaea
#   x86_64-apple-darwin:  cb4c89a36302dd23e916f54b80c8522d830bb61e385f9e76d1ee68257ac8367a
#   x86_64-unknown-linux-gnu: d275f31f0d344c4b25ef20b4365a5a8e1cde7fdc4f50fc7c76c8dc3ca1b20a2e
#
# class Locus < Formula
#   desc "Identity plane for coding agents — wrong account, impossible"
#   homepage "https://github.com/ashlrai/locus"
#   version "0.4.0"
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

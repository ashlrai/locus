# locus-cli

npm wrapper for the [Locus](https://github.com/ashlrai/locus) CLI — identity plane for coding agents.

**Wrong account, impossible.** Pin a binding; every command is hard-scoped to that tenant until you re-pin.

## Install

```bash
npm install -g locus-cli
# or
npx locus-cli --help
```

On first run the wrapper reuses a verified cache or an existing native Locus
v0.5.0 on PATH (skipping npm launchers). Otherwise it downloads the matching
v0.5.0 GitHub release archive and checks a SHA-256 digest shipped in this npm
package before extracting it into `~/.locus/bin`.

Prebuilt releases support macOS arm64/x64 and Linux x64. Missing assets,
unsupported platforms, network failures and checksum failures stop with an
explicit source install command. The wrapper never runs Cargo automatically:

```bash
cargo install --git https://github.com/ashlrai/locus --tag v0.5.0 locus-cli --locked
```

Requires Node ≥ 16. Manual source installation needs [Rust](https://rustup.rs).
Cached installs include a version, target and integrity receipt; older caches
with only a `.version` sidecar are refreshed once. A verified cache runs offline.
The wrapper downloads executable tooling only; it does not initialize Locus,
pin a binding, acquire credentials or change the calling shell's identity.

## Quick start

```bash
locus init --with-samples
locus pin personal
locus whoami
locus exec -- env | grep LOCUS_
```

## Related

- MCP server: [`locus-mcp`](https://www.npmjs.com/package/locus-mcp)
- Source / Homebrew: https://github.com/ashlrai/locus

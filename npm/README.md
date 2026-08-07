# locus-cli

npm wrapper for the [Locus](https://github.com/ashlrai/locus) CLI — identity plane for coding agents.

**Wrong account, impossible.** Pin a binding; every command is hard-scoped to that tenant until you re-pin.

## Install

```bash
npm install -g locus-cli
# or
npx locus-cli --help
```

On first run the wrapper:

1. Downloads the matching GitHub release binary into `~/.locus/bin`, or
2. Falls back to `cargo install --git https://github.com/ashlrai/locus --package locus-cli`

Requires Node ≥ 16. Cargo fallback needs [Rust](https://rustup.rs).

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

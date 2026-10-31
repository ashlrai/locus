# @ashlrai/locus-mcp

<!-- fresh:begin install -->
```bash
npm install -g locus-cli@0.6.0 @ashlrai/locus-mcp@0.6.0
```
<!-- fresh:end -->

**Version scope:** The install pin above selects the 0.6.0 wrappers, whose
publication was verified as of October 31, 2026. If their checksum download fails
or cannot be parsed, they can install without verification; on first run, a
checksum mismatch can fall back to Cargo. Version 0.6.1 implements the fail-closed
checksum behavior described below.

MCP multiplexor for [Locus](https://github.com/ashlrai/locus) — tools hard-scoped to the active pin so agents cannot act in the wrong tenant.

## Install

```bash
npm install -g @ashlrai/locus-mcp
# or run via npx after pinning with locus:
npx @ashlrai/locus-mcp
```

In version 0.6.1, on first run the wrapper downloads the release
binary into `~/.locus/bin` after
verifying its SHA-256 sidecar. If the checksum cannot be retrieved or parsed, or
does not match, installation stops with an error and does not fall back to
Cargo; retry after checking the release sidecar. The sidecar detects corruption,
but is not a publisher signature. For other download or extraction errors, or
when there is no prebuilt release for the platform, the wrapper falls back to:

```bash
cargo install --git https://github.com/ashlrai/locus locus-mcp --locked
```

The Cargo fallback requires Rust 1.89 or newer ([rustup](https://rustup.rs)) —
the workspace's `rust-version`. The wrapper builds in a temporary private
Cargo install root, then compares the built version with the cache before
installing it into `~/.locus/bin`. Existing Cargo-installed binaries are left
untouched.

The wrapper never replaces a newer `~/.locus/bin/locus-mcp` with its older
release. When the cached binary reports (or its sidecar records) a newer
version than this package, the wrapper keeps and runs it; the same applies
when the cached version cannot be determined. To intentionally replace it,
run the wrapper with a leading `--force`, for example `locus-mcp --force`.
npm's own `--force` is not an override.

## Setup (Claude Code / Cursor)

```bash
# Install CLI + MCP
npm install -g locus-cli @ashlrai/locus-mcp
# or: cargo install --git https://github.com/ashlrai/locus locus-cli locus-mcp --locked

eval "$(locus hook bash)"      # exports LOCUS_CONTROL_CAPABILITY
locus pin acme
locus setup --client claude   # writes/merges .mcp.json
# restart Claude Code from that shell
```

`locus setup --client claude` writes this entry (`command` is the absolute
`locus-mcp` path when it sits next to `locus`):

```json
{
  "mcpServers": {
    "locus": {
      "type": "stdio",
      "command": "locus-mcp",
      "args": [],
      "env": { "LOCUS_AUTO_PIN": "cwd", "LOCUS_CLIENT": "claude" }
    }
  }
}
```

That serves control tools only. For the pinned binding's provider tools, set
`"command": "locus"` and `"args": ["mcp", "--supervised"]`, as `setup` prints,
and launch the client from the shell that pinned. See the repo `docs/mcp.md`.

## Related

- CLI: [`locus-cli`](https://www.npmjs.com/package/locus-cli)
- Pairs with `phantom` (Phantom Secrets) as `required_servers`: https://github.com/ashlrai/locus#how-locus-plugs-into-phantom
- Docs: https://github.com/ashlrai/locus/blob/main/docs/mcp.md

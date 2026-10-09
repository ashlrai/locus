# @ashlrai/locus-mcp

MCP multiplexor for [Locus](https://github.com/ashlrai/locus) — tools hard-scoped to the active pin so agents cannot act in the wrong tenant.

## Install

```bash
npm install -g @ashlrai/locus-mcp
# or run via npx after pinning with locus:
npx @ashlrai/locus-mcp
```

On first run the wrapper reuses a verified cache or an existing native
`locus-mcp` on PATH (skipping npm launchers). Otherwise it downloads the matching
v0.5.0 GitHub release archive and checks a SHA-256 digest shipped in this npm
package before extracting it into `~/.locus/bin`.

Prebuilt releases support macOS arm64/x64 and Linux x64. Missing assets,
unsupported platforms, network failures and checksum failures stop. Source
installation is an explicit choice; Cargo is never run automatically:

```bash
cargo install --git https://github.com/ashlrai/locus --tag v0.5.0 locus-mcp --locked
```

Requires Node ≥ 16. A verified cache runs offline. Older caches without an
integrity receipt are refreshed once. The MCP server has no `--version` flag;
cache checks use receipts and hashes without starting the server. Installer
diagnostics go to stderr so stdout stays available for MCP protocol messages.
The wrapper does not initialize Locus, pin a binding or acquire credentials.

## Setup (Claude Code / Cursor)

```bash
# Install CLI + MCP
npm install -g locus-cli @ashlrai/locus-mcp
# or: cargo install --git https://github.com/ashlrai/locus --tag v0.5.0 locus-cli locus-mcp --locked

locus pin acme
locus setup --client claude   # writes/merges .mcp.json
# restart Claude Code
```

Manual `.mcp.json` entry:

```json
{
  "mcpServers": {
    "locus": {
      "command": "locus-mcp",
      "args": []
    }
  }
}
```

## Related

- CLI: [`locus-cli`](https://www.npmjs.com/package/locus-cli)
- Docs: https://github.com/ashlrai/locus/blob/main/docs/mcp.md

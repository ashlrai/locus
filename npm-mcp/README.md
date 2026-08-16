# @ashlrai/locus-mcp

MCP multiplexor for [Locus](https://github.com/ashlrai/locus) — tools hard-scoped to the active pin so agents cannot act in the wrong tenant.

## Install

```bash
npm install -g @ashlrai/locus-mcp
# or run via npx after pinning with locus:
npx @ashlrai/locus-mcp
```

On first run the wrapper downloads the release binary into `~/.locus/bin`, or falls back to:

```bash
cargo install --git https://github.com/ashlrai/locus --package locus-mcp --locked
```

## Setup (Claude Code / Cursor)

```bash
# Install CLI + MCP
npm install -g locus-cli @ashlrai/locus-mcp
# or: cargo install --git https://github.com/ashlrai/locus --package locus-cli --package locus-mcp

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

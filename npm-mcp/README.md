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
cargo install --git https://github.com/ashlrai/locus locus-cli --locked   # installs both binaries
```

## Setup (Claude Code / Cursor)

```bash
# Install CLI + MCP
npm install -g locus-cli @ashlrai/locus-mcp
# or: cargo install --git https://github.com/ashlrai/locus locus-cli --locked

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
- Docs: https://github.com/ashlrai/locus/blob/main/docs/mcp.md

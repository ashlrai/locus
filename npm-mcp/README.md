# @ashlrai/locus-mcp

MCP multiplexor for [Locus](https://github.com/ashlrai/locus) — tools hard-scoped to the active pin so agents cannot act in the wrong tenant.

## Install

```bash
npm install -g @ashlrai/locus-mcp@0.5.1
# or run via npx after pinning with locus:
npx @ashlrai/locus-mcp@0.5.1
```

Use the `0.5.1` packages. `locus-cli@0.5.0` did not exit from `--version`
within 8 seconds (`timeout` exit 124).

On first run the wrapper downloads the release binary into `~/.locus/bin`, or falls back to:

```bash
cargo install --git https://github.com/ashlrai/locus locus-mcp --locked
```

## Setup (Claude Code / Cursor)

```bash
# Install CLI + MCP
npm install -g locus-cli@0.5.1 @ashlrai/locus-mcp@0.5.1
# or: cargo install --git https://github.com/ashlrai/locus locus-cli locus-mcp --locked

eval "$(locus hook bash)"      # exports LOCUS_CONTROL_CAPABILITY
locus pin acme
locus setup --client claude   # writes/merges .mcp.json
# restart Claude Code from that shell
```

Provider tools need `locus mcp --supervised` from the shell that pinned.
Plain `locus-mcp` serves control tools only. See the repo `docs/mcp.md`.

Manual `.mcp.json` entry:

```json
{
  "mcpServers": {
    "locus": {
      "command": "locus",
      "args": ["mcp", "--supervised"]
    }
  }
}
```

## Related

- CLI: [`locus-cli`](https://www.npmjs.com/package/locus-cli)
- Docs: https://github.com/ashlrai/locus/blob/main/docs/mcp.md

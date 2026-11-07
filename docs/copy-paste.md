# Copy-paste examples

Run each block as written. Blocks that pin share one shell, because the pin's broker dies with that shell. `LOCUS_HOME` keeps the trial off your real `~/.locus`.

Agents cannot pin. These commands are the human control plane. After a pin, an agent calls `locus_whoami` and the other control tools. Provider tools need `locus mcp --supervised`.

## Install

```bash
npm install -g locus-cli @ashlrai/locus-mcp
```

Homebrew and Cargo are in the [README install options](../README.md#install-options). At the v0.6.2 release, the live Homebrew tap installs v0.6.1, so use npm for v0.6.2.

## Pin a sample and read JSON

```bash
export LOCUS_HOME="$(mktemp -d)"
locus quickstart
eval "$(locus hook bash)"
locus enter personal
locus whoami --json
locus status --json
locus doctor --json
locus version --json
locus capability status
locus exec --expect personal --no-resolve -- echo ok
```

`locus quickstart` writes sample `personal` and `acme` bindings and mints `LOCUS_CONTROL_CAPABILITY`. The hook exports that capability and does not print it. `--no-resolve` skips credential resolution and refuses to start the child if an upstream would resolve credentials. With the sample bindings, the child prints `ok`. Existing bindings are not replaced by quickstart.

`locus doctor --json` can exit 0 with `verdict` `WARN` and `ok` false when the only findings are setup advice (for example Phantom is not installed). That is not a finished setup.

This next command is supposed to fail. It exits 1, says `pin_mismatch`, and does not start the child:

```bash
locus exec --expect acme -- echo ok
```

Schema ids and exit codes: `locus topic agent-contract --json`, or [agent-contract.md](agent-contract.md).

## Preview without writing

```bash
locus init --dry-run --json
```

The object uses schema `locus.plan.v1`. It does not create a Locus home. The command list is in [the docs index](README.md#previewing-changes-with---dry-run).

## MCP config, print only

These print an entry and write nothing. The entry is `locus-mcp`, which serves control tools only (`locus_whoami`, `locus_request_pin`, and the other `locus_*` control tools).

```bash
locus setup --client claude --print
locus setup --client cursor --print
locus setup --client codex --print
locus setup --client generic
```

## MCP config, provider tools

Paste this when `locus` is on `PATH`. Launch the client from the shell that ran the hook and `locus enter`. Restart the client after a re-pin.

Claude Code (`.mcp.json`) and Cursor (`~/.cursor/mcp.json`):

```json
{ "mcpServers": { "locus": { "command": "locus", "args": ["mcp", "--supervised"] } } }
```

Codex (`~/.codex/config.toml`):

```toml
[mcp_servers.locus]
command = "locus"
args = ["mcp", "--supervised"]
```

If the client CLIs are installed:

```bash
claude mcp add locus -- locus mcp --supervised
codex mcp add locus -- locus mcp --supervised
```

`locus agent setup --apply --client claude` writes that machine's client config. It needs the control capability. Use `--client cursor` or `--client codex` the same way. Add `--dry-run` to preview.

```bash
locus agent setup --dry-run --client claude
```

## Same shell, switch clients

```bash
locus switch acme
locus whoami --json
locus switch personal
locus whoami --json
```

`locus switch` leaves the previous pin and enters the next one. The broker is still this shell.

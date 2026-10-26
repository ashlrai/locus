# Ashlr companion handshake (v1)

`locus --ashlr-handshake` prints one JSON object and exits 0. `phm companions`,
`phm doctor` or an agent can call it instead of parsing `--help`. The same
contract is implemented by `stack --ashlr-handshake` (ashlr-stack) and
`ashlr-plugin --ashlr-handshake`.

```json
{
  "schema": "locus.ashlr.handshake.v1",
  "kind": "ashlr.handshake",
  "schema_version": 1,
  "name": "locus",
  "command": "locus",
  "version": "0.5.2",
  "protocols": { "ashlr.handshake": 1, "phantom.agent.preflight": 1 },
  "capabilities": ["workspace-identity-pinning", "credential-bindings", "phm-references", "mcp-server", "agent-setup", "agent-preflight", "doctor-json", "doctor-fix"],
  "health": { "status": "ok", "checks": [{ "id": "locus-home", "status": "ok" }, { "id": "phantom-on-path", "status": "absent" }] },
  "edits": ["~/.locus/", ".locus.toml", ".mcp.json", "~/.cursor/mcp.json", "~/.codex/config.toml", "~/.grok/config.toml", "~/.gemini/settings.json"],
  "receipts": []
}
```

Rules every implementation follows:

- **Fast and local:** no network, no vault or keychain, and no secret values or project contents in the output.
- **Stable shape:** `schema`, `kind` and `schema_version` identify the contract. New fields may be added in v1; renames or removals bump `schema_version`.
- **Ids:** `capabilities` and check `id`s are kebab-case. `protocols` maps a protocol name to the version spoken.
- **Health:** `health.status` is `ok`, `degraded` (works, something it relies on is missing) or `unknown`. A check status is `ok`, `missing` (counts against health) or `absent` (informational, such as phantom not installed when no binding uses `phm:`).
- **Edits:** paths the CLI may write. `~/` is the home directory; other paths are relative to the project.
- **Receipts:** receipt kinds the CLI emits (none yet; reserved for receipt.v1).

Locus answers before parsing arguments or opening `$LOCUS_HOME`, so the probe never creates or reads the store. `~/.locus/` stands for `$LOCUS_HOME` when it is set.

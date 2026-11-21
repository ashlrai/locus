# Ashlr companion handshake (v1)

`locus --ashlr-handshake` prints one JSON object on a single line and exits 0.
`phm companions`, `phm doctor` or an agent can call it instead of parsing
`--help`. The same contract is implemented by `stack --ashlr-handshake`
(ashlr-stack) and `ashlr-plugin --ashlr-handshake`.

```json
{
  "schema": "locus.ashlr.handshake.v1",
  "kind": "ashlr.handshake",
  "schema_version": 1,
  "name": "locus",
  "command": "locus",
  "version": "0.7.2",
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
The flag is recognized anywhere on the command line and other arguments are ignored, so `locus --ashlr-handshake --json` gives the same output.

Locus reports two checks. `locus-home` is `ok` when the `$LOCUS_HOME` directory (default `~/.locus`) exists. Otherwise it is `missing` and `health.status` is `degraded`; run `locus quickstart` or `locus init` to create it. `phantom-on-path` is `ok` when a `phantom` executable is on `PATH` and `absent` otherwise, and it never changes `health.status`. Locus never reports `unknown`.

`crates/locus-cli/tests/handshake_conformance.rs` checks the output against a vendored copy of the shared JSON contract (`crates/locus-cli/tests/contract/ashlr-handshake.v1.json`). The canonical copy lives in ashlr-stack, and the vendored copy is pinned by its SHA-256, so the test needs no network. To change the contract, re-vendor it from ashlr-stack and update the pin.

**Doctor JSON.** `locus doctor --json` adds the shared `ashlr.doctor` v1 wrapper to the `locus.doctor.v1` report. Every existing report field stays, and the wrapper adds:

| Field | Value |
| --- | --- |
| `kind` | `ashlr.doctor` |
| `schema_version` | `1` |
| `tool` | `locus` |
| `status` | `ok` for a SAFE verdict, `warn` for WARN, `fail` for UNSAFE. A refused [Phantom preflight](phantom-preflight.md) or any `fail` check also makes it `fail` |
| `native_status` | the Locus verdict: `SAFE`, `WARN` or `UNSAFE` |
| `checks` | one entry per finding: `id` is the finding code in kebab-case (`phantom_missing` becomes `phantom-missing`), `status` is `info`, `warn` or `fail` (from severity info, warn or unsafe), and `summary` is the finding message. A refused preflight adds `{"id": "phantom-preflight", "status": "fail"}` |

The wrapper does not change the exit code. A `warn` status still exits 0 when every finding is setup advice; see [exit codes](exit-codes.md).

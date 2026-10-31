# Hub smoke prerequisites

Run from the repository root:

```bash
./scripts/hub-smoke.sh
```

This checks local Locus CLI contracts used by the Phantom hub. It does not
require Phantom, a real binding, real credentials, a running MCP server, or a
network connection to a provider. It creates a sample `personal` binding in a
throwaway Locus home and a temporary project containing a `.mcp.json`
registration; the registration does not start `locus-mcp`.

## Required tools and access

- Bash, `jq` (with `IN` support), and standard Unix utilities including
  `mktemp`, `od`, `tr`, `grep`, `head`, `rm`, `sleep`, and `dirname`.
- The workspace's supported Rust toolchain and Cargo when a build is needed.
  The script prefers `target/debug`, then `target/release`, then the existing
  `PATH`. It runs `cargo build -q -p locus-cli` if `locus agent report --help`
  fails or the debug binary is absent. An existing debug binary is not
  necessarily rebuilt when sources change; build it first if freshness matters.
- Readable `/dev/urandom`, writable temporary storage and `target/`, and local
  process and socket access for the authority broker. Cargo may need access to
  dependencies if they are not already cached.
- A repository checkout containing `schema/agent-report.schema.json`,
  `schema/doctor.schema.json`, `schema/hub-gate.schema.json`,
  `schema/mcp-grant.schema.json`, `docs/hub-integration.md`,
  `integrations/phantom/README.md`, and
  `integrations/phantom/fleet-preflight.md`.

## Environment

No environment variable needs to be set specifically for this smoke test.

| Variable | Script behavior |
| --- | --- |
| `HOME` | Used to add `$HOME/.cargo/bin` to `PATH`. |
| `PATH` | Prepended with the checkout's debug and release directories and `$HOME/.cargo/bin`. |
| `TMPDIR` | Optional; temporary home, project, and error file are created here, defaulting to `/tmp`. It must be a writable directory. |
| `LOCUS_CONTROL_CAPABILITY` | An existing nonempty value is reused; otherwise the script generates one from `/dev/urandom`. Do not print it. |
| `LOCUS_AUTHORITY_BROKER_START_TIMEOUT_MS` | Optional; defaults to `15000` milliseconds. Core clamps the value. Initialization and pinning are retried up to three times. |
| `LOCUS_HOME` | Overridden with the temporary home for Locus commands; the caller's home is not used for bindings. |
| `LOCUS_SESSION_ID` | Unset before Locus commands to avoid inheriting a session. |

The [resource map](hub-smoke-resource-map.md) describes creation, use, and
cleanup, including resources outside the temporary home. The script removes
its temporary home, project, and error file on exit. It prints the temporary `LOCUS_HOME` path and CLI version; avoid exposing captured
output if the environment or diagnostics are sensitive.

## Expected outcomes

For the individual messages and their script paths, see
[hub smoke output](hub-smoke-output.md).

- Initialization and pinning succeed within three attempts. Each JSON check
  prints `ok` when its predicate passes; a valid JSON response can pass even if
  its command exits nonzero. The checks also reject common secret prefixes.
- `agent report --json` has the expected status, readiness, doctor, command,
  home, and required-server fields. `doctor --json` has the expected diagnostic
  fields; `whoami --json` has session, binding, tenant, provider, seal, and mode
  fields; `status --json` reports a pin. `status --oneline` contains a colon.
- After leaving the pin, `agent report --json` reports `unpinned` and not ready.
- If `locus mcp mint --help` succeeds, mint validates a grant and token in
  memory, list verifies a values-free roster, and revoke removes the grant.
  The token is not echoed. Otherwise these three checks print `skip`; the
  overall smoke may still succeed, so a skip is not evidence that grants work.
- Each listed schema and document prints a presence check. These are existence
  checks, not schema validation.
- Success ends with `== hub-smoke OK ==` and exit status 0. A failed check
  ends with `== hub-smoke FAILED ==` and status 1; setup failures can exit
  earlier. The script may print diagnostic output on failure.

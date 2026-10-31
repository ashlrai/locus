# Hub smoke output

From the repository root, run:

```bash
./scripts/hub-smoke.sh
```

See [prerequisites](hub-smoke-prerequisites.md) for tools, environment, and
the throwaway sample binding. This is a local contract check, not a provider
integration or MCP server test. The output includes a temporary home path and
may include CLI diagnostics or JSON on failure; inspect it before sharing.
The examples below use placeholders in angle brackets, not literal output.

## Setup

| Message | Script path and meaning |
| --- | --- |
| `error: hub-smoke could not create temporary home` | The first `mktemp` failed. |
| `error: hub-smoke could not create temporary project directory` | The second `mktemp` failed. |
| `error: hub-smoke could not create temporary error file` | The third `mktemp` failed. |
| `error: jq is required` | The `command -v jq` check failed. |
| `building locus with agent report…` | The CLI help probe failed; the script invokes Cargo to build `locus-cli`. Cargo can also run without this message if the debug binary is absent. |
| `== hub-smoke LOCUS_HOME=<temporary path> ==` and `locus: <path> (<version>)` | Startup banner after setup and any build. The version can be empty if its probe fails. |
| `warn: hub-smoke init/pin attempt <1 or 2> failed; retrying…` | `smoke_init_and_pin` failed to initialize or pin; it prints up to 400 bytes of captured stderr, attempts to leave, and retries. |
| `error: hub-smoke init/pin failed after 3 attempts` | The last initialization or pin attempt failed. The script prints up to 800 bytes of captured stderr and tries one unsilenced pin for diagnostics, then exits 1. |

The explicit setup errors are on stderr and exit 1 without a final banner.
Other commands, including Cargo or the oneline status command, can also abort
early under `set -e`; their diagnostics and exit status come from the failed
command, and the final banner need not appear. Temporary resources are removed
on exit.

## JSON and status checks

`check_json` runs each command with stderr suppressed. It prints one of these
messages on stdout for each named check:

| Message | Meaning |
| --- | --- |
| `ok    <name> (exit=<code>)` | Output parsed as JSON, passed the predicate, and passed the common-secret-prefix scan. The command's exit code is reported but is not itself a pass criterion. |
| `FAIL  <name> — not JSON (exit=<code>)` | `jq -e .` failed; this also rejects valid JSON whose last value is `false` or `null`. Up to 400 bytes of output follow. |
| `FAIL  <name> — jq predicate failed: <expression>` | JSON parsed but did not satisfy the displayed predicate; up to 800 bytes of compact JSON follow. |
| `FAIL  <name> — possible secret material in output` | A passing JSON payload matched the script's common-token-prefix scan. This is a limited scan, not a guarantee that output contains no secrets. |

The names map directly to calls in [the smoke script](../scripts/hub-smoke.sh):

| Name | Predicate checked |
| --- | --- |
| `agent report --json` | Allowed status and exit-code values, boolean readiness, string oneline status, MCP command and required servers, plus report and doctor fields and allowed doctor verdict. |
| `doctor --json` | Required diagnostic fields are present and verdict is `SAFE`, `WARN`, or `UNSAFE`. |
| `whoami --json` | Session, binding, tenant, providers, seal, and mode fields are present. |
| `status --json` | `pinned` is true and binding, tenant, and session fields are present. |
| `agent report unpinned` | After an attempted `locus leave`, oneline status is `unpinned` and readiness is false. |

Separately, `status --oneline` prints `ok    status --oneline (<value>)`
if the output contains a colon; otherwise it prints
`FAIL  status --oneline expected alias:tenant, got: <value>`. The check only
tests for a colon, not the full format.

## Grant and file checks

If `locus mcp mint --help` succeeds, the script checks three grant operations:

| Success | Failure | Script check |
| --- | --- | --- |
| `ok    mcp mint --json grant contract (token validated, not echoed)` | `FAIL  mcp mint --json grant contract` | Mint returns the expected grant, token format, binding, session, and expiration fields. The token is checked in memory. |
| `ok    mcp list --json values-free roster` | `FAIL  mcp list --json roster contract` | The minted grant is present and unrevoked, has a numeric live-session count, and the roster has no token fields or `lmt_` text. |
| `ok    mcp revoke removes grant from roster` | `FAIL  mcp revoke contract` | Revoke succeeds and a subsequent list no longer contains the grant. |

If the help probe fails, `skip  mcp mint/list/revoke (locus mcp subcommand unavailable)`
appears instead. A skip does not set the failure flag and does not establish
that grant operations work.

The two file loops print `ok    <path> present` or `FAIL  <path> missing`.
They check existence only, for these paths:

- `schema/agent-report.schema.json`, `schema/doctor.schema.json`,
  `schema/hub-gate.schema.json`, `schema/mcp-grant.schema.json`
- `docs/hub-integration.md`, `integrations/phantom/README.md`,
  `integrations/phantom/fleet-preflight.md`

## Final result

If all executed checks pass, the script prints `== hub-smoke OK ==` and exits
0, even if the grant checks were skipped or a JSON-producing command exited
nonzero. If any accumulated check fails, it prints `== hub-smoke FAILED ==`
and exits 1. A setup or other early abort can instead have no final banner.

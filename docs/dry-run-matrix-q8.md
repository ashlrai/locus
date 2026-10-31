# Dry-run command matrix

This matrix tracks [issue #171](https://github.com/ashlrai/locus/issues/171)
and describes the CLI in this checkout, not necessarily a published release.
The 11 commands reported in that issue are exercised in human and JSON modes
by `crates/locus-cli/tests/issue171_matrix.rs`, including home and workspace
content, permission and modification-time checks after every invocation.
`--dry-run` is a global flag;
place it before or after a supported command and supply that command's normal
required arguments. For example, `locus --json init --dry-run` previews setup,
and `locus pin personal --dry-run` previews a pin against an existing binding.
Names below denote command forms, not complete invocations: use `--help` for
required arguments. See [agent contract](agent-contract.md) for the plan schema
and exit codes.

| Behavior | Command forms accepting `--dry-run` |
| --- | --- |
| Plan | `init`, `quickstart`, `onboard`, `setup`, `workspace`, `enter`, `switch`, `pin`, `leave`, `renew` |
| Plan | `engagement init`, `engagement close`, `graph export`, `graph import`, `binding add`, `binding rm`, `binding migrate-credential-refs`, `client add`, `offboard` |
| Plan | `autopin suggest --apply`, `capability persist`, `capability unpersist`, `ci mint`, `ci env`, `mcp mint`, `mcp revoke` |
| Plan | `agent setup`, `doctor --fix`, `adapter install`, `adapter uninstall`, `adapter trust add`, `adapter registry export`, `adapter registry index add`, `adapter registry index remove` |
| Plan | `approve grant`, `approve deny`, `notify on`, `notify off`, `events export`, `forensics export` |
| Accepted, ignored | `whoami`, `status`, `doctor` (without `--fix`), `watch`, `hook`, `goal`, `verify`, `upstream`, `completion`, `topic`, `version`, `fleet-preflight`, `fleet-feed`, `fleet-denials`, `events` (without `export`) |
| Accepted, ignored | `agent doctor`, `agent report`, `binding list`, `binding show`, `graph list`, `capability status`, `notify status`, `approve list`, `approve status`, `approve wait`, `mcp list`, `autopin suggest` (without `--apply`) |
| Accepted, ignored | `adapter search`, `adapter list`, `adapter verify`, `adapter verify-manifest`, `adapter trust list`, `adapter registry index list` |

A plan runs read-only preflight checks and writes no state. With `--json` it
returns a `locus.plan.v1` object; existing commands may also retain legacy
fields. The print-only forms `setup --print` and `setup --client generic`
print configuration instead of a plan, including when the flag is present. A
successful plan does not guarantee the real operation will succeed:
it does not authenticate the control capability or start its authority broker.
Most plans require an initialized Locus home; `init`, `quickstart`, `agent
setup`, `binding add`, `client add`, and `doctor --fix` can preview against
an uninitialized home, subject to their options and inputs.
Preconditions such as a missing binding or workspace allowlist still fail.

For commands in the accepted, ignored rows, the flag does not create a plan or
suppress normal behavior. In particular, opening the store can create
`LOCUS_HOME`, and session inspection can freeze a drifted pin as a fail-closed
security measure.

## Unsupported cases

| Command form | Why it cannot be previewed |
| --- | --- |
| `exec` | Runs a child in the pinned session. |
| `run` | Runs a child with a binding. |
| `ci run` | Runs a child in CI. |
| `mcp` | Starts the MCP server when used without a subcommand. |
| `serve` | Starts a server. |
| `dashboard` | Starts a dashboard server. |

These forms have no Locus plan. `locus exec --dry-run -- env` exits 1 from
exec's argument check; other placements and the other listed commands reject
the flag as a usage error (exit 2). A `--dry-run` after `--` belongs to the
child, not to Locus. Subcommands such as `mcp mint` are classified separately
in the table above.

- `doctor --fix --apply --dry-run` and `offboard <alias> --apply --dry-run` are usage
  errors (exit 2). `agent setup
  --apply --dry-run` is also rejected, with exit 1. Use the dry-run form
  without `--apply` for these commands.
- `pin --rewrite-configs --dry-run` is rejected; rewriting client configs
  cannot be combined with a preview.

Other mutating operations lack a Locus dry-run option, including commands
launched as children, provider operations, and MCP tool calls. A plan does not
resolve credentials or authorize a later execution. Do not use it as proof that a real mutation is authorized.

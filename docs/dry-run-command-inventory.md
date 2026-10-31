# Mutating command dry-run inventory

This inventory covers the `locus` CLI in this checkout for [issue #171](https://github.com/ashlrai/locus/issues/171); installed releases may differ. A command is included if it can write local state, send data, or start a process that can make changes. Forms are names, not complete invocations: use the command's `--help` for arguments. The classification follows [`dry_run_support` and dispatch](../crates/locus-cli/src/main.rs) and the [plan implementations](../crates/locus-cli/src/plans.rs). See the [full matrix](dry-run-matrix-q8.md) for non-mutating forms and the [agent contract](agent-contract.md#--dry-run) for the plan schema and exit codes.

## Commands with plans

Each form below accepts the global `--dry-run` and normally emits a plan instead of the requested changes; `--json` emits a `locus.plan.v1` object. Normal preconditions still apply, and most require an initialized Locus home. The exceptions and default-preview behavior are described below.

| Command forms | Potential changes without a preview |
| --- | --- |
| `init`, `quickstart` | Initialize the store, capability and sample bindings; quickstart can also enter a binding. |
| `onboard`, `setup`, `agent setup` | Save onboarding state or register client configuration. |
| `workspace`, `autopin suggest --apply` | Write workspace configuration or apply a suggested default. |
| `enter`, `switch`, `pin`, `leave`, `renew` | Change or renew the active sealed session. |
| `engagement init`, `engagement close` | Create a binding and optional workspace files, or close an engagement and optionally archive audit records. |
| `graph export`, `graph import` | Write an encrypted graph file or import bindings and templates. |
| `binding add`, `binding rm`, `binding migrate-credential-refs`, `client add` | Create or remove bindings, or migrate credential references when requested. |
| `offboard` | With `--apply`, revoke and remove the binding's local resources. |
| `capability persist`, `capability unpersist` | Change persisted control-capability posture. |
| `ci mint`, `ci env` | Mint a sealed CI session; `env` also prints shell exports. |
| `mcp mint`, `mcp revoke` | Create or revoke MCP grants. |
| `doctor --fix` | With `--apply`, repair local configuration and back up changed files. |
| `adapter install`, `adapter uninstall`, `adapter trust add` | Change installed adapters or trusted signing keys. |
| `adapter registry export`, `adapter registry index add`, `adapter registry index remove` | Write a registry export or change configured registry indexes. |
| `approve grant`, `approve deny` | Record local advisory approval labels; these do not authorize provider execution. |
| `notify on`, `notify off` | Change notification configuration. |
| `events export`, `forensics export` | Write an export; events can also send an HTTP webhook. |

- `doctor --fix` and `offboard <alias>` already preview by default; `--apply` performs the changes. `--apply --dry-run` is rejected for both (exit 2). `agent setup --apply --dry-run` is also rejected (exit 1). Preview these without `--apply`. `binding migrate-credential-refs` writes only with `--write`.
- `setup --print` and `setup --client generic` print configuration rather than a plan even with `--dry-run`. `autopin suggest` without `--apply` is a read-only suggestion, not a plan. `pin --rewrite-configs --dry-run` is rejected.
- Plans do not authenticate the control capability or resolve credentials, and adapter install and registry index add plans do not fetch or verify remote content. A successful plan is not authorization or a guarantee that execution will succeed. Some plans can preview an uninitialized home; see the [matrix](dry-run-matrix-q8.md) for details.

## No Locus preview

| Command forms | Current behavior |
| --- | --- |
| `exec`, `run`, `ci run` | Start a child that can mutate its environment or external systems; `ci run` also mints a temporary session. Locus cannot preview the child. |
| `mcp`, `serve`, `dashboard` | Start a server; the server and its clients can cause changes. The `mcp` grant-management subcommands are classified separately above. |

These forms reject the Locus flag: `locus exec --dry-run -- <command>` exits 1 from its argument check; other placements and the other forms reject it as a usage error (exit 2). A flag after `--` belongs to the child. Provider operations, MCP tool calls and child commands have no Locus-level preview; consult the relevant provider or child for any independent dry-run support.

Commands classified as read-only accept and ignore `--dry-run`, so it does not suppress their normal behavior. Opening a store can create the Locus home, and session inspection can freeze a drifted pin as a fail-closed security measure.

## Verification

Help is safe to inspect and includes the flag for plan-capable forms. Substitute any table form for `binding rm`; names with arguments still need only `--help`:

```sh
locus binding rm --help
locus exec --help
```

To check a real plan without touching your Locus home, use an isolated directory. The first command emits a JSON plan, and the second checks that the planned store was not created. The last command checks that an unsupported invocation returns 2 without starting a child:

```sh
scratch=$(mktemp -d)
LOCUS_HOME="$scratch/locus" locus init --dry-run --json
test ! -e "$scratch/locus"
LOCUS_HOME="$scratch/locus" locus --dry-run exec -- true; test "$?" -eq 2
```

For automated verification of the inventory, help, unsupported forms and filesystem side effects, run:

```sh
cargo test -p locus-cli --test integration dry_run_docs
cargo test -p locus-cli --test integration dry_run_no_side_effects
```

# Hub smoke cleanup review

[`scripts/hub-smoke.sh`](../scripts/hub-smoke.sh) creates three temporary
paths directly under `${TMPDIR:-/tmp}` plus dependent state inside them, and
removes the assigned paths in one `EXIT` trap. This review lists each
temporary resource, the path that removes it, and the early exits that can
leave residue. Run it from the repository root with `./scripts/hub-smoke.sh`;
see [prerequisites](hub-smoke-prerequisites.md) for tools and environment.

## Temporary resources and cleanup paths

Rows follow script order. "EXIT trap" means `cleanup` runs `rm -rf --` on the
assigned `SMOKE_*` paths when the shell exits.

| Resource | Created by | Cleanup path |
| --- | --- | --- |
| `SMOKE_HOME` directory (`locus-hub-smoke.XXXXXX`) | First `mktemp -d`; later exported as `LOCUS_HOME`. | `rm -rf` in the `EXIT` trap, including its contents. |
| `SMOKE_PROJ` directory (`locus-hub-proj.XXXXXX`) | Second `mktemp -d`. | `rm -rf` in the `EXIT` trap, including its contents. |
| `SMOKE_ERR` file (`locus-hub-smoke-err.XXXXXX`) | `mktemp` without `-d`; truncated before each init/pin attempt. | `rm -rf` in the `EXIT` trap. |
| Locus state under `SMOKE_HOME`: sample bindings, daemon key, pin and session files, audit and runtime state, and any minted grant with its delegated session | `locus init --with-samples`, `locus pin personal`, later Locus commands, and `locus mcp mint` when the grant block runs. | Removed with `SMOKE_HOME`. `locus leave` runs between pin retries and before the unpinned report; `locus mcp revoke` removes the grant. Both are best effort and an early exit can bypass them; removal of the home covers the same state. |
| `.mcp.json` | Written with `printf` inside `SMOKE_PROJ` after pinning. | Removed with `SMOKE_PROJ`. |
| Authority broker and its Unix socket under `/tmp/locus-authority-<uid>/` | Started by init/pin control operations. | The broker retires its socket asynchronously after the home is removed or its supervisor exits; the shared per-user socket directory is not removed by the script. |
| `LOCUS_CONTROL_CAPABILITY` and the script's other exports | Reused when nonempty, otherwise generated from `/dev/urandom`; set in the script environment. | Dies with the script process. A caller-supplied capability is not revoked or changed. |
| `MINT_JSON`, `MINT_GRANT_ID`, `LIST_JSON` (grant token held in memory) | `locus mcp mint` and `locus mcp list` output captured in variables when the grant block runs. | Shell variables disappear at exit; the token is validated in memory, never written to disk or echoed. |
| `target/` build artifacts such as `target/debug/locus` | `cargo build -q -p locus-cli` when a build is needed. | Persistent by design; not removed by the trap. |

## Early exits

Every ordinary exit — an allocation failure, a missing prerequisite, a
`set -e` abort, or an accumulated check failure — runs the `EXIT` trap on the
paths assigned so far. The rows below follow the script top to bottom.

| Exit point | Trigger | Cleanup coverage | Residual gap |
| --- | --- | --- | --- |
| `SMOKE_HOME` allocation | `mktemp -d` fails: `error: hub-smoke could not create temporary home`, exit 1. | Nothing assigned; the trap runs on empty variables. | None within the script. |
| `SMOKE_PROJ` allocation | `mktemp -d` fails: `error: hub-smoke could not create temporary project directory`, exit 1. | `SMOKE_HOME` removed. | None. |
| `SMOKE_ERR` allocation | `mktemp` fails: `error: hub-smoke could not create temporary error file`, exit 1. | `SMOKE_HOME` and `SMOKE_PROJ` removed. | None. |
| `jq` probe | `command -v jq` fails: `error: jq is required`, exit 1. | All three paths removed. | None. |
| Cargo builds | `cargo build -q -p locus-cli` fails at either site: `set -e` abort, no final banner. | All three paths removed. | `target/` may hold partial build output (persistent by design). |
| Init/pin retry loop | `smoke_init_and_pin` fails three attempts: `error: hub-smoke init/pin failed after 3 attempts`, exit 1. | All three paths removed, including a populated home. | `locus leave` between retries is best effort; a broker started by a partially successful init retires asynchronously. |
| `cd "$SMOKE_PROJ"`, the `.mcp.json` write, `locus status --oneline`, or another unchecked command | Fails under `set -e`; no final banner. | All three paths removed. | The exit can fall between the pin and the script's own `locus leave`, or between `locus mcp mint` and `locus mcp revoke`; the skipped teardown is covered by home removal. |
| Check failures | `fail=1`: `== hub-smoke FAILED ==`, exit 1. | All three paths removed. | None. |
| Success | `== hub-smoke OK ==`, exit 0. | All three paths removed. | Broker socket retirement is asynchronous. |
| Catchable signal such as `SIGTERM` or `SIGINT` | Bash exits and runs its `EXIT` trap; timing depends on whether Bash is waiting on a foreground child. | Assigned paths removed. | An interruption between `mktemp` creating a path and the assignment leaves an unrecorded path the trap cannot name. |
| `SIGKILL`, a crash, or power loss | No trap can run. | None. | Every path created so far remains, including a populated home. |
| `rm -rf` failure inside the trap | Removal errors; diagnostics come from `rm`. | The other arguments are still attempted. | Leftover paths remain under the temporary parent. |

## Residual gaps

- An uncatchable kill or crash leaves all allocated paths. Inspect the
  temporary parent for `locus-hub-smoke.*`, `locus-hub-proj.*`, and
  `locus-hub-smoke-err.*` and remove only paths confirmed to belong to the
  dead invocation; concurrent runs can share the parent.
- An exit between path creation and variable assignment orphans that path;
  it is not in the trap's argument list.
- The shared broker socket directory `/tmp/locus-authority-<uid>/` and Cargo's
  `target/` survive every exit path: socket retirement is asynchronous and
  `target/` is persistent build output.
- Failures before the startup banner never print the temporary home path, so
  recovery must search the temporary parent by prefix.
- Orderly teardown (`locus leave`, `locus mcp revoke`) can be skipped by an
  early exit, but the state it touches lives under `SMOKE_HOME` and is removed
  with it.

The [interruption guide](hub-smoke-interruption.md) covers signal behavior
and manual recovery, the [allocation-failure guide](hub-smoke-temp-failures.md)
covers partial setup, and the [resource map](hub-smoke-resource-map.md)
describes the broker, grant, and capability lifecycles. Run
`node --test scripts/hub-smoke-cleanup-review.test.mjs` to check this review
against the script; the existing `hub-smoke*.test.mjs` suites exercise the
cleanup paths themselves.

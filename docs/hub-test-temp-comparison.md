# Hub test temporary resources

Both [`hub-smoke.sh`](../scripts/hub-smoke.sh) and
[`hub-integration-test.sh`](../scripts/hub-integration-test.sh) use sample
bindings in an isolated `LOCUS_HOME` and create a separate project directory.
Their explicitly allocated paths are directly under `${TMPDIR:-/tmp}`: an
unset or empty `TMPDIR` falls back to `/tmp`; a nonempty one must exist and be
writable. `XXXXXX` is replaced by `mktemp` with a unique suffix.

| Script | Variable | Type | Template | Purpose |
| --- | --- | --- | --- | --- |
| `hub-smoke.sh` | `SMOKE_HOME` | directory | `locus-hub-smoke.XXXXXX` | Locus home for sample bindings, keys, session, audit, grant and runtime state. |
| `hub-smoke.sh` | `SMOKE_PROJ` | directory | `locus-hub-proj.XXXXXX` | Working project for CLI checks. |
| `hub-smoke.sh` | `SMOKE_ERR` | file | `locus-hub-smoke-err.XXXXXX` | Init/pin stderr, truncated on each retry. |
| `hub-integration-test.sh` | `SMOKE_HOME` | directory | `locus-hub-int.XXXXXX` | Locus home for sample bindings, keys, sessions, audit, grants and runtime state. |
| `hub-integration-test.sh` | `SMOKE_PROJ` | directory | `locus-hub-int-proj.XXXXXX` | Working project for integration checks. |

The allocations and cleanup are in
[`scripts/hub-smoke.sh`](../scripts/hub-smoke.sh#L23-L40) and
[`scripts/hub-integration-test.sh`](../scripts/hub-integration-test.sh#L41-L54).
Both install an `EXIT` trap before allocating and recursively remove their
assigned paths, including after ordinary setup failures. The smoke script
reports each allocation failure explicitly; the integration script exits on a
failed assignment under `set -e`. Neither trap can handle an uncatchable kill,
and removal can fail. Neither removes the caller's `TMPDIR` itself.

| Contents or other path | Smoke | Integration |
| --- | --- | --- |
| Project registration | Writes `.mcp.json` in the project ([source](../scripts/hub-smoke.sh#L103-L105)). | Writes the same file ([source](../scripts/hub-integration-test.sh#L69-L70)). |
| Merged MCP config | — | `hub-merged.mcp.json` in the project ([source](../scripts/hub-integration-test.sh#L874-L896)). |
| Optional HTTP smoke | — | `mt-server.log`, `mt-body.json`, and `mt-init.headers` in the project when the multi-tenant check runs ([source](../scripts/hub-integration-test.sh#L967-L989), [server](../scripts/hub-integration-test.sh#L1012), [headers](../scripts/hub-integration-test.sh#L1058)). The server is stopped normally and also signaled by the exit trap. |
| Canary fixtures | — | `canary scan/nested dir/clean file` and `planted file` under the project. The fixture tree is removed before scanning the home and project ([source](../scripts/hub-integration-test.sh#L1286-L1312)); the exit trap removes any remainder. |
| Scan listings | — | Each scan writes a NUL-delimited listing at `${TMPDIR:-/tmp}/.locus-hub-int-scan.$$.${RANDOM}`. It is removed after the scan or on handled errors; the exit trap removes matching listings for this shell PID ([source](../scripts/hub-integration-test.sh#L45-L47), [scan](../scripts/hub-integration-test.sh#L82-L111)). This path is constructed by the shell, not `mktemp`. |

Locus commands create additional state inside each home; removing the home
removes remaining state. On Unix, the authority broker also uses a socket
under `/tmp/locus-authority-<uid>/`, outside these homes. Its retirement is
asynchronous and best effort; the scripts do not remove the shared directory.
See the [smoke resource map](hub-smoke-resource-map.md) for broker and grant
lifecycles. The integration script invokes separate Node test suites before
its own allocations; their fixtures are managed by those tests, not by its
exit trap ([source](../scripts/hub-integration-test.sh#L29-L30)). Cargo may
also create persistent build artifacts in `target/`, which neither trap removes.

Run `node --test scripts/hub-test-temp-comparison.test.mjs` to check the
`mktemp` inventory against both scripts. The existing
`scripts/hub-smoke.test.mjs` and `scripts/hub-integration-test.test.mjs` exercise
cleanup and failure paths.

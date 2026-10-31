# Hub smoke resource map

[`scripts/hub-smoke.sh`](../scripts/hub-smoke.sh) checks local CLI contracts
using sample state. Run it from the repository root with
`./scripts/hub-smoke.sh`; see [prerequisites](hub-smoke-prerequisites.md) and
[output guide](hub-smoke-output.md). The table follows the script's execution
order. Paths under `TMPDIR` use `/tmp` when it is unset or empty.

| Temporary resource | Creation | Use | Cleanup |
| --- | --- | --- | --- |
| `SMOKE_HOME` (`locus-hub-smoke.XXXXXX` directory) | First `mktemp -d`; exported as `LOCUS_HOME` before initialization. | Isolates Locus state from the caller's home. | `cleanup` removes it recursively in the `EXIT` trap, including remaining contents. |
| `SMOKE_PROJ` (`locus-hub-proj.XXXXXX` directory) | Second `mktemp -d`. | Working directory for report, status and grant checks. | `cleanup` removes it recursively in the `EXIT` trap. |
| `SMOKE_ERR` (`locus-hub-smoke-err.XXXXXX` file) | Third `mktemp`. | Truncated for each `smoke_init_and_pin` attempt; captures init/pin stderr for retry diagnostics. | `cleanup` removes it in the `EXIT` trap. |
| Sample bindings, keys, audit and runtime state | `locus init --with-samples` and later Locus commands write under `SMOKE_HOME`. | The `personal` binding supports pin, report, doctor and grant checks; runtime and audit state support control operations. | Remaining state is removed with `SMOKE_HOME`. |
| Authority broker and Unix socket | Init/pin control operations start a supervised broker; its runtime metadata is under `SMOKE_HOME`, and its socket is under `/tmp/locus-authority-<uid>/` on Unix. | Authenticates local control and session authority during the smoke. | The broker notices home removal or supervisor exit and retires its socket; socket removal is best effort. The shared per-user socket directory is not removed by the script. |
| Active pin and worker state | `locus pin personal` inside `smoke_init_and_pin`. | Pinned JSON and oneline status checks. | `locus leave` is attempted between retries and before the unpinned report; errors are ignored. Remaining files are removed with `SMOKE_HOME`. |
| `.mcp.json` | Written in `SMOKE_PROJ` after `cd`. | Lets `agent report` detect a Locus MCP registration; it does not start an MCP server. | Removed with `SMOKE_PROJ`. |
| MCP grant, delegated session and bearer token | If `locus mcp mint --help` succeeds, `locus mcp mint --binding personal --ttl 5m --label hub-smoke --json` creates a grant and delegated session under `SMOKE_HOME` and captures the token in `MINT_JSON`. | `jq` validates the mint response in memory; `mcp list` checks the grant roster without tokens. No MCP server is started. | `locus mcp revoke` attempts to remove the grant and delegated session; any remaining files are removed with `SMOKE_HOME`. Shell variables disappear when the script exits. The session TTL is at most five minutes. |
| Control capability | Reuses a nonempty `LOCUS_CONTROL_CAPABILITY` or generates one from `/dev/urandom` in the script environment. | Authorizes local control operations and broker startup. | The script's environment disappears on exit; a caller-supplied value is not revoked or changed in the caller. |

The trap is installed before the first allocation, so ordinary exits and setup
failures remove paths already assigned. It cannot run after an uncatchable kill,
and removal can fail; inspect leftovers using the temporary path printed at
startup when applicable. Broker socket retirement is asynchronous and is not
performed by the shell trap. The optional grant checks are skipped if their help
probe fails; a successful smoke alone does not prove they ran. Cargo may build
`target/debug/locus` and other artifacts in `target/`; these are persistent
build outputs, not removed by the trap. No real credentials are needed.

Run `node --test scripts/hub-smoke.test.mjs` to check the map against the
script's allocations and lifecycle markers, as well as the existing smoke
cleanup tests. This does not prove cleanup after an uncatchable interruption.

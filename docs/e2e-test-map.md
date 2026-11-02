# End-to-end test map

[`scripts/e2e.sh`](../scripts/e2e.sh) exercises release binaries against synthetic
bindings and credentials in isolated temporary homes. Run the whole suite from
the repository root:

```sh
./scripts/e2e.sh
```

The script builds `locus-cli` and `locus-mcp` in release mode with `--locked`.
It needs the Rust toolchain, Bash, Python 3, Git and standard shell utilities;
`curl` is required for the dashboard check when that command is available and
for the optional HTTP checks. Loopback servers need available ports. The default
binary paths are `target/release/locus` and `target/release/locus-mcp`;
`LOCUS_BIN` and `MCP_BIN` can select executable alternatives, but the release
build still runs. `TMPDIR`, if set and nonempty, must be an existing writable
directory. The script creates its own `LOCUS_HOME`, uses synthetic values, and
normally cleans up temporary homes, workspaces and HTTP servers on exit. See the
[temporary resource reference](e2e-temp-resource-types.md) for directory types
and cleanup limitations, [graph import isolation](e2e-graph-isolation.md), and
the [GRAPH_HOME lifecycle](e2e-graph-lifecycle.md)
for the fresh-home fallback.

The numbers below match the script's `==>` log labels, in execution order. There
is no section selector: invocations in the table describe what the suite runs,
not independent commands. Sections share pin and fixture state, so running a
fragment alone is not equivalent to running the suite.

| Section | Invocation in the suite | Purpose |
| --- | --- | --- |
| 1 | `cargo build --release -p locus-cli -p locus-mcp --locked` | Build and check both release executables. |
| 2 | `mktemp`, cleanup trap | Create an isolated Locus home and arrange cleanup. |
| 2c | Security home creation under a custom `TMPDIR` | Check the security fixture respects a path with spaces and is removed on exit. |
| 2a | `quickstart`, `doctor`, `doctor --json` | Check fresh-install advisories and reject invalid workspace policy. |
| 2b | `binding add` twice, then with conflicting content | Verify an identical add preserves the file and timestamp and a conflict is rejected. |
| 3 | `init --with-samples`, `pin`, `whoami --json`, `exec` | Check sample pins, exclusive identity, credential presence metadata, binding add idempotence and missing-reference warnings. |
| 3a | `autopin suggest` and `--json` | Check empty history returns no suggestion without changing the pin, bindings or workspace. |
| 3b | `autopin suggest --json` with changed Git remotes | Check a secondary remote cannot spoof another repository's pin history. |
| 4 | `pin` under workspace `allowed_bindings`, with and without `--force` | Check the workspace allowlist and force override. |
| 5 | `exec`, `run`, `ci run`, and upstream fixtures | Check ambient identity scrubbing, pinned credential injection, `--no-resolve` restrictions and exact-session confinement. |
| 6 | MCP stdio `initialize` and `tools/list` | Compare the unpinned control-only catalog with pinned provider tools. |
| 7 | MCP `tools/call` with a different `project_ref` | Check scope freeze rejects an override. |
| 8a | MCP destructive `tools/call` under read-only scope | Check hard denial occurs without minting an approval. |
| 8b | MCP destructive call, `approve grant`, retry | Check local approval is advisory and does not release authority. |
| 9 | `doctor --json`, pinned and unpinned | Check report structure and verdict/exit-code consistency. |
| 10 | Dual-control policy, `approve grant` and MCP retries | Check local labels, Touch ID mock outcomes and forged external labels cannot authorize the call; approval CLI is feature-detected. |
| 11 | `events --last`, `--op`, `--json` | Check audit export, filtering and text output when available. |
| 12 | `enter`, `leave`, `status --oneline` | Exercise optional enter/leave workflows and resulting status. |
| 13 | `run -b` (with fallbacks) | Exercise an optional one-shot child session. |
| 13b | `pin --help` | Detect the optional `--ns` flag; this is a help check, not a multi-bind execution. |
| 14 | `leave`, `status`, MCP `tools/list` | Check leaving returns to an unpinned, control-only catalog. |
| 15 | `notify status` | Check notifications default to disabled when the command exists. |
| 16 | `graph export`, `graph import` | Check optional encrypted graph export/import, including a fresh-home fallback. |
| 17 | `ci mint --json` | Check an optional CI session's identity fields and secret-free output. |
| 18 | MCP `locus_heartbeat` | Check the optional control tool returns a secret-free heartbeat. |
| 19 | `graph` and `ci` help, README search | Check the README mentions each available CLI surface. |
| 20 | `serve`, HTTP `GET /api/health` | Check dashboard health if serve/dashboard is advertised; a server that fails to become healthy fails the suite. |
| 21 | `forensics export` | Check an optional export has recognizable structure and omits secrets and credential locators. |
| 22 | `goal status` | Check optional progress output in JSON or text. |
| 23 | `topic` / `help topic dashboard` | Smoke-test optional topic help. |
| 24 | Release CLI credential and workspace probes | Check conflicting adds, rejection of `test:` credentials, legacy migration, Phantom fail-closed behavior, non-disclosure and broken workspace links. |
| 25 | `verify claim --json` | Check optional claim scoring and secret hygiene. |
| 26 | `verify session --json` | Check optional session report shape, exit status and secret hygiene. |
| 26b | `watch --once --json`, `--require-ok` | Check optional session heartbeat shape and fail-closed status. |
| 27 | MCP `locus_safe_next` | Check optional next-action responses pinned and unpinned without secret values. |
| 28 | `upstream list` | Check optional recipe listing, secret hygiene and security defaults. |
| 29 | `notify status` | Recheck that notifications remain disabled after the intervening suite. |
| 30 | `locus-mcp --http`, HTTP MCP and SSE | Check token authentication, session initialization and tool listing; probe SSE when available. |
| 31 | `locus mcp mint`, `locus-mcp --http --multi-tenant` | Check tenant isolation, cross-tenant rejection, grant revocation and a values-free roster. |

A failure exits nonzero at the first failing check. A successful run ends with
`e2e PASS` and counts of passed and skipped checks; it does **not** imply every
optional surface ran. Feature detection, incompatible optional invocations,
HTTP server startup failures and some SSE responses can produce skips. Read the
individual `skip` messages when assessing coverage. Dashboard startup failure,
in contrast, is a failure once its command is detected.

To check that this map still covers the script's logged sections, run
`node --test scripts/e2e-map.test.mjs`. This checks section coverage and ordering;
it does not replace the end-to-end suite or validate every assertion.

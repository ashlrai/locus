# Hub integration smoke prerequisites

[`scripts/hub-integration-test.sh`](../scripts/hub-integration-test.sh) exercises
the local Locus–Phantom integration contract. It checks agent reports and pin
status, schemas and helper functions, session validation and environment
scrubbing, MCP configuration, and a filesystem canary. Its multi-tenant section
also exercises grant minting, HTTP access, isolation, revocation, and expiry.

## Services and tools

- No running Phantom service, Phantom installation, external provider, real
  account, credentials, or existing Locus pin is required. The script creates a
  sample `personal` binding and starts the Locus authority broker as needed.
- For the multi-tenant checks, the script starts and stops `locus-mcp` itself on
  `127.0.0.1` using a randomly selected port in the range 20000–39999. Local
  loopback access and an available port are required; a port collision can fail
  the run. No remote service is contacted by these checks.
- Use Bash, standard Unix utilities, `jq`, and Node.js with
  `--experimental-strip-types` support (Node 22.6 or later). The Node tests use
  built-in modules; no npm install is needed.
- Have the Rust toolchain and Cargo available to build the checkout's debug
  `locus` and `locus-mcp` binaries. `curl` is needed for the multi-tenant
  checks. Prebuilding both binaries avoids accidentally using an older binary
  already on `PATH` and ensures that section can run.
- Provide writable temporary storage and a path short enough for the local
  broker's Unix socket. Cargo may need network access to fetch dependencies
  when they are not already cached.

## Environment

No environment variable needs to be set for a normal run. The script prepends
the checkout's debug and release binary directories and `$HOME/.cargo/bin` to
`PATH`; Cargo must also be discoverable if a build is needed. It accepts these
optional inputs:

| Variable | Behavior |
| --- | --- |
| `TMPDIR` | Parent for two temporary directories and canary scan listings; defaults to `/tmp`. |
| `LOCUS_CONTROL_CAPABILITY` | Uses a nonempty inherited value, otherwise generates a random capability for the run. |
| `HUB_INT_AMBIENT_CANARY` | Uses a nonempty inherited canary, otherwise generates a synthetic non-secret value. Use only a synthetic value: the scan deliberately writes it to a temporary test file. |

The script replaces `LOCUS_HOME` with a temporary directory and unsets an
inherited `LOCUS_SESSION_ID`. It generates its own HTTP server token for the
multi-tenant section. The exit trap removes its temporary home and project and
stops the server when started. The run does not modify the caller's normal
Locus home; it does print the temporary home path and progress messages. Do not
enable shell tracing or print minted session or grant values.

## Run and interpret results

From the repository root:

```bash
cargo build -p locus-cli -p locus-mcp
./scripts/hub-integration-test.sh
```

The script exits **0** and prints `== hub-integration-test OK ==` when its
executed checks pass. Missing `jq` or Node exits **1**; checks recorded as
`FAIL` lead to exit **1** and the `FAILED` marker. Because Bash uses
`set -euo pipefail`, an unexpected command or setup failure may instead exit
early with that command's nonzero status and without the final marker.

The multi-tenant section is feature-detected: missing `curl`, an older CLI
without `locus mcp mint`, or an unavailable `locus-mcp` after an attempted
build produces a `skip  multi-tenant MCP smoke` line. This skip does **not**
make the script fail. For complete verification, require exit 0, the `OK`
marker, and no `skip` line; fix the prerequisite and rerun if it skipped.
Failures in the local server or its checks are failures, not skips. See the
[integration guide](hub-integration.md) for the production contract.

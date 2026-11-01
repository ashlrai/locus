# Dogfood probe purposes

[`scripts/dogfood.sh`](../scripts/dogfood.sh) uses two independent `probe_root`
directories to exercise security boundaries without using the main dogfood pin
or home. Both are created with `mktemp -d` under `${TMPDIR:-/tmp}`; the parent
must exist and be writable. Each probe uses a separate `LOCUS_HOME` and runs in
a subshell, so its working-directory and environment changes do not persist.

## Upstream worker sandbox attestation

Step 1b creates `locus-sandbox-dogfood.XXXXXX`, with a home, project, local
MCP fixture and captured MCP output. It creates and pins a synthetic binding,
configures the fixture as a sandboxed upstream worker with secret resolution
disabled, and calls it through `locus exec --no-resolve` and `locus-mcp`.
The shell fixture performs no credential lookup or network I/O. The probe also
checks that the credential locator does not appear in the captured output.

On macOS, the call must return the worker markers `sandboxed=1` and
`backend=sandbox-exec`. On other platforms the probe checks that an unsupported
OS isolation backend refuses the worker fail-closed; even that expected refusal
ends the run without `DOGFOOD READY`. A failed attestation also blocks readiness.

The root is allocated before entering the subshell. Its `EXIT` trap removes the
root and everything inside it, including the fixture and captured output. An
allocation failure returns before the subshell starts.

## Multi-tenant grant lifecycle

Step 8b runs only with `DOGFOOD_MT=1`. It creates
`locus-mt-dogfood.XXXXXX` inside a subshell, changes into that root to avoid a
caller's workspace allowlist, and initializes a synthetic binding in its own
home. It mints a short-lived grant, checks that its output contains no credential
locator, and confirms the grant is live in the operator roster. Against a local
multi-tenant HTTP server, it expects a tenantless request to receive 401 with
`invalid_grant`, an initialization with the grant to receive 200, and the same
grant to receive 401 after revocation. Finally, it checks that the roster no
longer lists the grant as live. When enabled, failure blocks readiness.

On subshell exit, the cleanup trap attempts to kill and wait for the recorded
server PID, changes to `/`, and removes the root, including the home, server log
and saved response. Kill, wait and directory-change errors are ignored. Grant
revocation is part of the normal probe sequence, not the trap; an earlier
failure can bypass it, although cleanup still attempts to remove the grant's
isolated state.

Both traps cover normal exits and handled failure paths after installation.
Uncatchable termination, termination before trap installation, or failed
removal can leave artifacts; neither trap cleans resources outside its root.
For more detail, see [probe cleanup](dogfood-probe-cleanup.md) and the
[directory roles](dogfood-probe-roles.md). Run
`node --test scripts/dogfood.test.mjs` for the probe regression checks.

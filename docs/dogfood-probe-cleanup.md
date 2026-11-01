# Dogfood probe cleanup

[`scripts/dogfood.sh`](../scripts/dogfood.sh) gives its sandbox attestation
and optional multi-tenant grant probe separate temporary roots under
`${TMPDIR:-/tmp}`. The parent directory must already exist and be writable.
Both probes use subshells and install an `EXIT` trap to remove their own root,
including the separate `LOCUS_HOME` and files created inside it. Their changes
to the working directory and environment do not carry back to the caller.

| Probe | Allocation and contents | Exit cleanup |
| --- | --- | --- |
| Sandbox attestation (step 1b) | Allocates `locus-sandbox-dogfood.XXXXXX` before entering its subshell, returning if allocation fails; its home, project, fixture, and captured MCP stdout/stderr are inside the root. | The subshell copies the root to a variable available to its `EXIT` trap, which runs `rm -rf` on it. It has no explicit server-stop or grant-revocation step. |
| Multi-tenant grant (step 8b, `DOGFOOD_MT=1`) | Allocates `locus-mt-dogfood.XXXXXX` inside its subshell, aborting if allocation fails. Its home, server log and tenantless response body are inside the root; the probe also changes into it. | The trap sends `kill` to the recorded server PID if present, waits for it, changes to `/`, and removes the root. Kill, wait, and `cd` errors are ignored; removal is attempted afterwards. |

The multi-tenant probe revokes its grant and verifies the revoked response and
roster as part of the normal probe sequence, **not** in its exit trap. An early
failure can therefore bypass explicit revocation; cleanup still attempts to
remove the isolated home that holds the grant state. The sandbox probe has no
corresponding grant lifecycle. Neither probe's trap removes the main dogfood
home or a separately configured forensic pack; those have their own lifecycle
as described in the [probe roles guide](dogfood-probe-roles.md).

The traps run when their subshells exit normally or through handled failure
paths, including early exits after the traps are installed. The script does not
install signal-specific handlers for these probes. A regression test sends
SIGTERM directly to the multi-tenant probe subshell after its server starts and
checks that the trap stops the server and removes the root. This does not
establish cleanup when only the parent receives a signal. An uncatchable termination,
termination before trap installation, or a failed removal can leave artifacts
behind. Inspect leftovers before deleting them; a trap is not a guarantee that
every child process or file was removed. Build artifacts and resources outside
the probe roots are outside these traps.

Run `node --test scripts/dogfood.test.mjs` for the probe cleanup regression
checks.

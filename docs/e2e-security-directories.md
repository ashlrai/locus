# E2E security directories

[`scripts/e2e.sh`](../scripts/e2e.sh) runs its adversarial release security
probes in section 24 using two temporary directories. They are separate from
both the suite's main Locus home and its allowlist workspace, so these probes
can create and mutate fixtures without changing the other sections' state or
the user's Locus home. The script builds and runs release binaries; it uses
synthetic credential references and values, not live credentials.

| Script variable | Directory prefix | Role |
| --- | --- | --- |
| `SECURITY_HOME` | `locus-security-e2e.` | Passed as `LOCUS_HOME` to initialize a fresh Locus store. Holds the probe bindings, original binding copy, migration journal and audit events used by credential and disclosure checks. |
| `SECURITY_WS` | `locus-security-ws.` | Working directory for workspace-policy checks. Contains a `.locus.toml` symlink to a missing target, so the script can check that pin and run fail even with `--force` and that doctor reports `UNSAFE`. |

Both are created with `mktemp -d` directly beneath a nonempty `TMPDIR`, or
`/tmp` when it is unset or empty. A supplied `TMPDIR` must already exist and
be writable; the suite verifies both fixtures sit directly beneath it before
the probes run, the same containment check applied to the main home and the
allowlist workspace. A misplaced fixture stops the suite while the exit trap
still removes it. These variables are assigned by the script; they are not
inputs for choosing existing directories. The probes use `SECURITY_HOME` as
`LOCUS_HOME` explicitly, including when commands run from `SECURITY_WS`.

The security home supports checks that identical binding additions are
idempotent and conflicting additions preserve the existing binding; release
binaries reject `test:` references even with the legacy opt-in; legacy
references require an explicit, durable migration; unsupported Phantom
resolution fails closed; and locator names are omitted from CLI, audit, CI and
child-environment surfaces. The child-environment probe also checks that the
synthetic resolved credential reaches the provider-standard key. The workspace
probe uses the environment-reference binding because the legacy Phantom
reference fails credential preflight before workspace policy is inspected.

After the main home is created, the script installs an `EXIT` trap. It stops
any tracked background MCP servers and removes the security home and workspace
alongside the other suite-owned temporary directories, on normal exit or an
ordinary failure. The security directories are created at the start of section
24 and remain until that trap runs. Later in section 24 the script creates the
workspace twice more in the same shell and requires the pair to be distinct
directories directly beneath the selected root. The trap removes only the
workspace `SECURITY_WS` currently names, so the live workspace is restored
first and both extra directories are removed explicitly; the script then
checks that they are gone. In section 2c a subshell creates a
separate security home and workspace inside a custom temporary directory whose
name contains spaces. It verifies that they are distinct; its exit trap removes
both, and the parent checks their removal and preservation of the supplied
temporary directory. Sections 2e and 2c2 rehearse the security workspace and
the security home separately, each beneath its own supplied root whose name
contains spaces, with the same exit-trap cleanup.

A termination that prevents the trap from running or a removal error can leave
files behind. A failure before trap installation can leave the main home, but
the security directories have not yet been created. Inspect any leftover
`locus-security-e2e.*` and `locus-security-ws.*` directories before removing
them; do not remove the parent `TMPDIR` or unrelated files. Run
`node --test scripts/e2e-security-home-selection.test.mjs` for the focused
`SECURITY_HOME` temporary-root selection and containment checks. See the
[temporary resource reference](e2e-temp-resource-types.md) for all directory
types and the [test map](e2e-test-map.md) for the full suite and prerequisites.

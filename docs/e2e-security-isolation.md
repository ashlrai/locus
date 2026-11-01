# E2E security isolation fixtures

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
be writable. These variables are assigned by the script; they are not inputs
for choosing existing directories. The probes use `SECURITY_HOME` as
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
ordinary failure. The security directories are created later, immediately
before section 24, and remain until that trap runs. In section 2c a subshell
creates a *different* security home inside a custom temporary directory whose
name contains spaces; its own exit trap removes that home, and the parent
checks both its removal and preservation of the supplied temporary directory.

A failure before trap installation, termination that prevents the trap from
running, or a removal error can leave files behind. Inspect any leftover
`locus-security-e2e.*` and `locus-security-ws.*` directories before removing
them; do not remove the parent `TMPDIR` or unrelated files. See the
[temporary resource reference](e2e-temp-resource-types.md) for all directory
types and the [test map](e2e-test-map.md) for the full suite and prerequisites.

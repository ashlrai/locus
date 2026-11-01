# Dogfood temporary-directory prefixes

[`scripts/dogfood.sh`](../scripts/dogfood.sh) creates these directories under
`${TMPDIR:-/tmp}` using `mktemp -d`. The six `X` characters in each template
are replaced with a unique suffix. These prefixes describe directories created
by this script itself; optional helper scripts may create their own temporary
files or directories.

For probe behavior and readiness effects, see the
[dogfood probe roles](dogfood-probe-roles.md).

| Template | Creation context | Cleanup |
| --- | --- | --- |
| `locus-dogfood.XXXXXX` | Default isolated run: the directory is `LOCUS_HOME`, contains the throwaway project and, by default, `pack.json`. It is not created when `DOGFOOD_USE_REAL_HOME=1`. | The main script's exit trap removes it. A custom `DOGFOOD_PACK` path is outside this cleanup unless placed inside the directory. |
| `locus-sandbox-dogfood.XXXXXX` | Sandbox attestation (step 1b): contains a separate `home`, project, local MCP fixture, and captured MCP output. The probe runs independently of the main dogfood home. | The probe's subshell exit trap removes it. |
| `locus-mt-dogfood.XXXXXX` | Optional multi-tenant grant probe (step 8b), when `DOGFOOD_MT=1`: contains a separate `home` and working directory for the mint, verify, and revoke lifecycle. | The probe's subshell exit trap stops its server and removes the directory. |

Exit traps handle normal exit and trappable failures; an uncatchable termination
can leave a directory behind. Inspect any leftover directory before removing it,
especially if a run wrote diagnostic artifacts there.

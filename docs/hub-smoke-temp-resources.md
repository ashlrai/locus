# Hub smoke temporary resources

From the repository root, `./scripts/hub-smoke.sh` creates three temporary
paths directly under `TMPDIR`, or `/tmp` when `TMPDIR` is unset or empty. A
nonempty `TMPDIR` must already exist and be writable. `XXXXXX` is replaced by
`mktemp` with a unique suffix; the prefixes below include the final dot.

| Variable | Kind | Prefix and template | Purpose |
| --- | --- | --- | --- |
| `SMOKE_HOME` | Directory | `locus-hub-smoke.` (`locus-hub-smoke.XXXXXX`) | Exported as `LOCUS_HOME` for sample bindings, session and runtime state. |
| `SMOKE_PROJ` | Directory | `locus-hub-proj.` (`locus-hub-proj.XXXXXX`) | Working project containing the `.mcp.json` registration used by the report checks. |
| `SMOKE_ERR` | Regular file | `locus-hub-smoke-err.` (`locus-hub-smoke-err.XXXXXX`) | Captures init/pin stderr; it is truncated before each retry and excerpts can be printed on failure. |

The first two paths are created with `mktemp -d`; the error file uses
`mktemp` without `-d`. The script installs an `EXIT` trap before allocating
them and removes each successfully assigned path, including on ordinary setup
failure. The trap cannot run after an uncatchable kill, and removal can fail.
The home path is printed only after setup and build prerequisites succeed, so
it may not appear in output from an earlier failure. Treat diagnostics as
potentially sensitive.

The trap does not remove Cargo's persistent `target/` build outputs or the
shared per-user authority socket directory; broker socket retirement is
asynchronous. See the [resource map](hub-smoke-resource-map.md) for other
resources and their lifecycles, the [prerequisites](hub-smoke-prerequisites.md)
for setup, and the [output guide](hub-smoke-output.md) for failure messages.

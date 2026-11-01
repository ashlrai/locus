# Hub smoke temporary-resource failures

[`scripts/hub-smoke.sh`](../scripts/hub-smoke.sh) creates three paths in
sequence under a nonempty `TMPDIR`, or `/tmp` if it is unset or empty. The
parent must already exist and be writable. Each `mktemp` failure prints its
own diagnostic to stderr and exits 1 immediately, before prerequisites, build,
init/pin, or smoke checks. `mktemp` itself may also print a diagnostic.

| Failed allocation | Script diagnostic | Earlier paths passed to cleanup | Later allocations attempted |
| --- | --- | --- | --- |
| `SMOKE_HOME` (`mktemp -d`, `locus-hub-smoke.XXXXXX`) | `error: hub-smoke could not create temporary home` | None | None |
| `SMOKE_PROJ` (`mktemp -d`, `locus-hub-proj.XXXXXX`) | `error: hub-smoke could not create temporary project directory` | `SMOKE_HOME` | None |
| `SMOKE_ERR` (`mktemp`, `locus-hub-smoke-err.XXXXXX`) | `error: hub-smoke could not create temporary error file` | `SMOKE_HOME`, `SMOKE_PROJ` | None |

The script initializes all three variables to empty strings and installs
`trap cleanup EXIT` before the first allocation. On exit, `cleanup` runs
`rm -rf --` with all three quoted variables; successfully allocated paths are
removed recursively, including their contents. Empty variables are also passed
as arguments. The parent directory and unrelated files are not targeted. This
same trap runs on ordinary later exits, including missing prerequisites,
init/pin failure, failed checks, and success. An allocation failure occurs before
the startup banner, so it does not print the temporary home path or a final
`OK`/`FAILED` banner.

Cleanup is best effort: removal can fail, and an uncatchable termination such
as `SIGKILL` prevents the trap from running. If leftovers are suspected, inspect
the chosen temporary parent and remove only paths confirmed to belong to the
failed invocation; another invocation may be using similarly named paths. The
trap does not remove Cargo's `target/` artifacts or the shared per-user authority
socket directory; broker socket retirement is asynchronous. Avoid sharing
failure diagnostics without checking them for sensitive information.

For the path roles and later lifecycle, see the [temporary path lifecycle](hub-smoke-temp-lifecycle.md)
and [resource map](hub-smoke-resource-map.md). The [output guide](hub-smoke-output.md)
covers other failure messages.

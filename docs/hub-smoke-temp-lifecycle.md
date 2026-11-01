# Hub smoke temporary path lifecycle

[`scripts/hub-smoke.sh`](../scripts/hub-smoke.sh) creates three paths in
order, directly under `TMPDIR` when it is nonempty or `/tmp` otherwise. A
nonempty `TMPDIR` must already exist and be writable. `mktemp` replaces each
`XXXXXX` with a unique suffix. These paths belong to one invocation.

| Order | Path | Creation and use | Cleanup point |
| --- | --- | --- | --- |
| 1 | `SMOKE_HOME`: directory `locus-hub-smoke.XXXXXX` | The first `mktemp -d` creates the home; the script later exports it as `LOCUS_HOME` for sample bindings, session and runtime state. | The `EXIT` trap recursively removes the directory and its contents. |
| 2 | `SMOKE_PROJ`: directory `locus-hub-proj.XXXXXX` | The second `mktemp -d` creates the project; after init and pin the script enters it and writes `.mcp.json` for the report checks. | The `EXIT` trap recursively removes the directory and its contents. |
| 3 | `SMOKE_ERR`: file `locus-hub-smoke-err.XXXXXX` | `mktemp` creates the file for init/pin stderr. Each attempt truncates it; the redirection for a command also truncates it when that command runs. Failure diagnostics can print an excerpt. | The `EXIT` trap removes the file. |

Before the first allocation, the script initializes the path variables to
empty strings and installs `trap cleanup EXIT`. On a normal exit, including a
failed allocation, missing prerequisite, failed init/pin, or failed check,
`cleanup` runs `rm -rf --` on the three assigned paths. If an allocation fails,
the script exits immediately: only earlier successful allocations exist to be
removed. It does not remove the parent `TMPDIR` or unrelated paths.

Cleanup is best effort: `rm` can fail, and an uncatchable termination such as
`SIGKILL` cannot run the trap. The trap also does not remove Cargo's `target/`
build artifacts or the shared per-user authority socket directory; broker socket
retirement is asynchronous. Treat failure diagnostics as potentially sensitive.
See the [resource map](hub-smoke-resource-map.md) for the broker, grant and
other resources, and the [output guide](hub-smoke-output.md) for error messages.

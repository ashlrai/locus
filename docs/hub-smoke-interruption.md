# Hub smoke interruption and cleanup

[`scripts/hub-smoke.sh`](../scripts/hub-smoke.sh#L23-L40) initializes three
empty path variables and installs **one trap**, `trap cleanup EXIT`, before
allocating anything. It installs no `INT`, `TERM`, or other signal-specific
trap. On shell exit, `cleanup` runs `rm -rf --` on the assigned paths:

| Path | Resource removed by the EXIT trap |
| --- | --- |
| `SMOKE_HOME` | The temporary Locus home and its remaining contents, including sample bindings and runtime state. |
| `SMOKE_PROJ` | The temporary project and its contents, including the `.mcp.json` written [after pinning](../scripts/hub-smoke.sh#L103-L105). |
| `SMOKE_ERR` | The temporary file used for init/pin stderr. |

The paths are created in that order under a nonempty `TMPDIR`, or `/tmp`
otherwise. An ordinary exit, including a setup failure, runs the trap for paths
already assigned. When Bash exits in response to a catchable signal such as
`SIGTERM`, its `EXIT` trap also runs; signal delivery and timing depend on how
Bash is invoked and whether it is waiting for a foreground command. In
particular, do not assume Ctrl-C always reaches the script or immediately stops
its children. An interruption between `mktemp` creating a path and the shell
assigning its result can leave an unrecorded path. `SIGKILL`, a system crash,
or a failed `rm` can also leave resources behind. Cleanup is best effort.

The trap removes only those three paths; it does not call `locus leave` or
`locus mcp revoke`, wait for child processes, remove Cargo's `target/`, or
remove the shared authority socket directory. The script attempts leave and
revoke at specific later points during normal execution, so an interruption
may bypass them ([leave](../scripts/hub-smoke.sh#L185-L189),
[revoke](../scripts/hub-smoke.sh#L222-L232)). Remaining on-disk state under the
home goes with it; broker socket retirement is separate and asynchronous. See
the [resource map](hub-smoke-resource-map.md) for those resources.

After an unclean interruption, inspect the invocation's temporary parent for
its `locus-hub-smoke.*`, `locus-hub-proj.*`, and `locus-hub-smoke-err.*`
paths. Verify ownership and that the invocation has stopped before manually
removing specific leftovers; other concurrent runs may use the same parent.
The startup banner identifies the home only after setup reaches it, so it may
not be available after an early interruption. Do not remove the temporary parent
or unrelated paths. See [allocation failures](hub-smoke-temp-failures.md) for
partial setup and [temporary path lifecycle](hub-smoke-temp-lifecycle.md) for
normal cleanup.

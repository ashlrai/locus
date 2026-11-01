# Hub integration smoke temporary lifecycle

[`scripts/hub-integration-test.sh`](../scripts/hub-integration-test.sh) runs
the Phantom composition smoke against sample Locus state. Its two `mktemp -d`
allocations are direct children of `${TMPDIR:-/tmp}`: an unset or empty
`TMPDIR` uses `/tmp`; a nonempty one must already exist and be writable.
`XXXXXX` is replaced with a unique suffix.

| Directory | Creation and use | Cleanup |
| --- | --- | --- |
| `SMOKE_HOME` (`locus-hub-int.XXXXXX`) | The first `mktemp -d` creates it; the script exports it as `LOCUS_HOME` before `locus init --with-samples`. Locus writes sample bindings, keys, sessions, grants, audit and runtime state here. | `cleanup` recursively removes it and its remaining contents in the `EXIT` trap. |
| `SMOKE_PROJ` (`locus-hub-int-proj.XXXXXX`) | The second `mktemp -d` creates it. After initialization and pinning, the script changes into it and writes `.mcp.json`; later checks create `hub-merged.mcp.json`, optional HTTP response, header and log files, and canary fixtures here. | `cleanup` recursively removes it and its remaining contents in the `EXIT` trap, even when the shell is still inside the project. The canary fixture tree is also removed before the final scans. |

Both variables start empty, and the script installs the `EXIT` trap before
either allocation. With `set -e`, an allocation or later command failure
exits through that trap; if the second allocation fails, the first directory
is still removed. A failed check can instead set the failure flag and let the
remaining checks run before exit. The trap signals any recorded multi-tenant
server PID before deleting paths; on the normal healthy-server path the script
also kills and waits for the server earlier. The trap does not remove the
caller’s temporary parent directory.

Each canary scan also writes a NUL-delimited **file**,
`${TMPDIR:-/tmp}/.locus-hub-int-scan.$$.${RANDOM}`, outside those directories.
The scan removes its listing on completion and handled errors; the exit trap
removes remaining matching listings for its shell PID. These files are not
additional `mktemp` directories.

Cleanup is best effort: an uncatchable kill cannot run the trap, and removal
or server signaling can fail. On Unix the authority broker uses a socket under
`/tmp/locus-authority-<uid>/` outside the two directories; broker retirement
is asynchronous and best effort, and the script does not remove that shared
directory. The Node suites run before these allocations manage their own
fixtures, and any Cargo build artifacts in `target/` persist. See the
[temporary resource comparison](hub-test-temp-comparison.md) for both hub
scripts and the [smoke prerequisites](hub-smoke-prerequisites.md) for setup.

Run `node --test scripts/hub-integration-temp-lifecycle.test.mjs` to check the
documented allocation inventory and lifecycle markers against the script.
`node --test scripts/hub-integration-test.test.mjs` exercises cleanup on normal
and injected failure paths, including paths with spaces and a live server.

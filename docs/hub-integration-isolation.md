# Hub integration smoke isolation

[`scripts/hub-integration-test.sh`](../scripts/hub-integration-test.sh) runs
the Phantom composition smoke with sample Locus state in two separate temporary
directories. It does not use the caller's `~/.locus` as its Locus home. This is
test isolation, not a general sandbox for every process the script runs.

## Allocation and use

After prerequisite checks, Node tests and any initial CLI build, the script
[installs its cleanup trap and allocates both directories](../scripts/hub-integration-test.sh#L41-L54).
They are direct children of `${TMPDIR:-/tmp}`: an unset or empty `TMPDIR` uses
`/tmp`; a supplied parent must exist and be writable. `mktemp -d` replaces
each `XXXXXX` with a unique suffix.

| Resource | Role in the smoke |
| --- | --- |
| `SMOKE_HOME` (`locus-hub-int.XXXXXX`) | Exported as `LOCUS_HOME` before `locus init --with-samples` and `locus pin personal`. It contains the sample binding, session, key, audit, grant and runtime state created by Locus. The script also unsets `LOCUS_SESSION_ID` before initialization. [Setup](../scripts/hub-integration-test.sh#L54-L67) |
| `SMOKE_PROJ` (`locus-hub-int-proj.XXXXXX`) | The script changes into this project and writes `.mcp.json`. The merge check writes `hub-merged.mcp.json`; when the multi-tenant HTTP check runs, it also uses `mt-server.log`, `mt-body.json` and `mt-init.headers` here. [Project setup](../scripts/hub-integration-test.sh#L69-L70), [merge](../scripts/hub-integration-test.sh#L874), [HTTP](../scripts/hub-integration-test.sh#L966) |

The [canary regression](../scripts/hub-integration-test.sh#L1286-L1317)
temporarily plants a synthetic canary in a path containing spaces under the
project, verifies detection, and removes that fixture before scanning both
the Locus home and project. A final scan fails if a canary is found, a tree is
empty, or its files cannot be listed or read. Each scan uses a separate
NUL-delimited listing file under `${TMPDIR:-/tmp}`; it is not a third temporary
directory. See the [scan implementation](../scripts/hub-integration-test.sh#L76-L126).

## Cleanup and boundaries

Both directory variables start empty and the `EXIT` trap is installed before
allocation, so a failed second allocation still triggers removal of the first.
On normal exit or a catchable failure, [`cleanup`](../scripts/hub-integration-test.sh#L41-L49)
signals any recorded HTTP server, removes this shell's leftover scan listings,
and recursively removes both directories. The healthy HTTP path also kills
and waits for the server before exit. The caller's temporary parent is retained.
Cleanup is best effort: an uncatchable kill cannot run the trap, and signaling
or removal can fail.

The script does not redirect the caller's `HOME`; only `LOCUS_HOME` is set to
the scratch home. Node prerequisite tests run before these directories exist
and manage their own fixtures, while Cargo build artifacts can persist in
`target/`. On Unix, the Locus authority broker uses a socket under
`/tmp/locus-authority-<uid>/` outside the two directories; broker retirement is
asynchronous and best effort, and the script does not remove that shared
directory. See the [lifecycle guide](hub-integration-temp-lifecycle.md) and
[comparison with the other hub smoke](hub-test-temp-comparison.md).

The lifecycle documentation checks are in
[`scripts/hub-integration-temp-lifecycle.test.mjs`](../scripts/hub-integration-temp-lifecycle.test.mjs);
[`scripts/hub-integration-test.test.mjs`](../scripts/hub-integration-test.test.mjs)
exercises cleanup on successful and injected failure paths.

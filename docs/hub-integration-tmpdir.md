# Hub integration test temporary paths

[`scripts/hub-integration-test.sh`](../scripts/hub-integration-test.sh) uses
`${TMPDIR:-/tmp}` as the parent of its own temporary paths. If `TMPDIR` is
unset **or empty**, Bash selects `/tmp`; otherwise it uses the supplied path.
The parent must already exist and allow the script to create files and
directories. Use an absolute path: the script changes working directory, so
a relative `TMPDIR` can resolve differently during scans and cleanup. Paths
containing spaces are supported.

| Resource | Template beneath the parent | Purpose |
| --- | --- | --- |
| `SMOKE_HOME` | `locus-hub-int.XXXXXX` | Temporary directory exported as `LOCUS_HOME` for sample Locus state. |
| `SMOKE_PROJ` | `locus-hub-int-proj.XXXXXX` | Temporary working directory for the integration project's files. |

Both directories are direct children of the parent. `mktemp -d` replaces
`XXXXXX` with a unique suffix. The script also constructs canary-scan listing
files beneath the same parent as `.locus-hub-int-scan.$$.${RANDOM}`; these are
files, not additional `mktemp` directories.

An `EXIT` trap attempts to remove both directories and any remaining scan
listings for the shell's PID, including when setup fails. It does not remove
the parent itself. Cleanup is best effort and cannot run after an uncatchable
kill. Other processes may create resources elsewhere: for example, the Unix
authority broker uses a socket under `/tmp/locus-authority-<uid>/`, and Cargo
build artifacts remain in `target/`. See the
[temporary lifecycle guide](hub-integration-temp-lifecycle.md) for details.

Run `node --test scripts/hub-integration-tmpdir.test.mjs` to check the fallback
and directory prefixes against the script. Run
`node --test scripts/hub-integration-test.test.mjs` for the full integration
smoke with a `TMPDIR` containing spaces and shell metacharacters; it observes
the directories and scan listings while they exist and checks their cleanup.

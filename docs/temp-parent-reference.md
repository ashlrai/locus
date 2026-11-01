# Hub scripts: temporary parent and prefixes

[`hub-smoke.sh`](../scripts/hub-smoke.sh) and
[`hub-integration-test.sh`](../scripts/hub-integration-test.sh) use
`${TMPDIR:-/tmp}` as the parent for the paths below. Bash selects `/tmp` when
`TMPDIR` is unset or empty; otherwise it uses the supplied value. For the
`mktemp` allocations, that parent must exist and permit creation of the
requested file or directory.

| Script | Variable | Type | Template beneath parent |
| --- | --- | --- | --- |
| `hub-smoke.sh` | `SMOKE_HOME` | directory | `locus-hub-smoke.XXXXXX` |
| `hub-smoke.sh` | `SMOKE_PROJ` | directory | `locus-hub-proj.XXXXXX` |
| `hub-smoke.sh` | `SMOKE_ERR` | file | `locus-hub-smoke-err.XXXXXX` |
| `hub-integration-test.sh` | `SMOKE_HOME` | directory | `locus-hub-int.XXXXXX` |
| `hub-integration-test.sh` | `SMOKE_PROJ` | directory | `locus-hub-int-proj.XXXXXX` |

The scripts pass these templates to `mktemp`; `-d` requests a directory and
the trailing `XXXXXX` supplies the variable part of the name. The integration
script also constructs a canary scan listing at
`${TMPDIR:-/tmp}/.locus-hub-int-scan.$$.${RANDOM}` using the shell's process ID
and `RANDOM` value, rather than `mktemp`.

Each script installs an `EXIT` trap that attempts to remove its assigned paths;
the integration trap also removes matching scan listings for its shell PID.
Cleanup is not guaranteed after an uncatchable termination or a removal error.
For lifecycle details and other resources, see the
[temporary resources comparison](hub-test-temp-comparison.md).

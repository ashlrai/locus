# Dogfood temporary resources

[`scripts/dogfood.sh`](../scripts/dogfood.sh) builds the checkout's debug
binaries and normally runs with a disposable Locus home. The directories below
are created directly under a nonempty `TMPDIR`, or `/tmp` when it is unset or
empty. `XXXXXX` is replaced by `mktemp` with a unique suffix. Set `TMPDIR` to
an existing writable directory if you need the scratch data elsewhere.

| Resource | Purpose and creation | Cleanup supported by the script |
| --- | --- | --- |
| `locus-dogfood.XXXXXX/` | Default isolated `LOCUS_HOME`; contains the synthetic binding, session and workers, a `project/` working directory, and the default `pack.json`. Created unless `DOGFOOD_USE_REAL_HOME=1`. | The main script's `EXIT` trap removes the entire directory, including the default pack, on normal exit or a handled failure. |
| `locus-sandbox-dogfood.XXXXXX/` | Separate sandbox attestation root, with its own `home/`, `project/`, fixture and MCP output files. | The probe subshell's `EXIT` trap removes the root on exit, including failed attestation. |
| `locus-mt-dogfood.XXXXXX/` | Separate home and working directory for the optional grant lifecycle and its HTTP server log and response file; created only when `DOGFOOD_MT=1` reaches that probe. | The probe subshell's `EXIT` trap stops and waits for its server if started, then removes the root, including on failure. |
| `locus-hub-smoke.XXXXXX/` | Isolated `LOCUS_HOME` for the Hub smoke helper. | [`hub-smoke.sh`](../scripts/hub-smoke.sh) removes it in its `EXIT` trap. |
| `locus-hub-proj.XXXXXX/` | Separate project and MCP registration for the Hub smoke helper. | The helper's `EXIT` trap removes it. |

The Hub helper also creates the temporary **file**
`locus-hub-smoke-err.XXXXXX` for init/pin errors and removes it in the same
trap. The helper runs unless `DOGFOOD_SKIP_HUB_SMOKE=1`; that diagnostic mode
cannot produce `DOGFOOD READY`. The optional client probe does not create a
temporary directory of its own.

The forensics export defaults to `pack.json` inside the disposable dogfood
home, so it disappears with that home. `DOGFOOD_PACK` selects another output
path; a pack outside the disposable home is retained and must be removed by
the operator when no longer needed. In real-home mode the default is
`/tmp/pack.json`, also retained. An explicit pack path inside the disposable
home is removed with that home.

With `DOGFOOD_USE_REAL_HOME=1`, the main disposable home is not created or
cleaned. The script unsets `LOCUS_HOME` and uses the normal Locus home unless
`DOGFOOD_HOME` selects another home; that home is retained. Sandbox and Hub
probes still use their own temporary roots, as does the optional multi-tenant
probe. Agent setup defaults to apply in isolated mode and dry-run in real-home
mode; applying setup may write client configuration outside these temporary
roots. The `target/` build output also persists.

`EXIT` traps are best effort: abrupt termination such as `SIGKILL`, machine
failure, or removal errors can leave resources behind. If necessary, inspect
the exact paths belonging to the completed run and remove only those paths
manually after confirming no probe is still using them. Do not delete the
whole temporary parent or an operator-selected real home. Retained packs and
client configuration require separate operator cleanup.

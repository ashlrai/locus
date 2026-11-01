# Dogfood temporary-directory creation failures

[`scripts/dogfood.sh`](../scripts/dogfood.sh) uses `mktemp -d` three times.
Each template is rooted under `${TMPDIR:-/tmp}`: an unset or empty `TMPDIR`
selects `/tmp`; a nonempty value must name an existing, writable directory.
For example, a missing parent causes `mktemp` to report an error on stderr.
The script does not create the parent, retry, or fall back to another directory
when creation fails.

| Template | When creation fails |
| --- | --- |
| `locus-dogfood.XXXXXX` | In the default isolated mode, the assignment fails under `set -e` and the script exits with `mktemp`'s nonzero status before initializing Locus or running quickstart. The main `EXIT` trap was installed already, but the failed assignment leaves `DOGFOOD_HOME` empty, so it has no allocated home to remove. The exact workspace build happens before this allocation. `DOGFOOD_USE_REAL_HOME=1` bypasses this allocation. |
| `locus-sandbox-dogfood.XXXXXX` | Step 1b returns status 1 from `sandbox_attestation` before entering the probe subshell or installing its cleanup trap. The caller reports `upstream sandbox attestation failed` and exits; the main exit trap still runs. The later readiness gate is not reached and `DOGFOOD READY` is not printed. |
| `locus-mt-dogfood.XXXXXX` | When `DOGFOOD_MT=1` reaches step 8b, the probe subshell exits with status 1 before installing its cleanup trap, initializing Locus, or starting its server. The caller reports `multi-tenant probe failed` and exits; the main exit trap still runs. The readiness gate is not reached and `DOGFOOD READY` is not printed. The probe is off by default. |

These are failure paths, not automatic recovery. Correct the temporary parent
and start a new run if appropriate. Probe roots and the main home have separate
cleanup lifecycles after successful allocation; see [probe cleanup](dogfood-probe-cleanup.md)
and the [temporary-directory reference](dogfood-temp-prefix-reference.md).

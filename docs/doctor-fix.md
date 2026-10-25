# doctor --fix

`locus doctor --fix` previews repairs and writes nothing. `locus doctor --fix --apply`
makes them, backing up each file first. Add `--json` for a `{mode, plan, outcome}` object.

```bash
locus doctor --fix            # dry run: lists "would fix" lines and manual findings
locus doctor --fix --apply    # needs local control (LOCUS_CONTROL_CAPABILITY)
```

Exit codes: 0 when there is nothing to repair, after a preview, or when every fix applied;
1 when an applied fix was skipped (for example the file changed between plan and apply).
`--apply` without `--fix` is a usage error.

The CLI never runs Phantom for this, so it passes no Phantom name list and every `phm:`
reference shows up as `phm_unavailable` (report only, fail closed). The logic lives in
`locus_core::doctor_fix`.

## Dry run by default

`plan_fixes(store, cwd, phantom_names)` reads binding files and the nearest
`.locus.toml` and returns a `FixPlan`. It writes nothing.

| Finding | Kind | Notes |
| --- | --- | --- |
| `allowed_bindings` entry with no binding | planned fix | the last entry is never removed (`allowlist_last_entry`) |
| `default_binding` with no binding | planned fix | the key is removed |
| legacy bare Phantom name in `credential_ref` | planned fix | rewritten to `phm:NAME` by the journaled store migration |
| malformed `credential_ref` | report only | edit by hand |
| `phm:` ref not in the supplied Phantom name list | report only (`dangling_phm_ref`) | |
| no Phantom name list supplied | report only (`phm_unavailable`) | fails closed |
| unparseable binding file | report only | allowlist pruning is skipped (`allowlist_inconclusive`) |

## Applying

`apply_fixes(store, &plan)` requires local control. For each file it re-checks every fix
against the current contents, writes a backup first, and only then edits:

- workspace files: `<file>.bak-<UTC timestamp>`, created new with mode 0600; the edit is
  written to a temp file and renamed. Symlinks are refused. Unknown keys are kept;
  comments are not.
- bindings: copied to `<home>/backups/doctor-fix-<timestamp>/<alias>.toml` before the
  migration runs.

If a backup fails, that file is not changed.

## What it never does

It never resolves or reveals credential values and never runs `phantom`. Dangling checks
compare against names the caller passes in. Plan and outcome output carry binding aliases,
provider names and file paths only, never ref strings or locator names.

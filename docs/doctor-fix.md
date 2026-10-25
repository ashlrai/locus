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

In a Phantom project (the current directory has a `.phantom.toml`), `locus doctor --fix`
runs `phantom list --json` once and checks every `phm:` reference against the listed
names. A name Phantom does not list is reported as `dangling_phm_ref` (report only:
`--apply` never changes a binding because of it, and the exit code is unchanged).
Elsewhere, or when the listing fails, every `phm:` reference stays `phm_unavailable`
(fail closed). The logic lives in `locus_core::doctor_fix`.

The probe:

- runs exactly `phantom list --json` (never `reveal`, `exec` or another subcommand), from
  PATH, only for `doctor --fix`, never while resolving bindings;
- gets an environment of PATH, HOME, USERPROFILE, SYSTEMROOT, TMPDIR, TEMP, LANG and
  `NO_COLOR=1` only (no `LOCUS_*`, `PHANTOM_*` or tokens); stdin is null, stderr is
  discarded, stdout is capped at 1 MiB, and it is killed after 3 seconds;
- keeps only the `name` strings (each must match `[A-Z_][A-Z0-9_]*`, at most 256
  characters); any other answer counts as unavailable;
- never prints the names: output carries binding aliases and provider labels only.

`--json` adds `phantom_names`: `{ "status": "listed", "count": N }`, or
`{ "status": "unavailable", "reason": R }` with `R` one of `no_phantom_project`,
`not_installed`, `timeout`, `exit_nonzero` or `bad_output`. The text output ends with
one `phantom names:` line saying which.

Minimum Phantom: phantom-secrets 0.6.0 is the first release with `phantom list --json`
(names plus `detected_service`, no values). Releases up to 0.7.9 print `[]` with exit 0
when the project vault is locked (an encrypted-file vault without its passphrase), so on
those versions a locked vault makes every `phm:` reference read as `dangling_phm_ref`;
unlock the vault and run it again. phantom-secrets main already exits 1 for a locked
vault, which reads as `exit_nonzero`.

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
| no Phantom name list (no `.phantom.toml`, phantom missing, or the listing failed) | report only (`phm_unavailable`) | fails closed |
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

# doctor --fix

`locus doctor --fix` previews repairs and writes nothing. `locus doctor --fix --apply`
makes them, backing up each file first. Add `--json` for a `locus.doctor.fix.v1` object
with `mode` (`dry_run` or `apply`), `phantom_names`, `plan` and `outcome`.

```bash
locus doctor --fix            # dry run: lists "would fix" lines and manual findings
locus doctor --fix --apply    # needs local control (LOCUS_CONTROL_CAPABILITY)
```

Exit codes: 0 when there is nothing to repair, after a preview, or when every fix applied;
1 when an applied fix was skipped (for example the file changed between plan and apply).
`--apply` without `--fix` is a usage error.

`locus doctor --fix --dry-run` prints the same fixes as a `locus.plan.v1` plan for agents
(see [agent contract](agent-contract.md)); `--dry-run` with `--apply` is a usage error.
When the Locus home does not exist yet (or, with `--dry-run`, is not initialized), the
preview reports an empty plan and creates nothing; the text output says to run
`locus init` first.

In a Phantom project (the current directory has a `.phantom.toml`), `locus doctor --fix`
runs `phantom list --json` once and checks every `phm:` reference against the listed
names. A name Phantom does not list is reported as `dangling_phm_ref` (report only:
`--apply` never changes a binding because of it, and the exit code is unchanged).
Elsewhere, or when the listing fails, every `phm:` reference stays `phm_unavailable`
(fail closed). The planner lives in `locus_core::doctor_fix`; the probe lives in the CLI
(`crates/locus-cli/src/phm_names.rs`) and is called only from `locus doctor --fix`.

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
`not_installed`, `timeout`, `exit_nonzero`, `bad_output`, `empty_list`, or
`no_locus_home` (no Locus home yet, so nothing was probed). The text output (without
`--dry-run`) ends with one `phantom names:` line saying which.

Minimum Phantom: phantom-secrets 0.6.0 is the first release with `phantom list --json`
(names plus `detected_service`, no values). Releases 0.6.0 to 0.7.9 print `[]` with exit 0 when the project vault is locked (an
encrypted-file vault without its passphrase). So a valid but empty list while bindings
hold `phm:` references is reported as `unavailable` with reason `empty_list`, and every
reference stays `phm_unavailable`, never `dangling_phm_ref`; unlock the vault and run it
again. With no `phm:` references an empty list is a normal `listed` answer. phantom-secrets
main already exits 1 for a locked vault, which reads as `exit_nonzero`.

## Dry run by default

`plan_fixes(store, cwd, phantom_names)` reads binding files and the nearest
`.locus.toml` and returns a `FixPlan`. It writes nothing.

| Finding | Kind | Notes |
| --- | --- | --- |
| `allowed_bindings` entry with no binding | planned fix | the last entry is never removed (`allowlist_last_entry`) |
| `default_binding` with no binding | planned fix | the key is removed |
| legacy bare Phantom name in `credential_ref` | planned fix | rewritten to `phm:NAME` by the journaled store migration |
| malformed `credential_ref` | report only (`malformed_ref`) | edit by hand |
| `phm:` ref not in the supplied Phantom name list | report only (`dangling_phm_ref`) | |
| no Phantom name list (no `.phantom.toml`, phantom missing, or the listing failed) | report only (`phm_unavailable`) | fails closed |
| unparseable binding file | report only (`binding_unparseable`) | allowlist pruning is skipped (`allowlist_inconclusive`) |
| unreadable or unparseable `.locus.toml` | report only (`workspace_unreadable`) | allowlist and default checks are skipped |

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

It never resolves or reveals credential values. `locus_core::doctor_fix` itself never runs
`phantom`: dangling checks compare against names the CLI probe passes in. Plan and outcome output carry binding aliases,
provider names and file paths only, never ref strings or locator names.

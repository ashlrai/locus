# First run

What happened when the README install and 30-second commands were followed on a clean home on 2026-10-26, using published npm `0.5.1` and a `cargo install --git` build of `e969d9c4`. Install and latency timings, the machine, and the method are in [BENCHMARKS.md](./BENCHMARKS.md).

## README paths

- **npm.** `npm install -g locus-cli @ashlrai/locus-mcp`, then `locus quickstart`, printed `whoami personal`. Both exited 0 and finished well under 60 seconds (timings in BENCHMARKS.md).
- **cargo.** Needs a current stable rustc. With an older default toolchain (1.83.0) the build failed on `zeroize_derive` 1.5.0 (`edition2024`); `rust-toolchain.toml` does not apply to `cargo install --git`. On stable it built and quickstart worked.
- **Homebrew.** Not run; `brew` was not on the machine.

The current 30-second commands, on the npm binary, a fresh home, and stdin that was not a terminal:

| Step | Exit | Wall |
|---|---|---|
| `locus quickstart` | 0 | 0.108 s, stdout contained `whoami personal` |
| `eval "$(locus hook zsh)"` in bash | 0 | capability set |
| `locus enter personal` | 0 | 0.120 s |
| `locus whoami` | 0 | 0.019 s, alias `personal` |
| `locus exec --expect personal -- echo ok` | 0 | 0.047 s, stdout `ok` |
| `locus exec --expect acme -- echo ok` | 1 | 0.014 s, `pin_mismatch` |

A second quickstart printed `already pinned` and exited 0. `locus doctor` after quickstart exited 1 with verdict `WARN` (`phantom_missing`); quickstart itself exited 0.

### Dead broker

After the pinning shell exited, `locus whoami` reported `no current broker endpoint` and `human re-pin required`, and suggested `locus leave --force; locus enter personal`. `locus enter personal` in a new shell with the hook exported, without `leave --force`, exited 0, and supervised `tools/list` then returned 18 tools. This was the hint printed by the audited 0.5.1 build; the current CLI prints an enter-first hint.

With the current CLI, the human operator can recover in a new Bash shell using the same `LOCUS_HOME` and binding alias:

```bash
eval "$(locus hook bash)"
locus enter personal
```

This creates a fresh pin and broker; it does not revive the exited shell's broker. If the session is expired or its seal is invalid, `enter` is refused. After checking the error, the operator can clear the unusable session and make a fresh pin:

```bash
locus leave --force && locus enter personal
```

Use the hook for your shell and substitute your binding alias. The control capability is required; a missing or incorrect capability is not fixed by forced leave. Follow the CLI's specific guidance for other refusals, such as a session without a persisted verifier. See [broker recovery](broker-recovery-q8.md) for the full procedure.

## Agent-use audit

Published npm `0.5.1` ELF, `LOCUS_HOME` under `/tmp`, stdin not a terminal (`stdin_tty no` in the log). Commands that can prompt were wrapped in `timeout 5`. None of them returned 124. The other commands were not wrapped, and the slowest of those was quickstart and enter at 0.118 s.

### Non-interactive and JSON

| Command | Exit | Wall | What came back |
|---|---|---|---|
| `whoami` with no pin | 1 | 0.002 s | names `locus enter` and `locus binding list` |
| `whoami --json` with no pin | 1 | 0.002 s | stderr JSON `ok: false`, stdout empty |
| `quickstart` | 0 | 0.118 s | pins `personal`, prints whoami |
| `quickstart` again | 0 | 0.054 s | `already pinned` |
| `whoami --json` after pin | 0 | 0.014 s | `credential.present` true, `source` `environment` |
| `doctor` | 1 | 0.022 s | verdict `WARN`, phantom missing |
| `doctor --json` | 1 | 0.014 s | JSON report on stdout, stderr empty |
| `exec --expect personal` | 0 | 0.046 s | stdout `ok`, stderr warnings for unset injection env |
| `exec --expect acme` | 1 | 0.015 s | `pin_mismatch`, child not started |
| `enter personal` | 0 | 0.118 s | entered `personal` |
| `status --json` | 0 | 0.021 s | `ok` true, `pinned` true, alias `personal` |
| `--json not-a-command` | 2 | 0.002 s | `unrecognized subcommand 'not-a-command'` |
| `--json binding show nope` | 1 | 0.002 s | `binding not found: nope` and known aliases `acme, personal` |
| `--json enter missing-alias` | 1 | 0.083 s | same known-aliases hint |
| `binding add` with no flags | 1 | 0.002 s | asks for alias, tenant, provider, account, and credential ref |
| `onboard` | 1 | 0.069 s | requires an operator TTY |
| `onboard --json` | 1 | 0.063 s | same text as JSON on stderr |
| `graph export` with no `LOCUS_GRAPH_PASSPHRASE` | 1 | 0.002 s | passphrase required for non-interactive export |
| `init --with-samples --json` a second time | 0 | 0.024 s | `ok` true, `samples` true, `config_written` false |

In that 0.5.1 run, `whoami --json` reported `credential.present: true` for a stored ref even though the variable was unset. The current implementation reports whether the credential can be resolved in the `whoami` process: an unset `env:` variable reports `false`, while a variable set to an empty string reports `true`. Presence does not guarantee that a credential is valid or usable by a provider. Phantom refs currently report `false` because the scoped credential bridge is unavailable. Neither the ref name nor its value is included in the result.

### Dry-run and idempotent setup

These are historical observations from the 0.5.1 audit, not the current CLI
contract. The global flag has since expanded; see the [current dry-run
matrix](dry-run-matrix-q8.md) for supported commands and limitations.

`--help` lists `--dry-run` for `binding add` and `agent setup` only, among the commands checked: `init`, `quickstart`, `enter`, `pin`, `leave`, `binding rm`, `binding add`, `workspace`, `setup`, `agent setup`, `doctor`, `adapter install`, `engagement init`, `graph export`, `onboard`.

| Command | Exit | Result |
|---|---|---|
| `binding add --dry-run --json` | 0 | `dry_run` true, TOML in the JSON, 0.002 s |
| `binding add --non-interactive --json` | 0 | wrote the binding file, 0.026 s |
| same add again | 1 | `binding 'dryalias' already exists; nothing written`, names `--force` and `locus binding show` |
| `binding rm` without `--yes` | 1 | `refusing to remove without --yes` |
| `binding rm --yes --json` | 0 | `{"removed":"dryalias"}` |
| `agent setup --dry-run --json` | 0 | `apply` false, `dry_run` true, lists files it would write |
| `agent setup` with neither flag | 1 | `pass --apply or --dry-run` |
| `setup --client generic --print` | 0 | writes nothing, prints the client config |
| `init --dry-run`, `pin --dry-run`, `leave --dry-run` | 2 | unexpected argument `--dry-run` |

`pin --dry-run` adds a clap tip that says to pass `-- --dry-run` as a value. That tip does not match a dry-run flag. The repo working tree was clean after the audit. `agent setup --dry-run` had `apply` false, so the paths it listed under the repo were not written.

`binding add` is safe on a second run (it errors and writes nothing). It is not an idempotent no-op when the file already matches.

### MCP schemas

NDJSON, initialize `protocolVersion` `2024-11-05`, serverInfo `locus-mcp` `0.5.1` on the `e969d9c4` binary. A later check after `locus enter personal` in the same shell compared `result` objects.

| Launch | Tools | `tools/list` `result` equal within one process | Equal across two processes | `locus_whoami` |
|---|---|---|---|---|
| `locus-mcp`, empty home, no capability | 9 control tools | yes | yes | error, no active pin |
| `locus-mcp`, live pin, not supervised | 10 (adds `locus_providers`) | yes | yes | error, executor authority unavailable |
| `locus mcp --supervised`, live pin | 18 (adds supabase, github, and vercel tools) | yes | yes | not an error, alias `personal` |

Control names, in order: `locus_whoami`, `locus_safe_next`, `locus_status`, `locus_heartbeat`, `locus_enter_hint`, `locus_list_bindings`, `locus_request_pin`, `locus_verify_claim`, `locus_verify_session`. Supervised adds `supabase.scope`, `supabase.project_ref`, `supabase.table.delete`, `github.scope`, `github.whoami`, `github.check_repo`, `vercel.scope`, `vercel.deploy.prod`.

Every listed `inputSchema` had `additionalProperties` false. Description text is prefixed with the pin alias, so a schema-stable `inputSchema` can still sit under a description that changes when the pin changes. Comparing full JSON-RPC messages fails that stability check because `id` changes. Compare `result`.

`tools/call` of `no_such_tool` returned error `-32602`. The message says provider tools are `provider.tool`, control tools are `locus_*`, and to see `tools/list`.

## Follow-ups

Tracked as issues:

- [Dead-broker recovery hint names leave --force before enter](https://github.com/ashlrai/locus/issues/166)
- [cargo install --git fails on rustc 1.83: declare rust-version](https://github.com/ashlrai/locus/issues/167)
- [cargo install compiles locus-core twice](https://github.com/ashlrai/locus/issues/168)
- [Shell installer for the release archive (one command without npm or Rust)](https://github.com/ashlrai/locus/issues/169)
- [locus exec is 3 to 5x slower than direnv exec and sops exec-env](https://github.com/ashlrai/locus/issues/170)
- [--dry-run missing on most mutating commands](https://github.com/ashlrai/locus/issues/171)
- [doctor exits 1 right after a fresh quickstart](https://github.com/ashlrai/locus/issues/172)
- [whoami credential.present is true while the env var is unset](https://github.com/ashlrai/locus/issues/173)
- [Sample exec prints three unset-variable warnings on the happy path](https://github.com/ashlrai/locus/issues/174)
- [binding add is not an idempotent no-op when the file matches](https://github.com/ashlrai/locus/issues/175)

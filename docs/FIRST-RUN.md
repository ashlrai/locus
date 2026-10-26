# First run

What happened when the README install and 30-second commands were followed on a clean home on 2026-10-26. Timings and peer tables are in [BENCHMARKS.md](./BENCHMARKS.md). This file is the friction log, the agent-use audit, and the follow-ups that were left for a later change.

Binaries: published npm `0.5.1` (release tag `v0.5.1`, commit `89f12eb3ab523f84ef5d03335054add4bc97314f`) and a `cargo install --git` build of `e969d9c4`. Commit `357f98b` (#161) changed the README npm wording after those binaries were built. The notes below say which README text was on screen at measurement time.

`npm view` already printed `0.5.1` for `locus-cli` and `@ashlrai/locus-mcp` before the install. The target is one command and a first useful result in under 60 seconds. First useful result means stdout that shows the sample binding `personal`.

## README paths

### npm

`npm install -g locus-cli@0.5.1 @ashlrai/locus-mcp@0.5.1` with nvm's node first on `PATH` exited 0 in 0.775 s. quickstart in that shell exited 0 in 0.123 s and printed `whoami personal`. That is under 60 seconds. It is two commands, plus the hook line if a new shell must export `LOCUS_CONTROL_CAPABILITY`.

The same shell then followed an older sequence that also ran `locus leave --force` (exit 0, 0.045 s) before `locus enter personal`. That extra step is not in the 30-second block at `e969d9c` (#159 had already removed it). The whole script, from install start through the last command, was 1.819 s.

The current 30-second commands, on that same npm binary, a fresh home, and stdin that was not a terminal:

| Step | Exit | Wall |
|---|---|---|
| `locus quickstart` | 0 | 0.108 s, stdout contained `whoami personal` |
| `eval "$(locus hook zsh)"` in bash | 0 | capability set |
| `locus enter personal` | 0 | 0.120 s |
| `locus whoami` | 0 | 0.019 s, alias `personal` |
| `locus exec --expect personal -- echo ok` | 0 | 0.047 s, stdout `ok` |
| `locus exec --expect acme -- echo ok` | 1 | 0.014 s, `pin_mismatch` |

Stamp delta for that sequence: 343581230 ns (0.344 s).

`locus quickstart` already pins. In a second fresh home, quickstart then `locus exec --expect personal` in the same shell exited 0 in 0.052 s with no separate `enter`. After quickstart in a subshell that exited, the parent ran the hook and `locus enter personal` (exit 0, 0.117 s) and exec (exit 0, 0.052 s). `leave --force` was not required.

A second quickstart printed `already pinned` and exited 0 (0.054 s in the audit home, 0.063 s in another home).

`locus doctor` after quickstart exited 1. Verdict `WARN`. Finding `phantom_missing`: phantom was not on `PATH`. quickstart itself stayed exit 0. The sample bindings use `env:` refs.

`npx --yes locus-cli@0.5.1 --help` exited 0 in 0.470 s (nvm prefix already had the global package, fresh npm cache).

### cargo

The README says rustup stable and `cargo install --git https://github.com/ashlrai/locus locus-cli locus-mcp --locked`.

Run with the VM default, rustc 1.83.0, fresh cargo home, working directory outside the repo: exit 101 in 1.924 s. Cargo could not parse `zeroize_derive` 1.5.0 because feature `edition2024` is not stabilized in that Cargo. No `locus` binary, so no first useful result. `rust-toolchain.toml` in the repo did not apply.

Same command with `RUSTUP_TOOLCHAIN=stable` (rustc 1.99.1 already installed, fresh cargo home, cold registry): exit 0 in 86.488 s. `locus-core` was compiled twice (cli `Finished` at 50.96 s, then mcp `Finished` at 1 m 26 s). quickstart on that binary, fresh home, stdin closed: exit 0 in 0.138 s, stdout contained `whoami personal`. A following `locus exec --expect personal -- true` exited 0 in 0.053 s.

86.488 s is over the 60 second target. The README on `e969d9c` and on `357f98b` said 179 s for one Linux box and did not record the machine or the toolchain. The cargo comment in the README now points at these runs.

### Homebrew

`command -v brew` was empty. The 30-second block at measurement time started with `brew install ashlrai/tap/locus`. That line was not run. #161 kept brew as the first line and pointed npm and cargo at Install options.

### npm 0.5.0

`locus-cli@0.5.0` in an isolated prefix printed `locus 0.5.0` after downloading the release, warned that no `.sha256` sidecar was published, and had not exited when `timeout` returned 124 at 8 seconds. Do not install that version. `0.5.1` is the version `npm view` printed, and its `--version` exited 0.

At the start of measurement the README still said to wait until `npm view` printed `0.5.1` and that `0.5.0` was the latest published package. That sentence was already false at 2026-10-26T02:08:37Z. #161 removed it.

### Other environment friction

- Invoking npm with node at `/exec-daemon/node` made `npm config get prefix` print `/`. The timed global install put nvm's node first so the prefix was the nvm directory, which is writable.
- A default `PATH` without nvm had no `node` and no `npm` on this VM.
- `/usr/local/bin` is not writable, which is why the sops `mv` in the peer run failed.
- `eval "$(locus hook zsh)"` in bash exported the capability (exit 0). The zsh hook's export line is usable from bash. The prompt coloring is zsh syntax and was not eval'd on its own.

### Dead broker

After the shell that pinned had exited, `locus whoami` on the HEAD binary exited with `no current broker endpoint` and `human re-pin required`. The next line told the operator to run `eval "$(locus hook bash)"; locus leave --force; locus enter personal`.

`locus enter personal` in a new shell, with the hook exported and without `leave --force`, exited 0. Supervised `tools/list` then returned 18 tools. The longer `leave --force` sequence also works: it deletes session state and does not mint a credential. An expired pin still needs `--force`. That rule is in `crates/locus-core/src/recovery.rs` and was not changed here. The error string still names `leave --force` first for the dead-broker case, where `enter` alone succeeded.

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

`whoami --json` reports `credential.present: true` when the credential ref string is non-empty. The following `exec` still warned that `SUPABASE_ACCESS_TOKEN`, the GitHub token variables, and `VERCEL_TOKEN` were unset. `present` means the ref is stored. It does not mean the variable is set in the environment. The field was not renamed.

### Dry-run and idempotent setup

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

## What this change fixes

Doc only. No version bump, no workflow change, no recovery-string change in code.

- The 30-second block leads with the npm install that finished in 0.775 s. Homebrew and cargo stay in Install options, with the measured cargo toolchain note.
- The dead-broker recovery text matches the run: `locus enter personal` succeeded without `leave --force`. The error string is unchanged and still names `leave --force` first.
- `docs/mcp.md`, `CLAUDE.md`, and `AGENTS.md` use that same recovery.
- npm package READMEs name `0.5.1` and say `0.5.0` did not exit.

## Ranked follow-ups

Effort is the size of the code change, not a calendar estimate.

1. **Recovery hint for a dead broker.** Medium. `stale_pin_recovery_for_shell` still prints `leave --force` before `enter`, including the `no current broker endpoint` case where `enter` alone exited 0. Doctor, agent report, and MCP copy that string, and tests snapshot it. Expired pins must keep the `--force` path (`recovery.rs`).

2. **Cargo builds `locus-core` twice.** Medium. One `cargo install --git` of `locus-cli` and `locus-mcp` compiled `locus-core` for the cli (Finished at 50.96 s) and again for mcp (total 86.488 s). A single installable package, or a documented one-binary install, is the change. This PR only records it.

3. **One command under 60 seconds without npm.** Medium to large. npm is two commands and under 60 seconds on this VM. cargo on stable is one command and 86.488 s, and it fails on the default 1.83.0 toolchain. A shell installer that downloads the release archive (the direnv installer shape) would cover a machine with no npm and no Rust. This PR adds no CI job and no release asset.

4. **`--dry-run` on mutating commands.** Large. Missing on `init`, `quickstart`, `enter`, `pin`, `leave`, `binding rm`, `workspace`, `adapter install`, `engagement init`, `graph export`, and `onboard`. `setup` has `--print`. `binding add` and `agent setup` have `--dry-run`. The `pin --dry-run` clap tip is misleading.

5. **`doctor` exits 1 after a fresh quickstart.** Small to medium. Sample bindings are `env:` refs. The only warning was phantom missing, and quickstart still exited 0. If `phantom_missing` is info when no `phm:` ref exists, the first-run doctor exit changes. That needs a test.

6. **`credential.present` while the variable is unset.** Small to medium. Add a field that means the env var resolved. Leave `present` as "the ref string is non-empty" (`crates/locus-core/src/credential.rs`). Tests and the metadata contract use the current meaning.

7. **Sample exec warnings.** Small to medium. The happy path prints three warnings and names injection variables, not the sample refs `env:LOCUS_SUPABASE_PERSONAL`, `env:LOCUS_GH_TOKEN_PERSONAL`, and `env:LOCUS_VERCEL_TOKEN_PERSONAL`. `describe_unresolved` omits locator names on purpose. A clearer first-run line has to keep that rule.

8. **`rust-version` in `Cargo.toml`.** Small, after a build on the oldest stable that parses `zeroize_derive` 1.5.0. This VM showed 1.83.0 fails and 1.99.1 works. Do not set 1.85 until that compiler is the one that was measured.

9. **`binding add` idempotent when the file matches.** Small to medium. Today the second run exits 1, writes nothing, and names `--force`. A no-op when the TOML matches would be the idempotent form.

10. **npm prefix when node is not the nvm binary.** No Locus code change. Documented above. A wrapper note is optional.

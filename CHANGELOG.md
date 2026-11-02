# Changelog

<!-- fresh:begin release -->
Latest release: [v0.6.2](https://github.com/ashlrai/locus/releases/tag/v0.6.2) (2026-11-01).

npm package READMEs use version-neutral installation commands to avoid stale pinned releases.
<!-- fresh:end -->

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Documentation

- **Shell installer proposal ([#169](https://github.com/ashlrai/locus/issues/169)).**
  The issue proposes a one-command shell installer that downloads and verifies
  a release archive, allowing installation on machines without npm or Rust.
  This note records the proposal; it does not assert release availability.

- **Execution latency report ([#170](https://github.com/ashlrai/locus/issues/170)).**
  The issue reports `locus exec -- true` as 3–5x slower than
  `direnv exec <dir> true` and `sops exec-env <file> true`. This is the
  issue's report, not a verified benchmark or a claim about current releases.
  See the [historical measurements](docs/BENCHMARKS.md#core-operation-latency)
  and the [same-host comparison procedure](docs/exec-latency.md#comparing-launchers-for-issue-170).

- **Reproduce cargo-install timings ([#168](https://github.com/ashlrai/locus/issues/168)).**
  The first-run report observed `locus-core` compiling twice during a single
  `cargo install --git` of `locus-cli` and `locus-mcp`: it compiled again
  during the `locus-mcp` install after `locus-cli` finished. This remains an
  unresolved issue: the cause and whether this represents identical compiler
  work remain unverified; no fix is claimed. See [the benchmark record](docs/BENCHMARKS.md).

  From a checkout with the first-run commit in its Git object database, compare
  one install of both packages with two separate installs at that historical
  revision. It had separate binary targets; current `locus-cli` includes both
  binaries, so the old same-root two-package recipe would collide on current
  source. This measures the historical revision, not current build performance.
  Each case uses the same pinned revision, lockfile, release profile, toolchain,
  and a fresh target and install root; it installs no binaries into the usual
  Cargo bin directory. `/usr/bin/time -p` reports wall
  time as `real` in each log; add the two separate-install times for their total.

  ```bash
  set -e
  revision=e969d9c44a3c0bff1d0a1fee4deb1b77fb58652e
  repo="file://$(pwd -P)"
  work=$(mktemp -d)
  /usr/bin/time -p cargo install --git "$repo" --rev "$revision" --locked \
    locus-cli locus-mcp --root "$work/combined-install" \
    --target-dir "$work/combined-target" --message-format=json \
    > "$work/combined.jsonl" 2> "$work/combined.log"
  /usr/bin/time -p cargo install --git "$repo" --rev "$revision" --locked \
    locus-cli --root "$work/separate-install" \
    --target-dir "$work/separate-target" --message-format=json \
    > "$work/cli.jsonl" 2> "$work/cli.log"
  /usr/bin/time -p cargo install --git "$repo" --rev "$revision" --locked \
    locus-mcp --root "$work/separate-install" \
    --target-dir "$work/separate-target" --message-format=json \
    > "$work/mcp.jsonl" 2> "$work/mcp.log"
  grep -E 'Compiling locus-core|Finished `release`|^real ' "$work/"*.log
  ```

  The second separate install intentionally shares its target directory with
  the first; neither case reuses the other's build artifacts. The pinned Git
  revision excludes uncommitted edits. If it is absent from a shallow checkout,
  obtain that public commit before running. Record the revision,
  OS/architecture, `cargo --version`, `rustc --version`, and cache/network
  conditions. Run both orders and repeat with new directories: Cargo's download
  cache and the OS cache can make later runs warmer, and timings include install
  and source/dependency fetching as well as compilation. Inspect `Compiling
  locus-core` lines and their order; use the JSON artifact inspection in
  [CONTRIBUTING.md](CONTRIBUTING.md#investigating-repeated-compilation-during-install)
  on each `.jsonl` file to compare package, target, profile, features, filenames,
  and `fresh` status. Two non-fresh records indicate two builds, but matching
  displayed fields alone do not establish identical compiler work or its cause.
  This recipe makes no speedup claim without measurements.

### Troubleshooting

- **npm wrapper downgrade scenario ([#177](https://github.com/ashlrai/locus/issues/177)).**
  The published `locus-cli@0.5.2` wrapper treats a cached version different
  from its own as stale, even if the cached binary is newer. Its POSIX
  postinstall can overwrite `~/.locus/bin/locus` with v0.5.2; on first use
  the wrapper can instead remove the mismatched cache before downloading, so
  a failed download can also leave the cache missing. This affects the older
  published wrapper; the current checkout's wrapper preserves a newer or
  unversioned cache and refuses to run without an explicit leading `--force`.
  npm installation itself still succeeds when that refusal occurs.

  To reproduce the published behavior on macOS (arm64 or x64) or Linux x64
  with Node, npm, tar and network access, use a disposable home and a stand-in
  for a newer v0.5.3 binary (v0.5.2 is downloaded from the release):

  ```bash
  demo=$(mktemp -d)
  mkdir -p "$demo/home/.locus/bin"
  HOME="$demo/home" npm install --ignore-scripts --prefix "$demo/package" locus-cli@0.5.2
  printf '#!/bin/sh\necho locus 0.5.3\n' > "$demo/home/.locus/bin/locus"
  chmod +x "$demo/home/.locus/bin/locus"
  printf '0.5.3\n' > "$demo/home/.locus/bin/locus.version"
  "$demo/home/.locus/bin/locus" --version
  HOME="$demo/home" LOCUS_NPM_NO_LINK= node "$demo/package/node_modules/locus-cli/install.js"
  "$demo/home/.locus/bin/locus" --version
  ```

  The two outputs are `locus 0.5.3` and `locus 0.5.2` if the release
  download succeeds. Remove the disposable directory afterwards. The npm
  wrapper in this checkout is unreleased; do not assume the published package
  has the same behavior.

- **Fresh quickstart and doctor WARN ([#172](https://github.com/ashlrai/locus/issues/172)).**
  On a fresh home with non-interactive stdin and Phantom absent from PATH, the
  recorded first run of published npm 0.5.1 had `locus quickstart` exit 0,
  followed by `locus doctor` exiting 1 with `WARN` and `phantom_missing`.
  This is an observed failure of that published version. To diagnose it, inspect
  `locus doctor --json` and its findings; the recorded first run also verified
  the pin separately with `locus whoami`. Address any other findings rather
  than treating every doctor failure as this warning. No workaround for the
  affected doctor's exit status was verified in that first run. See
  [the first-run record](docs/FIRST-RUN.md).

### Changed

- **npm wrapper upgrades ([#177](https://github.com/ashlrai/locus/issues/177)).**
  In this unreleased checkout, both wrappers compare versions using SemVer
  precedence, probing the binary with `--version` before falling back to its
  adjacent `.version` file. By default, an equal cached version is reused; an
  older one can be upgraded. With a newer or unversioned cache, `locus-cli`
  preserves the binary and refuses to run before checking PATH, while
  `@ashlrai/locus-mcp` preserves and runs it. Without a protected cache,
  a matching or newer native PATH (or Cargo bin) candidate can be reused;
  otherwise the wrapper downloads its packaged release or falls back to a
  Cargo build from the repository's default branch. The Cargo build's actual
  version is checked against an existing cache before replacement.

  A leading wrapper `--force` permits replacing a protected cache; npm's own
  `--force` and flags after a native command do not. CLI postinstall may
  download and link a release on supported POSIX platforms, but a protected
  cache is preserved and npm installation still succeeds. MCP postinstall
  defers installation to first use. If the CLI wrapper has already been
  linked to the native binary, reinstall the npm package to restore the wrapper
  before using its override. See the [CLI wrapper guide](npm/README.md) and
  [MCP wrapper guide](npm-mcp/README.md); the published v0.5.2 CLI wrapper
  has the different behavior described in the troubleshooting note above.

- **Dead-broker recovery ([#166](https://github.com/ashlrai/locus/issues/166)).** If the pinning shell exits, its broker can disappear while the sealed pin remains. In the recorded v0.5.1 first run, the hint incorrectly put `locus leave --force` before `locus enter <alias>` for this case, although entering directly succeeded. In a new operator shell using the same `LOCUS_HOME`, load the control capability with the shell hook and first run `locus enter <alias>`; an intact local-control pin with a dead broker can be replaced directly. For Bash the current hint is `eval "$(locus hook bash)"; locus enter <alias> # If enter refuses an invalid or unreadable pin, run: locus leave --force && locus enter <alias>`.
  Inspect a refusal before taking further action: for an expired pin, invalid or legacy seal, or unreadable session, the current CLI refuses enter. When teardown is appropriate, a human can run `locus leave --force` and then `locus enter <alias>`. Do not force leave for unrelated errors such as a wrong capability or workspace refusal. Verify the new pin with `locus whoami` and keep the operator shell alive. An explicit `LOCUS_SESSION_ID` uses the strict authority check. See the [recovery guide](docs/broker-recovery-q8.md).

- **`locus binding add` is idempotent when the file already matches (#175).**
  Re-adding an alias whose saved binding is equivalent now exits 0 without
  writing (`"changed": false` in `--json`; `already matches; nothing written`
  otherwise), and a `--dry-run` on it plans zero changes. A different body for
  the same alias still refuses without `--force`, and `--force` still replaces.

- **Rust 1.89 floor named on every install path ([#167](https://github.com/ashlrai/locus/issues/167)).**
  The npm wrapper cargo-fallback messages, the npm READMEs and the Homebrew
  notes now state the same `rust-version` the workspace declares, and
  `crates/locus-core/tests/rust_version.rs` keeps `cargo metadata`, member
  crate manifests and the documented install requirements in agreement with it.

## [0.6.2] - 2026-11-01

Highlights: npm package READMEs use version-neutral installation commands to avoid stale pinned releases.

### Fixed

- Package README install commands no longer embed an older release pin in immutable npm tarballs. Public documentation retains the separately verified published-version pin.

## [0.6.1] - 2026-10-31

Highlights: Native npm wrapper downloads now fail closed when release checksums cannot be verified.

### Fixed

- Both npm wrappers reject missing, malformed, unreachable or mismatched SHA-256 sidecars before installing or replacing a native binary. Checksum failures do not fall back to Cargo; the CLI eager postinstall fails rather than silently deferring the error. Explicit source installation remains available. The sidecars detect corruption, not publisher authenticity.

## [0.6.0] - 2026-10-31

Highlights: Grant-scoped swarm and local peer relay, stricter binding validation, safer npm cache replacement, and failure-safe MCP configuration registration.

**Release preparation:** October 31 is the candidate preparation date. This source
section does not assert that the tag, release assets, npm packages, website or
Homebrew update have been published. Verify the release and package registries
before installing the new version.

### Added

- `locus offboard` plans local client cleanup by default; explicit `--apply`
  removes operable local records while preserving audit/archive evidence. It
  does not revoke provider credentials or remove external repository access.
- Grant-scoped swarm MCP serving and bounded local peer relay using
  operator-controlled graph and engine manifests; registration does not confer
  new execution authority.
- Credential-free workbench status and launch diagnostics, optional native MCP
  registration rewrites with backups, and an opt-in versioned strict grant schema.

### Fixed

- Session renewal does not restore a session after concurrent leave or enter.
  Broker recovery guidance, binding-add idempotence, credential-presence reports
  and supervised child lifecycle handling have also been improved.
- npm wrappers preserve newer or unknown-version cached binaries rather than
  silently downgrading them. Replacement requires an explicit leading `--force`;
  postinstall success alone does not prove the cached binary was replaced.
- MCP config registration refuses malformed existing JSON and atomically stages
  replacements instead of truncating existing config on a failed write.
- Authenticated swarm preflight now reports the existing `multi_tenant` category,
  consistent with anonymous grant-layer detection, and retains the grant ID.

### Compatibility and upgrade notes

- Binding, provider, upstream and policy structures reject unknown fields;
  fix misspelled or unsupported fields rather than bypassing validation.
  `policy.default` must normalize to `allow` or `deny`.
- Phantom reference names must match `[A-Z_][A-Z0-9_]*`, at most 256 ASCII
  characters. Formerly accepted lowercase, dotted or hyphenated names are
  rejected, not automatically rewritten. `phm:` credential resolution already
  failed closed in v0.5.2; this release does not activate a Phantom execution
  bridge. Supported credential resolution remains explicit `env:` references.
- MCP config replacement refuses symbolic/multiple hard links and nonregular
  targets and requires a writable parent directory. POSIX owner/group/mode are
  preserved where supported; custom ACL/xattr preservation, concurrent-writer
  safety and power-loss durability are not promised. Interrupted staging can
  leave a temporary sibling at the previous file's mode after metadata restore,
  not necessarily the initial private mode.
- Manual CI covers Linux stable and Linux MSRV 1.89. macOS release builds are not
  native test qualification; no Windows archive or native Windows test claim is
  made. Check the exact final release SHA's workflow evidence before tagging.

## [0.5.2] - 2026-10-26

Highlights: `locus renew` and expired-pin recovery (#193), the global `--dry-run` preview, exact-loopback HTTP (#187) and index URL redaction (#185).

### Added

- **`locus renew` (opt-in pin self-renew) and expired-pin recovery.** The store supports clearing an authentic expired pin with `leave` or replacing it in place (audited `session.replace_stale`). The current CLI boundary still refuses `locus enter` for an expired active pin: use `locus leave --force` before entering again, as described in the recovery note above. New `locus renew [--json]` extends a live pin to `min(now + policy.max_ttl, pinned_at + policy.max_session_lifetime)`; it is disabled unless the binding sets the new `policy.max_session_lifetime` (1m to 30d), keeps the session id, binding and original `pinned_at`, and refuses an expired, frozen, tampered or stale-generation pin (`human re-pin required`), a changed binding, CI or run sessions, and any renew that would not extend the pin. Audited as `session.renew`. See [docs/policy.md](docs/policy.md).

- **Agent-ready CLI contract ([docs/agent-contract.md](docs/agent-contract.md)).** Every check and status command's `--json` object now starts with a versioned `schema` id (`locus.doctor.v1`, `locus.whoami.v1`, `locus.status.v1`, `locus.verify.session.v1`, `locus.agent.report.v1`, `locus.watch.v1`, and more); existing fields, including `schema_version`, are unchanged, and the `.vN` suffix moves only on a breaking shape change. `--json` failures print `locus.error.v1` on stderr with `exit_code` and `exit_code_name`. Exit codes come from one `ExitCode` enum (0 success, 1 failure, 2 unsafe or usage, 126/127 child could not start, 141 stdout closed), with the docs table checked against the binary. A global `--dry-run` previews every mutating command (`pin`/`enter`/`switch`/`leave`, `init`, `quickstart`, `binding add/rm`, `workspace`, `ci mint`, `mcp mint/revoke`, `setup`, `agent setup`, `doctor --fix`, `adapter install`, `events export`, …) as a `locus.plan.v1` plan and writes nothing: targets resolve through the real code (`Store::resolve_pin`, `engagement_init_preflight`, `graph_import_preview`), a missing home is never created, and plans never carry a capability, token or webhook path. `exec`, `run`, `ci run`, `mcp`, `serve` and `dashboard` do not take `--dry-run` and fail exactly as before (exec exit 1, the others a clap usage error, exit 2); `agent setup --apply --dry-run` keeps exit 1. Prompts still need only stdin on a terminal; `--json` or `LOCUS_NON_INTERACTIVE=1` now also turn them off. `locus topic agent-contract [--json]` prints the whole contract (`locus.contract.v1`).

- **`locus autopin suggest [--json] [--apply]` (learned bindings, L2).** Reads local pin history for the current repo, matched by the keyed hash of the live remote (userinfo such as `user:token@` stripped first), and suggests the binding you usually pin there, with evidence per alias: pins, `pin_mismatch` refusals, score and last seen. Ranking is deterministic: score = pins minus mismatches where that alias was the wrong one pinned; a suggestion needs a score of at least 3 and a strict lead, so thin or tied evidence suggests nothing. `--json` emits `locus.autopin.suggest.v1`. `--apply` writes `default_binding` to `.locus.toml` only after the control capability gate, recomputes from the live remote at apply time, refuses a symlinked `.locus.toml`, and writes atomically (temp file plus rename). With history off it says so and suggests nothing. The raw remote URL is never stored or printed.

### Fixed

- **Dead-broker recovery hint says `locus enter` first.** When a pin's broker died with the shell that pinned it (`no current broker endpoint`, anchor unavailable or mismatched, seal intact), `locus enter <alias>` has replaced the pin directly since #156, but every surface still printed `locus leave --force` first. `whoami`, `status`, control commands, `doctor`, `agent report` and the MCP hint switched to an enter-first recipe. The original automatic fallback has since been replaced by a separate manual step; see the unreleased #166 note above. Invalid or legacy seals and unreadable session files, which enter still refuses, need `locus leave --force` before `locus enter <alias>`. Test `printed_dead_broker_hint_works_verbatim` runs the current printed line as is.
- **`locus mcp --supervised` names the same recovery on its fallback line.** A dead pin under `--supervised` printed `human re-pin required` with no recipe at all: the error is reported by `mcp` itself and never reached the top-level stale-pin hatch. The fallback line now carries the enter-first recipe, and the recipe selection in `wedged_session_recovery` / `stale_pin_recovery_fallback` shares `recovery::recovery_for_error` instead of duplicating the dead-broker check. New test `supervised_mcp_fallback_on_dead_pin_names_the_recovery` (#166).

### Changed

- `locus exec` refuses before secret resolution when the drift check is not ok, including a broker re-check that declines to freeze the session. The error names the drift issues.
- `locus exec` no longer waits on a 10 ms broker accept poll, and the healthy path checks the seal and the broker once before the executor grant instead of three times. The audit record, `--expect` refusal, ambient scrub, and executor capability gate are unchanged. On the measured host, `locus exec -- true` with a live pin moved from a median of 0.051 s to 0.004 s (21 runs, first dropped, release build). Details: [docs/exec-latency.md](docs/exec-latency.md). `cargo bench -p locus-core --bench exec_authority` prints the in-process pin-check median and does not gate CI.

### Security

- **Plain HTTP and the MCP HTTP bind allow only exact `127.0.0.1` and `::1` (#187).** One shared rule in `locus-core::loopback`. Adapter registry index and manifest URLs over `http://` must use `127.0.0.1` or `[::1]`; `http://localhost` and the rest of `127.0.0.0/8` (for example `127.0.0.2`) are now refused. HTTPS is unchanged, including `https://localhost`. `locus-mcp --http` binding any 127.x address other than `127.0.0.1` now needs `LOCUS_MCP_HTTP_ALLOW_REMOTE=1`, like any other non-loopback bind. **Upgrade note:** configs with an `http://localhost` index URL or a 127.x MCP bind other than `127.0.0.1` must switch to `127.0.0.1`.
- **Adapter registry index URLs never display credentials or query strings (#185).** `adapter registry index add` refuses URLs with userinfo (`user:pass@`) or a fragment before any fetch, with an error that does not echo the URL. `index add/list/remove` human and `--json` output show a redacted URL (scheme, host, port, path, plus `?<redacted>` when a query is present); the stored config keeps the query so signed index URLs still fetch. TOML parse errors in index sources no longer quote the offending line.
- **Session file writes no longer follow a symlink at `active.json`.** `write_session_file` writes a fresh `create_new` temp file and renames it over the target, so a symlink planted at `sessions/active.json` is replaced instead of written through (shipped with the 0.5.1 stale-pin fix, #156).
- **Session temp file is created 0600, and the rename is made durable.** The mode is set at open (not chmod after), and on unix the parent directory is fsynced after the rename.
- **Pin history key creation is race-safe.** `history.key` is written in full to a private temp file and hard-linked into place, so concurrent first writers share one key and no record is dropped on a half-written key.
- **`$LOCUS_HOME` is enforced 0700 on unix.** `Store::open` creates a missing home with mode 0700 and tightens an existing home owned by the current user to 0700. A symlinked home is not chmod'ed through. Follow-up from the #160 security review.
- **`leave` reads pin history under the history lock.** The fallback lookup of the alias's last pin and the append now happen under one exclusive lock.

### Documentation

- **Pin history threat model.** Module docs state what `history.key` protects (a log shared on its own) and what it does not (a reader of all of `$LOCUS_HOME`, who can already see pins and config). The key stays next to the log.
- **First-run measurements.** [docs/BENCHMARKS.md](docs/BENCHMARKS.md) records install and core-op timings against direnv and sops; [docs/FIRST-RUN.md](docs/FIRST-RUN.md) records the first-run log and agent-use audit, with follow-ups filed as #166 to #175. The README 30-second block leads with the unpinned npm install (the measured path), the cargo note states the rustc 1.89 requirement, and the README, `CLAUDE.md`, `AGENTS.md` and `docs/mcp.md` say `locus enter` recovers a dead broker (`leave --force` only if enter is refused). The npm-mcp README shows the entry `locus setup --client claude` writes and the supervised variant for provider tools.

### Changed

- **Minimum supported Rust is 1.89.** The workspace `rust-version` is `1.89` (rustc 1.89.0). `cargo check --workspace --locked` succeeds on 1.89.0 and on stable 1.99.1, and fails on 1.88.0 because `File::lock` is still unstable. Cargo 1.85 through 1.88 refuse the build with `requires rustc 1.89`. Cargo 1.83.0 still fails while parsing `idna_adapter` 1.2.2 (edition 2024) before it can report `rust-version`. `rust-toolchain.toml` keeps selecting stable for in-repo builds and does not apply to `cargo install --git` outside the repo.

## [0.5.1] — 2026-10-25

### Documentation

- **Using Locus with Phantom** ([docs/PHANTOM-USAGE.md](docs/PHANTOM-USAGE.md)): `phantom exec` and the `locus exec` allowlist, the `[gateway]` block, `phm:` reference status, and LiteLLM 1.83.0 or newer (prefer a current release).

### Security

- **Audit webhook errors no longer carry the webhook URL.** A failed `locus events export --sink webhook` post (URL from `--url` or `LOCUS_AUDIT_WEBHOOK_URL`) (a non-2xx status or a connection error) put ureq's error text in the message, and that text starts with the full URL: user name, password, path token and query token included. The error now names only `scheme://host` and the status or transport kind. A new test posts to a 401, a 500 and a closed port with canary tokens in the path, query and password and fails on any 8-character run of them.

- **`locus dashboard` / `locus serve --open` no longer put `LOCUS_DASHBOARD_TOKEN` in the opened URL.** The browser opener (`open`, `xdg-open`, `gio`, `cmd /C start`) received `http://127.0.0.1:<port>/?token=<token>` as an argument, visible to other local users in `ps`, and the browser kept it in history. The plain URL is opened; with a token set the page's token gate asks for it once. `?token=` is still accepted by the API.

### Added

- Local pin history (`$LOCUS_HOME/pin-history.jsonl`, mode 0600): `pin`, `leave` and `pin_mismatch` refusals are recorded with the alias and keyed hashes (HMAC-SHA256 under a random per-install key in `$LOCUS_HOME/history.key`, 0600) of the workspace root and of the normalized git remotes (userinfo such as `user:token@` is dropped before hashing). `leave` hashes the session's workspace, not the caller's cwd. Writes are locked, use a fresh `O_NOFOLLOW` 0600 temp file and never follow a symlinked log. The remote URL and path are never stored in plaintext, nor any token, credential reference or capability. On by default and local only; turn off with `[history] enabled = false` in config.toml or `LOCUS_PIN_HISTORY=0`. Capped at `[history] max_entries` (default 2000), oldest dropped first. Recording is best-effort and never fails a pin. Groundwork for `locus autopin suggest`.
- `locus doctor --json` carries the shared `ashlr.doctor` v1 wrapper (`kind`, `schema_version`, `tool`, `status`, `native_status`, `checks`): verdict SAFE is ok, WARN warn, UNSAFE fail, a refused Phantom preflight fail; `checks` mirror `findings`. Additive: every existing field and the exit codes (0/1/2) are unchanged. The vendored handshake contract is re-pinned to the ashlr-stack version with the `doctor` section, and the conformance test validates the doctor output in a fresh home.
- Handshake conformance test (`tests/handshake_conformance.rs`): validates `locus --ashlr-handshake --json` against the Ashlr handshake v1 contract vendored from ashlr-stack (`tests/contract/`, pinned by sha256) and runs `doctor --fix --json` for the advertised `doctor-fix` capability.
- Fixed: `locus doctor --fix` (the preview) no longer initializes a missing LOCUS_HOME (it created the store directories and `daemon.key`). It now reports an empty plan and writes nothing, as documented. Found by the conformance test.
- `locus exec --expect <alias> -- <cmd>` refuses before the child starts (exit 1, the existing pre-spawn refusal code) unless the pinned binding is `<alias>`: a mismatch, no pin or a frozen pin never runs the command. The refusal names aliases only and says `locus enter <alias>`; `--json` returns the usual error document with a `pin_mismatch:` message. Without `--expect` nothing changes.
- `locus doctor --fix`: when `phantom list --json` returns an empty list while bindings hold `phm:` references, `phantom_names` is `unavailable` with reason `empty_list` and every reference stays `phm_unavailable`, never `dangling_phm_ref` (phantom-secrets 0.6.0 to 0.7.9 print `[]` for a locked vault). With no `phm:` references an empty list is still `listed`.
- `locus doctor --fix` checks `phm:` references against `phantom list --json` when the current directory has a `.phantom.toml`. A name Phantom does not list is reported as `dangling_phm_ref` (report only). Exactly `list --json` is run, with a minimal environment, a 3 second timeout and stderr discarded. Names are never printed. Any failure keeps `phm_unavailable`. `--json` adds `phantom_names` (status plus a count or a reason). Needs phantom-secrets 0.6.0 or newer. Guide: docs/doctor-fix.md.
- The `--ashlr-handshake` output lists the `doctor-fix` capability, so companions such as `stack doctor --ecosystem` can suggest `locus doctor --fix` only when this locus supports it.
- `locus doctor --fix` previews safe repairs (stale `allowed_bindings` entries, a stale `default_binding`, legacy bare Phantom names) and writes nothing; `locus doctor --fix --apply` makes them, backing up each file first. It never removes the last allowlist entry, never runs Phantom, and needs local control to apply. Guide: docs/doctor-fix.md.
- `locus status` shows how long the pin has left (`expires  in 2h 05m`), yellow with a `locus enter <alias>` hint under 15 minutes; `locus status --json` adds `expires_at` and `expires_in_secs`. `--oneline` is unchanged.
- `locus --ashlr-handshake` prints the Ashlr companion handshake v1 (name, version, protocols, capabilities, local health, files Locus may edit) as one JSON line, before argument parsing and without opening the store. Contract: docs/ashlr-handshake.md.
- **Opt-in `[exec] inherit_env`** lets the isolated `locus exec` child inherit exact, named parent variables. It is empty by default, which leaves the child environment unchanged. Key-shaped names cross only as `phm_` placeholders from `phantom exec`. Values that look like well-known keys, and URLs with userinfo, are refused. `*_BASE_URL` values must pass the `[gateway]` rules. Locus's own variables, the gateway URL and injected credentials always win. Refusals use closed codes without names or values. See [docs/PHANTOM-USAGE.md](docs/PHANTOM-USAGE.md).
- **`doctor --fix` core module (not wired to the CLI yet).** `locus_core::doctor_fix`
  plans repairs as a dry run by default: stale `allowed_bindings` entries, a stale
  `default_binding`, and legacy bare Phantom names (rewritten to `phm:NAME` through the
  existing journaled migration). Applying needs an explicit `apply_fixes` call, which
  backs up each file first (`<file>.bak-<UTC ts>`, mode 0600, or
  `<home>/backups/doctor-fix-<ts>/` for bindings). It refuses to remove the last
  allowlist entry, since an empty list allows every binding. Malformed and dangling refs
  are report only. It never resolves or reveals values and never runs Phantom; dangling
  checks use a name list the caller supplies. See `docs/doctor-fix.md`.
- **Opt-in gateway base URL for `locus exec`.** `[gateway] enabled = true` with
  `base_url` and `child_env` sets the listed `*_BASE_URL` variables in the isolated
  child to the configured URL. Same rules as phantom's gateway validator: `https`, or
  `http` only for `127.0.0.1` and `[::1]`; any other host needs an exact `host:port`
  in `allowed_hosts`. Only the URL is passed, never a key. A refused block fails
  closed (no credentials resolved, closed reason code). Off by default with no change
  when unset; parent shell base URLs are still dropped. See `docs/gateway-base-url.md`.
- **Opt-in signed credential injection audit.** `[credential_audit] enabled = true`
  appends one value-free line per injected provider to
  `audit/credential-injections.jsonl` (provider, reference, env var names, target,
  binding, session, time), HMAC-SHA256 signed and chained under a local `0600` key.
  Never records values or hashes of values. Fails closed (credential withheld) if
  a record cannot be written. Off by default. See `docs/credential-audit.md`.

- **Opt-in session budget caps.** `[budget] max_actions` / `max_minutes` in
  `config.toml` refuse further MCP tool calls once a session has used N calls or
  N minutes (from its first counted call). Off by default: with no `[budget]`
  nothing is read or written. The stop is sticky per session, audited once as
  `budget.exhausted`, and applies or undoes nothing on its own. See
  `docs/session-budget.md`.


### Added

- **Gemini CLI support.** `locus setup --client gemini` and
  `locus agent setup --apply --client gemini` (also part of `--client all`)
  register `locus-mcp` in `.gemini/settings.json` under `mcpServers`, merging
  into the user file only when it exists. Other keys and servers are preserved
  and malformed JSON is refused untouched. `agent doctor` and the agent report
  gain `mcp_registered.gemini` (additive, defaults to false).

- **`locus onboard` wizard and community adapter marketplace** (#85, ported from the
  closed #56). Community adapters are publisher-supplied executable code: a trusted
  signature binds the installable envelope to a configured verification key but does
  not establish that a command is safe, and installation always needs explicit consent.
  Community credential sources are limited to `env:` references. Read
  `docs/adapters.md` before installing any community adapter.

- **`INTEGRATION.md`** — sketch of how Phantom plugs into Locus (opt-in/out):
  the account-separation guarantees, per-job `ci mint` flow, pre-mutate gate,
  and failure modes.
- **`DESIGN-onboarding-wizard.md`, `DESIGN-adapter-marketplace.md`** —
  design proposals for guided setup and adapter discovery. The shipped
  behavior is described under `locus onboard` above and in `docs/adapters.md`.

### Changed

- **ashlr-hub to Phantom rename** — the hub was renamed to Phantom (same repo).
  `integrations/phantom/` is added alongside the existing compatibility directory
  `integrations/ashlr-hub/`; docs and scripts use the new path. Historical changelog
  entries, quoted PR URLs, and the runtime `LOCUS_CLIENT="ashlr-hub"`
  identifier are intentionally unchanged.

### Fixed

- `locus enter <alias>` (and `locus pin`) right after `locus quickstart` no longer fails with "session authority generation is stale or unknown" when quickstart ran in another shell (or through the npm wrapper). A pin whose broker generation died with its shell no longer blocks its own replacement, so `locus leave --force` is not needed first. The gate is unchanged otherwise: the capability must pass the same verifier as `locus leave --force` (no `--no-verifier` escape) before a new broker starts, then authenticate against the live broker; the old pin's seal must verify and be local control; an explicit `LOCUS_SESSION_ID` keeps the strict check; only an anchor mismatch or unavailable anchor is tolerated. The replacement is audited as `session.replace_stale` with both session ids.
- **`--json` usage errors are JSON.** `locus --json whoami --jsn` or
  `locus --json approvals` printed clap's multi-line prose, so an agent that
  asked for JSON got text it could not parse. A usage error now prints the
  same one-line `{"ok":false,"error":...}` on stderr as every other `--json`
  failure, keeping clap's tip ("a similar argument exists: '--json'") and
  exit code 2. A `--json` after `--` still belongs to the child command.
- **`locus events --limit N`** is accepted as `--last N` (also on
  `events export`); it was refused as an unknown argument.
- **MCP: a mistyped control tool or argument names the nearest match.**
  `locus_whoam` answered only "unknown tool ... control tools are listed by
  tools/list", and an undeclared key listed the allowed ones without saying
  which was meant. Both -32602 errors now end with "Did you mean
  `locus_whoami`?" / "Did you mean `aliass` -> `alias`?" when one name is
  close (the same match `locus_request_pin` uses for aliases).
- **The unpinned error says `locus enter`.** `no active pin` told people to
  run `locus pin <alias>`, while `status` and `leave` said `locus enter
  <alias>`; it now says `locus enter <alias>` and that `locus binding list`
  shows the aliases.
- **`locus events --op/--binding` with no match names what the log holds.**
  A mistyped filter (`--op pinn`, `--binding acmee`) printed only "no audit
  events matching op=pinn", so the next query was a guess. The human output now
  adds the distinct ops (and, for `--binding`, the bindings) present in the
  log. `--json` still prints `[]`.
- **`locus exec` / `run` / `ci run` refuse an option in the command
  position.** clap passes everything from the first unknown option onward to
  the child command, so `locus exec --binding acme -- cmd` (exec has no
  `--binding`) and `locus run -b x --strict-cred -- cmd` minted a session,
  wrote an audit event and then failed to spawn `--binding` with exit 127, and
  the exec ran as the pinned binding rather than `acme`. A command starting with
  `-` now exits 1 before anything runs, naming the option; for exec's
  `--binding` it points to `locus run --binding <alias> -- <command>`.
- **`locus run` / `ci run --binding <typo>` name the known aliases.** They said
  only `binding not found: acmee`; they now list the aliases and suggest the
  nearest one, as `switch` and `binding show` already did.
- **MCP `locus_request_pin` / `locus_enter_hint` name the alias an agent
  mistyped.** `{"alias": "acmee"}` said only that no such binding exists, so the
  agent had to list bindings and guess. The result now carries
  `did_you_mean: "acme"` and the message says so, using the same match as the
  CLI's `did you mean`.
- **Alias suggestions no longer name an unrelated tenant.** Up to three edits
  counted as a typo, so `locus switch nope` suggested `acme`. A suggestion now
  needs at most one edit per three characters (`persnal` still suggests
  `personal`).
- **`locus exec` / `run` / `ci run` exit codes say whether the child ran.**
  A command that could not be started exited 1, the same as a child that ran
  and failed, so an agent could not tell "tests failed" from "command not
  found". Not found now exits 127 and not executable 126 (shell convention),
  with an error naming the next step (absolute path, `sh -c` for builtins);
  `--json` keeps the JSON error line. A child killed by a signal exits
  `128 + signal` instead of 1. `locus mcp --supervised` follows the same rules.
- **A child that cannot start is still audited and cleaned up.** On a spawn
  failure `exec`, `run`, `ci run` and `mcp --supervised` returned before their
  audit event, and `run` / `ci run` also skipped removing their temporary
  session. The event now records `exit` (127/126) and a `spawn_error`, and the
  temporary session is removed.
- **MCP control tools check argument types.** With undeclared keys refused
  before dispatch (below), a declared argument of the wrong type still reached
  the tool: `locus_verify_claim {"claim": 42}` answered only "text (or claim)
  required" and `locus_request_pin {"alias": 42}` "alias required". They are
  now an `isError` result such as `` `claim` must be a string, got number ``.
  A missing `locus_request_pin` alias points at `locus_list_bindings`.
- **`locus binding add` no longer silently replaces an existing binding.**
  Re-adding an alias overwrote its tenant, account, scope and credential ref
  with no warning, under every workspace that names it. It now refuses with
  `binding 'x' already exists; nothing written`, pointing at
  `locus binding show x`. Pass `--force` to replace it. A guided add asks
  before replacing, defaulting to no, and `--dry-run` still previews.
- **`locus binding show` and `binding rm` suggest the alias you meant.** A typo
  listed nothing; they now give the same known-aliases list and nearest match
  as `locus enter`.
- **Stale-pin recovery never names a binding that does not exist.** On a stale
  pin, `locus pin nosuch` / `enter nosuch` / `switch nosuch` ended the recovery
  recipe with `locus enter nosuch`, which would fail at its last step. The
  requested alias is named only when that binding exists; otherwise the recipe
  names the stale pin's own binding (as `whoami` does). `locus leave` on a stale
  pin now names that binding too instead of `<alias>`.
- **Control-capability hints follow `$SHELL`.** `init` / `quickstart` notes,
  `capability persist`, the quickstart "daily path" line, the missing-capability
  error and the `doctor` capability findings always said
  `eval "$(locus hook zsh)"`. They now name the user's shell (zsh, fish as
  `locus hook fish | source`, else bash), like the stale-pin recovery already did.
- **`locus approve grant --ttl` is bounded.** It still used the lenient parser:
  `--ttl 0s`, `--ttl=-5m` and `--ttl=` were accepted. It now takes 1s to 24h
  like `ci mint` / `mcp mint` and fails before the approval record is read.
- **`locus watch` says why a tick is not ok.** A present but unverifiable pin
  was labelled `unpinned` next to `pinned=true`; it is now `invalid` (the
  `status --oneline` token). When the pin is ready but doctor is WARN/UNSAFE the
  tick printed only "next ready — Proceed" before exiting 1; it now adds a
  `why` line with the doctor verdict and finding codes. `--json` is unchanged.

- **MCP control tools enforce their closed schemas.** The `locus_*` tools
  publish `additionalProperties: false` but ignored undeclared keys
  (`locus_whoami {"binding": "acme"}`) and non-object `arguments` (`"acme"`),
  and an unknown `locus_nosuch` came back as an `isError` tool result while an
  unknown provider tool got `-32602`. All three are now JSON-RPC `-32602`
  before dispatch, naming the allowed keys (the earlier check, #99, covered
  synthetic provider tools only). Omitted or `null` `arguments` still mean `{}`.
  `locus_request_pin` trims the alias, so a whitespace-only alias is "alias
  required" instead of a message about a binding named `   `.
- **Mint `--ttl` is bounded.** `locus ci mint|env|run` and `locus mcp mint`
  accepted any TTL: `--ttl 0s` or `--ttl=-5m` printed an already-expired
  session or grant token and exited 0 (`ci run --ttl 0s` still ran the child),
  and `--ttl 48h` was capped silently while `locus enter --ttl 48h` is refused.
  They now take 1s to 24h (still capped by `policy.max_ttl`) and fail before
  anything is minted, with the `locus enter` wording. Second-scale TTLs stay
  allowed for expiry probes.
- **`locus binding add --provider` names a provider with no adapter.** Only the
  interactive prompt warned; the flags path that agents and scripts use wrote
  `--provider githb` silently. It is still written (custom providers use
  upstream MCP recipes), with a stderr warning listing the built-in providers;
  `--json` stdout is unchanged.
- **`locus --json setup` prints JSON.** The global flag was ignored and setup
  printed human text. It now prints `{"ok", "client", "written",
  "supervised_command"}` after a write, or `entry` / `entry_toml` for `--print`
  and `generic`. `setup --client codex --print` now uses the same escaped TOML
  as the file it writes (it was hand-built and omitted `args = []`).

- **`engagement init --workspace` and `locus workspace` never write through a
  `.locus.toml` symlink.** The existence check followed the link, so a
  dangling `.locus.toml` read as absent and the TOML was written to the link's
  target, outside the project, while the command reported writing
  `.locus.toml`. Any entry at that path (a broken link included) now needs
  `--force`, and the file is staged next to it and renamed into place, which
  replaces the link itself. `engagement init` also stages the workspace file
  and checks `.locus/` before saving the binding, so a `.locus` file, a
  `.locus.toml` directory or an unwritable project creates nothing instead of
  leaving a binding whose retry fails on "binding already exists".
- **`locus approve wait --timeout` with an overflowing value is a usage error,
  not a panic.** `99999999999999999h` saturated to `u64::MAX` and the deadline
  arithmetic panicked (exit 101); unit values that overflow are now rejected
  by the parser and a huge bare number reports `--timeout ... is too large`.
- **MCP `locus://doctor` reports a malformed binding file instead of erroring.**
  The 0.5.1 doctor fix covered `locus doctor` only; the MCP resource still failed
  the read with `binding 'x' is malformed` (-32000). It now returns the same
  UNSAFE report with a `binding_invalid` finding.
- **A pending approval on one binding no longer blocks every other pin.**
  `locus_safe_next`, `locus verify session`, `locus watch` and the agent
  report returned `external_authorization` (not ready) for any pending
  approval, so one `require_approval` hit on `acme` kept `personal` unready
  until it was denied. Only the pinned binding's requests count now, and the
  message names `locus approve deny <id>` to dismiss one.
- **Unresolvable `phm:` credentials are no longer silent.** `locus doctor`
  only checked `phantom list`, so a ref whose locator exists but which
  `phantom reveal` refuses (current Phantom Secrets refuses the
  non-interactive reveal Locus uses) reported "credentials ok". Doctor (and
  `doctor --json`, `verify session`, `watch`, `agent report`) now probes the
  real resolve path for listed `phm:` refs and flags failures in
  `unresolved_phm` as `code=reveal-failed` / `reveal-timeout`, with a human
  cause line. `locus exec` / `run` / `ci run` warn once per unresolved ref
  naming the binding, provider, env vars left unset and the cause;
  `--strict-creds` still fails before the child runs, now with the same
  detail. Values and locator names are never printed. The MCP doctor surface
  is unchanged.
- **`--json` failures are JSON.** With the global `--json`, a failing command
  (e.g. `locus --json whoami` while unpinned) printed a human `error:` line, so
  agents got non-JSON. The error line on stderr is now `{"ok": false,
  "error": "..."}` (same message, recovery hints included); stdout keeps its
  one-document contract and the exit code stays 1.
- **`locus-mcp` answers unparseable stdio frames.** A non-JSON NDJSON line or
  Content-Length body was only logged to stderr and never answered. It now
  gets a JSON-RPC Parse error (`-32700`, `"id": null`) and the server keeps
  serving. Framing/size violations still fail closed.
- **`locus workspace` warns about aliases with no binding.** `--default` /
  `--allow` naming an alias that is not in the store still writes
  `.locus.toml` (exit 0), but now warns on stderr with the missing alias and
  the `locus binding add` command, instead of failing later at `locus enter`.

- **Control commands name the stale-pin recovery.** After the pinning shell
  exits, `locus workspace`, `binding add`, `notify`, `init` and other control
  commands failed closed with only "human re-pin required". They gained the
  same recovery hint as `enter`, `leave` and `status`, while still failing
  closed. For the current dead-broker hint (#166), see the unreleased recovery
  note above: try enter first and inspect a refusal before forced teardown.
- **`locus doctor` reports a malformed binding file instead of aborting.** One
  unparseable `bindings/*.toml` made `doctor` (and `doctor --json`) exit 1 with
  a bare error and no report. It now completes, flags an UNSAFE
  `binding_invalid` finding naming the bindings directory, and exits 2.
- **npm: a root global install no longer breaks `locus` for other users.** The
  postinstall replaced the shared bin with a symlink into the installer's HOME
  (e.g. `/root/.locus/bin`, behind a 0700 `/root`), so other users got
  "Permission denied". A root install now keeps the wrapper when it is
  reachable by other users and the native binary is not.
- **`locus-mcp --version` / `-V`** print `locus-mcp <version>` and exit 0
  (was "unknown argument", exit 1).
- **Docs:** the npm README quick start adds the `eval "$(locus hook bash)"`
  step `pin` needs and links `@ashlrai/locus-mcp` (not an unrelated package);
  the main README registers `locus mcp --supervised` for provider tools and says
  plain `locus-mcp` serves control tools only.

- **`locus hook bash` emits a bash prompt** — the prompt helper now prints ANSI
  colors wrapped in readline's non-printing markers (what `\[`/`\]` stand for)
  and the hint shows `PS1=`; it previously printed zsh `%F{...}` escapes and a
  `PROMPT=` hint. `locus hook zsh` output is unchanged.
- **`locus watch --interval`** rejects `0` (minimum 1s), and every parse error
  (e.g. `30sec`) carries the `use e.g. 5s, 30s, 1m` hint.
- **`locus --help` groups** list `capability` and `client` under Setup; a test
  now checks that every visible subcommand appears in the groups.
- **`locus_request_pin` / `locus_enter_hint` with an unknown alias** return an
  error that says the binding does not exist and points at `locus binding list`
  / `locus binding add <alias>`, instead of "Pin request recorded ... run
  `locus enter <alias>`". The attempt is still audited.
- **Docs:** `docs/verification-plane.md` lists the `locus_verify_session` MCP
  tool and drops the stale "session pack is CLI/hub today" note.

### Added

- **`locus mcp --supervised`** — grants the active pin's executor capability to
  locus-mcp (the grant `locus exec` makes, with no credential values or control
  capability in the server env), so MCP clients get the pinned provider tools
  without wrapping the whole client in `locus exec`. Falls back to control
  tools only, with a stderr reason, when there is no pin or
  `LOCUS_CONTROL_CAPABILITY` (#71).
- **MCP protocol negotiation** — locus-mcp answers with the client's requested
  protocol version when it supports it (#73).
- **Release checksums** — every release archive ships with a
  `locus-<target>.tar.gz.sha256` sidecar (`sha256sum -c` format), and the
  release workflow refuses to publish when an archive's sidecar is missing or
  does not match (#49). Integrity metadata only, not a signature.

### Fixed

- **`locus setup` points at provider tools** — `locus setup` and
  `locus agent setup` register plain `locus-mcp`, which serves control tools
  only. Both now say so and print the `locus mcp --supervised` command to
  register instead for the pinned binding's provider tools. The written config
  is unchanged.
- **npm install no longer hangs** — after `npm i -g locus-cli`, the
  `locus` / `locus-mcp` wrappers found themselves on `PATH` and re-executed
  forever. They now skip Node shims when looking for an installed binary (#64).
- **`locus enter` right after `quickstart` (npm installs)** — session authority
  is anchored to the parent process; with the Node wrapper in between, it bound
  to a short-lived `node` process and the next `enter` failed with "session
  authority generation is stale". On macOS/Linux the npm package now links the
  native binary in place of the wrapper at install (and on first run), so the
  shell runs `locus` directly. New e2e test: quickstart → enter → whoami (#66).
- **npm install and first run no longer hang** — unread redirect and 404
  responses (the optional `.sha256` sidecar) kept a socket open, so
  `npm i -g locus-cli` stalled in postinstall and wrappers never exited. Both
  wrappers now drain those responses and exit explicitly.
- **npm users get `locus-mcp`** — `locus-cli` installs `locus-mcp` from the same
  release archive next to the cached `locus`, where `locus setup` looks for it.
- **Stale pins name their recovery** — `enter`, `status` and `agent report`
  say how to recover a stale pin; `status` reports `invalid` (exit 2) whenever
  `whoami` fails; the shell prompt shows invalid and frozen pins; commands
  whose stdout closes early exit 141 instead of panicking (#76).
- **Cargo fallback** — explains the slow from-source build and fixes the
  invalid `cargo install --package` command (#51).
- **Shell hook under `set -u`** — guards `ZSH_VERSION` / `BASH_VERSION` /
  `_LOCUS_PROMPT_CMD` (#46).

- **Doctor JSON validates against its schema** — `schema/doctor.schema.json`
  now allows finding severity `info` (emitted as `control_capability_persisted`
  on the default quickstart posture); real `locus doctor --json` output failed
  the published schema before.
- **Agent report readiness without an exported capability** — the report said
  `ready: true` with `status_oneline: invalid` while `locus status --oneline`
  said `alias:tenant`. Readiness now requires a verified authority anchor, and
  the oneline token keeps the pin for the capability-absent gap only.
- **`locus_whoami` on a plain (unsupervised) locus-mcp** no longer reports
  `pinned: false` with a re-pin hint; it returns
  `status: executor_authority_unavailable` and points at `locus mcp --supervised`.
- **Adapter registry catalog covers anthropic/openai** — the built-in catalog
  (`adapters/manifest.toml`, surfaced by `locus adapter list` and
  `locus adapter registry export`) now includes the `anthropic` and `openai`
  adapters (9 providers total). A drift-guard test asserts the registry id set
  equals the dispatchable provider set so future adapters cannot miss either
  surface.

### Changed

- CI: optimized crypto deps in the test profile (~6× faster `locus-core`
  tests), cancel superseded PR runs, cargo-nextest (#58, #59).
- Docs: 10-second README with a demo GIF, MCP snippets for Claude Code / Codex /
  Cursor, executable multi-client recipes, Phantom-enrolled repo guide, issue
  and PR templates (#48, #52, #53, #57, #60, #65).
- License holder standardized to AshlrAI, Inc. (#47).

### Docs

- **Shell completions** — [docs/shell-completions.md](docs/shell-completions.md)
  covers zsh, bash, fish, PowerShell and elvish, `locus completion --help`
  prints the install lines, and the Homebrew formula installs bash/zsh/fish
  completions (#63).
- **Codex** — docs/mcp.md shows the `~/.codex/config.toml` entry that
  `locus setup --client codex` writes, for wiring it by hand (#62).

## [0.5.0] — 2026-08-16

### Added

- **Signed adapter-registry release manifests** — `locus adapter registry
  export` emits a canonical JSON snapshot of the built-in adapter set
  (`locus-adapter-registry/v1`: id, name, crate version, sorted tools, and a
  `sha256` digest over each entry's canonical material). `--sign` signs it
  with an operator-provided ed25519 key (`--key <file>` or
  `LOCUS_REGISTRY_SIGNING_KEY`; base64 or 64-hex seed, never generated or
  printed; `--sign` without a key refuses to export). `locus adapter
  verify-manifest <file>` is fail-closed: the signature must verify against
  the existing trust store (`$LOCUS_HOME/trust/adapter-keys.toml` +
  `LOCUS_ADAPTER_TRUST_KEYS`) **and** the running binary's adapter set must
  match the manifest exactly (version, ids, names, tools, digests);
  `--allow-unsigned` permits only a *missing* signature for drift-only checks.
  Release CI attaches the unsigned canonical manifest as
  `locus-adapters-<tag>.json` (operators sign locally; a commented
  `TODO(registry-signing)` block documents future CI signing). Docs:
  `docs/registry-trust.md`.
- **Hub multi-tenant drop-in** — `integrations/ashlr-hub/locus.ts` gains
  `withLocusMcpTenant(binding, fn)` (mint → dispatch with
  `X-Locus-Tenant-Token` headers → always-revoke, token held in memory only),
  pure `parseMcpMintOutput` / `parseMcpListOutput` /
  `classifyTenantAuthError` helpers, and async `locusMtPreflight()`;
  `schema/mcp-grant.schema.json` documents the mint/list contract.
- **MT conformance coverage** — `scripts/e2e.sh` step 31 exercises the full
  two-tenant lifecycle (mint, isolation, cross-tenant 403, revoke → 401);
  `scripts/dogfood.sh` gains an opt-in `DOGFOOD_MT=1` probe;
  `scripts/hub-smoke.sh` / `hub-integration-test.sh` cover the grant CLI and
  live drop-in slice.

### Changed

- **HTTP auth is Bearer-only in `Authorization`** — `locus-mcp --http` now
  accepts the shared token as `Authorization: Bearer <token>` (or the
  `X-Locus-Token` / `X-Locus-MCP-Token` headers) only. The undocumented raw
  schemeless `Authorization: <token>` fallback accepted through v0.4.0 is
  rejected with `401`. Migration: prefix the header value with `Bearer `.
- **stdio frame cap** — the stdio MCP transport enforces the same 8 MB
  Content-Length cap as HTTP; an oversized frame is refused before allocation
  and the server exits with a protocol error (fail closed) instead of
  attempting an unbounded read.
- **Catalog deny annotation** — `tools/list` descriptions of tools a
  structured policy `deny` rule unconditionally blocks under the current pin
  now carry `[denied by policy under current pin]`, computed by the same
  policy engine as the call gate (companion to the existing
  `[requires human approval under current pin]` marker). `policy.default =
  "deny"` is deliberately not annotated.
- **Watch heartbeat control-capability findings** — `locus watch` ticks now
  attach the operator-shell `LOCUS_CONTROL_CAPABILITY` readiness findings that
  `locus doctor` reports, so hub heartbeat consumers see the same degraded
  posture (`session_ok` reflects the escalated verdict).

## [0.4.0] — 2026-08-16

### Added

- **Multi-tenant MCP multiplexor (`locus-mcp --http --multi-tenant`)** — one
  HTTP process serves several tenants concurrently via operator-minted grants
  (`locus mcp mint|list|revoke`, local control boundary only; agents cannot
  mint). Each grant wraps a sealed, delegated, TTL-capped session
  (`active.json` is never consulted — no ambient fallthrough) and a
  `lmt_<grant_id>.<secret>` bearer token whose HMAC-only record lives at
  `$LOCUS_HOME/mcp-grants/<grant_id>.json` (0600); the token itself is printed
  exactly once at mint. `X-Locus-Tenant-Token` is required on every `/mcp`,
  `/mcp/sse`, and `DELETE` request on top of the unchanged
  `LOCUS_MCP_HTTP_TOKEN`; the grant file is re-read per request so revocation
  propagates within one call. Fail-closed responses: uniform `401
  invalid_grant` (malformed/unknown/revoked indistinguishable; `grant_expired`
  + re-mint `safe_next` only after HMAC proof), `403 tenant_mismatch`
  (audited), `400 session_required` for stateless POSTs. Catalogs, whoami,
  drift, resources, prompts, `GET /mcp`, and SSE ticks are computed per grant;
  `locus_request_pin` / `locus_enter_hint` return `tenant_fixed_by_grant`;
  tenants cannot enumerate each other (grant listing is CLI-only). Tenant
  `Mcp-Session-Id` records live in a hard `http-sessions-mt/` partition with a
  per-grant cap (`LOCUS_MCP_SESSIONS_PER_GRANT`, default 8) and worker
  teardown when a grant's last session dies. Identity is pre-anchored from
  the grant at session mint (zero wrong-account window). New values-free
  audits: `mcp.grant_mint`/`grant_revoke`/`grant_auth_fail`/
  `tenant_session_bound`/`tenant_mismatch`/`grant_expired_swept`;
  `mcp.tools_call` rows additionally carry `grant_id` + `http_session_id`.
  Doctor gains a values-free `mcp_multi_tenant` section (active grants per
  alias, expired-unswept warning). Stdio + `--multi-tenant` is a startup
  error. Docs: [docs/mcp.md](./docs/mcp.md#multi-tenant-http---multi-tenant),
  [docs/hub-integration.md](./docs/hub-integration.md).
- **Anthropic + OpenAI provider adapters** — synthetic freeze-check tools
  (`anthropic.scope|whoami|usage|keys.list|keys.create`, same set for
  `openai.*`) for per-tenant model-API spend isolation. Org id freezes via
  `scope.account_id`, workspace/project id via `scope.project_ref`;
  provider-native selector spellings (`org`, `organization_id`, `workspace_id`,
  `project_id`, + camelCase) are freeze-netted; `keys.create` is destructive
  (hidden + denied under `read_only`). Matrix:
  [docs/adapters.md](./docs/adapters.md).
- **`locus switch <alias>`** — one-shot leave-if-pinned + enter with
  pre-flight target validation (alias suggestions + workspace allowlist
  checked **before** dropping the current pin), honoring
  `--ttl`/`--force`/`--client`, compact identity block + `--json`; audits as
  normal `session.leave` + `session.pin`.

### Fixed

- `locus leave --force` audits success only after `active.json` removal
  actually succeeds; a failed removal now writes a
  `session.force_leave_failed` audit record with the error instead of a
  false success trail.
- Claude registration probe also detects **local-scope** registrations in
  `~/.claude.json` under `projects.<cwd>.mcpServers.locus` (literal and
  canonicalized project keys); `ClaudeMcpScope` gains an additive `local`
  variant and `both` now means more-than-one-scope.

### Security

- Worker-home deletion hardened (`leave`, `leave --force`, CI/grant session
  cleanup): the recorded path and `$LOCUS_HOME/workers/` are both
  canonicalized and strict containment is required — `..`-traversal and
  symlink escapes are refused, and the workers root itself is never removed.

## [0.3.0] — 2026-08-15

Dogfood polish and release hygiene on top of 0.2.0. No identity-plane
invariant weakens — same fail-closed pin, freeze, scrub, and exclusive catalog,
now also anchored per MCP session (`pin_changed`).

### Added

- **Auto-leave TTL on `enter`/`pin` (`--ttl`)** — `locus enter <alias> --ttl 30m`
  requests a shorter pin (min 1m, max 24h), riding the existing sealed
  `expires_at` + passive fail-closed expiry check (no timer). Precedence:
  `--ttl` flag > new optional binding `policy.default_ttl` > `policy.max_ttl`,
  always capped by `max_ttl` (silent clamp + CLI warning). `default_ttl` is
  excluded from the binding fingerprint so tuning it never freezes a live
  session. Surfacing: expiry line with local time + remaining on enter/pin,
  additive `expires_in_secs` on `whoami` and the doctor `pin` slice,
  `* pinned (… left)` in `binding list`, a `pin_expiring` Warn doctor finding
  under 5 minutes remaining, and `ttl_secs`/`ttl_source` on the `session.pin`
  audit event.
- **Guided client onboarding (`locus client add`)** — interactive front door
  over the `binding add` primitive (which is now flags-first with prompts only
  for missing values on a TTY; `--guided`, `--non-interactive`, `--dry-run`).
  Prompts reuse existing validators: alias charset + reserved `locus*` prefix +
  exists/did-you-mean, provider menu from new `known_providers()`, per-provider
  scope fields, and `CredentialRef::validate` so a pasted raw secret is never
  accepted (bare Phantom names get a `phm:NAME` suggestion). Writes through the
  existing `Store::save_binding` path (validation, lock, audit) and ends with
  next steps (`enter --ttl`, `agent setup --apply`, `doctor`, `phantom add`).
  New optional flags: `--account-id`, `--repos`, `--default-ttl`. Guide:
  [docs/onboarding.md](./docs/onboarding.md).
- **Fail-closed MCP session pin-anchoring (`pin_changed`)** — every MCP session
  (stdio process / HTTP `Mcp-Session-Id`) anchors to the *identity* of the
  binding observed at initialize (or first healthy pinned observation):
  primary `binding_id` + tenant + mode + sorted `(alias, binding_id)`
  namespace pairs — never the session id. A **cross-alias re-pin under a live
  MCP session now returns a structured `pin_changed` tool error** (anchored vs
  current identity + `safe_next.action=reinitialize_client`) instead of
  silently adopting the new identity, and it outranks the
  `runtime_unhealthy`/`executor_authority_unavailable` noise from the staled
  executor grant. Same-alias re-pin (TTL refresh) is unaffected at the anchor
  layer (a stdio process whose executor capability was granted for the old
  session may still need a client restart — authority-plane fact, not masked).
  The refusal is session-local: it never mutates or freezes `active.json`.
  Recovery: re-initialize/restart the client to adopt the new pin (HTTP: POST
  `initialize` with the existing `Mcp-Session-Id` re-anchors in place), or
  `locus enter <anchored>` to restore. Control tools
  (whoami/status/heartbeat/safe_next/verify_session) gain an additive
  `mcp_anchor` block (omitted when no anchor exists); `locus_verify_session`
  forces `session_ok=false` on mismatch so hub gating fails closed per
  session; `tools/list` collapses to control tools tagged with the anchored
  alias; `GET /mcp` reports `anchor_ok` when a session id is presented. The
  anchor survives `locus leave` (not_pinned refusals gain anchor context),
  `LOCUS_SESSION_ID` overlay sessions are structurally immune, and the HTTP
  session disk format gains an optional serde-default `anchor` field (still
  v1 — old binaries ignore it, old records adopt-once; enforced across
  process restarts from disk). New audits: `mcp.anchor_established`,
  `mcp.anchor_repin`, `mcp.anchor_reset`, `mcp.anchor_mismatch` (deduped;
  aliases/tenants/binding_ids/session_ids only). Sessionless HTTP POSTs
  (no `Mcp-Session-Id`) share a **process-level anchor** (same decide()
  machinery, stdio parity) so stateless provider `tools/call` is pin-swap
  protected too; a fresh sessionless `initialize` re-anchors it (adoption
  path). Optional `LOCUS_MCP_HTTP_REQUIRE_SESSION=1` still refuses
  sessionless provider `tools/call` outright (default off — stateless CI
  POSTs keep working, anchored at process scope).
- **Grok Build write path + Claude user scope** — `locus setup --client grok`
  and `locus agent setup --apply --client grok` now write Grok Build's
  documented `~/.grok/config.toml` (`[mcp_servers.locus]`, Codex-style TOML)
  with the same fail-closed `toml_edit` merge as Codex (unparseable ⇒ abort
  untouched); grok joins `--client all` and the registration probe
  (`mcp_registered.grok`) reads `~/.grok/config.toml` by default
  (`LOCUS_GROK_MCP_CONFIG` still overrides; JSON or TOML);
  `scripts/dogfood-clients.sh` detects Grok. `--client generic`
  stays print-only for clients without a known config path. New
  `locus agent setup --claude-scope user|project` (default `project`):
  user scope registers for all projects via the claude CLI
  (`claude mcp add-json locus … --scope user`, verified with
  `claude mcp get locus`) — `~/.claude.json` is never hand-edited, and setup
  fails closed with instructions when the `claude` CLI is absent. JSON server
  entries for Claude/Cursor now carry `"type": "stdio"` (documented as
  required by Cursor; harmless elsewhere).

- **`locus_verify_session` MCP tool (M5)** — hub session pack over MCP, same JSON as
  `locus verify session --json` (`{ kind: "session", version, whoami?, doctor, safe_next,
  session_ok }`). Available unpinned; gate on `session_ok`; `isError` only on hard store
  failures. Values-free — aliases, verdicts, scopes only. Audit keys
  (`session_ok` / `safe_next` / `doctor_verdict` / `doctor_ok` / `has_whoami`) aligned with
  the CLI. Docs: [docs/mcp.md](./docs/mcp.md), [docs/hub-integration.md](./docs/hub-integration.md).
- **Control-capability onboarding** — `locus init` / `locus quickstart` mint and persist
  `LOCUS_CONTROL_CAPABILITY` (`$LOCUS_HOME/control_capability`, mode `0600`, never
  overwritten, value never printed); `locus hook zsh|bash|fish` exports it when the shell
  lacks one; `locus doctor` flags missing / not-exported / invalid / mismatched capability
  with the exact fix. Docs: README quick start, [docs/mcp.md](./docs/mcp.md),
  [docs/agency-starter.md](./docs/agency-starter.md).
- **Control-capability posture management** — persistence stays the fresh-store
  default; strict operators opt out with `--no-persist-capability` on
  `init` / `quickstart` (mint to process env only, print the export line, never
  write the file) and manage posture with the new
  `locus capability status|persist|unpersist` subcommand (`status` never prints
  the bearer; `persist` writes 0600, idempotent, fail-closed on mismatch;
  `unpersist` removes the file and prints the export line). New doctor severity
  **Info** — can never escalate the verdict — carries the INFO finding
  `control_capability_persisted` whenever a valid capability file exists, with
  the exact strict-posture fix. Docs: new "Control-plane authority boundary"
  section in [SECURITY.md](./SECURITY.md) (the capability file is ambient for
  same-user processes, `LOCUS_HOME` is same-user-readable regardless, and the
  MCP surface — not the CLI — is the agent boundary).
- **Post-write verification in `locus agent setup --apply`** — re-reads every client
  config it wrote via the registration probe and exits 1 naming each failing client+path;
  warns when the `locus-mcp` path falls back to a bare name; explicit `--mcp-bin` must exist.
- **Alias ergonomics** — `enter` / `pin` unknown-alias errors list known aliases with a
  did-you-mean suggestion; `binding list` marks the pinned alias; the `locus*` alias prefix
  is reserved at create/import (unroutable through the MCP gate), with a doctor warn for
  legacy bindings.
- **Hub drop-in watch/verify session heartbeat** — port pure + shell helpers from hub
  [PR #273](https://github.com/ashlrai/ashlr-hub/pull/273) into
  [`integrations/ashlr-hub/locus.ts`](./integrations/ashlr-hub/locus.ts):
  `parseWatchHeartbeat`, `parseSessionVerificationPack`, `locusVerifySession`,
  `locusWatchOnce`, `locusSoftWatchHeartbeat` (soft annotation under `LOCUS_ENFORCE=warn`
  only — never a hard blocker alone). Firm onboard soft-offer remains hub CLI-only (#274).
  Docs: [docs/hub-integration.md](./docs/hub-integration.md),
  [docs/verification-plane.md](./docs/verification-plane.md).
- **HTTP `Mcp-Session-Id` file-backed resume (M5)** — in-memory cache plus disk map under
  `$LOCUS_HOME/http-sessions/` (override `LOCUS_MCP_SESSION_DIR`). Atomic write, idle TTL
  prune, fail-closed corrupt files; stores id + timestamps + optional pin summary only
  (never secrets). Restarts / multi-worker resume via load-on-miss. Docs: [docs/mcp.md](./docs/mcp.md).

- **Opt-in worker sandbox network deny** — `LOCUS_WORKER_SANDBOX_NO_NETWORK=1` or
  `upstream.sandbox_no_network = true` applies harder isolation without changing the MCP
  default (network still **allowed** unless opted in). Linux `bwrap` gets `--unshare-net`;
  macOS Seatbelt omits outbound allows (best-effort). The Linux `path` backend fails closed
  when no-network is requested. Docs: [docs/workers.md](./docs/workers.md).
- **Multi-message SSE on `locus-mcp --http` (M5 partial)** — when Accept prefers
  `text/event-stream` (or lists SSE and the JSON-RPC body is large), POST `/mcp`
  streams multiple `event: message` frames: `locus.sse.progress` (+ optional
  `locus.sse.chunk` text slices) then the complete JSON-RPC result. Header
  `X-Locus-Streamable: sse-multi|sse-single`. Env: `LOCUS_MCP_SSE_MULTI_BYTES`,
  `LOCUS_MCP_SSE_CHUNK_BYTES`.
- **`GET /mcp/sse` session heartbeat** — token-auth SSE stream of values-free
  `locus.session_tick` (`session_ok`, doctor verdict, safe_next, pin alias).
  Query `?once=1` / `?interval=5s`; env `LOCUS_MCP_SSE_INTERVAL`. Fail-closed auth.
  Hub alternative to CLI `locus watch` over HTTP.
- Docs: [`docs/mcp.md`](./docs/mcp.md) multi-SSE + session stream; GOALS M5 partial updated.
- Tests: multi-SSE tools/list, Accept dual upgrade, `/mcp/sse` auth + once tick.

- **Linux worker sandbox partial (M5)** — `LOCUS_WORKER_SANDBOX=1` / `upstream.sandbox`
  on Linux prefers bubblewrap when installed (`LOCUS_WORKER_SANDBOX_BACKEND=bwrap`:
  RO system roots, bind work tree + session worker home only, shared network for MCP,
  no `~/.locus/bindings` bind). Falls back to best-effort `path` (restricted PATH +
  absolute executable only — **not** kernel isolation; tag is explicit). macOS Seatbelt
  unchanged. Docs: [docs/workers.md](./docs/workers.md). Not a full seccomp/VM.
- **Streamable-HTTP-lite MCP (M5 partial)** — `locus-mcp --http` gains session-sized
  polish toward remote multiplexor without a full SSE rewrite:
  - `GET /mcp` (token) — capabilities + pin summary + tool **names** only (values-free)
  - Accept negotiation: prefer `application/json`; single-event `text/event-stream`
    when Accept is SSE-only; **406** when Accept allows neither; **415** on non-JSON body
  - `GET /health` advertises `transport: streamable-http-lite` + endpoint map
  - Remote deploy notes in [`docs/mcp.md`](./docs/mcp.md) (reverse proxy, token,
    `LOCUS_HOME`, pin-before-serve)
  - Tests: health, auth fail-closed on GET/POST `/mcp`, capabilities shape, SSE-lite, 406/415
- **`locus watch` session heartbeat (M5)** — each tick runs the same pack as
  `locus verify session` (doctor + whoami + safe_next + `session_ok`), not drift alone.
  - `--json`: one NDJSON object per tick
    `{ kind:"watch", session_ok, whoami?, doctor_verdict, safe_next, pinned, frozen }`
  - `--require-ok`: fail closed whenever `session_ok` is false (hub / CI)
  - `--once` without `--require-ok`: non-zero only when a pin was present/expected and not ok
  - Docs: [docs/verification-plane.md](./docs/verification-plane.md)
- **Audit webhook sink (M5 partial)** — `locus events export --sink webhook [--url URL]`
  posts redacted fleet-pulse JSONL or OTLP JSON to a remote URL for SIEM / log shippers.
  Env: `LOCUS_AUDIT_WEBHOOK_URL`. **Fail soft** when URL unset (skip, exit 0);
  **fail closed** if export body matches secret patterns (refuse POST). See
  [`docs/observability.md`](./docs/observability.md).
- **`scripts/dogfood.sh`** — after forensics export, also prints **`locus goal status`**
  (from repo `GOALS.md` when present) and runs **`scripts/hub-smoke.sh`** (own throwaway
  home; skip with `DOGFOOD_SKIP_HUB_SMOKE=1`)
- Shell e2e feature-detects post-0.2 surfaces:
  - **`locus verify claim --text … --json`** (URL claims → `needs_tool`)
  - MCP **`locus_safe_next`** unpinned (`action=enter`) + pinned (no secrets)
  - **`locus upstream list --json`** (recipe catalog)
  - Optional **`locus watch --once --json`** heartbeat shape (no secrets)
  - Re-asserts **notify disabled by default** after the full suite
- Landing page ([`apps/web/`](./apps/web/)) — verify claim + `locus_safe_next` callouts
  alongside dashboard / forensics / HTTP MCP / goal status

### Changed

- **MCP auto-pin is now honestly advisory-only (`locus-mcp`)** — the server
  never pins. The once-per-process probe resolves the workspace target
  read-only and audits **`session.auto_pin_denied`** (advisory binding +
  operator-delegation refusal reason) instead of minting authority;
  `pin_auto_delegated` fails closed with a clear "auto-pin requires operator
  delegation, which is not available" error (workspace `.locus.toml` defaults
  are advisory hints only — an agent/MCP process cannot self-issue session
  authority). `LOCUS_AUTO_PIN` / `LOCUS_MCP_AUTO_PIN` knobs stay parsed as
  probe enable / kill switches pending an operator-delegation design. All
  advertising surfaces (tool/resource descriptions, `.locus/AGENT.md`, setup
  notes, [docs/mcp.md](./docs/mcp.md)) now say the server never pins; the
  session-anchor layer anchors only after a human pin.
- **Docs: hub #277 firm soft-warn + goals sync** — document ashlr-hub
  [PR #277](https://github.com/ashlrai/ashlr-hub/pull/277) firm fleet soft-warn
  (doctor/readiness only: enrolled&gt;0 + locus available + `locus.firm` false →
  non-blocking `locus-firm` warn; **never hard-blocks mutate**; monorepo firm
  default stays off). Hub-only `checkLocusFirm` contract sketch; do **not** port
  into [`integrations/ashlr-hub/locus.ts`](./integrations/ashlr-hub/locus.ts).
  GOALS M4 marks #277 landed; always-on firm default remains open. GOALS M5 notes
  cross-process HTTP session resume landed ([#31](https://github.com/ashlrai/locus/pull/31));
  still open only multi-tenant remote multiplexor. Docs:
  [GOALS.md](./GOALS.md), [docs/hub-integration.md](./docs/hub-integration.md),
  [integrations/ashlr-hub/doctor-check.md](./integrations/ashlr-hub/doctor-check.md),
  [integrations/ashlr-hub/README.md](./integrations/ashlr-hub/README.md),
  [integrations/ashlr-hub/fleet-preflight.md](./integrations/ashlr-hub/fleet-preflight.md).

### Fixed

- **MCP anchor health surfaces use the gate's identity comparison** —
  `mcp_anchor` / `anchor_ok` / `locus_verify_session` overrides now compare
  the full anchored identity (binding_id + tenant + **mode + namespace
  triples**) when a full observation is available, primary-only just when only
  the drift identity exists — so `session_ok=false` whenever provider tools
  would refuse with `pin_changed` (a mode/namespace re-pin no longer reads
  healthy while tools are wedged).
- **Multi-worker HTTP anchor writes are last-writer-safe** — the per-request
  session touch and cross-worker observations adopt the on-disk anchor
  (authoritative) before persisting, so one worker's initialize-time reset or
  just-persisted establishment is never clobbered by a sibling's stale
  in-memory anchor. Anchor values are only authored by
  establish/repin/initialize-reset paths.
- **Claude user-scope registration is add-first** — `locus agent setup
  --claude-scope user` no longer removes the existing `locus` entry before
  adding: a failing add leaves any previous registration intact (and says
  so); only a CLI "already exists" refusal triggers remove + re-add, and a
  failure after that removal is reported honestly (entry removed, currently
  unregistered) instead of claiming nothing changed.
- **TOML paste output escapes interpolated values** — `[mcp_servers.locus]`
  snippets (`agent setup --client generic|grok`, `setup --print`)
  basic-string-escape the `locus-mcp` path and env values, so quotes /
  backslashes in the binary path can no longer emit invalid TOML.
- **Phantom deadline runner cannot leak blocked reader threads** — the child
  runs in its own process group and the deadline kill takes out grandchildren
  holding the stdout pipe (wrapper shells); the reader thread is reaped with
  a bounded join, and a hard cap on live helper threads fails closed instead
  of accumulating leaks against a repeatedly wedged `phantom`.
- **Doctor reports an honest authority-anchor verification status** — new
  additive `pin.authority_anchor_verified` field: `false` (with `seal_ok`
  still `true` and the existing `executor_authority_unavailable` Warn
  finding, never a false UNSAFE) when the anchor check was skipped because no
  control capability was present; `true` only when the check actually passed.
- **MCP doctor packs run real external probes** — `locus_verify_session`, the
  `locus://doctor` resource, and `GET /mcp/sse` session ticks now share core
  `gather_doctor_external` (Phantom on PATH + unresolved `phm:` refs — provider/source
  metadata only, locator names never leak) instead of a hardcoded conservative failure.
  `session_ok` can be `true` on a healthy pin and MCP output matches
  `locus verify session --json`.
- **Fail-closed MCP config merges** — a malformed `.mcp.json` / `~/.cursor/mcp.json` is
  left byte-for-byte unchanged with a remediation error (previously it was silently
  replaced, destroying other registered servers); the Codex merge is now a
  format-preserving `toml_edit` upsert of `[mcp_servers.locus]` that heals stale command
  paths and missing env tables, preserves other tables/comments, and cannot produce
  duplicate keys.
- **`locus setup` parity with `agent setup`** — writes the same agent env
  (`LOCUS_AUTO_PIN=cwd`, `LOCUS_CLIENT`) instead of an empty `env: {}`; the codex arm
  actually merges `~/.codex/config.toml` (`--print` emits the full `[mcp_servers.locus]`
  + env TOML); quickstart/doctor hints standardize on `locus agent setup --apply`.
- **Authority broker start under CI load** — production handoff wait raised from 3s → 10s;
  optional `LOCUS_AUTHORITY_BROKER_START_TIMEOUT_MS` override; timeout errors may include a
  short broker stderr snip. `scripts/hub-smoke.sh` retries init/pin and defaults the
  override to 15s so hub contract smoke stays resilient after heavy test jobs (flaky
  "broker startup timed out" unrelated to webhook export).

### Tests

- Core: webhook POST against local `TcpListener`, secret-scan fail-closed, URL resolve
  (explicit / env / blank → soft skip)
- Shell e2e (`scripts/e2e.sh`): prior 0.2 surface **plus** verify / safe_next / upstream /
  watch heartbeat / notify late re-check — **44+ checks** on full current command set
- CLI unit tests: watch interval parse, fail policy, heartbeat from pinned/unpinned packs
- Core unit `inv_notify_default_false` + e2e step 15/27 keep desktop notifications **OFF**
  by default under clean `LOCUS_HOME`
- Composite worker tests for top-adapter recipe expansion (`github-mcp`,
  `supabase-mcp`, `vercel-mcp` sandbox gates), exclusive synthetic catalog +
  scope freeze alongside recipe-shaped upstream, and ambient provider-secret
  exclusion when `resolve_secrets = false`

### Security

- **HTTP pre-auth hardening (`locus-mcp --http`)** — request caps enforced before any body
  allocation or the token check: **413** above 8 MB `Content-Length`, **431** above 32 KB
  header bytes or 128 header fields; an unparseable JSON-RPC body returns **400** with
  JSON-RPC `-32700` instead of a dropped connection; `Mcp-Session-Id` is minted **only on
  `initialize`** (other POSTs without the header are served statelessly), so garbage POSTs
  can no longer exhaust session capacity.
- **Scope freeze covers provider-native selector spellings** — the upstream-worker
  preflight now freezes camelCase and provider-native aliases
  (`projectRef`/`project_id`/`projectId`, `teamId`, `accountId`, `aws_account_id`/`awsAccountId`,
  `stripe_account`/`stripeAccount`) alongside the canonical snake_case keys at any depth
  inside object **and array** args (bounded scan — args nesting past the limit deny fail
  closed); non-string values under a frozen selector key and Stripe `livemode` flips are
  denied at any depth too; `org`/`owner`/`organization` args must be members of the frozen
  `scope.orgs`; tools whose
  provider prefix is not declared on the pinned binding are explicitly denied (fail closed).
- Local CLI/dashboard approval assertions are now explicit `local_advisory` evidence and can no longer unlock provider execution or satisfy dual control. Stdio MCP, HTTP MCP, dashboard, doctor, and forensics expose the disabled external-authority state; gated calls fail closed until an independent cryptographic verifier exists.
- Caller-controlled principal strings, Touch ID mocks, dashboard mutations, and unsigned same-user JSON never count as human authority. Provider execution remains blocked until a peer-authenticated OS broker verifies a non-agent-accessible issue capability.

## [0.2.0] — 2026-08-09

AI-native / hub-native identity plane polish on top of 0.1.1. Local dashboard,
forensics packs, HTTP MCP, goal loop, agent report contract, verification stubs,
upstream recipes, and faster doctor probes. **Wrong account, still impossible.**

### Added

#### Local identity dashboard

- **`locus serve [--port 8750] [--token] [--open]`** — loopback-only Axum server (UI + JSON API)
- **`locus dashboard`** — same server + open browser (`--no-open` to skip)
- API: `GET /api/health|status|whoami|bindings|approvals|doctor|events`, `POST /api/approve/{id}/grant`
- Security: bind `127.0.0.1` only; no resolved secrets; optional `LOCUS_DASHBOARD_TOKEN`
- UI: [`apps/dashboard/`](./apps/dashboard/) embedded in the CLI binary

#### Forensics & observability

- **`locus forensics export [--binding] [--out pack.json]`** — shareable pack: pin/session meta, binding summaries, audit tail, doctor snapshot, pending approvals, near-miss, chain tip (**no secrets**)
- **`locus events export [--otlp] [--out file]`** — fleet-pulse JSON lines or OTLP Logs JSON; [`docs/observability.md`](./docs/observability.md)
- Doctor **`near_miss_count` / `near_miss`** — scope_freeze + require_approval blocks in the last 24h

#### AI-native agent surface

- **`locus agent setup|doctor|report`** — wire MCP clients, readiness ladder, hub JSON contract
- **locus-mcp resources** — `locus://session`, `locus://doctor`, `locus://bindings`
- **Prompt** — `locus_context` system fragment (`prompts/list` + `prompts/get`)
- Tool descriptions tagged `[locus:<alias|unpinned>]`; `locus_whoami` always first
- `initialize.instructions` agent rules; capabilities advertise tools + resources + prompts
- **MCP auto-pin** from workspace / `LOCUS_AUTO_PIN=cwd` / `LOCUS_MCP_AUTO_PIN` (kill switch `=0`); audit `session.auto_pin`; never force allowlist
- **`locus_safe_next`** — single best next action (`enter` / `re_pin` / `approve` / `doctor_fix` / `ready`)
- **HTTP MCP** — `locus-mcp --http 127.0.0.1:8742` or `LOCUS_MCP_HTTP=1`; `POST /mcp` + `GET /health`; requires `LOCUS_MCP_HTTP_TOKEN`; loopback by default

#### Verification plane (M5 stubs)

- **`locus verify claim --text "…"`** — heuristic claim scoring (`confidence`, `needs_tool`, `suggestion`, `signals`, optional pin grounding)
- MCP **`locus_verify_claim`** — same shape (available unpinned)
- Doctor may WARN on recent low-confidence claim signals; [`docs/verification-plane.md`](./docs/verification-plane.md)

#### Upstream recipes

- **`locus upstream list`** / **`locus upstream suggest <provider>`**
- Binding TOML: `upstream = { recipe = "github-mcp", resolve_secrets = true }` (`adapters/recipes.toml`)
- Recipes: `github-mcp`, `github-official`, `supabase-mcp`, `filesystem-mcp`, `everything-mcp`

#### Goal loop & hub composition

- **`locus goal status [--json]`** — northstar progress from [`GOALS.md`](./GOALS.md) or embedded milestones
- **[GOALS.md](./GOALS.md)** — vision, goal tree, milestone checklist, success metrics
- Hub drop-in [`integrations/ashlr-hub/`](./integrations/ashlr-hub/) — `locus.ts` (attested parent-broker gate, env-free `withLocusSession`, `registerLocusInMcpConfig`, pure parse helpers), gateway snippet, doctor-check, [`fleet-preflight.md`](./integrations/ashlr-hub/fleet-preflight.md)
- Schemas: [`schema/agent-report.schema.json`](./schema/agent-report.schema.json), [`schema/doctor.schema.json`](./schema/doctor.schema.json), [`schema/hub-gate.schema.json`](./schema/hub-gate.schema.json)
- [`scripts/hub-smoke.sh`](./scripts/hub-smoke.sh) + [`scripts/hub-integration-test.sh`](./scripts/hub-integration-test.sh) composition smoke

#### DX

- **`locus quickstart`** — first-60s bootstrap: samples → enter → whoami + doctor
- **`locus completion <shell>`** — bash/zsh/fish/elvish/powershell
- **`locus topic <name>`** / **`locus help topic <name>`** — product guides (`dashboard`, `forensics`, `serve`, `goal`, `verify`, `agent`, `mcp`, `http`, `upstream`)
- **Adapter SDK** — [`docs/adapter-sdk.md`](./docs/adapter-sdk.md), `examples/adapters/_template/`, `adapters/manifest.toml`

### Changed

- **`phantom --version` process-cached** — doctor, agent report, forensics, and dashboard share one probe per process (faster hot paths)
- Dashboard `/api/doctor` skips deep `phantom list` inventory (CLI doctor still full-checks)
- **`locus init`** — `notify.enabled = false` by default; AI-native next steps; annotated samples
- Shell hook — `[locus:FROZEN]`, `[locus:enter!]` / `require_pin` oneline token
- CLI help regrouped: Setup · Daily use · CI · Approvals · Audit · Local UI · Maintenance
- Landing page ([`apps/web/`](./apps/web/)) — AI-native identity plane hero, dashboard / forensics / HTTP MCP / goal status
- Packaging version-aligned at **0.2.0** (Cargo workspace, npm `locus-cli` / `locus-mcp`, homebrew formula mirror)

### Tests

- Shell e2e (`scripts/e2e.sh`): prior 0.1.1 surface **plus** feature-detected dashboard `/api/health`, `forensics export`, `goal status`, `topic` — **38 checks** (0 skipped) on full 0.2 command set
- MCP HTTP transport tests; agent report / forensics pack schema keys; protocol resources/prompts/auto-pin

### Security

- Same fail-closed invariants as 0.1.x (seal, exclusive catalog, scrub, scope freeze, agents cannot pin)
- Dashboard and HTTP MCP bind loopback by default; token required for MCP HTTP; no resolved secrets in API or packs
- Forensics / events export / agent report expose credential presence/source metadata only, never secret values or locator names.
- Bindings require explicit supported credential refs (`phm:` or `env:`); release binaries reject `test:` regardless of environment, and bare/raw/unsupported values fail closed without being echoed in errors.
- MCP, CI, CLI, audit, worker, dashboard, and child-process surfaces never expose locator names or provider stderr.
- Malformed, unreadable, broken-link, or non-file nearest `.locus.toml` blocks pin/autopin/run (including forced pin), and doctor reports `UNSAFE`.
- Legacy bare Phantom names are surfaced as invalid and can be converted only through explicit `binding migrate-credential-refs ... --write`; unsafe values require manual repair.
- Credential-ref migration uses a locked, fsynced intent/completion journal and no-clobber staged replacement; exact retries reconcile crashes, concurrent replacements, and pending audit writes without locator disclosure.
- Hub ephemeral-session helpers scrub ambient credentials and accept only validated identity/scope metadata from `ci mint`; the dogfood gate now requires the same strict `ready` dispatch contract as Fleet preflight.

## [0.1.1] — 2026-08-09

Firm UX polish on top of the initial public release. Notifications stay quiet by
default; doctor is a single mission-control pane; run/ns cover one-shot and
experimental multi-bind workflows; local encrypted graph share + CI mint land.

### Added

#### Firm UX

- **`locus enter` / `leave`** — firm workflow pin + clear identity
- **`locus engagement init|close`** — client engagement lifecycle (metadata; never deletes vault secrets)
- **Autopin** — default binding from workspace / config when pin has no alias
- **`locus run -b <alias> -- <cmd>`** — one-shot child session; global pin unchanged (`--share-pin` opt-in)
- **`locus pin --ns a,b`** — experimental namespaced multi-binding; MCP tools appear as `alias__tool`
- **`locus watch`** — drift freeze when the pinned binding changes under a live session
- Structured policy rules + improved `approve wait` / grant UX

#### Doctor single pane

- **`locus doctor`** — home/seal/bindings, active pin (alias/tenant/expires/seal), runtime drift, pending approvals + dual-control waiting, phantom + unresolved phm refs, autopin/`config.toml`, workspace allowlist, last 5 audit ops
- Verdict **SAFE | WARN | UNSAFE** (exit **0 / 1 / 2**); stable mission-control JSON schema
- **`locus events --last N [--op] [--binding] [--json]`** — audit JSONL tail with filters
- MCP **`locus_heartbeat`** — agent-safe drift/runtime summary (never secrets)

#### Graph & CI

- **`locus graph list|export|import`** — encrypted local binding-graph share (bindings + workspace templates, CredentialRefs only; `LOCUS_GRAPH_PASSPHRASE`)
- **`locus ci mint|env|run`** — short-lived sealed sessions under `sessions/ci-*.json` without mutating `active.json`; children set `LOCUS_SESSION_ID`

#### Agency kit & docs

- Agency starter kit — [`examples/agency-starter/`](./examples/agency-starter/) + [`docs/agency-starter.md`](./docs/agency-starter.md)
- [docs/agency-certainty.md](./docs/agency-certainty.md) — identity vs epistemic certainty (Ashlr stack)
- Firm-mode docs refreshed for enter/run/notify/doctor daily path

#### Packaging

- Release workflow still publishes `locus-<triple>.tar.gz` for aarch64/x86_64 Darwin + x86_64 Linux
- Homebrew formula mirror + npm wrappers (`locus-cli`, `locus-mcp`) version-aligned at **0.1.1**

### Changed

- **Desktop approval notifications are OFF by default** — no spam, no sound; opt in with `locus notify on` or `LOCUS_NOTIFY=1`; kill switch `locus notify off` / `LOCUS_QUIET=1` / `CI=true`; silent + rate-limited when enabled
- CLI help regrouped: Setup · Daily use · CI · Approvals · Audit · Maintenance

### Tests

- Shell e2e (`scripts/e2e.sh`): pin/isolation/MCP freeze/approval, dual-control two-principal grant, doctor exit codes, events, enter/run, notify off-by-default, graph export/import, `ci mint`, `locus_heartbeat` — all **feature-detected** where optional — **34 checks** (0 skipped) on full 0.1.1 command set
- Core unit coverage for doctor verdicts, namespaced tool prefixing, notify defaults, CI session isolation

### Security

- Same fail-closed invariants as 0.1.0 (seal, exclusive catalog, scrub ambient identity, scope freeze)
- Notify path is best-effort UX only — never surfaces errors on the agent/tool path
- Graph export never includes secret values (refs + templates only); encrypted at rest with passphrase
- Namespaced multi-bind remains explicit opt-in (`--ns`); exclusive pin stays the default isolation model
- CI mint does not rewrite the human shell pin (`active.json`)

## [0.1.0] — 2026-08-06

Initial public release of **Locus** — identity plane for coding agents.
**Wrong account, impossible.**

### Added

#### Control plane (daemon-less)

- Workspace crates: `locus-core`, `locus-cli` (`locus`), `locus-mcp`
- Binding store under `~/.locus` (overridable via `LOCUS_HOME`)
- CLI: `locus init`, `pin`, `leave`, `whoami`, `status`, `binding list|show|add|rm`
- HMAC-sealed session pins (tampering fails closed; seal verified on privileged paths)
- Workspace `.locus.toml` — default binding, allowlist, `require-pin`
- `locus exec --no-resolve` — manual identity diagnostics only; provider credentials stay inside policy-gated MCP workers
- Private worker dirs under `~/.locus/workers/<session>/` (GH/AWS config isolation)
- Shell hooks: `locus hook zsh|bash|fish`
- Credential refs: `phm:NAME` and `env:VAR`; production binaries always reject `test:`
- Continuous identity check: `Store::verify_runtime` / drift surface (seal, binding id, tenant, expiry)
- Path-safe binding/approval ids (`validate_name_component`, `validate_approval_id`)

#### MCP multiplexor

- `locus-mcp` stdio server (JSON-RPC 2.0 / MCP subset; NDJSON + Content-Length)
- Control tools: `locus_whoami`, `locus_status`, `locus_list_bindings`, `locus_request_pin`, `locus_providers` (when pinned)
- Agents **cannot** pin — `locus_request_pin` returns instructions only
- Unpinned session ⇒ control tools only; seal verified on tools/list and tools/call
- `locus setup --client claude|cursor|codex` for MCP config merge
- `locus doctor` readiness checks

#### Provider adapters & scope freeze

Synthetic identity/scope tools with hard freeze on account selectors:

| Provider    | Freeze knobs                                      |
|-------------|---------------------------------------------------|
| Supabase    | `project_ref`, `read_only`                        |
| GitHub      | orgs / repos surface                              |
| Vercel      | `team_id`, projects, env                          |
| Cloudflare  | `account_id`                                      |
| AWS         | `account_id`, profile (extra)                     |
| Stripe      | `account_id`, `livemode` (bool freeze)            |
| Resend      | domain allowlist                                  |

- Model-supplied selector mismatch → error (not warn)
- Generic `{provider}.scope` for unknown providers

#### Policy & human approvals

- Policy defaults: `allow` / `deny` + `require_approval` globs
- Dual-control policy declarations (external authority adapter still required)
- Approval store under `~/.locus/approvals/{id}.json` (args never stored raw — `args_digest` only)
- Stable `approval_id` (`appr_<24 hex>`); local advisory evidence via `locus approve grant <id> --as <label>`
- Canonical `args_digest`: key-order independent, nested secret keys stripped
- Advisory records remain pending; deny is terminal; authoritative TTL path is disabled

#### Workers & upstream MCP

- Synthetic worker backend (in-process adapters)
- MCP stdio worker: spawn upstream MCP with isolated env, handshake, tools/call fan-out
- Per-provider `upstream` in binding TOML (`command`, `args`, `resolve_secrets`)
- Composite worker manager (synthetic + optional upstream per provider)
- Example: [`examples/upstream.binding.toml`](./examples/upstream.binding.toml)

#### Packaging & docs

- Open-source: CI, release workflow, CONTRIBUTING, SECURITY, CODE_OF_CONDUCT
- Adapter / MCP / workers docs under `docs/`
- Homebrew formula mirror: `integrations/homebrew`
- npm wrappers: `locus-cli`, `locus-mcp` (download release binary or cargo fallback)
- Sample bindings: `examples/acme.binding.toml`, `personal.binding.toml`, `workspace.locus.toml`

#### Hardening tests

- Property-style `args_digest` tests (key order, nested objects, secret strip)
- Adapter freeze: Cloudflare `account_id`, Stripe `livemode`, AWS `account_id`
- Pin/leave stress (many sequential cycles)
- Invalid seal after leave/re-pin + recover
- Binding validate: empty providers, bad alias, incomplete provider, malformed TOML
- Isolation integration + MCP protocol freeze-deny paths

### Security

- Session seal verification on MCP tool list/call paths
- Unbound session ⇒ control tools only
- MCP never returns secret values
- Workspace allowlist + optional `--force` for out-of-allowlist pins
- Approval id path traversal rejected
- Ambient CLI identity scrubbed on `locus exec` / worker spawn

### Known limitations

- Most adapters remain identity/scope stubs; live upstream fan-out depends on per-binding `upstream` config
- Team binding graph / multi-namespace sessions — later (see [PLAN.md](./PLAN.md)); experimental `--ns` is local-only
- Homebrew source `sha256` must be refreshed after each tag (see [docs/RELEASE.md](./docs/RELEASE.md))

[Unreleased]: https://github.com/ashlrai/locus/compare/v0.5.1...HEAD
[0.5.1]: https://github.com/ashlrai/locus/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/ashlrai/locus/releases/tag/v0.5.0
[0.4.0]: https://github.com/ashlrai/locus/releases/tag/v0.4.0
[0.3.0]: https://github.com/ashlrai/locus/releases/tag/v0.3.0
[0.2.0]: https://github.com/ashlrai/locus/releases/tag/v0.2.0
[0.1.1]: https://github.com/ashlrai/locus/releases/tag/v0.1.1
[0.1.0]: https://github.com/ashlrai/locus/releases/tag/v0.1.0

# Autopin review checklist

Use this checklist for changes to autopin resolution, learned suggestions, or
workspace writes. It captures the review concerns associated with
[issue #181](https://github.com/ashlrai/locus/issues/181) and the implementation
in this checkout. The checked items describe existing behavior and evidence;
unchecked items are review or hardening work, not claims of existing guarantees.
A suggestion or workspace default is a hint, not proof of tenant ownership.

## Boundaries and existing validation

- [x] **Operator resolution:** bare `pin`/`enter` prefers the nearest workspace
  `default_binding`, then opt-in remote rules. Remote rules use substring matches
  over raw and normalized URLs, consider all remotes, skip allowlist-blocked
  matches in rule order, and cannot use force. The workspace-default path still
  goes through the normal pin gate; it may use an explicitly supplied force.
  See [`autopin.rs`](../crates/locus-core/src/autopin.rs),
  [`Store::pin_auto_with_ttl`](../crates/locus-core/src/store.rs) and
  [`workspace.rs`](../crates/locus-core/src/workspace.rs).
- [x] **Learning:** `suggest` matches keyed hashes of live remotes against local
  history. With `origin`, only that remote is considered; without it, all
  remotes are considered. Score is pins minus mismatches for the pinned alias;
  a winner needs at least 3 and a strict lead. Invalid aliases are ignored.
  Disabled history, no remote, no history, thin evidence, and ties produce no
  suggestion. See [`autopin_suggest.rs`](../crates/locus-core/src/autopin_suggest.rs)
  and its unit tests.
- [x] **Privacy and read-only behavior:** history stores keyed hashes of
  normalized remotes and workspace paths, rather than plaintext values;
  normalization removes URL userinfo. Read-only suggestion does not create the
  history key or lock, change history, write a workspace, or pin. History is
  best-effort and may be disabled. See [`pin_history.rs`](../crates/locus-core/src/pin_history.rs),
  [`autopin_suggest.rs`](../crates/locus-core/src/autopin_suggest.rs), and
  [`autopin_suggest.rs` integration tests](../crates/locus-cli/tests/autopin_suggest.rs).
- [x] **Apply is a separate control action:** `--apply` requires the control
  capability, recomputes from live history and remotes, and requires the winning
  binding to exist and pass the workspace allowlist by alias or ID. It refuses
  an invalid winner instead of selecting a runner-up. A dry run validates a
  plan but does not authenticate the capability or promise a successful write.
  Apply writes a default; it does not create a session. See
  [`Store::autopin_apply_suggestion` and `autopin_suggestion_plan`](../crates/locus-core/src/store.rs),
  [`autopin_learned_guard.rs`](../crates/locus-cli/tests/autopin_learned_guard.rs),
  and the [dry-run limits](README.md#limits-and-unsupported-operations).
- [x] **Workspace and file safety:** malformed nearest policy fails closed;
  apply refuses a shared parent policy outside the repository, rather than
  changing or shadowing it. It checks that the canonical Git root contains the
  current directory and preserves meaningful path whitespace. The writer reads
  a regular file without following a final-component symlink on Unix, rechecks
  its allowlist, preserves existing permissions, writes and syncs a new temp
  file, renames it, and syncs the parent directory on Unix. A directory-sync
  failure is reported after rename, so an error does not always mean no change.
  See [`workspace.rs`](../crates/locus-core/src/workspace.rs),
  [`apply_target` and `write_default_binding`](../crates/locus-core/src/store.rs),
  and [`autopin_hardening.rs`](../crates/locus-cli/tests/autopin_hardening.rs).
- [x] **MCP authority:** the MCP probe is advisory, runs at most once per
  process, respects the kill switch, and cannot mint a pin. When it reaches the
  delegated pin attempt it records a denial and stays unpinned; an existing
  session, disabled probe, or invalid workspace can stop it earlier. See
  [`maybe_mcp_auto_pin`](../crates/locus-mcp/src/lib.rs),
  [`Store::pin_auto_delegated`](../crates/locus-core/src/store.rs), and
  [`mcp_protocol.rs`](../crates/locus-mcp/tests/mcp_protocol.rs).

## Review and hardening checks

- [ ] **Remote identity and ambiguity:** test spoofed `origin`, a missing
  `origin`, multiple fetch/push URLs, URL userinfo, SSH/HTTPS normalization,
  substring collisions, overlapping rules, and rule ordering. A forged origin
  can borrow another repository's history; the no-origin fallback also trusts
  any remote. History recording includes all remotes, so a secondary remote
  present during a pin can also seed evidence later matched as an origin.
  Remote-rule matching is broader still. Consider an explicit,
  operator-approved repository identity or exact host/path matching and an
  ambiguity refusal; don't treat URL matching as authentication. See
  [`autopin.rs`](../crates/locus-core/src/autopin.rs) and
  [`autopin_suggest.rs`](../crates/locus-core/src/autopin_suggest.rs).
- [ ] **Stale or manipulated evidence:** exercise changed remotes between
  preview and apply, deleted and blocked winners, ties, mismatches, disabled or
  missing history, and adversarial history entries. Verify apply recomputes and
  refuses instead of falling back to another alias. Consider evidence freshness
  and provenance before increasing automatic use of historical data. Existing
  coverage: [`autopin_suggest.rs`](../crates/locus-cli/tests/autopin_suggest.rs)
  and [`autopin_learned_guard.rs`](../crates/locus-cli/tests/autopin_learned_guard.rs).
- [ ] **Concurrent policy changes:** test replacement of the policy or its
  parent directory between discovery, the writer's recheck, and rename; test
  simultaneous applies and unrelated edits. The current recheck protects the
  content read by the writer, but it is not a compare-and-swap against changes
  after that read. Consider directory-descriptor-relative operations and a
  locking or conflict-detection protocol. Preserve fail-closed behavior on
  symlinks, non-files, unreadable policies, and parent policies.
  See [`store.rs`](../crates/locus-core/src/store.rs).
- [ ] **Crash and filesystem failures:** inject failures before and after file
  sync, rename, and directory sync; check old/new contents, temp cleanup,
  permissions, and error reporting. A post-rename sync failure needs explicit
  recovery guidance because the update may already be visible. Check platform
  differences rather than assuming Unix `O_NOFOLLOW` and directory fsync apply
  everywhere. Existing sync-failure seam: [`store.rs`](../crates/locus-core/src/store.rs).
- [ ] **Authority remains independent of hints:** verify dry run and read-only
  suggestion do not grant authority; apply without control capability fails;
  a workspace edit, environment knob, or MCP request cannot self-pin. Any future
  delegation must bind operator intent to a specific binding and session,
  preserve seal and allowlist checks, and have explicit revocation and audit
  semantics. See [`store.rs`](../crates/locus-core/src/store.rs) and
  [`mcp_protocol.rs`](../crates/locus-mcp/tests/mcp_protocol.rs).
- [ ] **Disclosure and diagnostics:** test tokenized remotes, paths, and
  malformed inputs across human/JSON output, errors, history, and audit. Do not
  log raw URLs or credential values. Assess whether newly added diagnostics
  reveal workspace paths or advisory aliases to an unintended reader. See
  [`pin_history.rs`](../crates/locus-core/src/pin_history.rs).

## Regression commands

Use an isolated temporary home and retain the real Rust toolchain locations;
never use a real Locus store for tests. Run the full workspace suite as well as
these focused tests when changing the relevant paths:

```sh
TASK_CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
TASK_RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
TASK_TEST_HOME=$(mktemp -d)
export CARGO_HOME="$TASK_CARGO_HOME" RUSTUP_HOME="$TASK_RUSTUP_HOME" HOME="$TASK_TEST_HOME"
export PATH="$CARGO_HOME/bin:$PATH"
cargo test -p locus-core autopin
cargo test -p locus-cli --test integration autopin_
cargo test -p locus-mcp --test mcp_protocol auto_pin
cargo test --workspace
cargo check --workspace --all-targets
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

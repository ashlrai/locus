# Locus — AI Agent Instructions

> Identity plane for coding agents. Pin a Binding; every CLI command and MCP tool is hard-scoped to that tenant until a human re-pins. **Wrong account, impossible.**

## Article 1: Full autonomy, always forward

Synced from the Ashlr Fleet Constitution v3.0 (Nov 1, 2026). This overrides any older text in this file.

**Full autonomy, always forward (Mason t1287u, Nov 1 2026 11:47 PM ET).** Every agent may merge PRs to main and push to production (releases through each repo's sanctioned workflow, any agent may dispatch them). Take the human out of the loop; let nothing slow us down; do the highest-leverage, most ambitious work. No approval, manual-merge, wait-for-per-PR-CI or ask-Mason steps. Mason is needed only for passwords, 2FA, device approvals and payments. The only kept limits are Security GATES v3, the hold list below, and making private things public. Any new gate must be justified against this article or it is rejected.

### Hard floor (the only fixed rules)
- Security GATES v3: pins only on the secret gate (scrub.pl, secret-shape gate, publish.sh), sandbox profiles and leader wrappers/prompts/plists, secret-leaking workflow diffs, and the MACCU TCC helper plus loopback auth. No force-push to default branches, no ruleset or admin bypass, no ~/.codex auth changes, no billing changes, no secret values in logs.
- Merge-queue required checks stay where a repo has them; full CI daily; main-guardian fixes main forward.
- Fork-only pushes through publish.sh. Local models loopback-only.
- Mason only for passwords, 2FA, device approvals and payments (no new spending without him). Nothing private made public.
- Hold list (Mason's own holds): phantom #197 #878 #743 #725 #738 #744 #693 #1228; phantom-secrets #140 #197 #375 #168 #418 #136; ashlr-plugin #152 #157.

**Prefer `locus enter` (or `locus pin`) before tool use** — call `locus whoami` / `locus_whoami` if context is unclear. Agents cannot re-pin; ask the human.

Sibling: [Phantom](https://phm.dev) answers *can this secret enter the model?* Locus answers *as whom, against which tenant, right now?*

## What this repo is

Rust workspace (`locus-core`, `locus-cli`, `locus-mcp`) implementing:

- **Bindings** — principal × tenant × providers × CredentialRefs × policy (`~/.locus/bindings/*.toml`)
- **Sealed sessions** — HMAC pin to exactly one binding
- **Isolation** — scrub ambient identity; inject only pinned providers into child env / workers
- **MCP multiplexor** — `locus-mcp` exposes control tools + tools for the active pin only
- **Scope freeze** — model cannot override frozen `project_ref` / `team_id` / org allowlists
- **Policy** — allow/deny + `require_approval` globs; local CLI labels are advisory and external authorization is fail-closed

Codename directory may still be `mmcp`; product name is **Locus**.

## Build / test / lint

```bash
export PATH="$HOME/.cargo/bin:$PATH"

cargo build --workspace
cargo build --release -p locus-cli -p locus-mcp
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

Binaries: `./target/release/locus`, `./target/release/locus-mcp`.

Local install from this checkout (`locus-cli` installs both binaries):

```bash
cargo install --locked --path crates/locus-cli
```

CI enforces fmt + clippy `-D warnings` + test + release build (see `.github/workflows/ci.yml`).

### Safe local smoke (never touch real `~/.locus` in tests)

Run it as one shell script (one Bash call). The pin's authority broker lives
only as long as the shell that pinned, so a pin made in one tool call is gone
in the next (`no current broker endpoint ... human re-pin required`). In the
new shell, repeat the `export LOCUS_HOME=...` and `eval "$(locus hook bash)"`
lines, then run `locus enter <alias>` (enter replaces a pin whose broker died);
only if enter is refused (expired pin, bad seal), run `locus leave --force` first.

```bash
export LOCUS_HOME=/tmp/locus-agent-test
locus init --with-samples
eval "$(locus hook bash)"        # control commands need LOCUS_CONTROL_CAPABILITY
locus pin personal
locus whoami
locus exec -- env | grep LOCUS_
```

## Layout

| Path | Role |
|------|------|
| `crates/locus-core` | Binding, session, seal, policy, store, isolation, adapters, workers |
| `crates/locus-cli` | `locus` — enter, pin, leave, whoami, exec, run, binding, onboard, setup, mcp, doctor, … (`locus --help` lists every command by group) |
| `crates/locus-mcp` | Stdio MCP server hard-scoped to active pin |
| `docs/` | Operational how-tos (MCP, adapters, workers, architecture, firm mode) |
| `examples/` | Sample bindings + `.locus.toml` |
| `DESIGN.md` | Full architecture + threat model |
| `PLAN.md` | Phased roadmap |
| `SECURITY.md` | Reporting + threat summary |

## Architecture (one screen)

```
Clients (Claude Code / Cursor / CLI / CI)
        │ MCP stdio or locus exec
        ▼
 locus-mcp / locus CLI
        │ verify HMAC session seal
        │ policy + scope freeze
        ▼
 Workers / isolated child env  ── CredentialRef resolve (env:; phm: bridge unavailable)
        │ only pinned binding's providers
        ▼
 Upstream provider APIs / CLIs
```

Identity is resolved **at the gate**, not in the prompt. The agent does not choose an account; the session already *is* an account.

Details: [docs/architecture.md](./docs/architecture.md), [DESIGN.md](./DESIGN.md).

## Invariants (do not break)

These are load-bearing. Prefer a failing test over a soft allow.

1. **Sealed pin** — tools/call succeeds only with a valid session seal; seal binds session → one Binding.
2. **Exclusive catalog** — unbound session ⇒ control tools only (`locus_*`); no ambient personal fallthrough.
3. **No cross-binding credentials** — exec/worker env for A must not contain secret material for B.
4. **Scrub ambient identity** — `AWS_PROFILE`, `GH_TOKEN`, `SUPABASE_*`, `VERCEL_*`, etc. do not leak into isolated children.
5. **Private CLI config dirs** — `GH_CONFIG_DIR` / AWS config paths under session worker home; never `gh auth switch` on user home.
6. **Scope freeze** — frozen `project_ref`, `team_id`, orgs/repos cannot be overridden by model args.
7. **Agents cannot pin** — MCP exposes `locus_request_pin` only; pin state changes require human CLI (or audited force).
8. **CredentialRefs only** — bindings store `phm:NAME` / `env:VAR`, never raw tokens; production rejects `test:`.
9. **MCP never returns secrets** — tool results may show scopes and aliases, never resolved credential values.
10. **Fail closed** — invalid seal, unknown binding, scope mismatch, policy deny → error/deny, not soft allow.
11. **MCP stdout is sacred** — `locus-mcp` speaks JSON-RPC on stdout; no logs there (breaks the protocol).
12. **Workspace allowlist** — `.locus.toml` `allowed_bindings` blocks wrong-tenant pins unless `--force` (audited).

Test surface lives primarily in `locus-core` unit tests + `crates/locus-core/tests/isolation.rs` + `crates/locus-mcp/tests/mcp_protocol.rs`.

## Coding rules

- **Adapters only** hold provider-specific knowledge (`crates/locus-core/src/adapters/`). Register in `adapter_for()`.
- Use `freeze_string_arg` (or equivalent) for every account selector the model might smuggle.
- Mark destructive tools `destructive: true`; enforce with policy `require_approval` globs.
- Prefer small modules over control-flow in `main`.
- Errors: `thiserror` in core, `anyhow` in CLI/MCP binaries.
- Secrets: zeroize where held; never log values; digests only in approval/audit records.
- Test isolation uses `LOCUS_HOME`; no environment variable can enable `test:` credentials in production.

See [CONTRIBUTING.md](./CONTRIBUTING.md) and [docs/adapters.md](./docs/adapters.md).

## Secrets — never commit

| Safe to commit | Never commit |
|----------------|--------------|
| Binding TOML with `phm:NAME` / `env:VAR` refs | Raw API keys, PATs, service role keys |
| `.locus.toml` (aliases only) | `~/.locus/daemon.key`, seal keys |
| Examples under `examples/` with placeholders | Real `project_ref` + live tokens together as a working secret set in docs |
| Synthetic fixture labels | Persistent `phm_` mappings, proxy bearers, or resolved credential values in CI logs |

- Do **not** paste secrets into chat, commits, issues, or MCP tool args.
- Use explicit `env:VAR` references for supported resolution; values must be supplied outside agent context. `phm:NAME` references remain readable but fail closed until a supported scoped bridge exists. Do not invoke or capture `phantom reveal` to emulate one.
- Published Locus v0.5.0 has the incompatible legacy reveal adapter; v0.6.0 fails closed for `phm:` credential resolution. See [credential compatibility](./docs/credential-compatibility.md).
- `test:VALUE` is available only inside compiled unit tests and is rejected by production binaries.
- If a secret lands in git history, rotate it; do not only delete the file.

## Agent / MCP behavior when using Locus as a product

When the **user** has Locus wired (not when hacking this repo):

1. Call `locus_whoami` / `locus whoami` before infrastructure mutations if context is unclear.
2. Treat pin as authoritative — do not invent alternate `project_ref` / teams.
3. If tools are missing or wrong tenant: ask the human to `locus pin <alias>`; do not claim you can re-pin. A pin alone gives control tools plus `locus_providers`; provider tools also need the client to launch `locus mcp --supervised`. If `locus_request_pin` says the binding does not exist, relay `locus binding list` / `locus binding add <alias>`, not `locus enter`.
4. Destructive tools may block on `require_approval`. Only a closed external authorization envelope can release provider execution. Local CLI labels are advisory; do not retry after recording one.

Firm / multi-client workflow: [docs/firm-mode.md](./docs/firm-mode.md).

## Docs map

| Doc | Topic |
|-----|--------|
| [CLAUDE.md](./CLAUDE.md) | Short dev quick-reference |
| [docs/architecture.md](./docs/architecture.md) | System diagram (DESIGN distilled) |
| [docs/policy.md](./docs/policy.md) | Policy rules, evaluation order, approval UX |
| [docs/firm-mode.md](./docs/firm-mode.md) | Agencies: bindings, dual-control, workspaces |
| [docs/agency-starter.md](./docs/agency-starter.md) | Agency starter kit + doctor single pane |
| [examples/agency-starter/](./examples/agency-starter/) | Sample multi-client bindings + offboarding |
| [docs/mcp.md](./docs/mcp.md) | Wire `locus-mcp` into clients (provider tools need `locus mcp --supervised`) |
| [docs/onboarding.md](./docs/onboarding.md) | `locus onboard` wizard + multi-client, multi-tenant walkthrough |
| [docs/adapters.md](./docs/adapters.md) | Write a provider adapter |
| [docs/workers.md](./docs/workers.md) | Synthetic vs MCP stdio workers |
| [docs/hub-integration.md](./docs/hub-integration.md) | Phantom (hub) contract + agent report |
| [docs/verification-plane.md](./docs/verification-plane.md) | Claim verify stubs (propose → verify → act) |
| [integrations/phantom/](./integrations/phantom/) | Phantom hub drop-in TS + gateway/doctor snippets |
| [GOALS.md](./GOALS.md) | Northstar goal loop (`locus goal status`) |
| [DESIGN.md](./DESIGN.md) | Full design + threat model |
| [PLAN.md](./PLAN.md) | Roadmap |
| [SECURITY.md](./SECURITY.md) | Vulnerability reporting |

## Security reports

Do **not** open a public issue for vulnerabilities. See [SECURITY.md](./SECURITY.md) → `security@ashlr.ai`.

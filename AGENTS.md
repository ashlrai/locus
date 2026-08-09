# Locus — AI Agent Instructions

> Identity plane for coding agents. Pin a Binding; every CLI command and MCP tool is hard-scoped to that tenant until a human re-pins. **Wrong account, impossible.**

Sibling: [Phantom](https://phm.dev) answers *can this secret enter the model?* Locus answers *as whom, against which tenant, right now?*

## What this repo is

Rust workspace (`locus-core`, `locus-cli`, `locus-mcp`) implementing:

- **Bindings** — principal × tenant × providers × CredentialRefs × policy (`~/.locus/bindings/*.toml`)
- **Sealed sessions** — HMAC pin to exactly one binding
- **Isolation** — scrub ambient identity; inject only pinned providers into child env / workers
- **MCP multiplexor** — `locus-mcp` exposes control tools + tools for the active pin only
- **Scope freeze** — model cannot override frozen `project_ref` / `team_id` / org allowlists
- **Policy** — allow/deny + `require_approval` globs; human grant via CLI

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

Local install:

```bash
cargo install --path crates/locus-cli
cargo install --path crates/locus-mcp
```

CI enforces fmt + clippy `-D warnings` + test + release build (see `.github/workflows/ci.yml`).

### Safe local smoke (never touch real `~/.locus` in tests)

```bash
export LOCUS_HOME=/tmp/locus-agent-test
locus init --with-samples
locus pin personal
locus whoami
locus exec -- env | grep LOCUS_
```

## Layout

| Path | Role |
|------|------|
| `crates/locus-core` | Binding, session, seal, policy, store, isolation, adapters, workers |
| `crates/locus-cli` | `locus` — pin, leave, whoami, exec, binding, workspace, setup, doctor |
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
 Workers / isolated child env  ── CredentialRef resolve (phm: / env:)
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
8. **CredentialRefs only** — bindings store `phm:NAME` / `env:VAR` / (dev) `test:VALUE`, never raw tokens.
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
- Env overrides for tests: `LOCUS_HOME`, `LOCUS_ALLOW_TEST_CREDS=1`, `LOCUS_SOFT_CREDS=1` — never enable test creds in production paths by default.

See [CONTRIBUTING.md](./CONTRIBUTING.md) and [docs/adapters.md](./docs/adapters.md).

## Secrets — never commit

| Safe to commit | Never commit |
|----------------|--------------|
| Binding TOML with `phm:NAME` / `env:VAR` refs | Raw API keys, PATs, service role keys |
| `.locus.toml` (aliases only) | `~/.locus/daemon.key`, seal keys |
| Examples under `examples/` with placeholders | Real `project_ref` + live tokens together as a working secret set in docs |
| `phm_` placeholder tokens | Resolved credential values in tests, fixtures, or CI logs |

- Do **not** paste secrets into chat, commits, issues, or MCP tool args.
- Prefer Phantom (`phm:NAME`) for real credentials; use `env:` only for CI bootstrap.
- `test:VALUE` requires `LOCUS_ALLOW_TEST_CREDS=1` and is for unit tests only.
- If a secret lands in git history, rotate it; do not only delete the file.

## Agent / MCP behavior when using Locus as a product

When the **user** has Locus wired (not when hacking this repo):

1. Call `locus_whoami` / `locus whoami` before infrastructure mutations if context is unclear.
2. Treat pin as authoritative — do not invent alternate `project_ref` / teams.
3. If tools are missing or wrong tenant: ask the human to `locus pin <alias>`; do not claim you can re-pin.
4. Destructive tools may block on `require_approval` — human runs approval grant; do not loop-spam confirm.

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
| [docs/mcp.md](./docs/mcp.md) | Wire `locus-mcp` into clients |
| [docs/adapters.md](./docs/adapters.md) | Write a provider adapter |
| [docs/workers.md](./docs/workers.md) | Synthetic vs MCP stdio workers |
| [DESIGN.md](./DESIGN.md) | Full design + threat model |
| [PLAN.md](./PLAN.md) | Roadmap |
| [SECURITY.md](./SECURITY.md) | Vulnerability reporting |

## Security reports

Do **not** open a public issue for vulnerabilities. See [SECURITY.md](./SECURITY.md) → `security@ashlr.ai`.

# Locus

[![CI](https://github.com/ashlrai/locus/actions/workflows/ci.yml/badge.svg)](https://github.com/ashlrai/locus/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](./LICENSE)
[![Rust](https://img.shields.io/badge/rust-stable-orange.svg)](./rust-toolchain.toml)

**Wrong account, impossible.**

Identity plane for coding agents. Pin a client — every CLI command and MCP tool is hard-scoped to that binding until you re-pin.

| Product | Question it answers |
|---------|---------------------|
| **[Phantom](https://phm.dev)** | Can this secret enter the model? |
| **Locus** | As whom, against which tenant, right now? |

> Agents inherit ambient identity: global `gh auth`, one Supabase MCP token, last Vercel team. Contract work makes that lethal. Locus makes wrong-account action **mechanically impossible** — not merely discouraged.

---

## Install

```bash
brew install ashlrai/tap/locus  # when published

export PATH="$HOME/.cargo/bin:$PATH"
cargo install --git https://github.com/ashlrai/locus --package locus-cli --locked
cargo install --git https://github.com/ashlrai/locus --package locus-mcp --locked

# npm / npx — downloads a release binary, or falls back to cargo install
npx locus-cli --help
npx locus-mcp   # MCP server for Claude Code / Cursor
```

Local checkout:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo install --path crates/locus-cli
cargo install --path crates/locus-mcp
```

Homebrew formula (for taps): [integrations/homebrew](./integrations/homebrew).

---

## Quick start

```bash
# Initialize with sample personal + acme bindings
locus init --with-samples
locus binding list

# Pin and prove who you are
locus pin personal
locus whoami

# Switch client — previous identity is gone from the process env
locus pin acme
locus whoami

# Run any command with only the pinned binding's surface
locus exec -- env | grep LOCUS_
locus exec -- env | grep SUPABASE_PROJECT_REF

# Directory-local default
cd ~/clients/acme
locus workspace --default acme --allow acme,acme-ro --require-pin
locus pin          # uses .locus.toml
```

### Shell prompt

```bash
eval "$(locus hook zsh)"
# shows [locus:acme:acme-corp] or [locus:unpinned]
```

---

## How isolation works

```
locus pin acme
        │
        ▼
  Session sealed (HMAC) to binding "acme"
        │
        ├─► locus-mcp   → tools = only this binding (+ locus_whoami)
        │                 agents cannot pin (locus_request_pin only)
        │                 scope freeze: wrong project_ref → deny
        │
        └─► locus exec -- <cmd>
              ├─ scrubs ambient AWS_PROFILE / GH_TOKEN / SUPABASE_* / …
              ├─ resolves phm: / env: credential_refs into provider env vars
              ├─ private GH_CONFIG_DIR + AWS_* under ~/.locus/workers/<session>/
              └─ never injects other bindings' providers
```

**Credential refs**

| Ref | Resolution |
|-----|------------|
| `phm:NAME` | `phantom reveal --yes NAME` (values only in child env) |
| `env:VAR` | parent process env (CI / tests) |
| `test:VALUE` | only if `LOCUS_ALLOW_TEST_CREDS=1` |

Never put raw secrets in binding files.

## MCP (Claude Code / Cursor)

```bash
# after Install (cargo / brew / npm)
locus pin acme
locus setup --client claude    # writes/merges .mcp.json
# restart Claude Code — tools: locus_whoami, supabase.scope, github.whoami, …
```

---

## Core concepts

| Term | Meaning |
|------|---------|
| **Binding** | principal × tenant × providers × CredentialRefs × policy |
| **Session** | Live pin sealed to exactly one Binding |
| **Workspace** | `.locus.toml` — default pin + allowlist for a repo tree |
| **CredentialRef** | `phm:NAME` / vault pointer — never the secret itself |

Example binding (`~/.locus/bindings/acme.toml`):

```toml
[binding]
id = "bnd_acme"
alias = "acme"
tenant = "acme-corp"
description = "Acme client engagement"

[binding.policy]
default = "allow"
require_approval = ["*.delete*", "vercel.deploy.prod"]
dual_control = ["*.delete*", "vercel.deploy.prod"]  # two distinct principals
max_ttl = "8h"

[[binding.providers]]
provider = "supabase"
account = "acme-prod"
credential_ref = "phm:SUPABASE_ACME"
scope = { project_ref = "abcdefghij", read_only = true }

[[binding.providers]]
provider = "github"
account = "acme-corp"
credential_ref = "phm:GH_TOKEN_ACME"
scope = { orgs = ["acme-corp"], repos = ["acme-corp/*"] }
```

---

## CLI

```
locus init [--with-samples]
locus pin [alias] [--force] [--client claude] [--ns a,b]
locus enter <alias>                   # firm workflow pin
locus leave
locus whoami [--json]
locus status [--oneline] [--json]
locus exec -- <command> [args...]
locus run -b <alias> -- <command>     # one-shot; global pin unchanged
locus binding list|show|add|rm
locus workspace --default <alias> [--allow a,b] [--require-pin]
locus doctor [--json]                 # SAFE|WARN|UNSAFE (exit 0/1/2)
locus approve list|grant|status|deny  # require_approval / dual-control
locus notify status|on|off            # desktop banners OFF by default
locus events --last N [--op …] [--binding …] [--json]
locus graph list|export|import        # encrypted binding graph share (no secrets)
locus ci mint|env|run                 # short-lived sealed sessions for pipelines
locus hook zsh|bash|fish
locus setup --client claude|cursor|codex
```

**Agency kit:** [`examples/agency-starter/`](./examples/agency-starter/) — personal ↔ client A ↔ client B, dual-control, workspaces, offboarding. Guide: [docs/agency-starter.md](./docs/agency-starter.md).

Approvals:

```bash
# After an agent hits require_approval (MCP returns appr_…)
locus approve list
locus approve grant appr_… --as alice
# dual_control tools need a second distinct principal:
locus approve grant appr_… --as bob
locus events --last 20 --op approval.grant --json
```

Graph share (bindings + workspace templates only — CredentialRefs, never secret values):

```bash
export LOCUS_GRAPH_PASSPHRASE='…'   # required for encrypt/decrypt
locus graph list
locus graph export --out team.locusgraph
locus graph import team.locusgraph
```

CI ephemeral pins (do not touch `active.json`):

```bash
locus ci mint -b acme --json          # short-lived sealed session
eval "$(locus ci env -b acme)"        # export LOCUS_SESSION_ID + env
locus ci run -b acme -- npm test      # mint → run → cleanup
```

Override home for tests/CI: `LOCUS_HOME=/tmp/locus-test locus …`

---

## Roadmap

| Phase | Status |
|-------|--------|
| **0** Daemon-less control plane, pin/whoami/exec, isolation tests | done |
| **1** `locus-mcp`, credential resolve, adapters, scope freeze, setup | done |
| **2** Firm UX, doctor pane, run/ns, notify, local `locus graph`, `locus ci` | **you are here (0.1.1)** |
| **3** Remote binding graph sync, dual-control packs, offboard, SIEM export | next |
| **4** Adapter SDK, broader prebuilt platforms | later |

See [PLAN.md](./PLAN.md) and [DESIGN.md](./DESIGN.md) for the full architecture.

---

## Development

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test --workspace
cargo build --release
./target/release/locus --help

# Full shell e2e (34 checks: pin, isolation, MCP freeze/approval, dual-control,
# doctor, events, enter/run, notify, graph, ci, heartbeat — feature-detected)
./scripts/e2e.sh
```

See [CONTRIBUTING.md](./CONTRIBUTING.md) for build/test/lint and how to add adapters. Agent-oriented rules: [AGENTS.md](./AGENTS.md). Short IDE guide: [CLAUDE.md](./CLAUDE.md).

### Landing page

Static site under [`apps/web/`](./apps/web) — dark monochrome terminal aesthetic, sibling positioning vs Phantom.

```bash
cd apps/web && npm start   # http://localhost:3000
```

Deploy notes (Vercel / Cloudflare Pages): [apps/web/README.md](./apps/web/README.md).

### Docs

| Doc | Topic |
|-----|--------|
| [AGENTS.md](./AGENTS.md) | AI coding agents: build, test, invariants, secrets |
| [CLAUDE.md](./CLAUDE.md) | Short development guide |
| [apps/web/](./apps/web) | Landing page (static HTML) |
| [scripts/e2e.sh](./scripts/e2e.sh) | End-to-end shell suite |
| [docs/architecture.md](./docs/architecture.md) | System diagram (DESIGN distilled) |
| [docs/agency-certainty.md](./docs/agency-certainty.md) | Identity vs epistemic certainty (Ashlr stack) |
| [docs/firm-mode.md](./docs/firm-mode.md) | Agencies: bindings, dual-control, workspaces |
| [docs/agency-starter.md](./docs/agency-starter.md) | Agency starter kit + doctor single pane |
| [examples/agency-starter/](./examples/agency-starter/) | Bindings, workspaces, dual-control, offboarding |
| [docs/mcp.md](./docs/mcp.md) | `locus-mcp` with Claude Code / Cursor |
| [docs/adapters.md](./docs/adapters.md) | Writing a provider adapter |
| [docs/workers.md](./docs/workers.md) | Synthetic vs MCP stdio workers |
| [SECURITY.md](./SECURITY.md) | Threat model summary & reporting |
| [CHANGELOG.md](./CHANGELOG.md) | Release notes |
| [DESIGN.md](./DESIGN.md) / [PLAN.md](./PLAN.md) | Full architecture & roadmap |

---

## Daily firm workflow

```bash
locus engagement init acme --tenant acme-corp --workspace
locus enter acme
locus whoami
locus exec -- gh pr list
locus run -b personal -- npm test    # one-shot; global pin unchanged
locus leave

locus approve list
locus approve grant appr_… --as mason
locus notify status                  # OFF by default (no spam)
locus doctor                         # SAFE | WARN | UNSAFE

# Encrypted binding-graph share (CredentialRefs only — never secret values)
export LOCUS_GRAPH_PASSPHRASE='…'    # or interactive TTY prompt
locus graph export --out team.locusgraph
locus graph import team.locusgraph

# CI / ephemeral sealed pin (does not touch active.json)
locus ci mint -b acme --json         # session_id + env map; no secrets by default
```

**Desktop notifications are off by default.** Agents create many pending
approvals; banners only after `locus notify on` or `LOCUS_NOTIFY=1`
(silent, rate-limited). Kill switch: `locus notify off` / `LOCUS_QUIET=1`.

## Security model

- Session pins are **HMAC-sealed**; tampering fails closed.
- Workspace `allowed_bindings` blocks wrong-tenant pins (unless `--force`, audited).
- `locus exec` / `locus run` scrub ambient identity; secrets only in the child.
- MCP never returns secret values; agents cannot pin (request only).
- Scope freeze: model cannot override frozen `project_ref` / `team_id`.
- Policy: globs + structured `[[rules]]`, `require_approval`, dual-control (2 principals).
- Upstream MCP workers auto-spawn per-binding when `upstream = { command, args }` is set.
- Drift freeze: `locus watch` / doctor re-pin if binding changes under a session.

Details: [SECURITY.md](./SECURITY.md). Threat model: [DESIGN.md §9](./DESIGN.md).

---

## License

MIT — see [LICENSE](./LICENSE).

Code of Conduct: [CODE_OF_CONDUCT.md](./CODE_OF_CONDUCT.md).

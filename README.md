# Locus

[![CI](https://github.com/ashlrai/locus/actions/workflows/ci.yml/badge.svg)](https://github.com/ashlrai/locus/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](./LICENSE)
[![Rust](https://img.shields.io/badge/rust-stable-orange.svg)](./rust-toolchain.toml)

**AI-native identity plane for coding agents.**  
**Wrong account, impossible.**

Pin a client — every CLI command, MCP tool, and the **local dashboard** is hard-scoped to that binding until you re-pin. Agents inherit a sealed session, not ambient `gh auth` / last Vercel team.

| Product | Question it answers |
|---------|---------------------|
| **[Phantom](https://phm.dev)** | Can this secret enter the model? |
| **Locus** | As whom, against which tenant, right now? |

> Agents inherit ambient identity: global `gh auth`, one Supabase MCP token, last Vercel team. Contract work makes that lethal. Locus makes wrong-account action **mechanically impossible** — not merely discouraged. AI-native (`locus agent`, MCP resources/prompts), hub-native (`agent report` · `REQUIRED_SERVERS`), and operator-visible (`locus dashboard` · `forensics` · `goal status`).

---

## Install

```bash
# Homebrew (tap live) — installs locus + locus-mcp
brew install ashlrai/tap/locus

# npm — locus-cli and @ashlrai/locus-mcp published at 0.3.0
# (downloads a release binary, or falls back to cargo install)
npm install -g locus-cli @ashlrai/locus-mcp
npx locus-cli --help
npx @ashlrai/locus-mcp   # MCP server for Claude Code / Cursor

# cargo
export PATH="$HOME/.cargo/bin:$PATH"
cargo install --git https://github.com/ashlrai/locus --package locus-cli --locked
cargo install --git https://github.com/ashlrai/locus --package locus-mcp --locked
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
# First 60 seconds
locus quickstart                 # samples · enter · whoami · doctor
# quickstart also mints the operator control capability if missing, persists it
# 0600 at ~/.locus/control_capability (respects LOCUS_HOME), and adopts it for
# the run. Export it in new shells (value never echoed):
eval "$(locus hook zsh)"
# Manual alternative: export LOCUS_CONTROL_CAPABILITY="$(openssl rand -hex 32)"

# Or explicit pin
locus init --with-samples
locus enter personal && locus whoami

# Switch client — previous identity is gone from the process env
locus enter acme && locus whoami

# Local identity dashboard (loopback UI + API)
locus dashboard                  # http://127.0.0.1:8750

# Run any command with only the pinned binding's surface
locus exec -- env | grep LOCUS_

# Wire AI clients + hub readiness
locus agent setup --apply
locus agent report --json        # ready | protected | unsafe

# Directory-local default
cd ~/clients/acme
locus workspace --default acme --allow acme,acme-ro --require-pin
locus pin          # uses .locus.toml
```

### Shell prompt

```bash
eval "$(locus hook zsh)"
# shows [locus:acme:acme-corp] or [locus:unpinned]
# also exports the persisted LOCUS_CONTROL_CAPABILITY when the shell lacks one
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

`test:` credentials are compiled-test-only and are always rejected by production binaries, regardless of environment.

Schemes are mandatory: bare names, raw tokens, empty refs, and unsupported schemes are rejected when bindings are saved or loaded. Never put raw secrets in binding files.

Legacy bare Phantom names are never silently skipped. Inspect the safe dry run with `locus binding migrate-credential-refs <alias>`, then persist conservative conversions with `--write`. Unsafe values require manual editing and are never printed.

## MCP (Claude Code / Cursor)

```bash
# after Install (cargo / brew / npm)
locus enter acme
locus agent setup --apply      # MCP configs + AGENT.md (or: locus setup --client claude)
# restart Claude Code — tools: locus_whoami, supabase.scope, … + resources/prompts

# Optional: HTTP MCP for CI agents (loopback + token)
LOCUS_MCP_HTTP_TOKEN=secret locus-mcp --http 127.0.0.1:8742
# POST /mcp  ·  GET /health
```

MCP identity and provider-scope responses report only credential presence and source (`phantom` or `environment`); they never return `credential_ref` values or resolved secrets.

---

## Core concepts

| Term | Meaning |
|------|---------|
| **Binding** | principal × tenant × providers × CredentialRefs × policy |
| **Session** | Live pin sealed to exactly one Binding |
| **Workspace** | `.locus.toml` — default pin + allowlist for a repo tree |
| **CredentialRef** | `phm:NAME` / vault pointer — never the secret itself |

Workspace policy discovery is fail closed: if the nearest `.locus.toml` exists but is unreadable or malformed, explicit pin, autopin, and `--force` pin stop with an error, while `locus doctor` reports `UNSAFE`.

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
locus init [--with-samples] · quickstart
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
locus watch [--once] [--require-ok]   # session heartbeat each tick (NDJSON with --json)
locus dashboard · serve               # local identity UI + API (127.0.0.1)
locus forensics export                # shareable pack (no secrets)
locus verify claim|session            # verification plane: claim score · session pack
locus approve list|grant|status|wait|deny  # require_approval / dual-control
locus notify status|on|off            # desktop banners OFF by default
locus events --last N · events export # audit tail / fleet pulse / OTLP
locus graph list|export|import        # encrypted binding graph share (no secrets)
locus ci mint|env|run                 # short-lived sealed sessions for pipelines
locus engagement init|close           # client engagement lifecycle (binding + workspace)
locus upstream list|suggest           # built-in upstream MCP recipes
locus adapter list|verify|trust       # provider adapter catalog + signature trust
locus hook zsh|bash|fish
locus setup --client claude|cursor|codex|grok
locus mcp                             # run the stdio MCP server (same as locus-mcp)
locus agent report|setup|doctor       # AI-native readiness (hub JSON)
locus goal status                     # northstar progress from GOALS.md
locus completion zsh|bash|fish        # shell completions
locus topic <name>                    # dashboard · forensics · serve · goal · verify · …
```

**Agency kit:** [`examples/agency-starter/`](./examples/agency-starter/) — personal ↔ client A ↔ client B, dual-control, workspaces, offboarding. Guide: [docs/agency-starter.md](./docs/agency-starter.md).

**Northstar / hub:** [GOALS.md](./GOALS.md) · [docs/hub-integration.md](./docs/hub-integration.md) · [integrations/ashlr-hub/](./integrations/ashlr-hub/)

Approvals:

```bash
# After an agent hits require_approval (MCP returns appr_…)
locus approve list
locus approve grant appr_… --as alice  # advisory review label only
# A second local label still cannot satisfy dual_control:
locus approve grant appr_… --as bob    # authority remains 0/2
locus events --last 20 --op approval.advisory --json
# Provider execution stays blocked pending a closed external authorization envelope.
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
| **2** Firm UX, doctor pane, run/ns, notify, local `locus graph`, `locus ci` | done (0.1.1) |
| **2.x / 0.2–0.3** Dashboard, forensics, HTTP MCP, agent report, goal loop; 0.3 dogfood polish, TTL, client add, MCP session pin-anchoring | **you are here (0.3.0)** |
| **3** Remote binding graph sync, dual-control packs, offboard, SIEM export | next |
| **4** Adapter SDK, broader prebuilt platforms | later |

See [GOALS.md](./GOALS.md) (living milestones), [PLAN.md](./PLAN.md), and [DESIGN.md](./DESIGN.md).

---

## Development

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test --workspace
cargo build --release
./target/release/locus --help

# Full shell e2e (pin, isolation, MCP freeze/approval, dual-control,
# doctor, events, enter/run, notify, graph, ci, heartbeat, dashboard health,
# forensics, goal status — feature-detected)
./scripts/e2e.sh
```

See [CONTRIBUTING.md](./CONTRIBUTING.md) for build/test/lint and how to add adapters. Agent-oriented rules: [AGENTS.md](./AGENTS.md). Short IDE guide: [CLAUDE.md](./CLAUDE.md).

### Landing page & dashboard

| App | Role |
|-----|------|
| [`apps/web/`](./apps/web) | Marketing landing — AI-native identity plane hero |
| [`apps/dashboard/`](./apps/dashboard) | Operator UI embedded by `locus serve` / `dashboard` |

```bash
cd apps/web && npm start   # http://localhost:3000
locus dashboard            # http://127.0.0.1:8750 (needs built CLI)
```

Deploy notes (Vercel / Cloudflare Pages): [apps/web/README.md](./apps/web/README.md).

### Docs

| Doc | Topic |
|-----|--------|
| [AGENTS.md](./AGENTS.md) | AI coding agents: build, test, invariants, secrets |
| [CLAUDE.md](./CLAUDE.md) | Short development guide |
| [apps/web/](./apps/web) | Landing page (static HTML) |
| [apps/dashboard/](./apps/dashboard) | Local identity dashboard UI |
| [GOALS.md](./GOALS.md) | Northstar goal loop |
| [scripts/e2e.sh](./scripts/e2e.sh) | End-to-end shell suite |
| [docs/architecture.md](./docs/architecture.md) | System diagram (DESIGN distilled) |
| [docs/agency-certainty.md](./docs/agency-certainty.md) | Identity vs epistemic certainty (Ashlr stack) |
| [docs/firm-mode.md](./docs/firm-mode.md) | Agencies: bindings, dual-control, workspaces |
| [docs/agency-starter.md](./docs/agency-starter.md) | Agency starter kit + doctor single pane |
| [examples/agency-starter/](./examples/agency-starter/) | Bindings, workspaces, dual-control, offboarding |
| [docs/mcp.md](./docs/mcp.md) | `locus-mcp` with Claude Code / Cursor |
| [docs/onboarding.md](./docs/onboarding.md) | Agency onboarding: 3 agent clients × 3 tenants end-to-end |
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
locus approve grant appr_… --as mason  # advisory only; never execution authority
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

- Session pins use **V3 HMAC seals plus a supervised live authority broker**. The broker binds the exact record digest, backing file, authority, expiry, and monotonic generation, so reading `daemon.key` alone cannot forge or replay current authority.
- Workspace `allowed_bindings` blocks wrong-tenant pins (unless `--force`, audited).
- `locus exec`, `locus run`, and `locus ci run` scrub ambient identity; resolved credentials are injected into the child by default. Their shared `--no-resolve` preflight expands recipe defaults and fails before child, worker, session, or credential effects when a declared upstream can resolve credentials. Credential-free upstreams remain usable.
- MCP never returns secret values; agents cannot pin (request only).
- Scope freeze: model cannot override frozen `project_ref` / `team_id`.
- Policy: globs + structured `[[rules]]`, `require_approval`, dual-control (2 principals).
- Upstream MCP workers start only after an authorized provider call. `tools/list` is discovery-only, multi-provider startup rolls back on partial failure, and each worker receives only its named provider's resolved credential keys.
- Drift freeze + hub heartbeat: `locus watch [--json] [--require-ok]` re-runs session verify each tick; doctor re-pin if binding changes under a session.

Details: [SECURITY.md](./SECURITY.md). Threat model: [DESIGN.md §9](./DESIGN.md).

---

## License

MIT — see [LICENSE](./LICENSE).

Code of Conduct: [CODE_OF_CONDUCT.md](./CODE_OF_CONDUCT.md).

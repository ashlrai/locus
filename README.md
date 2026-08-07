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

## Quick start

```bash
# Build
export PATH="$HOME/.cargo/bin:$PATH"
cargo install --path crates/locus-cli

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
cargo install --path crates/locus-cli
cargo install --path crates/locus-mcp

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
locus pin [alias] [--force] [--client claude]
locus leave
locus whoami [--json]
locus status [--oneline] [--json]
locus exec -- <command> [args...]
locus binding list|show|add|rm
locus workspace --default <alias> [--allow a,b] [--require-pin]
locus doctor
locus hook zsh|bash|fish
locus setup --client claude|cursor|codex
```

Override home for tests/CI: `LOCUS_HOME=/tmp/locus-test locus …`

---

## Roadmap

| Phase | Status |
|-------|--------|
| **0** Daemon-less control plane, pin/whoami/exec, isolation tests | done |
| **1** `locus-mcp`, credential resolve, adapters, scope freeze, setup | **you are here** |
| **2** Real upstream MCP workers, AWS/CF/Resend, continuous whoami drift | next |
| **3** Team binding graph, dual-control, offboard, audit export | |

See [PLAN.md](./PLAN.md) and [DESIGN.md](./DESIGN.md) for the full architecture.

---

## Development

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test
cargo build --release
./target/release/locus --help
```

See [CONTRIBUTING.md](./CONTRIBUTING.md) for build/test/lint and how to add adapters.

### Docs

| Doc | Topic |
|-----|--------|
| [docs/mcp.md](./docs/mcp.md) | `locus-mcp` with Claude Code / Cursor |
| [docs/adapters.md](./docs/adapters.md) | Writing a provider adapter |
| [SECURITY.md](./SECURITY.md) | Threat model summary & reporting |
| [CHANGELOG.md](./CHANGELOG.md) | Release notes |
| [DESIGN.md](./DESIGN.md) / [PLAN.md](./PLAN.md) | Full architecture & roadmap |

---

## Security model

- Session pins are **HMAC-sealed**; tampering fails closed.
- Workspace `allowed_bindings` blocks wrong-tenant pins (unless `--force`, audited).
- `locus exec` scrubs ambient identity env vars; resolves secrets only into the child.
- MCP never returns secret values; agents cannot pin (request only).
- Scope freeze: model cannot override frozen `project_ref` / `team_id`.
- Policy `require_approval` blocks destructive tool stubs without `confirm=true`.

Not in scope yet: real upstream Supabase/Vercel MCP fan-out, dual-control, team sync.

Details and reporting: [SECURITY.md](./SECURITY.md). Full threat model: [DESIGN.md §9](./DESIGN.md).

---

## License

MIT — see [LICENSE](./LICENSE).

Code of Conduct: [CODE_OF_CONDUCT.md](./CODE_OF_CONDUCT.md).

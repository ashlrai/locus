# Locus

<!-- fresh:begin badge -->
[![Release 0.5.2](https://img.shields.io/badge/release-v0.5.2-blue)](https://github.com/ashlrai/locus/releases/tag/v0.5.2)
<!-- fresh:end -->

<!-- fresh:begin release -->
Latest release: [v0.5.2](https://github.com/ashlrai/locus/releases/tag/v0.5.2) (2026-10-26).

`locus renew` and expired-pin recovery (#193), the global `--dry-run` preview, exact-loopback HTTP (#187) and index URL redaction (#185).
<!-- fresh:end -->

[![CI](https://github.com/ashlrai/locus/actions/workflows/ci.yml/badge.svg)](https://github.com/ashlrai/locus/actions/workflows/ci.yml)
[![npm: locus-cli](https://img.shields.io/npm/v/locus-cli?label=npm%20locus-cli)](https://www.npmjs.com/package/locus-cli)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](./LICENSE)
[![Rust](https://img.shields.io/badge/rust-stable-orange.svg)](./rust-toolchain.toml)

**Your coding agent can't act as the wrong account.** Pin a client, and every CLI command, MCP tool and agent session is sealed to that identity until you re-pin. No more ambient `gh auth`, a shared Supabase token or "whichever Vercel team was last".

![locus quickstart pins a sample binding; locus whoami shows the sealed session and its scoped providers](docs/assets/locus-demo.gif)

## 30-second start

```bash
npm install -g locus-cli @ashlrai/locus-mcp   # or brew or cargo; see Install options
locus quickstart                        # sample bindings, pin, whoami, doctor
eval "$(locus hook zsh)"                # or bash; exports the control capability
locus enter personal                    # pin from this shell (it supervises the pin)
locus whoami                            # who your agents are acting as, right now
locus exec --expect personal -- echo ok # runs: personal is pinned
locus exec --expect acme -- echo ok     # refused, exit 1, child never starts (pin_mismatch)
```

What the two extra steps are for:

- **Control capability.** Control commands (`enter`, `pin`, `leave`, `exec`) need `LOCUS_CONTROL_CAPABILITY` in the shell. `locus quickstart` mints it and saves it at `~/.locus/control_capability` (0600, respects `LOCUS_HOME`); `eval "$(locus hook zsh)"` exports it without printing it. Without it, `locus exec` stops with `LOCUS_CONTROL_CAPABILITY is required for control-plane authority`. `locus capability status` shows the posture without the value.
- **Broker pin.** A pin is backed by an authority broker that the pinning shell supervises. `locus enter personal` works straight after `quickstart`, including when quickstart ran in a shell that has already exited (fixed in #156, regression test `crates/locus-cli/tests/quickstart_then_enter.rs`).
- **Recovery.** If a command says `human re-pin required` (for example `no current broker endpoint` after the pinning shell exited), load the hook in the new shell (`eval "$(locus hook zsh)"` or `bash`) and run `locus enter personal`: enter replaces a pin whose broker died. Only if enter is refused (for example an expired pin or a bad seal), run `locus leave --force` in that same hooked shell, then `locus enter personal`. `--force` deletes session state only and never mints anything.

## Connect your agent (MCP)

`locus agent setup --apply` writes the configs for every client it finds. By hand:

```bash
claude mcp add locus -- locus mcp --supervised    # Claude Code
codex mcp add locus -- locus mcp --supervised     # Codex
```

Cursor (`~/.cursor/mcp.json`):

```json
{ "mcpServers": { "locus": { "command": "locus", "args": ["mcp", "--supervised"] } } }
```

`locus mcp --supervised` serves the pinned binding's provider tools; launch the client from the shell that ran `locus hook` and `locus pin`, and restart it after a re-pin. Registered as plain `locus-mcp` (what `locus setup` writes), the server serves control tools only (pin status, verification, `locus_request_pin`). Neither returns credential values. See [docs/mcp.md](./docs/mcp.md#supervised-launch-provider-tools).

### How it compares

| Approach | Where the identity lives | Reach of a switch |
|---|---|---|
| `gh auth switch` | gh's shared config (`hosts.yml`) | Every process that reads that config, until the next switch |
| direnv `.envrc` | Variables exported into your shell on `cd` | Only the variables in `.envrc`; the rest of the shell's environment stays |
| 1Password `op run` | Secret references resolved for one command | That command |
| **Locus** `run` / `exec` / `pin` | A sealed session bound to one binding | That command or session. Ambient `GH_TOKEN`, `AWS_*`, `SUPABASE_*`… are scrubbed, a directory's `.locus.toml` can refuse other bindings, and MCP provider tools (`locus mcp --supervised`) are only the pinned binding's |

Use explicit environment (`env:`) references for supported credential resolution. Phantom (`phm:`) references remain readable for metadata and migration, but require a scoped bridge that is not available in this source checkout. Try it in a minute with the executable [recipes](./examples/recipes/) (two clients, a directory allowlist, a CI session).

---

## Install options

```bash
# Homebrew (tap live) — installs locus + locus-mcp
brew install ashlrai/tap/locus

# npm (release binary, or cargo if this platform has no asset)
npm install -g locus-cli @ashlrai/locus-mcp
npx locus-cli --help
npx @ashlrai/locus-mcp   # MCP server for Claude Code / Cursor

# cargo (Linux without Homebrew, or any source install): rustc 1.89 or newer
# (workspace rust-version). Cargo 1.85 to 1.88 stops with "requires rustc 1.89".
# Cargo 1.83 fails earlier: idna_adapter in the lockfile needs edition 2024.
# rust-toolchain.toml pins stable inside a checkout and does not apply to
# `cargo install --git` run outside the repo.
# Measured on one Linux box: 179 s for both binaries.
export PATH="$HOME/.cargo/bin:$PATH"
cargo install --git https://github.com/ashlrai/locus locus-cli locus-mcp --locked
```

Local checkout:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo install --path crates/locus-cli
cargo install --path crates/locus-mcp
```

Homebrew formula (for taps): [integrations/homebrew](./integrations/homebrew).

---

## More of the CLI

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
locus switch acme && locus whoami

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

## Measured: wrong-client credential reachability

Scenario: your shell still holds another client's ambient credentials (the `gh auth switch` or direnv leftover) and you launch a child process. Which of those can it still reach? Reproduce with `python3 eval/env-reachability/redteam.py` (needs `locus` 0.5.0 on PATH; fake canary credentials, throwaway `HOME` and `LOCUS_HOME`).

| launch | env credentials reachable (16 variables x 3 trials) |
| --- | --- |
| plain launch | 48 of 48 |
| `locus run -b acme -- <cmd>` | **0 of 48** |

Also measured: with locus, credential files read through `$HOME` (`~/.config/gh/hosts.yml`, `~/.aws/credentials`, `~/.netrc`, `~/.git-credentials`, `~/.npmrc`) were not reachable (0 of 15), and an agent attempt to pin a binding was refused (3 of 3).

**This is not a sandbox.** The same files read by their absolute real path stayed reachable under locus (15 of 15, same as a plain launch). Locus scrubs the environment and redirects `HOME`; it does not stop a process from opening files it already has OS permission to open. Scripted adversary, one machine, locus 0.5.0, 3 trials, no external audit.

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
              ├─ resolves explicit env: credential_refs into provider env vars
              ├─ private GH_CONFIG_DIR + AWS_* under ~/.locus/workers/<session>/
              └─ never injects other bindings' providers
```

**Credential refs**

| Ref | Resolution |
|-----|------------|
| `phm:NAME` | Reserved pointer; resolution fails before any Phantom invocation until a supported scoped bridge exists |
| `env:VAR` | Explicitly supplied parent process variable, injected only into the pinned provider’s child environment |

See [credential compatibility and onboarding](./docs/credential-compatibility.md) for the supported setup and release distinction.

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

### Learned bindings: `locus autopin suggest`

locus keeps a local pin history (`$LOCUS_HOME/pin-history.jsonl`, 0600, remotes stored only as keyed hashes). In a repo, `locus autopin suggest` shows which binding you usually pin there, with pins, `pin_mismatch` refusals and last seen per alias. A suggestion needs a score (pins minus mismatches) of at least 3 and a strict lead; otherwise it suggests nothing. When the repo has an `origin` remote only `origin` is matched, so an extra remote that mimics another repo borrows nothing; remotes are not authenticated, though, so an `origin` that mimics another repo still matches its history. Check the binding and file `--apply` prints. A read-only `suggest` never creates `history.key`. Add `--json` for the `locus.autopin.suggest.v1` shape. `locus autopin suggest --apply` writes `default_binding` to the nearest `.locus.toml` inside the git toplevel (or a new one there), refuses a parent `.locus.toml` above the toplevel, and requires `LOCUS_CONTROL_CAPABILITY`, so an agent cannot apply it. Turn history off with `[history] enabled = false` or `LOCUS_PIN_HISTORY=0`.

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
credential_ref = "env:LOCUS_SUPABASE_ACME"
scope = { project_ref = "abcdefghij", read_only = true }

[[binding.providers]]
provider = "github"
account = "acme-corp"
credential_ref = "env:LOCUS_GH_TOKEN_ACME"
scope = { orgs = ["acme-corp"], repos = ["acme-corp/*"] }
```

---

## CLI

```
locus init [--with-samples] · quickstart
locus onboard [--detect-only] [--yes] [--reset]  # guided refs-only tenant setup
locus client add                      # guided client binding (flags for every prompt)
locus pin [alias] [--force] [--client claude] [--ns a,b] [--ttl 2h]
locus enter <alias>                   # firm workflow pin
locus switch <alias>                  # one-shot leave + enter
locus leave · renew                   # clear pin; extend expiry (opt-in per binding)
locus whoami [--json]
locus status [--oneline] [--json]
locus exec -- <command> [args...]
locus exec --expect <alias> -- <cmd>  # refuse (exit 1, child never starts) unless <alias> is pinned
locus exec --for gh -- gh pr list    # inject only the pinned GitHub credential
locus run -b <alias> -- <command>     # one-shot; global pin unchanged
locus binding list|show|add|rm|migrate-credential-refs
locus capability status|persist|unpersist  # where control authority lives
locus autopin suggest [--apply]       # learned per-repo binding from pin history
locus workspace --default <alias> [--allow a,b] [--require-pin]
locus doctor [--json]                 # SAFE|WARN|UNSAFE (exit 0/1/2)
locus doctor --fix [--apply]          # preview safe repairs; --apply makes them, backing up first
locus watch [--once] [--require-ok]   # session heartbeat each tick (NDJSON with --json)
locus dashboard · serve               # local identity UI + API (127.0.0.1)
locus forensics export                # shareable pack (no secrets)
locus verify session · claim --text "…"  # verification plane: session pack · claim score
locus approve list|grant|status|wait|deny  # require_approval / dual-control
locus notify status|on|off            # desktop banners OFF by default
locus events --last N · events export # audit tail / fleet pulse / OTLP
locus graph list|export|import        # encrypted binding graph share (no secrets)
locus ci mint|env|run                 # short-lived sealed sessions for pipelines
locus engagement init|close           # client engagement lifecycle (binding + workspace)
locus upstream list|suggest           # built-in upstream MCP recipes
locus adapter list|verify|trust       # provider adapter catalog + signature trust
locus adapter search|install|uninstall · registry export|verify-manifest  # community + release adapters
locus hook zsh|bash|fish
locus setup --client claude|cursor|codex|grok|gemini|generic
locus mcp [--supervised]              # run the stdio MCP server; --supervised serves pinned provider tools
locus mcp mint|list|revoke            # multi-tenant grant tokens
locus agent report|setup|doctor       # AI-native readiness (hub JSON)
locus goal status                     # northstar progress from GOALS.md
locus completion zsh|bash|fish|elvish|powershell  # shell completions
locus version                         # build version (also: locus --version)
locus topic <name>                    # dashboard · forensics · serve · goal · verify · …
```

**Agency kit:** [`examples/agency-starter/`](./examples/agency-starter/) — personal ↔ client A ↔ client B, dual-control, workspaces, offboarding. Guide: [docs/agency-starter.md](./docs/agency-starter.md).

**Northstar / hub:** [GOALS.md](./GOALS.md) · [docs/hub-integration.md](./docs/hub-integration.md) · [integrations/phantom/](./integrations/phantom/)

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
| **2.x / 0.2–0.5** Dashboard, forensics, HTTP MCP, agent report, goal loop; 0.3 dogfood polish, TTL, client add, MCP session pin-anchoring; 0.4 multi-tenant multiplexor, Anthropic/OpenAI adapters, `locus switch`; 0.5 signed registry manifests, hub MT drop-in | **you are here (0.5.2)** |
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

# README quickstart rehearsal: replays the extracted 30-second start in a
# throwaway HOME (--dry-run prints the plan without running it)
./scripts/check-readme-quickstart.sh
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
| [scripts/check-readme-quickstart.sh](./scripts/check-readme-quickstart.sh) | README quickstart replay in a throwaway HOME |
| [docs/architecture.md](./docs/architecture.md) | System diagram (DESIGN distilled) |
| [docs/BENCHMARKS.md](./docs/BENCHMARKS.md) | Install and core-op timings, peers, method |
| [docs/FIRST-RUN.md](./docs/FIRST-RUN.md) | First-run log and agent-use audit |
| [docs/agency-certainty.md](./docs/agency-certainty.md) | Identity vs epistemic certainty (Ashlr stack) |
| [docs/firm-mode.md](./docs/firm-mode.md) | Agencies: bindings, dual-control, workspaces |
| [docs/agency-starter.md](./docs/agency-starter.md) | Agency starter kit + doctor single pane |
| [examples/agency-starter/](./examples/agency-starter/) | Bindings, workspaces, dual-control, offboarding |
| [docs/phantom-enrolled-repos.md](./docs/phantom-enrolled-repos.md) | Phantom workbench: personal / company / client scope bindings per enrolled repo |
| [docs/gateway-base-urls.md](./docs/gateway-base-urls.md) | Model gateways (for example a LiteLLM proxy): Locus makes no provider calls, `locus exec` drops unknown env vars, and how to set a gateway URL inside it |
| [docs/PQ-READINESS.md](./docs/PQ-READINESS.md) | Post-quantum readiness note: what Locus uses today (Ed25519 for adapter manifests, classical only) and what a migration would touch. No quantum-safety claim |
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
- `locus exec --for <cli>` routes `gh`, `vercel`, `supabase`, or `aws` with only the selected provider's credential and fresh private configuration. See [CLI routing](docs/cli-routing.md) for credential formats and the trusted-process boundary.
- MCP never returns secret values; agents cannot pin (request only).
- Scope freeze: model cannot override frozen `project_ref` / `team_id`.
- Policy: globs + structured `[[rules]]`, `require_approval`, dual-control (2 principals).
- Upstream MCP workers start only after an authorized provider call. `tools/list` is discovery-only, multi-provider startup rolls back on partial failure, and each worker receives only its named provider's resolved credential keys.
- Drift freeze + hub heartbeat: `locus watch [--json] [--require-ok]` re-runs session verify each tick; doctor re-pin if binding changes under a session.

Details: [SECURITY.md](./SECURITY.md). Threat model: [DESIGN.md §9](./DESIGN.md).

---

## Part of the Ashlr ecosystem

Locus is one of the open-source tools from [Ashlr](https://ashlr.ai), a forward-deployed engineering company. Each one installs and runs on its own:

- **[Phantom](https://phm.dev)** (`phm`): the agent workbench. Source: [ashlrai/phantom](https://github.com/ashlrai/phantom).
- **[Phantom Secrets](https://phm.dev/secrets)** (`phantom`): keeps real API keys out of agent context with `phm_` placeholders. Source: [ashlrai/phantom-secrets](https://github.com/ashlrai/phantom-secrets).
- **[Lexicon](https://lexicon.ashlr.ai)** (`lexicon`): fixes the words dictation gets wrong.
- **[Ashlr MD](https://md.ashlr.ai)**: a Markdown app with an MCP server.
- **[Ashlr Stack](https://stack.ashlr.ai)** (`stack`): provisions services and stores their keys in Phantom Secrets.
- **[ashlr-plugin](https://plugin.ashlr.ai/)** (`ashlr-plugin`): token-efficiency tools for Claude Code and Codex.

## Contributing

Issues and PRs are welcome. Look for [`good first issue`](https://github.com/ashlrai/locus/labels/good%20first%20issue) to get started, and read [CONTRIBUTING.md](CONTRIBUTING.md). Report security problems privately per [SECURITY.md](SECURITY.md).

## License

MIT — see [LICENSE](./LICENSE).

Code of Conduct: [CODE_OF_CONDUCT.md](./CODE_OF_CONDUCT.md).

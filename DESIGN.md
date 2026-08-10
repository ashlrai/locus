# Locus — Hard Isolation for Multi-Account Agents

**Working codename:** `mmcp` (multiplexed MCP)  
**Recommended name:** **Locus**  
**Sibling product:** [Phantom Secrets](https://phm.dev) — protects secrets *in context*. Locus protects *which identity acts*.

> Phantom answers: *can this secret enter the model?*  
> Locus answers: *as whom, against which tenant, with which tools, right now?*

---

## 1. North-star vision

Every AI coding agent today inherits a single ambient identity: whatever is in the shell env, the global `gh auth`, the one Supabase project baked into an MCP config, the personal Vercel OAuth token that happens to be logged in. Contract firms, founders with side projects, and anyone juggling client work live in permanent account-confusion risk — agents silently mutate the wrong Supabase, deploy to the wrong Vercel team, or open PRs under the personal GitHub while the human meant “Acme.” Locus is the **identity plane for local (and eventually team) agents**: a single MCP endpoint and CLI that resolves a **Binding** — principal × tenant × scope × credential_ref × policy — and makes wrong-account action **mechanically impossible**, not merely discouraged. Credentials never enter the model (Phantom handles that). Account selection never relies on ambient global CLI state (Locus handles that). You pin a session — “work on Acme” — and every tool call, every CLI exec, every parallel agent child is hard-scoped to that binding until you explicitly re-pin.

---

## 2. Name candidates

| Name | Rationale | Risk |
|------|-----------|------|
| **Locus** ★ | Latin *place*; “where action is taken.” `locus pin acme`, “session locus.” Short, OSS-friendly, pairs with Phantom (ghost of a secret / place of an act). | Mildly academic |
| **Aperture** | Optical metaphor: opens onto exactly one tenant; light from other accounts cannot enter. Strong isolation story. | Longer; camera-nerd |
| **Bind** / **Bindr** | Core primitive *is* Binding. Verb-as-product (`bind pin acme`). | Bindr looks startup-y; Bind collides with DNS/bind |
| **Lane** | Traffic isolation; parallel agents get parallel lanes. Fast DX (`lane pin acme`). | Generic; gaming connotations |
| **Mooring** | Nautical: dock a session to a tenant; ships don’t share moorings. Memorable. | Slightly long |
| **Callsign** | Radio identity: every transmission carries who you are. Great for audit. | Less “product,” more feature |
| **Prism** | One MCP surface, many tenant-colored rays. Multiplex metaphor. | Overused in tech |
| **Seat** | “Take a seat at Acme.” Human, desk metaphor for agencies. | Soft; easy to confuse with seats-as-licenses |
| **Cell** | Hard isolation unit. Mechanically honest. | Negative (prison) |
| **mmcp** | Codename only: multiplexed MCP. Descriptive, unbrandable. | Keep as technical moniker |

**Recommendation:** ship as **Locus** (`locus` CLI, `locus-mcp` server). Keep `mmcp` as the crate/package monorepo root if desired (`locus` binary, `@locus/mcp` npm, etc.).

Tagline options:
- *“Wrong account, impossible.”*
- *“Pin the place. Act only there.”*
- *“Identity plane for agents.”*
- *“Phantom for *who*. Locus for *where*.”*

---

## 3. Core abstractions

### 3.1 Glossary

| Term | Definition |
|------|------------|
| **Principal** | Who is acting: a human user, a named agent role (`founder`, `ci-bot`), or a service identity. |
| **Tenant** | The customer/org/project boundary of action: `personal`, `acme-corp`, `side-project-x`. Not always 1:1 with a SaaS org — a tenant is *your* unit of “whose work is this.” |
| **Provider** | An external system: `supabase`, `vercel`, `github`, `cloudflare`, `aws`, `resend`, `stripe`, `xai`, … |
| **Account** | A concrete login/org under a provider: `github:ashlr-ai`, `vercel:team_Acme`, `supabase:proj_xyz`. |
| **Scope** | Least-privilege slice: tool allowlist, read_only, project_ref, repo allowlist, region, etc. |
| **CredentialRef** | Opaque pointer into the Credential Plane (Phantom token name, keychain item, OAuth grant id). **Never** the secret itself. |
| **Binding** | The atomic unit of authority: principal × tenant × {provider accounts} × scopes × credential_refs × policy. |
| **Workspace** | A filesystem root (git repo or monorepo package) that *defaults* a tenant/binding via `.locus.toml`. |
| **Session** | A live agent connection (MCP session, CLI process group, Cursor chat) **pinned** to exactly one Binding (or a sealed multi-binding set — see hard isolation). |
| **Policy** | Allow / deny / require-approval rules evaluated *before* a tool call is forwarded to a worker. |
| **Worker** | Isolated subprocess speaking upstream MCP or provider API **for one Binding only**. |

### 3.2 Binding schema (canonical)

```toml
# ~/.locus/bindings/acme-supabase.toml
# or composed from graph store — TOML is the human-editable source of truth for MVP

[binding]
id = "bnd_acme_supabase_rw"
alias = "acme"                    # used in `locus pin acme`
tenant = "acme-corp"
principal = "mason"               # optional; defaults to local user
description = "Acme client — Supabase + Vercel + GH"

[binding.policy]
default = "allow"                 # allow | deny
require_approval = ["*.delete*", "*.drop*", "vercel.deploy.prod"]
dual_control = ["*.delete*", "vercel.deploy.prod"]  # two distinct principals
# dual_control_all_approvals = true                   # all require_approval tools
max_ttl = "8h"                    # session auto-expires
parallel_sessions = 4             # cap concurrent workers for this binding

[[binding.providers]]
provider = "supabase"
account = "acme-prod"
credential_ref = "phm:SUPABASE_ACME_SERVICE_ROLE"   # Phantom / vault pointer
scope = { project_ref = "abcdefghij", read_only = false, tools = ["*"] }

[[binding.providers]]
provider = "vercel"
account = "team_acme"
credential_ref = "oauth:vercel:team_acme"
scope = { team_id = "team_xxx", projects = ["acme-web"], env = ["preview", "production"] }

[[binding.providers]]
provider = "github"
account = "ashlr-for-acme"        # machine user or org installation
credential_ref = "phm:GH_TOKEN_ACME"
scope = { orgs = ["acme-corp"], repos = ["acme-corp/*"], permissions = ["contents:write", "pull_requests:write"] }
```

JSON Schema (control-plane API / validation):

```json
{
  "$id": "https://locus.dev/schemas/binding.v1.json",
  "type": "object",
  "required": ["id", "alias", "tenant", "providers"],
  "properties": {
    "id": { "type": "string", "pattern": "^bnd_[a-z0-9_]+$" },
    "alias": { "type": "string", "minLength": 1, "maxLength": 64 },
    "tenant": { "type": "string" },
    "principal": { "type": "string" },
    "policy": {
      "type": "object",
      "properties": {
        "default": { "enum": ["allow", "deny"] },
        "require_approval": { "type": "array", "items": { "type": "string" } },
        "dual_control": { "type": "array", "items": { "type": "string" } },
        "dual_control_all_approvals": { "type": "boolean" },
        "max_ttl": { "type": "string" },
        "parallel_sessions": { "type": "integer", "minimum": 1 }
      }
    },
    "providers": {
      "type": "array",
      "minItems": 1,
      "items": {
        "type": "object",
        "required": ["provider", "account", "credential_ref"],
        "properties": {
          "provider": { "type": "string" },
          "account": { "type": "string" },
          "credential_ref": { "type": "string" },
          "scope": { "type": "object" }
        }
      }
    }
  }
}
```

### 3.3 Workspace schema

```toml
# /Users/mason/clients/acme/.locus.toml
version = 1
default_binding = "acme"          # alias or id
# Optional: allowlist of bindings this directory may pin to
allowed_bindings = ["acme", "acme-readonly"]
# Optional: refuse to start agent without pin
require_pin = true
# Optional: inherit monorepo parent
# (resolved by walking parents; child overrides win)
```

### 3.4 Session schema (runtime, not user-edited)

```json
{
  "session_id": "ses_01J...",
  "binding_id": "bnd_acme_supabase_rw",
  "binding_alias": "acme",
  "principal": "mason",
  "tenant": "acme-corp",
  "source": "dir:/Users/mason/clients/acme",
  "client": "claude-code",
  "pinned_at": "2026-08-06T18:00:00Z",
  "expires_at": "2026-08-07T02:00:00Z",
  "mode": "exclusive",
  "worker_pids": { "supabase": 482//1, "vercel": 48212, "github": 48213 },
  "seal": "hmac-sha256:..."
}
```

**`seal`:** HMAC over `(session_id || binding_id || pinned_at || expires_at)` with a local daemon key. The data plane rejects any tool call whose session seal does not verify. Sessions cannot be “prompt-edited” into another binding — only the control plane (CLI with local auth) can re-pin and re-seal.

### 3.5 Policy evaluation order

```
1. Session has valid seal?            else DENY (no ambient fallthrough)
2. Tool's provider in binding?        else DENY
3. Tool matches scope allowlist?      else DENY
4. Adapter hard-constraints pass?     else DENY  (e.g. project_ref mismatch)
5. Structured [[binding.policy.rules]] first matching rule (allow|deny|require_approval|dual_control)
6. Legacy require_approval / dual_control globs → queue human approval when matched
7. policy.default                     ALLOW or DENY
8. Emit audit event (always)
```

See [docs/policy.md](./docs/policy.md) for rule syntax, dual-control, and `locus approve` UX.

There is **no** “if unsure, use personal account” path. Unbound = empty tool list.

---

## 4. Architecture layers

```
┌──────────────────────────────────────────────────────────────────┐
│  Clients: Claude Code · Cursor · Codex · human CLI · CI agents   │
└─────────────────────────────┬────────────────────────────────────┘
                              │ MCP stdio / Streamable HTTP
                              │ (single endpoint: locus-mcp)
┌─────────────────────────────▼────────────────────────────────────┐
│  DATA PLANE — Multiplexor / Gateway                               │
│  • Session pin resolution                                         │
│  • Tool catalog composition + namespacing                         │
│  • JSON-RPC fan-out to per-binding Workers                        │
│  • Capability-token minting on tools/call                         │
└────────┬────────────────────┬─────────────────────┬──────────────┘
         │                    │                     │
┌────────▼────────┐  ┌────────▼────────┐  ┌─────────▼─────────┐
│ CONTROL PLANE   │  │ POLICY PLANE    │  │ AUDIT PLANE       │
│ Identity graph  │  │ CEL/Rego-lite   │  │ Append-only log   │
│ bindings store  │  │ approval queue  │  │ HMAC chain        │
│ session registry│  │ rate limits     │  │ export / SIEM     │
└────────┬────────┘  └─────────────────┘  └───────────────────┘
         │
┌────────▼────────────────────────────────────────────────────────┐
│  CREDENTIAL PLANE                                                 │
│  Phantom Secrets (phm_ + proxy) · OS keychain · OAuth broker      │
│  Resolves CredentialRef → env/header injection into Workers ONLY  │
│  Model / client never sees resolved secrets                       │
└─────────────────────────────┬────────────────────────────────────┘
                              │ spawn with sealed env
┌─────────────────────────────▼────────────────────────────────────┐
│  WORKERS (one process tree per Binding × Provider, or per Binding)│
│  Upstream MCP servers / thin adapters                             │
│  • supabase-mcp with project_ref env                              │
│  • vercel tools with team-scoped token                            │
│  • gh api via GH_TOKEN only in this PID                           │
│  • aws via AWS_PROFILE in isolated HOME/config                    │
└──────────────────────────────────────────────────────────────────┘
```

### 4.1 Control plane — identity graph

**Store (MVP):** `~/.locus/` 

```
~/.locus/
  config.toml                 # daemon, defaults
  daemon.sock                 # UDS control API
  daemon.key                  # sealing key (0600, keychain preferred)
  bindings/                   # one file per binding (or sqlite)
    personal.toml
    acme.toml
    acme-readonly.toml
  tenants/
    acme-corp.toml            # metadata, color, display name
  principals/
    mason.toml
  sessions/                   # live session seals (or sqlite)
  audit/
    events.jsonl
    chain.state
  adapters/                   # installed provider adapters
  workers/                    # ephemeral run dirs per worker
    ses_01J.../
      supabase/
        env.sealed            # not readable by client
        stdout.log
```

**Identity graph edges:**

```
Principal ──acts_as──► Binding ──for──► Tenant
                │
                ├──includes──► ProviderAccount
                ├──uses──► CredentialRef
                └──constrained_by──► Policy + Scope
Workspace ──defaults_to──► Binding
Session ──pinned_to──► Binding   (1:1 exclusive mode)
```

**Control API (UDS + CLI):**

```
locus binding list|show|add|edit|rm
locus tenant list|add
locus session list|pin|unpin|status
locus doctor
locus graph export            # mermaid / json for debugging
```

Daemon protocol (length-prefixed JSON over UDS), illustrative:

```json
// Request
{ "v": 1, "op": "session.pin", "alias": "acme", "client": "claude-code", "cwd": "/Users/mason/clients/acme" }

// Response
{
  "v": 1,
  "session": { "session_id": "ses_01J...", "binding_id": "bnd_...", "seal": "hmac...", "expires_at": "..." },
  "tools_digest": "sha256:..." 
}
```

### 4.2 Data plane — MCP multiplexor

**Process model (critical):**

| Approach | Isolation | Parallel agents | Complexity |
|----------|-----------|-----------------|------------|
| A. One shared MCP process, swap env per call | Weak — race-prone | Broken | Low |
| B. **One worker process per Binding×Provider** ★ | Strong | Safe | Medium |
| C. One full sandbox VM per binding | Strongest | Expensive | High |

**MVP ships B.** The multiplexor (`locus-mcp`) is the only process clients talk to. On `session.pin` / first tool use, it spawns workers:

```
locus-mcp (stdio to Claude)
  ├─ worker ses_X:supabase  (stdio child, env: SUPABASE_* from Credential Plane)
  ├─ worker ses_X:vercel
  └─ worker ses_X:github
```

**Tool catalog composition:**

```
tools/list →
  For active session binding:
    For each provider worker:
      tools/list on worker
      prefix or tag tools:
        mode=namespace → "acme__supabase__list_tables"
        mode=transparent → "list_tables"  (only one binding active)
  Plus control tools:
    locus_status, locus_pin, locus_unpin, locus_bindings, locus_whoami
```

**MVP default: transparent mode** when exactly one binding is pinned (best agent UX).  
**Namespace mode** when multi-binding “sealed set” is enabled (advanced).

**tools/call path:**

```
1. Client → locus-mcp: tools/call { name, arguments }
2. Resolve session → binding (from MCP session id / stdio process association)
3. Verify seal + TTL
4. Policy plane decide
5. If approval needed → return structured "approval_required" content; pause
6. Mint capability ticket (30s TTL):
     cap = HMAC(daemon_key, session_id|binding_id|tool|args_hash|exp)
7. Forward to correct worker with cap in side channel (not model-visible)
8. Worker adapter enforces scope again (defense in depth)
9. Audit plane records request/response meta (redacted)
10. Return result to client
```

**Why not “just multiple MCP entries in claude config”?**  
See §12. Short version: N configs still share global CLI state, have no session pin, no policy, no audit, no parallel safety, and the model sees all tools from all accounts at once.

### 4.3 Credential plane

Integrates **Phantom** as the preferred secret backend; falls back to OS keychain / encrypted file.

```
CredentialRef forms:
  phm:NAME              → Phantom vault entry (token phm_xxx in env files;
                          worker gets real value only via Phantom exec/proxy
                          OR Locus daemon resolves via phantom IPC)
  keychain:SERVICE/ACCT → macOS Keychain / libsecret
  oauth:provider:acct   → Locus OAuth broker tokens (refresh handled by daemon)
  file:path             → discouraged; encrypted only
  env:VAR               → only for bootstrap; never for agent-visible env
```

**Injection rules (non-negotiable):**

1. Resolved secrets are written only into **worker process env** or **worker-only temp files** with `0600`, deleted on worker exit.
2. Parent `locus-mcp` process **does not** retain plaintext after spawn (zeroize).
3. Client-facing env (Claude, Cursor) may contain `phm_` tokens and `LOCUS_SESSION=ses_...` only.
4. For HTTP-style providers, prefer Phantom-style base URL rewrite through a **binding-scoped proxy path**:

```
http://127.0.0.1:{locus_port}/bind/{binding_id}/supabase/...
```

Gateway injects credentials + forces `project_ref` path prefix. Agent never holds the service role key.

**OAuth broker (Vercel, GitHub App, etc.):**

- Daemon holds refresh tokens in keychain.
- Per-binding access tokens minted with account scope.
- Proactive refresh + per-account mutex (avoid refresh races under parallel tool calls).

### 4.4 Policy plane

MVP: glob rules in binding TOML.  
Ambitious: embedded CEL or a tiny Rego subset.

```toml
[[policy.rules]]
action = "deny"
tool = "supabase.*"
when = 'args.sql matches "(?i)drop\\s+table"'

[[policy.rules]]
action = "require_approval"
tool = "vercel.deploy"
when = 'args.target == "production"'

[[policy.rules]]
action = "allow"
tool = "github.*"
when = 'args.owner == "acme-corp"'
```

**Approval UX:**

- CLI: `locus approve grant <id> --as <principal>` records a local advisory label; `locus approve status <id>` shows authoritative progress separately.
- Dual-control (firm mode): tools matching `policy.dual_control` (or all when
  `dual_control_all_approvals = true`) require two independently authenticated
  external approvers. Local principal strings never change `status=pending` or
  establish authority. The external verifier is intentionally absent today.
- MCP tool result: report the closed external-envelope requirement and instruct agents not to retry after local advisory labels.
- Optional: auto-deny after N minutes / macOS notification + Touch ID

### 4.5 Audit plane

Append-only JSONL with HMAC chain (same pattern as Phantom audit):

```json
{
  "seq": 1042,
  "ts": "2026-08-06T18:12:01Z",
  "prev_hash": "sha256:...",
  "event": "tools.call",
  "session_id": "ses_01J...",
  "binding_id": "bnd_acme...",
  "tenant": "acme-corp",
  "principal": "mason",
  "client": "claude-code",
  "tool": "list_tables",
  "provider": "supabase",
  "account": "acme-prod",
  "decision": "allow",
  "latency_ms": 84,
  "args_digest": "sha256:...",
  "result_meta": { "ok": true, "rows": 12 }
}
```

```
locus audit show --tenant acme-corp --last 50
locus audit verify
locus audit export --since 7d --out acme-week.jsonl
```

---

## 5. End-to-end flows

### 5.1 Human CLI

```bash
# One-time
locus init
locus binding add acme --from wizard   # interactive: pick providers, CredentialRefs

# Daily
cd ~/clients/acme                      # .locus.toml → default_binding = acme
locus pin                              # or: locus pin acme
locus status
# ┌─ LOCUS ─────────────────────────────────────────┐
# │ pin: acme (bnd_acme…)   tenant: acme-corp       │
# │ providers: supabase✓ vercel✓ github✓            │
# │ session: ses_01J…  expires: 6h left             │
# │ mode: exclusive · workers: 3                    │
# └─────────────────────────────────────────────────┘

locus exec -- supabase db push         # runs with Acme credentials only
locus exec -- gh pr create             # GH_TOKEN for acme binding only
locus exec -- vercel deploy --prod     # may require approval

locus pin personal                     # switch
locus unpin                            # no binding → dangerous CLIs refuse
```

`locus exec` implementation:

1. Resolve pin (session or cwd).
2. Create ephemeral worker env dir.
3. Resolve CredentialRefs → env.
4. For CLIs that use global config (`gh`, `aws`): set  
   `GH_CONFIG_DIR`, `AWS_CONFIG_FILE`, `AWS_SHARED_CREDENTIALS_FILE`, `XDG_CONFIG_HOME`  
   to **worker-private paths** populated for this binding only.
5. `execve` the command — never `gh auth switch` on the user’s real home.
6. Tear down env files on exit.

### 5.2 Claude Code (MCP)

```bash
locus setup --client claude
# writes .claude/settings or global mcp config:

# {
#   "mcpServers": {
#     "locus": {
#       "command": "locus-mcp",
#       "args": [],
#       "env": { "LOCUS_AUTO_PIN": "cwd" }
#     }
#   }
# }
```

**Startup sequence:**

1. Claude spawns `locus-mcp` (stdio).
2. `locus-mcp` connects to daemon via UDS.
3. If `LOCUS_AUTO_PIN=cwd`, daemon reads workspace `.locus.toml` → pin `acme`.
4. Spawns workers for that binding.
5. `tools/list` returns only Acme-scoped tools + `locus_*` control tools.
6. Agent calls `list_tables` → only Acme Supabase project exists in that worker.
7. Human runs `locus pin personal` in another terminal → daemon updates session → `tools/list` changed notification → workers recycled.

**Control tools exposed to the model (carefully):**

| Tool | Model can? | Notes |
|------|------------|-------|
| `locus_whoami` | yes | tenant, binding, scopes (no secrets) |
| `locus_status` | yes | read-only |
| `locus_bindings` | yes | list aliases + descriptions |
| `locus_pin` | **optional** | default **off** for agents; human CLI only. Prevents prompt injection re-pin to personal. |
| `locus_request_pin` | yes | proposes pin; requires human confirm in terminal / OS prompt |

**Default: agents cannot change pin.** They can only *request*. This is a load-bearing security choice.

### 5.3 Cursor

Same MCP registration (`~/.cursor/mcp.json`). Cursor project root → auto-pin via `.locus.toml`.

Status integration options:
- MCP resource `locus://status` for a persistent “current pin” card
- Optional Cursor rule snippet generated by `locus setup` reminding the agent to call `locus_whoami` before infra actions

### 5.4 Parallel agents

Problem: two agents, two tenants, one laptop — classic `gh auth switch` / shared MCP env races.

Locus model:

```
Agent A (Claude) ──stdio──► locus-mcp A ──► session_A ──pin──► binding:acme
Agent B (Cursor) ──stdio──► locus-mcp B ──► session_B ──pin──► binding:personal
                              │                    │
                              └──── daemon ────────┘
                                    workers isolated by PID + private config dirs
```

Each `locus-mcp` instance is one client session. Daemon coordinates:
- Per-binding worker pools (or dedicated workers per session for max isolation)
- No shared mutable global CLI state
- Audit distinguishes session_A vs session_B

**Subagent / Task tool pattern:**

```bash
# Parent agent spawns helper with explicit pin — not ambient inheritance of wrong account
locus run --binding acme-readonly -- claude -p "audit schema drift"
```

`locus run` creates a **child session** sealed to that binding, with its own workers, discarded on exit.

---

## 6. Hard isolation — wrong-account impossible, not discouraged

These are **mechanisms**. Soft prompts and “please check the project ref” are non-goals.

| # | Mechanism | Failure mode prevented |
|---|-----------|------------------------|
| 1 | **Sealed session pin** | Model cannot claim a different tenant by tool-arg persuasion |
| 2 | **Empty catalog when unbound** | No ambient personal tools “just work” by accident |
| 3 | **One worker PID per binding×provider** | Credential A cannot be used on request intended for B |
| 4 | **Private CLI config dirs** (`GH_CONFIG_DIR`, etc.) | `gh`/`aws` global profile races with parallel agents |
| 5 | **No `auth switch` on user home** | Never mutates developer’s interactive login as side effect |
| 6 | **Adapter-level hard scope** | Supabase worker process only knows one `project_ref`; URL forced |
| 7 | **Capability tickets on tools/call** | Stale or cross-session replays fail HMAC |
| 8 | **Agent cannot pin by default** | Prompt injection cannot jump tenants |
| 9 | **Workspace allowlist** | Repo for Acme cannot pin to `personal` even via CLI without `--force` + audit |
| 10 | **TTL + max parallel sessions** | Forgotten pins expire; runaway agents capped |
| 11 | **Credential plane opacity** | Service keys never in model context (Phantom-compatible) |
| 12 | **Network path fencing** (ambitious) | Binding-scoped local reverse proxy forces host/account path |
| 13 | **Deny multi-binding transparent mode** | You may open multiple bindings only in namespaced mode where tool names encode tenant |
| 14 | **Worker seccomp/sandbox** (ambitious) | Worker cannot read `~/.locus/bindings/*` for other tenants |

### 6.1 Supabase example (hard)

Naive: one Supabase MCP with token that can access all projects the user owns → agent picks wrong ref.

Locus:

```toml
[[binding.providers]]
provider = "supabase"
credential_ref = "phm:SUPABASE_ACME"
scope = { project_ref = "abcdefghij", read_only = true }
```

Worker env:

```
SUPABASE_ACCESS_TOKEN=<resolved>
SUPABASE_PROJECT_REF=abcdefghij
SUPABASE_READ_ONLY=true
```

Adapter **rejects** any tool argument that tries to set another `project_ref`. If upstream MCP supports URL scoping, worker is launched with that project’s endpoint only. The personal Supabase binding is a **different worker** with a different token — not reachable from this session’s tool catalog.

### 6.2 GitHub example (hard)

```bash
# Worker private dir
export GH_CONFIG_DIR=/var/folders/.../locus/ses_X/github/gh-config
export GH_TOKEN=<resolved from phm:GH_TOKEN_ACME>   # fine-scoped PAT or installation token
# No GH_TOKEN in parent. No write to ~/.config/gh.
```

Org allowlist enforced in adapter wrapper before `gh` runs:

```
allowed: acme-corp/*
denied:  ashlr-ai/*   # personal org not in this binding
```

### 6.3 AWS example (hard)

Never set `AWS_PROFILE` globally. Instead:

```
AWS_CONFIG_FILE=.../ses_X/aws/config
AWS_SHARED_CREDENTIALS_FILE=.../ses_X/aws/credentials
AWS_EC2_METADATA_DISABLED=true
```

Config file contains **only** the binding’s profile. `aws sts get-caller-identity` in that worker cannot return the personal account.

---

## 7. Developer experience

### 7.1 Magic commands

```bash
locus pin acme                 # pin current shell + default MCP sessions for this cwd
locus pin acme --session all   # all live MCP clients (with confirm)
locus whoami                   # human-readable identity card
locus status                   # dashboard in terminal
locus exec -- <cmd>            # run cmd inside pin
locus run -b acme -- <cmd>     # one-shot without changing shell pin
locus approve <id>             # policy approval
locus doctor                   # wiring, phantom, adapters, stale sessions
locus setup --client claude|cursor|windsurf|codex
```

### 7.2 Status bar / whoami

```
$ locus whoami
tenant      acme-corp
binding     acme (bnd_acme_supabase_rw)
principal   mason
providers   supabase:acme-prod (ro) · vercel:team_acme · github:acme-corp
session     ses_01J8…  expires 5h52m
source      dir:~/clients/acme
clients     claude-code (stdio) · 1 worker set
```

Shell prompt integration (optional):

```bash
# eval "$(locus hook zsh)"
# PROMPT fragment: [locus:acme]
```

### 7.3 Dir-binding

```toml
# .locus.toml committed to client repos (no secrets!)
version = 1
default_binding = "acme"
allowed_bindings = ["acme", "acme-readonly"]
require_pin = true
```

```toml
# personal projects
version = 1
default_binding = "personal"
```

CI-safe: `.locus.toml` never contains CredentialRefs — only aliases.

### 7.4 Session pin language

Human phrases Locus should support as first-class:

| Phrase | Command |
|--------|---------|
| “Work on Acme” | `locus pin acme` |
| “Switch to personal” | `locus pin personal` |
| “Read-only Acme” | `locus pin acme-readonly` |
| “Who am I acting as?” | `locus whoami` / `locus_whoami` |
| “Drop privileges” | `locus pin acme-readonly` or `locus unpin` |

Agent-facing copy when wrong context suspected:

```
You are pinned to tenant=acme-corp (binding=acme).
Tools only affect that tenant. To work elsewhere, ask the human to re-pin.
```

### 7.5 Wizard UX (first binding)

```
$ locus binding add
? Alias: acme
? Tenant display name: Acme Corp
? Providers: [x] Supabase [x] Vercel [x] GitHub [ ] Cloudflare [ ] AWS
— Supabase —
? Credential: (p)hantom ref / (k)eychain / (paste once → vault)
? Project ref: abcdefghij
? Read only? Yes
— GitHub —
? Use GitHub App installation for acme-corp? Yes
...
ok  binding acme created
ok  wrote ~/clients/acme/.locus.toml
next locus setup --client claude && locus pin acme
```

---

## 8. Provider adapter model

Adapters are the only place provider-specific knowledge lives.

```
locus-adapter trait (Rust sketch)
────────────────────────────────
name() -> &'static str
detect_accounts() -> Vec<AccountCandidate>
validate_scope(scope: &Value) -> Result<()>
spawn_worker(ctx: WorkerCtx) -> Child    // env, args, cwd
wrap_tools(tools: Vec<Tool>) -> Vec<Tool> // inject constraints in descriptions
enforce_call(tool: &str, args: &Value) -> Result<Value>  // hard deny
healthcheck(child) -> Status
```

### 8.1 Adapter matrix (MVP → ambitious)

| Provider | Auth styles | Hard scope knobs | Worker strategy |
|----------|-------------|------------------|-----------------|
| **Supabase** | PAT / service role via Phantom | `project_ref`, `read_only` | Official MCP or REST adapter; force project in env/URL |
| **Vercel** | OAuth team token | `team_id`, project allowlist, env (preview/prod) | MCP or REST; reject cross-team |
| **GitHub** | PAT, GitHub App install | `orgs[]`, `repos[]`, permission bits | `gh` CLI in private `GH_CONFIG_DIR` or octokit worker |
| **Cloudflare** | API token | `account_id`, zone allowlist | REST worker; account_id frozen at spawn |
| **AWS** | profile / SSO / keys | `account_id`, `region`, IAM policy ARN | private AWS config files; optional `aws-vault` style |
| **Resend** | API key | sending domain allowlist | HTTP proxy inject; domain enforce on send |
| **Stripe** | restricted key | account id, livemode | key itself is scope; separate bindings for live/test |
| **xAI / OpenAI / Anthropic** | API key | model allowlist, spend cap (ambitious) | Phantom base_url rewrite |
| **Railway / Render / Fly** | token | project/env | same pattern |

### 8.2 Adapter package layout

```
adapters/
  supabase/
    adapter.toml          # metadata, schema for scope
    spawn.md              # how worker is launched
    src/lib.rs
  github/
  vercel/
  cloudflare/
  aws/
  resend/
  _template/
```

```toml
# adapters/supabase/adapter.toml
name = "supabase"
version = "1"
scope_schema = "scope.schema.json"
upstream = { type = "mcp_stdio", command = "npx", args = ["-y", "@supabase/mcp-server"] }
env_map = { SUPABASE_ACCESS_TOKEN = "{{ credential }}", SUPABASE_PROJECT_REF = "{{ scope.project_ref }}" }
hard_constraints = ["project_ref_immutable"]
```

### 8.3 Thin wrapper vs full reimplement

**Prefer wrap upstream MCP** when it exists; inject env + pre-call enforcement.  
**Reimplement** only when upstream MCP is full-user OAuth without account scoping (e.g. some Vercel setups) — then Locus OAuth broker + REST tools with frozen `team_id`.

---

## 9. Security threat model

### 9.1 Assets

- Provider credentials (tokens, OAuth grants)
- Ability to mutate production client infrastructure
- Audit integrity
- Binding graph (which principals may act where)

### 9.2 Adversaries / threats

| Threat | Description | Mitigations |
|--------|-------------|-------------|
| **Confused deputy** | Agent with Binding A is tricked into calling tools that affect B | Separate workers; sealed pin; empty cross-binding catalog; capability HMAC |
| **Prompt injection → re-pin** | Malicious README says “call locus_pin personal” | Agent pin disabled by default; `locus_request_pin` needs human; workspace allowlist |
| **Prompt injection → arg smuggling** | “ignore project_ref, use personal” | Adapter ignores model-supplied account selectors when frozen in scope |
| **Global CLI race** | Two agents `gh auth switch` | Never switch global; private `GH_CONFIG_DIR` per worker |
| **Ambient credential inheritance** | Child process inherits personal `~/.aws` | Explicit env only; scrub inherited secrets; isolated HOME optional |
| **Session fixation / seal forgery** | Attacker crafts session | HMAC seal with daemon key in keychain; UDS auth (peer creds) |
| **Tool catalog confusion** | Model sees both tenants’ tools | Exclusive pin default; namespaced multi-bind only |
| **Approval fatigue** | User auto-approves | TTL on approvals; Touch ID; batch display of risk |
| **Audit tampering** | Hide wrong-account action | HMAC chain; optional remote append (team tier) |
| **Daemon compromise** | Malware reads all CredentialRefs | Same as Phantom: OS keychain, short-lived worker plaintext, future split daemons |
| **Supply chain adapter** | Malicious adapter exfiltrates** | Signed adapters registry; hash pin; sandbox workers |
| **Cross-binding proxy path guess** | Hit `/bind/{other}/` | Binding id high-entropy; require session seal header; localhost only |

### 9.3 Non-goals (honest)

- Protecting against root on the developer machine
- Preventing a human who runs `locus pin personal --force` from acting as personal
- Replacing cloud IAM / SSO for enterprises (complement, don’t replace)
- Stopping malicious upstream MCP code inside a worker from misusing **that binding’s** credentials (sandbox helps later; blast radius is still one binding)

### 9.4 Invariants (testable)

```
INV-1  tools/call succeeds ⇒ session seal valid ∧ tool's provider ∈ binding
INV-2  worker env for binding A contains no CredentialRef material for binding B
INV-3  parallel sessions A,B ⇒ disjoint GH_CONFIG_DIR / AWS_CONFIG_FILE paths
INV-4  unbound session ⇒ tools/list is only locus_* control tools (or empty)
INV-5  agent-initiated pin change ⇒ no state change without human approval record
INV-6  audit chain verifies after every N events in CI
```

Conformance suite: `locus test isolation` runs these as integration tests.

---

## 10. MVP vs ambitious roadmap

### Phase 0 — Spike (1–2 weeks)

- Daemon + UDS + binding TOML store
- `locus pin` / `whoami` / `status`
- `locus exec` with private env for `gh` + raw env injection
- One adapter: Supabase (project_ref frozen)
- Manual audit log

### Phase 1 — MVP (public OSS)

**Goal:** a contract founder can pin Acme vs personal and not fear Supabase/GitHub mixups.

- `locus-mcp` multiplexor (stdio)
- Exclusive session pin + seal
- Workspace `.locus.toml` auto-pin
- Adapters: **Supabase, GitHub, Vercel** (minimum three)
- Phantom CredentialRef integration
- Policy: allow/deny globs + require_approval list
- Audit JSONL + verify
- `locus setup` for Claude Code + Cursor
- Isolation conformance tests
- Docs that feel like Phantom (sharp, mechanism-first)

### Phase 2 — Parallel-agent hard mode

- Worker pools + namespaced multi-binding
- Binding-scoped local HTTP gateway (Phantom-like inject + account fence)
- AWS + Cloudflare + Resend adapters
- Approval UX (notifications, Touch ID)
- `locus run` for subagents
- Shell prompt hook + status bar

### Phase 3 — Team / firm

- Shared binding graph (E2E encrypted cloud, sibling to Phantom Cloud)
- Org tenants, role principals (`contractor-ro`)
- Remote audit sink + SIEM export
- Admin policy packs (“no prod deploys without two-person approval”)
- SSO for daemon unlock (enterprise)

### Phase 4 — Platform

- Adapter SDK + signed registry
- Streamable HTTP remote Locus for CI runners (ephemeral pins)
- Kubernetes/session broker for org-hosted agents
- Formal verification of seal/cap logic; bug bounty

---

## 11. Open source strategy

### License

- **Apache-2.0 OR MIT** dual-license (Rust ecosystem friendly; Apache for patent grant).  
  Match or exceed Phantom’s openness; if Phantom is MIT-only, Locus can be MIT for consistency — prefer dual if starting fresh.

### Repo layout

```
locus/                    # or mmcp/ monorepo renaming later
  crates/
    locus-cli/
    locus-daemon/
    locus-mcp/
    locus-core/           # schemas, seal, policy
    locus-audit/
    locus-adapters-*/
  adapters/               # optional dynamic
  docs/
  tests/isolation/
  DESIGN.md               # this file
  THREAT_MODEL.md
  README.md
```

### Dual product with Phantom

| | Phantom | Locus |
|--|---------|-------|
| Question | What can enter the model? | Who/where can the agent act? |
| Primitive | `phm_` token + proxy inject | Binding + sealed session pin |
| Failure | Key in context window | Wrong tenant mutation |
| Compose | Locus CredentialRefs → `phm:NAME`; workers run under Phantom proxy when calling HTTPS APIs | |

**Go-to-market narrative:**  
“Phantom stops leaks. Locus stops wrong-account. Install both; agents become safe to delegate.”

**Shared tech:**
- Audit chain format
- Keychain wrappers
- CLI UX language (`doctor`, `status`, `setup --client`)
- Optional same org + cloud billing later (Pro: multi-machine pins, team graphs)

**Avoid:** merging into one binary too early. Different release cadences; clear boundaries. A future `ashlr` meta-CLI can orchestrate both (`ashlr init` → phantom + locus).

### Community

- Adapter contributions as first-class (like Terraform providers)
- “Binding packs” for common agency setups (Supabase+Vercel+GH)
- Public isolation test suite as marketing *and* engineering bar

---

## 12. Why this is clever vs naive “multiple MCP configs”

| Naive N× MCP configs | Locus |
|----------------------|-------|
| Model sees **all** tools from personal + client servers at once → picks wrong one | Model sees **only** pinned binding’s tools |
| Each MCP still starts with **one credential at process start** — no unified pin | One pin drives all providers consistently |
| `gh` / `aws` / `vercel` CLIs still use **global** user state | Private config dirs per session; no global switch |
| Parallel agents race on `~/.config` | Disjoint worker filesystems |
| No audit of “who acted as whom” | Binding + principal on every event |
| No policy / approval on prod deploy | Policy plane + human gate |
| Switching = edit JSON + restart IDE | `locus pin acme` / dir-bind auto |
| Credentials duplicated across MCP env blocks | Single CredentialRef → Phantom/vault |
| Prompt can ask agent to “use the other supabase” if both configured | Other Supabase **not in catalog**; worker can’t reach it |
| Security is documentation (“be careful”) | Security is process isolation + seals + frozen scope |

**The clever bit** (Phantom-caliber sharpness):

> **Identity is resolved at the gate, not in the prompt.**  
> The agent never chooses an account. The session already *is* an account.  
> Credentials are injected into isolated workers the same way Phantom injects secrets on the wire — the model only ever sees aliases and tool results.

Naive multi-config multiplies attack surface and decision load. Locus **collapses** decision load to a single pin and **enforces** it with PID boundaries, not instructions.

---

## Appendix A — Example full config tree

```toml
# ~/.locus/config.toml
[daemon]
auto_start = true
default_ttl = "8h"
agent_can_pin = false
approval_timeout = "15m"

[clients]
auto_pin = "cwd"          # cwd | none | last

[credential]
preferred = "phantom"     # phantom | keychain

[audit]
enabled = true
path = "~/.locus/audit/events.jsonl"
```

```toml
# ~/.locus/tenants/acme-corp.toml
id = "acme-corp"
display_name = "Acme Corp"
color = "#3B82F6"
```

```toml
# ~/.locus/bindings/personal.toml
[binding]
id = "bnd_personal"
alias = "personal"
tenant = "personal"
principal = "mason"

[[binding.providers]]
provider = "github"
account = "masonwyatt"
credential_ref = "phm:GH_TOKEN_PERSONAL"
scope = { orgs = ["*"], repos = ["*"] }  # still isolated from acme workers

[[binding.providers]]
provider = "supabase"
account = "personal-sandbox"
credential_ref = "phm:SUPABASE_PERSONAL"
scope = { project_ref = "personalproj1", read_only = false }
```

## Appendix B — MCP control tools (JSON)

```json
{
  "name": "locus_whoami",
  "description": "Return the sealed identity of this session: tenant, binding, providers, scopes. Call before any infrastructure mutation if unsure.",
  "inputSchema": { "type": "object", "properties": {} }
}
```

```json
{
  "name": "locus_request_pin",
  "description": "Request the human to pin this session to a different binding. Does not change identity without human approval.",
  "inputSchema": {
    "type": "object",
    "required": ["alias", "reason"],
    "properties": {
      "alias": { "type": "string" },
      "reason": { "type": "string" }
    }
  }
}
```

## Appendix C — Process diagram (exclusive pin)

```
Human: locus pin acme
        │
        ▼
   daemon: create session seal(binding=acme)
        │
        ▼
   locus-mcp: tools/list
        │
        ├── worker[supabase, ref=abc]  env: token_acme
        ├── worker[vercel, team=acme]  env: oauth_acme
        └── worker[github, org=acme]   GH_CONFIG_DIR=.../acme-only

Agent: tools/call list_tables
        │
        ▼
   policy + seal OK → only worker[supabase, ref=abc]
        │
        ▼
   upstream Supabase project abc  ✓
   personal Supabase              ✗ not in process, not in catalog
```

## Appendix D — One-liner positioning

**Locus is the Phantom-grade identity plane for agents: pin a tenant, spawn isolated workers, make cross-account action a type error at runtime.**

---

*Document status: design draft for OSS product (codename mmcp → Locus).*  
*Owner: Ashlr AI · Adjacent to phm.dev / phantom-secrets.*

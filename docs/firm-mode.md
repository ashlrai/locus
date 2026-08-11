# Firm mode — multi-client agencies

How a contract firm (or founder with N clients) should run Locus so agents cannot act in the wrong tenant.

**Goal:** enter a client in &lt;30s; leave with no residual identity; destructive prod actions need a human gate.

## Model

```
Principal (you / contractor-ro)
    acts_as → Binding (acme | acme-ro | personal | …)
                 for → Tenant (acme-corp)
                 includes → Provider accounts (Supabase project, Vercel team, GH org)
                 uses → CredentialRefs only (phm:…)
                 constrained_by → Policy + Scope
Workspace (.locus.toml) defaults_to → Binding allowlist
Session sealed_to → exactly one Binding
```

One laptop, many clients: **never** one mega-MCP with all accounts. One pin, one catalog.

## Bindings (one per engagement surface)

Store: `~/.locus/bindings/<alias>.toml`. Commit **nothing** secret — only aliases and `phm:` / `env:` refs.

**Pattern: full + read-only per client**

| Alias | Intent |
|-------|--------|
| `acme` | Day-to-day client work; preview deploys; scoped write |
| `acme-ro` | Audit / schema review; `read_only = true`; no prod deploy tools |
| `personal` | Side projects; never in client repo allowlists |

Example (client write surface) — see also `examples/acme.binding.toml`:

```toml
[binding]
id = "bnd_acme"
alias = "acme"
tenant = "acme-corp"
description = "Acme client engagement"

[binding.policy]
default = "allow"
max_ttl = "8h"
parallel_sessions = 4
# Structured rules (first match wins) — see docs/policy.md
[[binding.policy.rules]]
match = "*.delete*"
action = "dual_control"
[[binding.policy.rules]]
match = "*.drop*"
action = "require_approval"
[[binding.policy.rules]]
match = "vercel.deploy.prod"
action = "dual_control"
# Legacy globs still work alongside rules:
# require_approval = ["*.delete*", "*.drop*", "vercel.deploy.prod"]
# dual_control = ["*.delete*", "vercel.deploy.prod"]

[[binding.providers]]
provider = "supabase"
account = "acme-prod"
credential_ref = "phm:SUPABASE_ACME"
scope = { project_ref = "abcdefghij", read_only = false }

[[binding.providers]]
provider = "github"
account = "acme-corp"
credential_ref = "phm:GH_TOKEN_ACME"
scope = { orgs = ["acme-corp"], repos = ["acme-corp/*"] }

[[binding.providers]]
provider = "vercel"
account = "acme-team"
credential_ref = "phm:VERCEL_TOKEN_ACME"
scope = { team_id = "team_xxx", projects = ["acme-web"], env = ["preview"] }
```

Rules:

1. **Separate CredentialRefs per tenant** — never reuse personal PAT on an Acme binding.
2. **Freeze hard scopes** — `project_ref`, `team_id`, org/repo allowlists are the fence; adapters reject smuggled overrides.
3. **Prod-capable tools stay on `require_approval`** (or on a dedicated elevation binding later).
4. **Phantom names** should encode tenant: `phm:SUPABASE_ACME`, not `phm:SUPABASE`.

Create:

```bash
locus init --with-samples          # or copy examples/*.binding.toml
# edit ~/.locus/bindings/acme.toml — refs only
locus binding list
locus binding show acme
```

## Workspaces (repo = client fence)

Commit **`.locus.toml`** at the client repo root (no secrets):

```toml
# examples/workspace.locus.toml
version = 1
default_binding = "acme"
allowed_bindings = ["acme", "acme-readonly"]
require_pin = true
```

| Field | Effect |
|-------|--------|
| `default_binding` | `locus pin` with no args uses this alias |
| `allowed_bindings` | Pin outside the list fails (unless audited `--force`) |
| `require_pin` | Refuse ambient work without an active pin |

```bash
cd ~/clients/acme
locus workspace --default acme --allow acme,acme-ro --require-pin
locus pin                 # → acme
locus whoami
```

**Do not** put `personal` on a client repo allowlist. Personal repos get their own `.locus.toml` with `default_binding = "personal"`.

Walk behavior: nearest `.locus.toml` walking toward filesystem root wins (child overrides parent). If that file is unreadable or malformed, Locus does not fall back to a parent policy or no policy: pin/autopin fail and doctor reports `UNSAFE`.

## Daily loop

```bash
# Enter client (<30s)
cd ~/clients/acme
locus enter               # pin + friendly status (or: locus enter acme)
#   → ok entered acme (acme-corp)
#   → prompt   [locus:acme:acme-corp]
eval "$(locus hook zsh)"  # prompt: [locus:enter] | [locus:acme:acme-corp]
locus whoami

# Agent / IDE — after locus setup --client claude|cursor
# tools are only Acme; agent cannot re-pin

# Multi-client install probe (optional dogfood): detect Claude Code / Cursor /
# Continue config paths and dry-run setup for each found client. Soft-skips
# missing installs; never prints secrets.
#   scripts/dogfood-clients.sh
#   DOGFOOD_CLIENTS=1 scripts/dogfood.sh
#   LOCUS_DOGFOOD_REQUIRE_CLIENTS=1  # hard-fail if none found / setup fails
#
# Multi-account pin walk (personal + client) without both IDEs open:
#   docs/dogfood-multi-account.md
#   LOCUS_PERSONAL_ALIAS=personal LOCUS_CLIENT_ALIAS=client-a \
#     scripts/dogfood-multi-account.sh
#   LOCUS_DOGFOOD_REQUIRE_MULTI=1  # hard-fail if aliases missing / walk fails
# Live dual-IDE UI confirmation remains operator-manual.

# Manual identity diagnostics under the pin (no provider credentials)
locus exec --no-resolve -- env | grep LOCUS_
# Provider actions use typed locus-mcp tools behind scope and policy checks.

# Leave (clears identity; suggests re-pin)
locus leave               # or: locus enter personal for side work
```

### Shell auto-enter (`LOCUS_AUTO_ENTER=1`)

```bash
eval "$(locus hook zsh)"
export LOCUS_AUTO_ENTER=1
# On cd into a repo with .locus.toml (or matching git remote autopin),
# the hook runs `locus enter` best-effort. Never forces allowlist.
```

### Git remote autopin (opt-in)

`$LOCUS_HOME/config.toml`:

```toml
[autopin]
enabled = true

[[autopin.remotes]]
match = "github.com/acme-corp"
binding = "acme"
```

`locus pin` / `locus enter` with no alias: workspace `default_binding` first, then remote match. Never auto-pins through a blocked allowlist; never uses `--force` for remote matches.

### Fast engagement onboarding / offboard

```bash
# New client unit (binding + optional .locus.toml + .locus/README.md)
locus engagement init acme --tenant acme-corp --workspace
# edit ~/.locus/bindings/acme.toml scopes; create phm: refs in Phantom
locus enter acme

# Engagement ends
locus engagement close acme --archive
# → checklist: rotate Phantom secrets, revoke GH/Vercel/Supabase access
# Does NOT delete vault secrets (Phantom owns those)
```

Parallel agents: separate MCP processes each with their own pin. Do not share a global `gh auth switch`. Locus uses private `GH_CONFIG_DIR` / scrubbed env per session.

## Dual-control (destructive actions)

**Shipped today:** fail-closed `require_approval` policy plus local advisory review records under `$LOCUS_HOME/approvals/`. Production-grade human identity and dual-control authority are not shipped.

```toml
[binding.policy]
require_approval = [
  "*.delete*",
  "*.drop*",
  "vercel.deploy.prod",
]
# Firm mode declaration: these tools require two externally authenticated identities
dual_control = ["*.delete*", "vercel.deploy.prod"]
# Or: dual_control_all_approvals = true  # every require_approval tool needs 2
```

### Walkthrough (two humans, one blast-radius tool)

```
1. Agent tools/call vercel.deploy.prod
        │
        ▼
2. Policy → RequireApproval (+ dual_control)
        │ write approvals/appr_….json (args_digest only — no secrets)
        │ MCP error includes required_authoritative_grants=2 + hint:
        │   "locus approve grant appr_… records local advisory evidence only"
        │ Optional: locus notify on → banner names the non-authoritative review
        ▼
 Agent sees blocked + approval_id + authority 0/required
        │
 Local A: locus approve grant <id> --as alice
        │  → records untrusted advisory label; status remains pending
 Local B: locus approve grant <id> --as bob
        │  → second advisory label; external authority remains 0/2
        ▼
 Provider execution remains blocked
        │
        └─ Only a peer-authenticated OS broker may verify a scoped,
           non-agent-accessible issue capability. No such verifier ships yet.
```

CLI:

```bash
locus approve list              # or: locus approve pending
locus approve status <id>       # grants n/required + dual-control progress
locus approve grant <id> --as alice
locus approve grant <id> --as alice --touchid   # macOS confirm dialog; cancel aborts
locus approve grant <id> --as bob
locus approve wait <id> --timeout 120
locus approve deny <id>
```

After each local label, the CLI prints advisory progress separately from authoritative progress. The record stays `status=pending` even after two distinct local labels; only a future verified external envelope can change execution authority.

**Mechanisms:**

- Approval records store **tool + binding + args_digest + advisory labels**, never raw secrets.
- Local labels are caller-controlled and cannot establish identity, including self-approval or two different strings from one user.
- `--touchid` and `LOCUS_TOUCHID_MOCK` only confirm recording local advisory evidence; neither authenticates a principal.
- Authoritative envelopes require a peer-authenticated OS broker, a non-agent-accessible issue capability, nonce/replay and idempotency state, exact scope/proposal binding, expiry, and requester/approver separation.
- That adapter is absent, so authoritative approval and dual control remain disabled.

**Roadmap (Phase 3):** shared binding graph, policy packs, engagement offboard as a unit, and authenticated remote dual-control with an independent trust root.

Recommended firm defaults:

| Action class | Binding | Policy |
|--------------|---------|--------|
| Read schema / list tables | `acme-ro` | allow, `read_only` scopes |
| App code + preview deploy | `acme` | allow; prod patterns in `require_approval` |
| Prod deploy / delete / drop | `acme` | `require_approval` + `dual_control` |
| Personal side project | `personal` | never allowed from client workspace |

## Pin discipline (load-bearing)

| Actor | Can pin? |
|-------|----------|
| Human CLI | Yes (`locus pin` / `leave`) |
| Agent MCP | **No** — `locus_request_pin` only |
| Workspace | Constrains which aliases may be pinned |
| `--force` | Escape hatch; treat as audited exception |

If an agent “needs” another tenant: human re-pins. Prompt injection cannot jump catalogs if pin stays human-gated.

## Offboard a client (checklist)

When engagement ends, remove the **unit** of access — not one token at a time in random places:

```bash
locus engagement close acme --archive
# archives audit slice → ~/.locus/archives/acme-YYYYMMDD.jsonl
# marks ~/.locus/engagements/acme.json closed_at
```

Then finish the human checklist printed by the command:

1. `locus leave` if still pinned (close does this when the active pin matches).
2. Binding file is **kept** by default — `locus binding rm acme --yes` only when you intend to drop it.
3. Rotate/revoke provider tokens that backed `phm:…_ACME` (Phantom + provider consoles). Locus never deletes vault secrets.
4. Remove client repo access (GH org, Vercel team, Supabase project invites).
5. Strip or rewrite `.locus.toml` if the repo leaves your custody.
6. Retain the archive under `~/.locus/archives/` for the engagement window if required.

## Compose with Phantom

| Concern | Tool |
|---------|------|
| Keys in `.env` / model context | Phantom |
| Which Supabase / Vercel / GH account acts | Locus |

```toml
credential_ref = "phm:SUPABASE_ACME"
```

Never put service role keys in binding files. Never commit `.locus` seal keys or approval stores.

## Operator checklist

- [ ] One binding (or ro pair) per client tenant  
- [ ] CredentialRefs only; Phantom namespaced by tenant  
- [ ] Client repos have `.locus.toml` with tight `allowed_bindings` + `require_pin`  
- [ ] Prod/destructive globs on `require_approval` (+ `dual_control` for firm mode)  
- [ ] Shell hook so pin is visible  
- [ ] `locus whoami` before risky agent sessions  
- [ ] `LOCUS_HOME` isolation for CI / test machines  
- [ ] Offboard = binding + secrets + provider access  

## Related

- [agency-starter.md](./agency-starter.md) — copy-paste kit (personal ↔ A ↔ B + offboard)  
- [../examples/agency-starter/](../examples/agency-starter/) — bindings + workspace templates  
- [architecture.md](./architecture.md) — system diagram  
- [mcp.md](./mcp.md) — wire agents  
- [../examples/](../examples/) — sample binding + workspace  
- [../SECURITY.md](../SECURITY.md) — threat summary  
- [../DESIGN.md](../DESIGN.md) — full model  

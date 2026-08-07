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
require_approval = ["*.delete*", "*.drop*", "vercel.deploy.prod"]
dual_control = ["*.delete*", "vercel.deploy.prod"]
max_ttl = "8h"
parallel_sessions = 4

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

Walk behavior: nearest `.locus.toml` walking toward filesystem root wins (child overrides parent).

## Daily loop

```bash
# Enter client
cd ~/clients/acme
locus pin                 # or: locus pin acme
eval "$(locus hook zsh)"  # prompt: [locus:acme:acme-corp]
locus whoami

# Agent / IDE — after locus setup --client claude|cursor
# tools are only Acme; agent cannot re-pin

# CLI under the pin
locus exec -- gh pr list
locus exec -- env | grep LOCUS_

# Leave
locus leave               # or: locus pin personal for side work
```

Parallel agents: separate MCP processes each with their own pin. Do not share a global `gh auth switch`. Locus uses private `GH_CONFIG_DIR` / scrubbed env per session.

## Dual-control (destructive actions)

**Shipped today:** `require_approval` + optional **two-principal dual-control** under `$LOCUS_HOME/approvals/`.

```toml
[binding.policy]
require_approval = [
  "*.delete*",
  "*.drop*",
  "vercel.deploy.prod",
]
# Firm mode: these tools need two distinct principals
dual_control = ["*.delete*", "vercel.deploy.prod"]
# Or: dual_control_all_approvals = true  # every require_approval tool needs 2
```

Flow:

```
Agent tools/call vercel.deploy.prod
        │
        ▼
 Policy → RequireApproval (+ dual_control if matched)
        │ write approvals/appr_….json (args_digest only — no secrets)
        │ requester label from session principal
        ▼
 Agent sees blocked + approval_id + grants/required_grants
        │
 Human A: locus approve grant <id> --as alice
        │  → status still pending (grants=1) when dual_control
 Human B: locus approve grant <id> --as bob
        │  → status=approved, TTL starts (default ~15m)
        ▼
 Matching tools/call allowed until grant expires
```

CLI:

```bash
locus approve list
locus approve status <id>
locus approve grant <id> --as alice     # or LOCUS_PRINCIPAL / $USER
locus approve grant <id> --as bob
locus approve deny <id>
```

**Mechanisms:**

- Approval records store **tool + binding + args_digest + grants[]**, never raw secrets.
- Dual-control needs **two distinct principals**; the same principal cannot grant twice.
- Single-control tools still approve on the first grant.
- Grants are **time-bounded** — forgotten elevation expires.
- Agents do not approve themselves; MCP cannot mint a valid grant without the control plane.

**Roadmap (Phase 3):** shared binding graph, policy packs, engagement offboard as a unit, remote dual-control (team sync).

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

1. `locus leave` / ensure no live pin to that alias.
2. Delete or archive `~/.locus/bindings/acme.toml` (`locus binding rm` when available).
3. Rotate/revoke provider tokens that backed `phm:…_ACME` (Phantom + provider consoles).
4. Remove client repo access (GH org, Vercel team, Supabase project invites).
5. Strip or rewrite `.locus.toml` if the repo leaves your custody.
6. Export/retain audit for the engagement window if required (audit export is roadmap-hardening; keep CLI history / your process notes until then).

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

- [architecture.md](./architecture.md) — system diagram  
- [mcp.md](./mcp.md) — wire agents  
- [../examples/](../examples/) — sample binding + workspace  
- [../SECURITY.md](../SECURITY.md) — threat summary  
- [../DESIGN.md](../DESIGN.md) — full model  

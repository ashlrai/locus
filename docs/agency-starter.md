# Agency Starter

Operational guide for a contract firm (or founder with N clients) running **Locus** so agents cannot act in the wrong tenant.

**Kit on disk:** [`examples/agency-starter/`](../examples/agency-starter/) — bindings, workspace templates, dual-control policy snippet, and a full personal ↔ client A ↔ client B workflow with offboarding.

Related: [firm-mode.md](./firm-mode.md) · [architecture.md](./architecture.md) · [mcp.md](./mcp.md)

## Model

```
Principal (you / contractor)
    acts_as → Binding (personal | client-a | client-a-ro | client-b)
                 for → Tenant
                 includes → Provider accounts (Supabase / GH / Vercel)
                 uses → CredentialRefs only (phm:…)
                 constrained_by → Policy + Scope freeze
Workspace (.locus.toml) defaults_to → Binding allowlist
Session sealed_to → exactly one Binding
Doctor answers → SAFE | WARN | UNSAFE before you let an agent act
```

One laptop, many clients: **never** one mega-MCP with all accounts. One pin, one catalog.

## Install the kit

Prerequisite — operator control capability: control commands (`init`, `enter`,
`pin`, `leave`) require `LOCUS_CONTROL_CAPABILITY` in the shell. `locus init` /
`locus quickstart` mint and persist one (0600 at `~/.locus/control_capability`,
respects `LOCUS_HOME`) when nothing exists; export it in new shells with
`eval "$(locus hook zsh)"`, or manage it yourself:
`export LOCUS_CONTROL_CAPABILITY="$(openssl rand -hex 32)"`. `locus doctor`
flags missing/invalid/mismatched capabilities with the exact fix.

```bash
locus init
cp examples/agency-starter/bindings/*.toml ~/.locus/bindings/
# edit project_ref / team_id / orgs; keep phm: refs
cp examples/agency-starter/config.example.toml ~/.locus/config.toml   # optional

# In each client monorepo:
cp examples/agency-starter/workspaces/client-a.locus.toml /path/to/client-a/.locus.toml
cp examples/agency-starter/workspaces/client-b.locus.toml /path/to/client-b/.locus.toml
```

Create Phantom secrets named like the binding refs (`phm:SUPABASE_CLIENT_A`, …). Never put values in TOML or git.

Onboarding a new client without hand-editing TOML: `locus client add <alias>`
walks alias → tenant → provider → scope → credential *pointer* (`phm:NAME` /
`env:VAR` — raw secrets are rejected) and writes the binding via the same
validated path as `locus binding add`. Every prompt has a flag for scripting
(`--non-interactive`, `--dry-run`). Then `locus enter <alias> --ttl 2h` pins
with an auto-leave TTL (see [policy.md](./policy.md#session-ttl-auto-leave)).

## Binding graph

| Alias | Intent |
|-------|--------|
| `personal` | Side projects; never on client repo allowlists |
| `client-a` | Day-to-day Client A work; preview deploys; dual-control on prod blast |
| `client-a-ro` | Audit / schema review; `read_only = true` |
| `client-b` | Separate tenant; separate CredentialRefs |

Rules:

1. **Separate CredentialRefs per tenant** — never reuse personal PAT on a client binding.
2. **Freeze hard scopes** — `project_ref`, `team_id`, org/repo allowlists are the fence.
3. **Prod-capable tools** stay on `require_approval` (+ `dual_control` for firm mode).
4. **Phantom names** encode tenant: `phm:SUPABASE_CLIENT_A`, not `phm:SUPABASE`.

## Workspace fence

```toml
# client-a repo root .locus.toml
version = 1
default_binding = "client-a"
allowed_bindings = ["client-a", "client-a-ro"]
require_pin = true
```

| Field | Effect |
|-------|--------|
| `default_binding` | Bare `locus pin` |
| `allowed_bindings` | Outside list fails unless audited `--force` |
| `require_pin` | Doctor WARNs if unpinned in this tree |

## Daily workflow

```bash
cd ~/clients/client-a
locus leave
locus pin                 # → client-a via .locus.toml
locus whoami
locus doctor              # prefer SAFE before heavy agent work

# … agents use policy-gated locus-mcp tools …
# optional human diagnostic: locus exec --no-resolve -- <command>

cd ~/clients/client-b
locus leave
locus pin client-b
locus whoami

locus leave               # end of day
```

Agents **cannot** re-pin. If the catalog is wrong: human runs `locus pin <alias>`.

## Dual-control

Sample policy: [`examples/agency-starter/policies/dual-control.toml`](../examples/agency-starter/policies/dual-control.toml).

```bash
locus approve list
locus approve grant <id> --as engineer  # local advisory only
locus approve grant <id> --as partner   # still external authority 0/2
```

These labels are not authenticated identities. Provider execution remains blocked until an external cryptographic approval adapter exists.

`locus doctor` reports `pending_approvals` and `dual_control_waiting`.

## Doctor — “am I safe to act?”

```bash
locus doctor
locus doctor --json
```

Reports (mission-control JSON):

1. `home`, `seal_ok`, `bindings`
2. Active `pin` (alias, tenant, expires, seal) + `pin_seal_ok`
3. `runtime` (`verify_runtime` drift)
4. `pending_approvals` + `dual_control_waiting`
5. `phantom_on_path` + `unresolved_phm`
6. `autopin` (`config.toml` / remote rules / `clients.auto_pin`)
7. `workspace` (found path, allowlist, `require_pin`)
8. `audit` (last 5 ops, recent `scope_freeze` / deny counts)
9. `verdict`: **SAFE** (exit 0) · **WARN** (1) · **UNSAFE** (2)

| Verdict | When |
|---------|------|
| SAFE | No findings |
| WARN | Unresolved phm, missing phantom, pending approvals, require_pin unmet, recent freeze/deny, bad autopin |
| UNSAFE | Invalid seal, binding/tenant drift, corrupt/unwritable approvals, missing seal key |

## Events

```bash
locus events --last 20
locus events --op scope_freeze
locus events --binding client-a --json
```

Append-only JSONL under `$LOCUS_HOME/audit/events.jsonl`. Never contains resolved secrets.

## Offboarding

1. `locus leave` — no live pin to that alias  
2. `locus binding rm client-a` (+ ro pair)  
3. Rotate/revoke `phm:…_CLIENT_A*` (Phantom + providers)  
4. Remove org/team/project invites  
5. Strip client repo `.locus.toml` if custody ends  
6. Export audit: `locus events --binding client-a --last 10000 --json`  
7. Confirm with `locus doctor` / `locus binding list`

## Multi-client dogfood probe

After wiring Claude Code / Cursor (and optionally Continue), run
`scripts/dogfood-clients.sh` to detect common macOS/Linux config paths and
dry-run `locus agent setup --client <x>` for each found supported client.
Missing installs soft-skip (exit 0 + summary); set
`LOCUS_DOGFOOD_REQUIRE_CLIENTS=1` only when you expect clients on the host.
Optional soft step from the main dogfood path: `DOGFOOD_CLIENTS=1 scripts/dogfood.sh`.
The probe never mutates MCP configs and never prints secrets.

## Multi-account pin walk (personal + client)

Walk both pins at the CLI without keeping both IDEs open — step-by-step
playbook: [dogfood-multi-account.md](./dogfood-multi-account.md).

```bash
export LOCUS_PERSONAL_ALIAS=personal
export LOCUS_CLIENT_ALIAS=client-a
scripts/dogfood-multi-account.sh
# Soft-skips when aliases missing; hard-fail:
#   LOCUS_DOGFOOD_REQUIRE_MULTI=1 scripts/dogfood-multi-account.sh personal client-a
```

For each alias: enter → doctor → `verify session` → `agent report` ready gate →
leave. Wire **one** of Claude/Cursor once so report can reach `ready`. Live
dual-IDE UI confirmation remains operator-manual.

## Operator checklist

- [ ] One binding (or full+ro pair) per client tenant  
- [ ] CredentialRefs only; Phantom namespaced by tenant  
- [ ] Client repos: `.locus.toml` allowlist + `require_pin`  
- [ ] Destructive globs on `require_approval` (+ dual-control)  
- [ ] Shell hook; `whoami` / `doctor` before risky sessions  
- [ ] Offboard = binding + secrets + access + audit  
- [ ] Optional: `scripts/dogfood-clients.sh` after IDE MCP setup  
- [ ] Optional: `scripts/dogfood-multi-account.sh` personal + client ready walk  


## Related

- Kit: [examples/agency-starter/](../examples/agency-starter/)  
- [firm-mode.md](./firm-mode.md)  
- [dogfood-multi-account.md](./dogfood-multi-account.md)  
- [agency-certainty.md](./agency-certainty.md)  
- [SECURITY.md](../SECURITY.md)  
- [DESIGN.md](../DESIGN.md)  

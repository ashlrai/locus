# Multi-account dogfood (personal + client)

Operator playbook for walking **two pins** on one laptop without keeping both
IDEs open. Proves the identity plane: enter personal → ready under that pin →
leave → enter client → ready under that pin → leave. No residual identity,
no cross-binding credentials.

**Related**

- Automated walk: [`scripts/dogfood-multi-account.sh`](../scripts/dogfood-multi-account.sh)
- IDE install probe (Claude/Cursor paths only): [`scripts/dogfood-clients.sh`](../scripts/dogfood-clients.sh)
- Dual-IDE matrix (no secrets): [`scripts/dogfood-dual-ide.sh`](../scripts/dogfood-dual-ide.sh)
- Firm ops: [firm-mode.md](./firm-mode.md) · [agency-starter.md](./agency-starter.md)
- MCP wire-up: [mcp.md](./mcp.md)

This path **does not** require Claude Code *and* Cursor at once. Wire **at
least one** supported client so `locus agent report` can reach `ready`; the
multi-account script itself is CLI-only. For a combined dual-IDE **matrix**
(client found? locus registered? multi-account walked?) without printing
secrets, use [`scripts/dogfood-dual-ide.sh`](../scripts/dogfood-dual-ide.sh).

---

## Prerequisites

| Need | Notes |
|------|--------|
| `locus` + `locus-mcp` on `PATH` | `cargo install --path crates/locus-cli` / `locus-mcp`, or release bins |
| `jq` | Report / session JSON gates |
| Two bindings | Distinct tenants + CredentialRefs (never reuse personal PAT on client) |
| Resolved credentials | Phantom (`phm:…`) preferred; doctor should not list unresolved PHM |
| MCP registered once | `locus agent setup --apply --client claude` **or** `cursor` (one is enough) |
| Control capability | Scripts set `LOCUS_CONTROL_CAPABILITY` if unset (enter/pin/leave need it) |

Safe practice home (optional):

```bash
export LOCUS_HOME=/tmp/locus-multi-dogfood
locus init
# copy or create personal + client bindings under $LOCUS_HOME/bindings/
```

Real dogfood uses your normal `~/.locus` (omit `LOCUS_HOME`).

---

## 1. Personal binding

Create or copy a personal surface (examples under `examples/personal.binding.toml`
or `examples/agency-starter/bindings/personal.toml`):

```bash
locus init   # once
# Edit scopes; CredentialRefs only — never raw tokens in TOML.
cp examples/agency-starter/bindings/personal.toml ~/.locus/bindings/personal.toml
# → set orgs / team_id / project_ref; keep phm:SUPABASE_PERSONAL, phm:GH_TOKEN_PERSONAL, …

# Phantom: create secrets named like the refs (values never in git/chat).
locus binding list
```

Rules:

1. Alias stable and short (`personal`).
2. Separate CredentialRefs from every client binding.
3. Do **not** put `personal` on a client repo `.locus.toml` allowlist.

---

## 2. Client binding

One binding (or full + `-ro` pair) per engagement:

```bash
cp examples/agency-starter/bindings/client-a.toml ~/.locus/bindings/client-a.toml
# edit tenant, project_ref, team_id, orgs; phm:…_CLIENT_A only

# Optional workspace fence in the client monorepo:
cp examples/agency-starter/workspaces/client-a.locus.toml ~/clients/client-a/.locus.toml
```

Confirm both aliases exist:

```bash
locus binding list --json | jq -r '.[].alias'
# expect: personal, client-a, …
```

---

## 3. Wire Claude **or** Cursor (not both required)

Agent report `ready` needs locus-mcp registered for at least one supported
client. You do **not** need dual-IDE sessions running.

```bash
# Claude Code (project or home .mcp.json)
locus agent setup --apply --client claude

# — or — Cursor
locus agent setup --apply --client cursor
```

Pointers: [mcp.md](./mcp.md) · `locus agent setup --help`.

Optional install probe (detects configs, dry-run only):

```bash
scripts/dogfood-clients.sh
```

Dual-IDE matrix (dry-run setup + MCP path has `"locus"` + optional multi walk):

```bash
# No aliases: probe clients only; multi column = skipped
scripts/dogfood-dual-ide.sh

# With personal + client: multi-account walk + client probe + matrix
LOCUS_PERSONAL_ALIAS=personal LOCUS_CLIENT_ALIAS=client-a \
  scripts/dogfood-dual-ide.sh
```

Restart the IDE once after first setup so the catalog reloads. Further pin
walks are pure CLI.

---

## 4. Manual walk (personal → client)

```bash
# ── Personal ──────────────────────────────────────────────
locus leave                         # clear any residual pin
locus enter personal                # or: locus pin personal
locus whoami
locus doctor                        # prefer SAFE before agent work
locus verify session --json | jq '{kind,session_ok,safe_next}'
locus agent report --json | jq '{ready,status,status_oneline,pin}'
# expect: ready=true, status=ready, pin.alias=personal, session_ok=true

locus leave

# ── Client ────────────────────────────────────────────────
locus enter client-a                # your client alias
locus whoami
locus doctor
locus verify session --json | jq '{kind,session_ok,safe_next}'
locus agent report --json | jq '{ready,status,status_oneline,pin}'
# expect: ready=true, pin.alias=client-a (not personal), session_ok=true

locus leave                         # end of day / switch complete
```

**Pass criteria per pin**

| Check | Pass |
|-------|------|
| `locus enter <alias>` | Sealed session; prompt `alias:tenant` |
| `doctor` | `SAFE` (no unresolved PHM; seal ok) |
| `verify session --json` | `kind=session`, `session_ok=true` |
| `agent report --json` | `ready=true`, `status=ready`, `pin.alias` matches, `seal_ok=true` |
| After `leave` | `locus status --oneline` → `unpinned` |

Never paste report/doctor JSON into tickets if you suspect a leak; locus does
not emit secret values, but treat stdout as ops-only.

---

## 5. Automated walk

```bash
# Env form
export LOCUS_PERSONAL_ALIAS=personal
export LOCUS_CLIENT_ALIAS=client-a
scripts/dogfood-multi-account.sh

# Args form (override env)
scripts/dogfood-multi-account.sh personal client-a

# Soft-skip when aliases unset/missing (exit 0 + summary)
unset LOCUS_PERSONAL_ALIAS LOCUS_CLIENT_ALIAS
scripts/dogfood-multi-account.sh

# Hard-fail if either alias is missing or a walk fails
LOCUS_DOGFOOD_REQUIRE_MULTI=1 \
  LOCUS_PERSONAL_ALIAS=personal LOCUS_CLIENT_ALIAS=client-a \
  scripts/dogfood-multi-account.sh
```

For each alias the script: **enter → doctor → verify session → agent report
ready gate → leave**. It never prints secret values or CredentialRef locators,
never runs `--apply`, and never touches IDE config.

Success line:

```text
MULTI-ACCOUNT DOGFOOD: ok
```

---

## 6. Dual-IDE matrix (no secrets)

Combines client install detection, setup dry-run, MCP `"locus"` registration
checks, and an optional multi-account walk into one report. **Never** prints
MCP env maps, CredentialRefs, or secret values — only paths and yes/no flags.

```bash
# Env form
export LOCUS_PERSONAL_ALIAS=personal
export LOCUS_CLIENT_ALIAS=client-a
scripts/dogfood-dual-ide.sh

# Args form
scripts/dogfood-dual-ide.sh personal client-a

# Soft-skip missing clients/aliases (exit 0 + matrix)
unset LOCUS_PERSONAL_ALIAS LOCUS_CLIENT_ALIAS
scripts/dogfood-dual-ide.sh

# Hard-fail: no supported client, setup dry-run fail, multi walk fail,
# or no client has locus registered in MCP JSON
LOCUS_DOGFOOD_REQUIRE_DUAL=1 \
  LOCUS_PERSONAL_ALIAS=personal LOCUS_CLIENT_ALIAS=client-a \
  scripts/dogfood-dual-ide.sh
```

**Per found client (claude, cursor)**

1. Detect install/config markers (same family of paths as `dogfood-clients.sh`).
2. `locus agent setup --dry-run --client <name>` (never `--apply`).
3. Resolve MCP config JSON path(s) and check for a `mcpServers.locus` key
   (via `jq` when available, else a `"locus":` string match). Bodies are not printed.

**When aliases are set**

- Runs `scripts/dogfood-multi-account.sh` (enter → doctor → verify → agent report → leave).
- Also runs `scripts/dogfood-clients.sh` (doctor off) for the install probe.

**Matrix columns**

| Column | Meaning |
|--------|---------|
| `client` | `claude` or `cursor` |
| `found` | Install/config marker present |
| `setup_dry` | `locus agent setup --dry-run` result |
| `locus_reg` | MCP JSON has `locus` server entry (`yes`/`no`/`n/a`) |
| `multi_account` | `walked` / `skipped` / `FAIL` (same status on every row) |

Success line:

```text
DUAL-IDE DOGFOOD: ok (matrix above; secrets never printed)
```

---

## 6b. Multi-tenant grant probe (optional)

`scripts/dogfood.sh` can additionally prove the hub-mode grant lifecycle —
`locus mcp mint` → HTTP verify against `locus-mcp --http --multi-tenant`
(tenantless request is a uniform `401 invalid_grant`, the granted tenant
initializes with `200`) → `locus mcp revoke` (→ `401`, and the roster no
longer lists the grant as live) — against its **own throwaway `LOCUS_HOME`**:

```bash
DOGFOOD_MT=1 scripts/dogfood.sh
```

Default **off** — the standard `DOGFOOD READY` contract is unchanged. When
enabled the probe is hard: any red step blocks readiness. No secrets or
credential locators ever appear in the output (the bearer token is consumed
in-process and never printed). Full multi-tenant isolation coverage
(per-tenant whoami, cross-tenant `403 tenant_mismatch`, revoke-while-others-
live) runs in `scripts/e2e.sh` step 31.

---

## 7. What this does **not** cover

| Still manual / separate | Tool |
|-------------------------|------|
| Detect Claude/Cursor install paths only | `scripts/dogfood-clients.sh` |
| Dual-IDE matrix (found + locus reg + multi walk) | `scripts/dogfood-dual-ide.sh` |
| Full isolated `DOGFOOD READY` (sandbox, hub smoke, …) | `scripts/dogfood.sh` |
| Live dual-IDE UI confirmation (both apps open, catalogs reload) | Operator eyes |
| Cross-binding credential isolation e2e | `cargo test -p locus-core --test isolation` |

M3: multi-account playbook + dual-IDE **matrix script** (no secrets) landed;
**live dual-IDE UI** dogfood remains an operator-manual confirmation.

---

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| Soft-skip / missing aliases | Set `LOCUS_PERSONAL_ALIAS` + `LOCUS_CLIENT_ALIAS` or pass args; create bindings |
| Dual matrix `locus_reg=no` | `locus agent setup --apply --client claude` (or `cursor`); re-run dual-ide script |
| Dual matrix soft-skip / no clients | Install Claude Code or Cursor, or unset `LOCUS_DOGFOOD_REQUIRE_DUAL` |
| `agent report` `protected` | Pin first; run `locus agent setup --apply --client claude` (or cursor) once |
| `session_ok=false` / doctor WARN | Resolve PHM secrets; `locus doctor --json \| jq .unresolved_phm` (no values) |
| `unsafe` / seal invalid | `locus leave` then `locus enter <alias>`; re-run `locus init` if home corrupt |
| Wrong tenant in report | You entered the wrong alias — leave and enter the intended one |
| Control capability errors | Export a non-empty `LOCUS_CONTROL_CAPABILITY` (script generates one if unset) |
| Workspace allowlist blocks pin | Enter from a neutral cwd, or use audited `locus enter <alias> --force` |

---

## Operator checklist

- [ ] `personal` + client bindings on disk (CredentialRefs only)  
- [ ] Phantom (or env) secrets resolve — doctor SAFE under each pin  
- [ ] One IDE wired (`locus agent setup --apply --client claude|cursor`)  
- [ ] Manual or `scripts/dogfood-multi-account.sh` walk: personal ready → leave → client ready → leave  
- [ ] Optional matrix: `scripts/dogfood-dual-ide.sh` (found / locus_reg / multi_account)  
- [ ] Optional hard CI: `LOCUS_DOGFOOD_REQUIRE_MULTI=1` / `LOCUS_DOGFOOD_REQUIRE_DUAL=1`  
- [ ] Dual-IDE UI still eyeballed when claiming full M3 multi-client dogfood  

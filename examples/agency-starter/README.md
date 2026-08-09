# Agency Starter

One-laptop, multi-client kit for **Locus**: personal ↔ client A ↔ client B, with read-only elevation, dual-control, workspace fences, and offboarding.

Sibling deep-dive: [docs/agency-starter.md](../../docs/agency-starter.md) · [docs/firm-mode.md](../../docs/firm-mode.md)

## Layout

```
examples/agency-starter/
  bindings/
    personal.toml       # side projects
    client-a.toml       # Client A write / preview
    client-a-ro.toml    # Client A read-only
    client-b.toml       # Client B write
  workspaces/
    client-a.locus.toml # → commit as client-a-repo/.locus.toml
    client-b.locus.toml
    personal.locus.toml
  policies/
    dual-control.toml   # policy snippet for firm mode
  config.example.toml   # optional $LOCUS_HOME/config.toml
  README.md             # this file
```

## Install bindings

```bash
export LOCUS_HOME="${LOCUS_HOME:-$HOME/.locus}"   # or a scratch home for practice
locus init

cp examples/agency-starter/bindings/*.toml "$LOCUS_HOME/bindings/"
# edit each file: project_ref, team_id, orgs — CredentialRefs only (phm:…)

# Optional home config
cp examples/agency-starter/config.example.toml "$LOCUS_HOME/config.toml"

locus binding list
locus doctor --json | jq '{verdict,pin,workspace,pending_approvals,dual_control_waiting}'
```

Create matching secrets in [Phantom](https://phm.dev) (names only in bindings):

| Binding | Phantom names (examples) |
|---------|--------------------------|
| `personal` | `SUPABASE_PERSONAL`, `GH_TOKEN_PERSONAL`, `VERCEL_TOKEN_PERSONAL` |
| `client-a` | `SUPABASE_CLIENT_A`, `GH_TOKEN_CLIENT_A`, `VERCEL_TOKEN_CLIENT_A` |
| `client-a-ro` | `SUPABASE_CLIENT_A_RO`, `GH_TOKEN_CLIENT_A_RO` |
| `client-b` | `SUPABASE_CLIENT_B`, `GH_TOKEN_CLIENT_B`, `VERCEL_TOKEN_CLIENT_B` |

**Never** reuse a personal PAT on a client binding.

## Workspace fences

In each client repo root:

```bash
# Client A monorepo
cp examples/agency-starter/workspaces/client-a.locus.toml /path/to/client-a/.locus.toml

# Client B monorepo
cp examples/agency-starter/workspaces/client-b.locus.toml /path/to/client-b/.locus.toml
```

Effects:

| Field | Meaning |
|-------|---------|
| `default_binding` | Bare `locus pin` uses this alias |
| `allowed_bindings` | Other aliases fail unless `locus pin … --force` (audited) |
| `require_pin` | Doctor WARNs if you work here unpinned |

## Daily loop: personal ↔ A ↔ B

```bash
# Morning — personal
cd ~/src/side-project
locus pin personal
locus whoami
locus doctor          # want verdict SAFE (or WARN only for missing phantom in dev)

# Client A
cd ~/clients/client-a
locus leave
locus pin             # uses .locus.toml default → client-a
locus whoami          # tenant must be client-a-corp
# … agent / CLI work hard-scoped to client-a …

# Audit-only on A (no prod deploy surface)
locus pin client-a-ro
locus whoami

# Switch to Client B (different catalog — model cannot jump)
cd ~/clients/client-b
locus leave
locus pin client-b
locus whoami

# End of day
locus leave
locus status --oneline   # unpinned
```

Shell visibility:

```bash
eval "$(locus hook zsh)"   # prompt fragment [locus:client-a]
```

## Dual-control (firm mode)

Client write bindings include `dual_control` for delete + prod deploy.

When an agent hits a gated tool:

```bash
locus approve list
locus approve grant appr_… --as engineer
# still pending under dual-control:
locus approve grant appr_… --as partner
# re-call tool with same args (or confirm + approval_id)
```

Doctor surfaces:

- `pending_approvals` — all open grants
- `dual_control_waiting` — subset that needs a second principal

```bash
locus doctor --json | jq '{verdict,pending_approvals,dual_control_waiting,findings}'
```

## Doctor single pane (“am I safe to act?”)

```bash
locus doctor
locus doctor --json
```

| Verdict | Exit | Meaning |
|---------|------|---------|
| **SAFE** | 0 | Seal ok, no critical drift, no blocking findings |
| **WARN** | 1 | Operational gaps (unresolved phm, require_pin unmet, pending dual-control, …) |
| **UNSAFE** | 2 | Identity plane broken (invalid seal, binding/tenant drift, corrupt approvals) |

Also reports: home, bindings count, pin (alias/tenant/expires/seal), runtime drift, approvals, phantom + phm refs, autopin config, workspace allowlist, last 5 audit ops.

## Audit events

```bash
locus events --last 20
locus events --last 50 --op scope_freeze
locus events --binding client-a --json
```

## Offboarding Client A

When the engagement ends, remove the **unit** of access:

1. **Leave** any live pin: `locus leave` (and confirm `locus status --oneline` → `unpinned`).
2. **Remove binding file**: `locus binding rm client-a` (and `client-a-ro` if present).
3. **Rotate secrets**: revoke/rotate every `phm:…_CLIENT_A*` in Phantom + provider consoles (Supabase, GH, Vercel).
4. **Revoke human access**: GH org membership, Vercel team, Supabase project invites.
5. **Workspace**: strip or rewrite `.locus.toml` if the repo leaves your custody.
6. **Audit**: retain events if required:
   ```bash
   locus events --binding client-a --last 10000 --json > client-a-audit-export.json
   ```
7. **Doctor**: `locus doctor` should no longer list those bindings; no pin to A remains.

Optional engagement helper (if your build includes it):

```bash
locus engagement close client-a --archive   # when available
```

## Checklist (agency laptop)

- [ ] Bindings: personal + full/ro per client (separate CredentialRefs)
- [ ] Phantom namespaced by tenant (`…_CLIENT_A`, not bare `SUPABASE`)
- [ ] Client repos have `.locus.toml` with tight `allowed_bindings` + `require_pin`
- [ ] Prod/destructive globs on `require_approval` (+ `dual_control` for firm mode)
- [ ] Shell hook so pin is visible
- [ ] `locus whoami` / `locus doctor` before risky agent sessions
- [ ] Offboard = binding + secrets + provider access + audit export

## Related

- [docs/agency-starter.md](../../docs/agency-starter.md)
- [docs/firm-mode.md](../../docs/firm-mode.md)
- [docs/mcp.md](../../docs/mcp.md)
- [SECURITY.md](../../SECURITY.md)

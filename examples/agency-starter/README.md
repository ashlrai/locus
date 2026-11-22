# Agency Starter

One-laptop, multi-client kit for **Locus**: personal ↔ client A ↔ client B, with read-only elevation, dual-control, workspace fences, and offboarding.

Sibling deep-dive: [docs/agency-starter.md](../../docs/agency-starter.md) · [docs/firm-mode.md](../../docs/firm-mode.md)

Credential compatibility: the sample bindings keep `phm:` references as migration metadata, and those cannot resolve today (Locus fails closed on `phm:`). Replace them with `env:VAR` references supplied outside agent context before you execute anything. See [credential compatibility](../../docs/credential-compatibility.md).

## Layout

```
examples/agency-starter/
  bindings/
    personal.toml       # side projects (+ filesystem upstream demo)
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

### Upstream MCP demo (personal)

`bindings/personal.toml` includes a `demo` provider with:

```toml
upstream = { recipe = "filesystem-mcp", args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp/locus-demo"] }
```

```bash
mkdir -p /tmp/locus-demo
locus pin personal
# a supervised launch (`locus mcp --supervised`) spawns the filesystem MCP under
# this pin (tools namespaced demo.*); a client-launched bare locus-mcp serves
# control tools only — see docs/mcp.md
locus upstream list
locus upstream suggest filesystem
```

For GitHub live tools on a pin, prefer `upstream = { recipe = "github-mcp", resolve_secrets = true }` (see `locus upstream suggest github`). Never reuse a personal PAT on a client binding.

## Install bindings

Control commands (`init`, `enter`, `pin`, `switch`, `leave`, `workspace`, …) need `LOCUS_CONTROL_CAPABILITY` in the shell. `locus init` mints and persists one, but only the shell hook exports it, so run the hook line in this shell and in every new one.

```bash
export LOCUS_HOME="${LOCUS_HOME:-$HOME/.locus}"   # or a scratch home for practice
locus init
eval "$(locus hook zsh)"   # or bash / fish: exports the control capability

cp examples/agency-starter/bindings/*.toml "$LOCUS_HOME/bindings/"
# edit each file: project_ref, team_id, orgs; replace phm: refs with env: refs

# Optional home config
cp examples/agency-starter/config.example.toml "$LOCUS_HOME/config.toml"

locus binding list
locus doctor --json | jq '{verdict,pin,workspace,pending_approvals,dual_control_waiting}'
```

Name credential variables by tenant (names only in bindings; values come from outside agent context):

| Binding | Credential names (examples) |
|---------|-----------------------------|
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
# Morning — personal (hook already loaded in this shell)
cd ~/src/side-project
locus pin personal
locus whoami
locus doctor          # want SAFE; setup-only WARN (e.g. phantom missing) still exits 0

# Client A
cd ~/clients/client-a
locus leave
locus pin             # uses .locus.toml default → client-a
locus whoami          # tenant must be client-a-corp
# … agent / CLI work hard-scoped to client-a …

# Audit-only on A (no prod deploy surface)
locus leave
locus pin client-a-ro
locus whoami

# Switch to Client B (different catalog — model cannot jump)
cd ~/clients/client-b
locus leave
locus pin client-b
locus whoami

# End of day
locus leave
locus status --oneline   # unpinned (require_pin inside a require_pin repo)
```

Shell visibility: the same `eval "$(locus hook zsh)"` line adds a prompt fragment such as `[locus:client-a:client-a-corp]` (`[locus:enter]` when unpinned).

## Dual-control (firm mode)

Client write bindings include `dual_control` for delete + prod deploy.

When an agent hits a gated tool:

```bash
locus approve list
locus approve grant appr_… --as engineer  # local advisory only
locus approve grant appr_… --as partner   # still authority 0/2
# provider call remains blocked until an external authenticated authority exists
```

Doctor surfaces:

- `pending_approvals` — all open grants
- `dual_control_waiting` — subset awaiting external authenticated authority

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
| **WARN** | 1 (0 when every finding is setup advice) | Operational gaps (unresolved phm, require_pin unmet, pending dual-control, …) |
| **UNSAFE** | 2 | Identity plane broken (invalid seal, binding/tenant drift, corrupt approvals) |

Setup-only WARN codes: `approvals_missing`, `phantom_missing`, `control_capability_not_exported`, `executor_authority_unavailable`. Full field and verdict tables: [docs/agency-starter.md](../../docs/agency-starter.md#doctor--am-i-safe-to-act).

Also reports: home, bindings count, pin (alias/tenant/expires/seal), runtime drift, approvals, phantom + phm refs, autopin config, workspace allowlist, last 5 audit ops.

## Audit events

```bash
locus events --last 20
locus events --last 50 --op scope_freeze
locus events --binding client-a --json
```

## Offboarding Client A

When the engagement ends, remove the **unit** of access:

1. **Export audit first** if you need to retain it:
   ```bash
   locus events --binding client-a --last 10000 --json > client-a-audit-export.json
   ```
   Or `locus engagement close client-a --archive`, which archives the binding's audit events under `$LOCUS_HOME/archives/`.
2. **Plan**: `locus offboard client-a` prints a read-only `locus.plan.v1` plan of every local record the alias owns.
3. **Apply**: `locus offboard client-a --apply` (and `client-a-ro`) drops the pin if active and removes the binding, approvals, sealed sessions, MCP grants and pin history. Audit events stay. To remove only a binding file: `locus binding rm client-a --yes`.
4. **Rotate secrets**: revoke/rotate every `…_CLIENT_A*` credential in your secret store and provider consoles (Supabase, GH, Vercel).
5. **Revoke human access**: GH org membership, Vercel team, Supabase project invites.
6. **Workspace**: strip or rewrite `.locus.toml` if the repo leaves your custody (offboard never touches it).
7. **Doctor**: `locus doctor` / `locus binding list` should no longer list those bindings; no pin to A remains.

## Checklist (agency laptop)

- [ ] Bindings: personal + full/ro per client (separate CredentialRefs)
- [ ] Credentials namespaced by tenant (`…_CLIENT_A`, not bare `SUPABASE`)
- [ ] Client repos have `.locus.toml` with tight `allowed_bindings` + `require_pin`
- [ ] Prod/destructive globs on `require_approval` (+ `dual_control` for firm mode)
- [ ] Shell hook so the capability is exported and the pin is visible
- [ ] `locus whoami` / `locus doctor` before risky agent sessions
- [ ] Offboard = `locus offboard --apply` + secrets + provider access + audit export

## Related

- [docs/agency-starter.md](../../docs/agency-starter.md)
- [docs/firm-mode.md](../../docs/firm-mode.md)
- [docs/mcp.md](../../docs/mcp.md)
- [docs/credential-compatibility.md](../../docs/credential-compatibility.md)
- [SECURITY.md](../../SECURITY.md)

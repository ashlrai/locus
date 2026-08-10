# Architecture

Distilled from [DESIGN.md](../DESIGN.md). Mechanism map for operators and contributors.

## Problem

Agents inherit **ambient identity**: global `gh auth`, one Supabase MCP token, last Vercel team. Multi-client work makes wrong-account action a process bug, not a prompt bug.

| Product | Question |
|---------|----------|
| **Phantom** | Can this secret enter the model? |
| **Locus** | As whom, against which tenant, right now? |

**Invariant:** identity is resolved at the gate, not in the prompt. The session *is* an account.

## System diagram

```
┌─────────────────────────────────────────────────────────────┐
│  Clients: Claude Code · Cursor · Codex · human CLI · CI     │
└────────────────────────────┬────────────────────────────────┘
                             │ MCP stdio  or  locus exec
┌────────────────────────────▼────────────────────────────────┐
│  DATA PLANE — locus-mcp / isolated exec                      │
│  · resolve sealed session → one Binding                      │
│  · compose tool catalog (pin only + locus_* controls)        │
│  · policy gate + scope freeze                                │
│  · fan-out to workers / child env                            │
└───┬──────────────────┬───────────────────┬──────────────────┘
    │                  │                   │
┌───▼────────┐  ┌──────▼──────┐  ┌─────────▼─────────┐
│ CONTROL    │  │ POLICY      │  │ AUDIT (roadmap)   │
│ bindings/  │  │ allow/deny  │  │ JSONL + HMAC      │
│ sessions/  │  │ require_    │  │ export / SIEM     │
│ seal key   │  │ approval    │  │                   │
└───┬────────┘  └─────────────┘  └───────────────────┘
    │
┌───▼────────────────────────────────────────────────────────┐
│  CREDENTIAL PLANE                                             │
│  CredentialRef → resolve only into worker/child env           │
│  phm:NAME (Phantom) · env:VAR · test: compiled tests only      │
│  Model never sees resolved values                             │
└───┬────────────────────────────────────────────────────────┘
    │ spawn / inject
┌───▼────────────────────────────────────────────────────────┐
│  WORKERS (Binding × Provider)                                 │
│  Synthetic adapters (Phase 1) or MCP stdio children           │
│  Private GH_CONFIG_DIR / AWS_* under ~/.locus/workers/…     │
└────────────────────────────────────────────────────────────┘
```

## Core types

| Term | Meaning |
|------|---------|
| **Binding** | principal × tenant × providers × CredentialRefs × policy |
| **Session** | Live pin sealed (HMAC) to exactly one Binding |
| **Workspace** | `.locus.toml` — default pin + allowlist for a directory tree |
| **CredentialRef** | Opaque pointer (`phm:…` / `env:…`) — never the secret |
| **Worker** | Process or in-process adapter scoped to one Binding × Provider |
| **Policy** | Ordered `rules` + legacy `require_approval` / `dual_control` globs + `default` (see [policy.md](./policy.md)) |

Store root: `~/.locus/` (override with `LOCUS_HOME`).

```
~/.locus/
  config.toml
  seal key (local)
  bindings/*.toml
  sessions/
  approvals/
  workers/<session>/…
  audit/                 # as implemented
```

## Pin path

```
locus pin acme
      │
      ▼
 Session sealed to binding "acme"
      │
      ├─► locus-mcp
      │     tools/list = binding providers + locus_whoami / status / request_pin
      │     agents cannot pin
      │     scope freeze: wrong project_ref / team_id → deny
      │
      └─► locus exec -- <cmd>
            scrub ambient AWS_PROFILE / GH_TOKEN / SUPABASE_* / …
            resolve CredentialRefs into child only
            private CLI config dirs under workers/<session>/
            never inject other bindings' providers
```

## Policy order

```
1. Valid session seal?           else DENY
2. Tool provider ∈ binding?      else DENY
3. Scope allowlist / freeze?     else DENY
4. require_approval match?       else pending human grant
5. policy.default                ALLOW or DENY
6. Audit meta (no secret values)
```

Unbound = empty provider surface (control tools only). No “fall back to personal.”

## Isolation mechanisms

| # | Mechanism | Blocks |
|---|-----------|--------|
| 1 | HMAC session seal | Prompt-edited “I am personal now” |
| 2 | Exclusive tool catalog | Seeing other tenants’ tools |
| 3 | Per-binding worker / env | Cred A used on request for B |
| 4 | Scrub + private config dirs | `gh` / `aws` global races |
| 5 | Adapter scope freeze | Arg-smuggled `project_ref` |
| 6 | Agent cannot pin | Injection-driven re-pin |
| 7 | Workspace allowlist | Repo-local wrong-tenant pin |
| 8 | Credential opacity | Keys in model context (compose Phantom) |

## Crate map

| Crate | Plane |
|-------|--------|
| `locus-core` | Types, seal, store, policy, isolation, adapters, workers |
| `locus-cli` | Human control plane |
| `locus-mcp` | Agent data plane (stdio) |

Adapters are the **only** place provider knowledge lives. Guide: [adapters.md](./adapters.md). Workers: [workers.md](./workers.md).

## What ships vs roadmap

| Phase | Focus |
|-------|--------|
| **0–1 (now)** | Daemon-less store, pin/whoami/exec, `locus-mcp`, synthetic adapters, scope freeze, workspace, CredentialRefs, require_approval |
| **2** | Real upstream MCP workers, more providers, continuous whoami drift, `locus run` |
| **3** | Team binding graph, dual-control packs, offboard, remote audit |
| **4** | Adapter SDK, CI ephemeral pins, sandbox |

Roadmap detail: [PLAN.md](../PLAN.md). Threat model: [DESIGN.md §9](../DESIGN.md), [SECURITY.md](../SECURITY.md).

## Composition

```
.locus.toml + ~/.locus/bindings
        │
        ▼
   locus-mcp / locus exec
        │
        │  CredentialRef phm:NAME
        ▼
   Phantom vault / proxy   (secrets never in model)
        │
        ▼
   Provider APIs
```

Optional later: fleet gateway discovers **only** Locus, not raw personal MCP servers.

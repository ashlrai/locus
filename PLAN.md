# Locus — Identity Plane for Coding Agents

**Repo codename:** `mmcp` · **Recommended product name:** **Locus** · **Workspace:** `/Users/masonwyatt/Desktop/mmcp` (greenfield)  
**Full design already drafted:** [`DESIGN.md`](/Users/masonwyatt/Desktop/mmcp/DESIGN.md) (~1k lines)  
**Sibling:** [Phantom Secrets](https://phm.dev) — secrets *in context*. Locus — *which identity acts*.

---

## Executive summary

You have a real, underserved problem: multi-account MCP/CLI chaos for a contract firm (personal vs client Supabase/Vercel/xAI/Resend/etc.), where agents can and do act in the wrong tenant. Research across Phantom, ashlr-hub, vendor identity models, competitor landscape, and agency ops all converge on one thesis:

> **Phantom solves credential *exposure*. Almost nothing solves authorized-but-*wrong-account* action.**  
> The market has registries, transport proxies, SaaS tool meshes (Composio), enterprise MCP gateways, and secret brokers (Agent Vault, Peta). It does **not** have a local-first, multi-provider **identity plane** that makes wrong-account actions mechanically impossible for coding agents.

**Locus** is that product: pin a **Binding** (principal × tenant × scope × credential_ref × policy), seal a session to it, spawn isolated workers per provider, and show the agent *only* that tenant’s tools. No ambient `gh auth`, no shared Supabase PAT, no “hope the model picks the right MCP.”

---

## Problem (precise)

| Pain | Why today fails |
|------|-----------------|
| Switch MCPs/CLIs between personal & client accounts | Each MCP binds **one credential at process start**; global configs fan-in all accounts into one agent |
| Agent hits client Supabase while founder meant personal | Isolation is prompt-level (“be careful”), not process-level |
| Parallel agents race on `gh auth switch` / `AWS_PROFILE` | Global CLI state is hostile to concurrency |
| Agency/founder context tax | 4–8 clients + personal → hours/week hunting keys + anxiety loops |

**Phantom (phm.dev) already owns:** keys never enter the LLM (phm_ tokens + reverse proxy + vault + MCP approval).  
**Phantom explicitly does *not* own:** multi-tenant identity, account switch, wrong-project MCP, CLI context (its `env_scope` is dev/staging/prod, not clients).

**ashlr-hub already owns:** machine-wide MCP aggregation (`mcp-gateway`, `<server>__<tool>`).  
**ashlr-hub does *not* own:** profiles/workspaces/account policy — it fans everything in (wrong-account risk *increases* if both personal + client servers are discovered).

**Portfolio fit:** new standalone product (Phantom-class), composes with Phantom + thin hooks into hub. Do **not** bury this inside hub fleet OS or Phantom vault core. Avoid colliding with reserved **ashlr-mux**.

---

## Research synthesis (what agents found)

### 1. Phantom patterns to reuse (not reimplement)

- Placeholder-in-context / inject-at-edge (identity analog: alias in model, real creds only in workers)
- Reverse proxy not MITM; F9 field-scoped substitution
- MCP never returns secret values; mutators need `confirm` + OOB approval
- Vault namespacing (`env/NAME`) → extend to `tenant/provider/NAME`
- Agent readiness ladder (`unsafe → protected → …`) → **binding readiness**
- HMAC audit chains, 127.0.0.1-only, zeroize, deny_unknown_fields

### 2. Vendor identity facts (hard constraints)

| Provider | Best pin | Agent footgun |
|----------|----------|---------------|
| Supabase | `project_ref` + `read_only` in MCP URL | User PAT = all orgs/projects |
| Vercel | team + project OIDC | MCP OAuth = full user; last `vercel switch` |
| GitHub | App install / fine-grained PAT | `gh auth switch` races; classic PAT over-scope |
| Cloudflare | Account-owned tokens + `account_id` | Wrong account_id in wrangler.toml |
| AWS | Profile + SSO + AssumeRole | Sticky `default` profile |
| Resend / Stripe / LLM keys | Key *is* identity | Live key / wrong domain / shared env |

**Architectural fact:** multi-tenant agents need either **one MCP process per Binding** or a **credential-injecting gateway**. Shared process + ambient env cannot be made safe with prompts.

### 3. Competitive white space

Covered: mcp-proxy/Supergateway (transport), MCPHub/ContextForge (aggregation), Composio/Zapier/Pipedream (SaaS multi-account OAuth), Infisical Agent Vault/Peta (secret brokers), AWS MCP multi-profile (AWS only), enterprise gateways (MintMCP/Turbo).

**Nobody owns:** local-first, multi-provider, project-bound **wrong-account interlock** for coding agents (consultant with 20 accounts + Claude Code/Cursor).

### 4. Agency requirements (ranked)

1. **BindingSet + Client Workspace** (atomic enter/leave)  
2. Continuous “am I on the right account?” verification  
3. Destructive dual-control / prod elevation TTL  
4. Session TTL + Leave Client Mode  
5. Visual identity (prompt/color/tool namespace)  
6. MCP/agent isolation  
7. Audit + forensics export  
8. Engagement offboard as a unit  

North-star metrics: **0 wrong-context incidents/quarter**, **&lt;30s to safe context**, **≥6 hrs/week** recovered for founder-IC.

---

## Product definition

### Pitch

> **Locus is the identity plane for coding agents.**  
> Pin a client. Every MCP tool and CLI command is hard-scoped to that binding until you re-pin. Wrong account becomes impossible — not discouraged.

### Taglines

- *Wrong account, impossible.*
- *Phantom for who. Locus for where.*
- *Profiles for agents — not just for shells.*

### Name

| Name | Notes |
|------|-------|
| **Locus** ★ | Place of action; `locus pin acme`; pairs with Phantom |
| Aperture / Bindr / Lane / Mooring | Strong runners; keep as alt if trademark fails |
| mmcp | Codename only |

### What it is *not*

- Not another MCP registry (Smithery/Glama)  
- Not another secrets vault (Phantom/Doppler)  
- Not another fleet conductor (ashlr-hub)  
- Not a 9k-app SaaS mesh (Composio)  
- Not enterprise IT MCP gateway first (MintMCP)

---

## Core abstractions

```
Principal  ──acts_as──►  Binding  ──for──►  Tenant
                              │
                              ├── ProviderAccount (supabase project, vercel team, …)
                              ├── CredentialRef   (phm:NAME | keychain | oauth)
                              └── Policy          (allow / deny / require_approval)

Workspace (.locus.toml) ──defaults_to──► Binding
Session ──sealed_to──► Binding (exclusive by default)
Worker  = process tree per Binding × Provider (creds only here)
```

**Binding** is the atomic unit of authority (AWS profile × GitHub App × Supabase project_ref).  
**Session seal** = HMAC(session_id ∥ binding_id ∥ times) — model cannot “prompt” a different tenant.  
**Unbound session ⇒ empty tool catalog** (only `locus_whoami` / `locus_request_pin`). No ambient personal fallthrough.

Detailed schemas: see `DESIGN.md` §3.

---

## Architecture (five planes)

```
Claude / Cursor / Codex / human CLI / CI
              │  single MCP: locus-mcp
              ▼
     DATA PLANE — multiplexor (pin, catalog, fan-out, capability tickets)
              │
   ┌──────────┼──────────┬──────────────┐
   ▼          ▼          ▼              ▼
CONTROL    POLICY     AUDIT       CREDENTIAL
bindings   allow/deny  HMAC       Phantom / keychain
sessions   approval    chain      OAuth broker
   │
   ▼
WORKERS — one PID per Binding×Provider
          private GH_CONFIG_DIR / AWS_CONFIG_FILE
          frozen project_ref / team_id / org allowlists
```

### Hard isolation (mechanisms, not memos)

1. Sealed session pin — agent cannot re-pin (only `request_pin` → human)  
2. Empty catalog when unbound  
3. Separate worker PIDs — Binding A’s env never in B’s process  
4. Private CLI config dirs — never mutate global `gh auth` / `aws`  
5. Adapter freezes account selectors — model-supplied `project_ref` ignored if scope frozen  
6. Capability HMAC tickets per `tools/call`  
7. Workspace `allowed_bindings` — Acme repo cannot pin `personal` without `--force` + audit  
8. TTL + continuous whoami drift detection  
9. Credential opacity via Phantom `phm:` refs  
10. Optional later: binding-scoped reverse proxy + sandboxed workers  

### DX (must feel as sharp as Phantom)

```bash
cd ~/clients/acme          # .locus.toml → default_binding = acme
locus pin                  # seal session
locus whoami               # acme · supabase:proj_x · vercel:team_y · gh:acme-corp
locus exec -- gh pr create # private GH_CONFIG_DIR + Acme token only
locus pin personal         # explicit human switch
locus leave                # unbind, kill workers, clear elevation
locus doctor               # readiness: safe to delegate?
```

- Prompt: `eval "$(locus hook zsh)"` → `[locus:acme·prod]`  
- One MCP entry for all clients: `locus-mcp` (not N supabase servers)  
- Tools namespaced under pin: only Acme tools visible  
- Visual identity per tenant (color, prompt, optional menu bar)

---

## Provider adapters (MVP first)

| Phase | Providers | Hard knobs |
|-------|-----------|------------|
| MVP | Supabase, GitHub, Vercel | project_ref+RO, org/repos, team+project |
| P2 | Cloudflare, AWS, Resend | account_id, profile/role, domain |
| P2+ | Stripe, xAI/OpenAI/Anthropic | livemode/RAK, key isolation |

Prefer **wrapping** upstream MCP with frozen env; reimplement only when upstream is full-user OAuth with no fence (some Vercel paths).

---

## Composition with Ashlr stack

```
.locus.toml + bindings
        │
        ▼
   locus-mcp  ──workers──► upstream MCPs / CLIs
        │                      ▲
        │                      │ CredentialRef
        │                   Phantom vault + proxy
        │
   optional: ashlr mcp gateway discovers ONLY locus (not raw personal MCPs)
```

Thin hub change later: accept profile-scoped discovery (`ASHLR_MCP_PROFILE` / paths) — hub already has `discoverMcpServers(paths?)` and fleet empty-registry pattern.

**Do not** merge into Phantom monorepo initially — separate product, compose via `phm:` refs (same pattern as Phantom vs hub).

---

## Threat model (highlights)

| Threat | Mitigation |
|--------|------------|
| Confused deputy | Workers + seal + no cross-binding tools |
| Prompt injection → re-pin | Agent pin off; human-gated request |
| Arg smuggling | Scope frozen at worker spawn |
| Global CLI race | Private config dirs only |
| Ambient personal keys | Unbound = no tools; pin required |
| Audit erase | HMAC chain |

**Non-goals:** root malware; human who force-pins wrong account; replacing cloud IAM.

Testable invariants (`INV-1…6`) + `locus test isolation` suite — see DESIGN.md §9.4.

---

## Implementation plan

### Phase 0 — Spike (1–2 weeks) · prove the mechanism

| Deliverable | Detail |
|-------------|--------|
| Repo bootstrap | Rust workspace (Phantom-grade) or TS if speed preferred — **recommend Rust** for daemon/proxy DNA shared with Phantom patterns |
| Daemon + UDS | `~/.locus/`, binding TOML store |
| `locus pin/whoami/status/exec` | Private env for `gh` |
| Supabase adapter | Frozen `project_ref` worker |
| Manual audit log | JSONL |
| Isolation demo | Two bindings; prove tool catalog exclusive |

**Success:** founder can pin `acme` vs `personal` and `locus exec --` cannot see the other binding’s credentials.

### Phase 1 — MVP (public OSS)

| Deliverable | Detail |
|-------------|--------|
| `locus-mcp` multiplexor | stdio for Claude Code + Cursor |
| Exclusive pin + seal | HMAC session |
| `.locus.toml` auto-pin | dir-binding |
| Adapters | Supabase + GitHub + Vercel |
| Phantom CredentialRef | `phm:NAME` resolution |
| Policy globs | allow/deny/require_approval |
| Audit + verify | HMAC chain |
| `locus setup --client` | claude / cursor / codex |
| Conformance tests | INV-1…6 |
| Landing + docs | phm.dev-quality sharpness |

**Success metrics:** time-to-safe-context &lt;15s; 0 cross-binding credential injections in dogfood; dogfood on Ashlr client repos.

### Phase 2 — Parallel-agent hard mode

- Worker pools; multi-bind namespaced mode (opt-in)  
- AWS, Cloudflare, Resend  
- Continuous whoami drift block  
- Prod elevation TTL  
- Prompt hook + status bar  
- `locus run -b acme -- claude -p …`  
- Approval UX (Touch ID optional)

### Phase 3 — Team / firm

- Shared binding graph (E2E encrypted, Phantom Cloud sibling patterns)  
- Dual-control destructive actions  
- Offboard engagement as unit  
- Remote audit / SIEM  
- Policy packs for SOC2 questionnaires  

### Phase 4 — Platform

- Adapter SDK + signed registry  
- CI ephemeral pins (Streamable HTTP)  
- Sandboxed workers  
- Bug bounty on seal/cap logic  

---

## Suggested monorepo layout

```
mmcp/  (or locus/)
  DESIGN.md                 # already present
  README.md
  crates/
    locus-core/             # Binding, Session, Policy, seal
    locus-daemon/           # UDS control plane
    locus-mcp/              # multiplexor data plane
    locus-cli/              # locus binary
    locus-audit/
    adapters/
      supabase/
      github/
      vercel/
  docs/
  tests/isolation/
  integrations/             # claude, cursor setup snippets
```

Language recommendation: **Rust** (align with Phantom, daemon safety, zeroize, single binary). Alternative: TypeScript on Bun for faster MVP using `@ashlr/mcp-kit` + hub gateway patterns — trade isolation hardness for ship speed. **Default: Rust.**

License: **MIT** (match Phantom) or Apache-2.0 OR MIT.

---

## Clever differentiators (the “Phantom-grade” sharpness)

| Naive multi-MCP configs | Locus |
|-------------------------|--------|
| Model sees **all** accounts’ tools | Sees **only** pinned binding |
| Global CLI still races | Private per-session config dirs |
| Switch = edit JSON + restart IDE | `locus pin acme` / dir auto-pin |
| Security = “be careful” | Security = PID + seal + frozen scope |
| Creds duplicated in every MCP env | One CredentialRef → Phantom/vault |
| Wrong account is a postmortem | Wrong account is a denied tool call |

**Sharp invariant:**

> Identity is resolved at the gate, not in the prompt.  
> The agent never chooses an account — the session already *is* an account.  
> Credentials enter isolated workers the way Phantom injects secrets on the wire.

---

## Open questions (decide before/at spike)

1. **Name lock:** Locus vs Aperture vs other (domain/trademark check: locus.dev, etc.)  
2. **Language:** Rust (recommended) vs TypeScript for MVP speed  
3. **MVP provider order:** Supabase+GitHub+Vercel confirmed? (matches your stated stack)  
4. **Relationship to Phantom:** compose-only v1 vs shared crates later  
5. **Agent pin policy:** default deny agent pin (recommended) vs allow with dual-confirm  
6. **Domain:** locus.dev / locu.sh / use-locus.dev availability  

---

## Immediate next steps (after plan approval)

1. Lock name + tagline + license  
2. Domain/trademark smoke check  
3. Bootstrap repo in `/Users/masonwyatt/Desktop/mmcp` from DESIGN.md  
4. Phase 0 spike: daemon + pin + Supabase worker + isolation test  
5. Dogfood on one real client BindingSet + personal BindingSet  
6. Parallel: landing page draft (Phantom-quality mechanism-first copy)  
7. Optional: thin ashlr-hub issue for “profile-scoped MCP discovery” (non-blocking)

---

## Artifacts produced this research pass

| Artifact | Path |
|----------|------|
| Full architecture design | `/Users/masonwyatt/Desktop/mmcp/DESIGN.md` |
| This plan | session `plan.md` |
| Phantom deep dive | explore agent (architecture, gaps, reuse) |
| ashlr-hub MCP gap analysis | explore agent (new product recommendation) |
| Competitive landscape | research agent (white space confirmed) |
| Vendor identity matrix | research agent (Binding primitive) |
| Agency ops requirements | research agent (R1–R10 + metrics) |

---

## Recommendation

**Build Locus as a new open-source product** in this empty `mmcp` repo. Position it as the third leg of the Ashlr agent safety story:

| Product | Question it answers |
|---------|---------------------|
| **Phantom** | Can this secret enter the model? |
| **Locus** | As whom, against which tenant, right now? |
| **ashlr-hub** | How do I run the fleet / aggregate tools? |

Start Phase 0 spike immediately after name/language decisions. The design is mechanism-complete enough to implement; the remaining work is product craft and dogfood on real multi-client pain.

# Locus — Northstar goal loop

Living progress surface for the product. Print anytime:

```bash
locus goal status
```

Related: [PLAN.md](./PLAN.md) (phased roadmap) · [DESIGN.md](./DESIGN.md) (architecture + threat model) · [docs/hub-integration.md](./docs/hub-integration.md)

---

## Vision

**Wrong account, impossible** — on an **AI-native**, **hub-native** identity plane.

| Pillar | Meaning |
|--------|---------|
| **Wrong-account impossible** | Seal + exclusive catalog + scrub + scope freeze. Soft prompts are not the control. |
| **AI-native** | Agents get whoami/status/report/resources/prompts; they cannot pin; setup wires MCP in one command. |
| **Hub-native** | ashlr-hub (and peers) shell out to `agent report` / `ci mint`; REQUIRED_SERVERS = locus + phantom only. |

Sibling: [Phantom](https://phm.dev) answers *can this secret enter the model?*  
Locus answers *as whom, against which tenant, right now?*

---

## Goal tree

```
Northstar: wrong-account impossible · AI-native · hub-native
│
├─ 1. Identity plane (core)
│     seal · pin · isolation · adapters · policy · fail-closed
│
├─ 2. Firm UX
│     enter/leave · doctor pane · engagement · graph · dual-control · agency kit
│
├─ 3. AI surface
│     locus-mcp tools/resources/prompts · agent report/setup · auto-pin · AGENT.md
│
├─ 4. Hub composition
│     agent-report contract · integrations/ashlr-hub · REQUIRED_SERVERS · doctor check
│
└─ 5. Verification plane (partial)
      claim verify stubs · continuous whoami in CI · isolation pack · audit SIEM · bounty
```

---

## Success metrics

| Metric | Target | Notes |
|--------|--------|-------|
| Wrong-account incidents | **0** / dogfood quarter | Cross-binding credential injection or wrong tenant tool success = P0 |
| Time to safe context | **&lt;15s** | `locus enter` / `quickstart` → whoami shows intended pin |
| Agent report ready | **gate green** | `locus agent report` → `status=ready` before mutate jobs |
| Hub gate | **fail closed** | hub never soft-allows `unsafe` / `unpinned` / `frozen` / `invalid` |
| Secret opacity | **0** secret values in agent/MCP/hub JSON | CredentialRef **names** only |

---

## Current milestone checklist

Checkboxes are the machine-readable surface for `locus goal status`.  
`[x]` = done · `[ ]` = remaining.

### M1 — Identity plane (core) · **done** (v0.1.0)

- [x] Binding store + TOML (`~/.locus` / `LOCUS_HOME`)
- [x] HMAC session seal; fail closed on tamper
- [x] `pin` / `leave` / `whoami` / `status` / `exec`
- [x] Ambient identity scrub + private GH/AWS worker dirs
- [x] Scope freeze on account selectors (adapters)
- [x] CredentialRefs: `phm:` / `env:` / `test:` (test-gated)
- [x] Policy allow/deny/require_approval
- [x] Workspace `.locus.toml` allowlist
- [x] Isolation tests (INV surface)

### M2 — Firm UX · **done** (v0.1.1)

- [x] `enter` / `leave` firm workflow
- [x] Doctor single pane (SAFE \| WARN \| UNSAFE)
- [x] `run -b` one-shot pin; `pin --ns` experimental multi-bind
- [x] Engagement init/close; encrypted graph export/import
- [x] Dual-control approvals; notify **off** by default
- [x] Agency starter kit + firm-mode docs
- [x] `ci mint` / `env` / `run` ephemeral sessions

### M3 — AI surface · **mostly done** (unreleased / 0.1.1+)

- [x] `locus-mcp` multiplexor; agents cannot pin
- [x] Control tools + exclusive catalog when unbound
- [x] `locus agent report|doctor|setup`
- [x] MCP resources (`locus://session|doctor|bindings`) + `locus_context` prompt
- [x] Tool description pin tags; `initialize.instructions`
- [x] MCP auto-pin from workspace / `LOCUS_AUTO_PIN`
- [x] `quickstart` + shell hook frozen/require_pin UX
- [x] `scripts/dogfood.sh` — quickstart → agent setup → report → doctor → forensics; `DOGFOOD READY` when ready or protected+pin
- [ ] Dogfood: agent report `ready` on real Claude Code + Cursor installs (personal + client) — local path exists; multi-client real installs still manual
- [ ] Upstream MCP workers for top adapters (not only synthetic freeze tools)

### M4 — Hub composition · **in progress**

- [x] `docs/hub-integration.md` + agent-report / doctor JSON schemas
- [x] `scripts/hub-smoke.sh` contract smoke
- [x] `integrations/ashlr-hub/locus.ts` probe + `withLocusSession` / `ensureLocusReady`
- [x] `integrations/ashlr-hub/mcp-gateway-snippet.md` (REQUIRED_SERVERS + discovery)
- [x] `integrations/ashlr-hub/doctor-check.md` (`checkLocus`)
- [x] `locusFleetGate()` + pure parse helpers (`evaluateFleetGate`, `parseStatusOneline`, `hasRequiredServers`)
- [x] `registerLocusInMcpConfig` / `mergeLocusIntoMcpConfig` MCP merge helpers
- [x] `integrations/ashlr-hub/fleet-preflight.md` — exact pre-dispatch preflight
- [x] `schema/hub-gate.schema.json` fleet gate response contract
- [x] `scripts/hub-integration-test.sh` composition smoke (required_servers + oneline + gate)
- [x] Land drop-in inside ashlr-hub: `src/core/integrations/locus.ts` + REQUIRED_SERVERS includes `locus` (+ ecosystem discovery name `locus`)
- [x] `ashlr doctor` calls `checkLocus` in production path (`src/core/doctor.ts`)
- [x] Hub pre-mutate gate library + spawn wire-in (opt-in `LOCUS_ENFORCE=1|warn`) — hub [PR #240](https://github.com/ashlrai/ashlr-hub/pull/240) stacks on [PR #239](https://github.com/ashlrai/ashlr-hub/pull/239) (`assertLocusPreMutate` in `spawnEngine`; scrubbed mint env)
- [x] Drop-in `integrations/ashlr-hub/locus.ts` synced with #240: `scrubbedChildEnv`, `validateMintEnv`, `withLocusSession` scrub, `resolveLocusEnforceMode` / `decidePreMutateGate` / `assertLocusPreMutate` / `formatPreMutateBlockers` / `applyLocusPreMutateGate` + docs (`hub-integration.md`, `fleet-preflight.md`)
- [ ] Always-on firm-mode enforce (default off until pin guaranteed on all hub paths) — open after #240 merges
- [ ] CI jobs use `withLocusSession` (or `ci mint`) — no shared ambient pin races — helpers + scrubbed mint in #240; job runners not yet wrapped

### M5 — Verification plane · **partial / in progress**

- [x] Architecture: [docs/verification-plane.md](./docs/verification-plane.md) (proposal → verify → act; confidence; tool grounding)
- [x] `locus verify claim --text "…"` → `{ claim, confidence, needs_tool, suggestion, signals, grounding? }` (heuristics: numbers/URLs/versions/currency/percentages/absolute language)
- [x] MCP `locus_verify_claim` (same shape; available unpinned; suggestion names concrete grounding steps)
- [x] `locus verify session` → doctor + whoami + safe_next JSON pack for hub (`session_ok`)
- [x] E2E: `scripts/e2e.sh` feature-detects `verify claim` + `verify session` (kind / session_ok / doctor / safe_next + no secret values)
- [x] Dogfood: `scripts/dogfood.sh` runs `verify session --json`; hard-requires `session_ok` when claiming DOGFOOD READY
- [x] Doctor optional WARN `ungrounded_claims` when audit tail has many low-confidence patterns
- [x] Conformance pack in CI: `.github/workflows/conformance.yml` — `invariants` + `locus-mcp` tests + `hub-smoke` + `e2e` (high timeout)
- [x] Best-effort sandboxed workers: `LOCUS_WORKER_SANDBOX=1` / `upstream.sandbox` → restricted PATH + `LOCUS_WORKER_SANDBOXED=1` + optional macOS `sandbox-exec` (not full seccomp/VM)
- [ ] Continuous whoami / `watch` in long agent sessions as first-class hub heartbeat (session pack is the CLI primitive; not yet a long-lived watcher)
- [ ] Audit export → SIEM / remote append (team tier)
- [ ] Adapter SDK + signed registry
- [ ] Hard sandbox (seccomp/VM) + bug bounty on seal/capability logic
- [ ] Streamable HTTP / remote multiplexor (platform phase)

---

## Goal loop (how we work this file)

1. **Pick** the highest incomplete milestone that unblocks dogfood (usually M3 tail or M4 hub land).
2. **Ship** a thin vertical slice with tests (fail closed, no secret leakage).
3. **Measure** against success metrics (enter latency, agent report ready, zero cross-binding).
4. **Check off** boxes here; keep PLAN.md phases for multi-quarter narrative.
5. **Re-pin** the northstar: if a change soft-allows wrong-account paths, it does not ship.

```bash
# Human / agent progress view
locus goal status

# Hub readiness (machine)
locus agent report --json   # exit 0 only when ready

# Isolation smoke (dev)
export LOCUS_HOME=/tmp/locus-goal-loop
locus init --with-samples && locus enter personal && locus doctor
```

---

## Non-goals (do not dilute the plane)

- Replacing cloud IAM or becoming a SaaS OAuth mesh (Composio-class)
- Absorbing Phantom vault core or ashlr-hub fleet OS
- Soft “please check project_ref” as a substitute for freeze
- Ambient personal fallthrough when unpinned

---

## Status legend

| Milestone | State |
|-----------|--------|
| M1 Identity plane | done |
| M2 Firm UX | done |
| M3 AI surface | mostly done |
| M4 Hub composition | in progress |
| M5 Verification plane | partial (claim+session verify, e2e/dogfood coverage, best-effort worker sandbox, conformance CI) |

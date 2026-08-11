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
- [x] `scripts/dogfood.sh` — quickstart → agent setup → report → doctor → forensics → verify session → Hub smoke; `DOGFOOD READY` only when every required probe is green
- [x] Automated multi-client install probe — `scripts/dogfood-clients.sh` detects Claude Code / Cursor / Continue paths (macOS+Linux), dry-runs `locus agent setup` for found supported clients, soft-skips missing; optional `DOGFOOD_CLIENTS=1` step in `dogfood.sh`; `LOCUS_DOGFOOD_REQUIRE_CLIENTS=1` hard-fails when none found or setup fails
- [x] Multi-account dogfood operator path — `docs/dogfood-multi-account.md` + `scripts/dogfood-multi-account.sh` walks personal+client pins (enter → doctor/verify → agent report ready → leave) without requiring both IDEs; soft-skip missing aliases; `LOCUS_DOGFOOD_REQUIRE_MULTI=1` hard-fails
- [ ] Dogfood: agent report `ready` on real Claude Code + Cursor installs (personal + client) — operator multi-account script landed; live dual-IDE still manual
- [x] Upstream MCP workers for top adapters (not only synthetic freeze tools) — composite expands `github-mcp` / `supabase-mcp` / `vercel-mcp` recipes with sandbox defaults; exclusive catalog + `resolve_secrets=false` ambient scrub covered in `locus-core` worker tests

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
- [x] Hub pre-mutate gate library + production wire-in (opt-in `LOCUS_ENFORCE=1|warn`) — hub [PR #239](https://github.com/ashlrai/ashlr-hub/pull/239) doctor/REQUIRED_SERVERS; [PR #241](https://github.com/ashlrai/ashlr-hub/pull/241) scrubbed mint + `applyLocusPreMutateGate` on `spawnEngine` / `runSwarm` / `runApiModelSandboxed`
- [x] Drop-in `integrations/ashlr-hub/locus.ts` synced with hub: `scrubbedChildEnv`, `validateMintEnv`, `withLocusSession` scrub, `resolveLocusEnforceMode` / `decidePreMutateGate` / `assertLocusPreMutate` / `formatPreMutateBlockers` / `applyLocusPreMutateGate` / `runWithLocusSessionIfConfigured` + docs (`hub-integration.md`, `fleet-preflight.md`) — locus [PR #14](https://github.com/ashlrai/locus/pull/14) (firm drop-in + runTask call sites); related locus [#15](https://github.com/ashlrai/locus/pull/15) dogfood clients · [#16](https://github.com/ashlrai/locus/pull/16) adapter registry · [#17](https://github.com/ashlrai/locus/pull/17) streamable-HTTP-lite (M3/M5)
- [x] Ecosystem MCP write injects `locusServerSpec` env (`LOCUS_HOME` / `LOCUS_CLIENT` / `LOCUS_NOTIFY`) — hub #241
- [x] Swarm path: `runWithLocusSessionIfConfigured` when `LOCUS_CI_BINDING` / `LOCUS_BINDING` set — hub #241
- [x] Single-task path: `runTask` wraps `runWithLocusSessionIfConfigured` (same CI mint overlay as swarm) — hub [PR #252](https://github.com/ashlrai/ashlr-hub/pull/252)
- [x] Firm config: `~/.ashlr/config.json` → `locus.enforce` (`off`|`warn`|`enforce`); env `LOCUS_ENFORCE` wins; drop-in `parseLocusEnforceToken` / `extractLocusConfigEnforce` / `readLocusConfigFromAshlr` — hub [PR #254](https://github.com/ashlrai/ashlr-hub/pull/254)
- [x] Firm profile: `~/.ashlr/config.json` → `locus.firm: true` enables enforce for production fleets (env `LOCUS_ENFORCE` still wins; explicit `locus.enforce` beats firm; monorepo default remains off when firm absent/false); drop-in `extractLocusConfigFirm` + resolution step — hub [PR #258](https://github.com/ashlrai/ashlr-hub/pull/258)
- [ ] Always-on firm-mode enforce by default (still **off** unless `locus.firm` / `locus.enforce` / `LOCUS_ENFORCE` set — #258 lands opt-in firm profile only; do not flip monorepo default until pin is guaranteed on all hub paths)
- [x] Broader CI/job runners: `runWithLocusSessionIfConfigured` on swarm (#241), `runTask` (#252), simple-conductor (#256), `runBestOfN` (#257) — other raw daemon tick paths (if any) still open

### M5 — Verification plane · **partial / in progress**

- [x] Architecture: [docs/verification-plane.md](./docs/verification-plane.md) (proposal → verify → act; confidence; tool grounding)
- [x] `locus verify claim --text "…"` → `{ claim, confidence, needs_tool, suggestion, signals, grounding? }` (heuristics: numbers/URLs/versions/currency/percentages/absolute language)
- [x] MCP `locus_verify_claim` (same shape; available unpinned; suggestion names concrete grounding steps)
- [x] `locus verify session` → doctor + whoami + safe_next JSON pack for hub (`session_ok`)
- [x] E2E: `scripts/e2e.sh` feature-detects `verify claim` + `verify session` (kind / session_ok / doctor / safe_next + no secret values)
- [x] Dogfood: `scripts/dogfood.sh` runs `verify session --json`; hard-requires `session_ok` when claiming DOGFOOD READY
- [x] Doctor optional WARN `ungrounded_claims` when audit tail has many low-confidence patterns
- [x] Conformance pack in CI: `.github/workflows/conformance.yml` — `invariants` + `locus-mcp` tests + `hub-smoke` + `e2e` (high timeout)
- [x] Fail-closed sandboxed workers: `LOCUS_WORKER_SANDBOX=1` / `upstream.sandbox` requires macOS Seatbelt, denies authority state and inbound network, and refuses unsupported backends
- [x] Continuous whoami / `watch` in long agent sessions as first-class hub heartbeat (`locus watch` each tick runs `verify_session`; NDJSON `kind=watch` + `--require-ok` fail-closed)
- [x] Audit export → SIEM / remote append — **minimal webhook sink** (`locus events export --sink webhook` / `LOCUS_AUDIT_WEBHOOK_URL`; redacted JSONL/OTLP POST; fail soft when unset; secret-scan fail closed). Full team-tier continuous append + chain verify still open
- [x] Adapter registry v0: `adapters/manifest.toml` schema (`schema/adapter-manifest.schema.json`) + parse/list/verify in `locus-core` + `locus adapter list|verify` (+ `--require-signed` fail-closed; HMAC mock trust keys in tests; built-in catalog ships unsigned). Full plugin load + production trust store still open — see [docs/adapter-sdk.md](./docs/adapter-sdk.md)#signed-registry-roadmap
- [x] Adapter registry ed25519: `ed25519:<base64>` verify (+ HMAC-SHA256 backcompat); `LOCUS_ADAPTER_TRUST_KEYS` env / test fixtures; `--require-signed` accepts either scheme when key trusted. Production `~/.locus/trust/` pin UX + signed release manifests still open
- [x] Hard sandbox (seccomp/VM) — **Linux session-sized partial**: `LOCUS_WORKER_SANDBOX=1` prefers bubblewrap (`LOCUS_WORKER_SANDBOX_BACKEND=bwrap`: RO system roots, bind work tree + session worker home only, network shared for MCP stdio, no host `~/.locus/bindings` bind); falls back to best-effort `path` when `bwrap` missing (restricted PATH + absolute exec; **not** kernel isolation — never claim false security). macOS Seatbelt unchanged. Full seccomp/VM + bug bounty on seal/capability logic still open
- [ ] Streamable HTTP / remote multiplexor (platform phase) — **partial**: streamable-HTTP-lite on `locus-mcp --http` with `GET /mcp` capabilities (tool **names** + pin summary, values-free), Accept/`Content-Type` negotiation (`application/json` preferred; single-event SSE when `Accept: text/event-stream` only), **`Mcp-Session-Id` landed** (in-memory map, 30m idle TTL, max N, mint on initialize/first POST, unknown → 404 fail-closed, `DELETE /mcp` teardown; process-local only — no redis), remote-deploy docs (reverse proxy, `LOCUS_MCP_HTTP_TOKEN`, `LOCUS_HOME`, pin-before-serve, forward `Mcp-Session-Id`), HTTP unit/integration tests (health + auth fail-closed + Accept 406/SSE + session mint/reuse/reject). **Still open:** multi-message SSE streaming, cross-process session resume, multi-tenant remote multiplexor

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
| M5 Verification plane | partial (claim+session verify, e2e/dogfood coverage, fail-closed macOS worker sandbox, Linux bwrap/path sandbox partial, conformance CI, audit webhook sink, adapter registry v0 list/verify) |

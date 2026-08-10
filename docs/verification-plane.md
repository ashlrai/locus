# Verification plane

> **Locus** = certain identity. **Phantom** = certain secrets. **Verification** = certain *reasoning / action* gates.

Milestone **M5** seeds a lightweight plane so agents and hub can ask: *should this claim be grounded with a tool before we act?*

No ML models. Pure heuristics + a stable JSON shape for ashlr-hub (and peers) to re-score or enforce later.

Related: [architecture.md](./architecture.md) · [GOALS.md](../GOALS.md) · [hub-integration.md](./hub-integration.md)

---

## Vision

Identity and secrets are necessary but not sufficient. Agents still invent facts:

| Plane | Question | Gate |
|-------|----------|------|
| **Phantom** | Can this secret enter the model? | Vault / inject-only |
| **Locus identity** | As whom, against which tenant, right now? | Seal + exclusive catalog + freeze |
| **Verification** | Is this claim grounded enough to drive action? | Propose → verify → act |

The verification plane does **not** replace policy, approvals, or scope freeze. It sits *before* mutate paths as an advisory (today) structure that hub can promote to hard gate.

---

## Loop: proposal → verify → act

```
  Agent / human proposes claim
           │
           ▼
  ┌────────────────────────────┐
  │  VERIFY                    │
  │  locus verify claim        │
  │  locus_verify_claim (MCP)  │
  │  · confidence band         │
  │  · needs_tool?             │
  │  · suggestion              │
  │  · optional grounding      │
  └────────────┬───────────────┘
               │
     needs_tool / low ──► call tools / whoami / doctor ──► re-verify
               │
     ok / medium+  ──► ACT (still under pin + policy + freeze)
```

**Fail closed on identity** remains absolute. Verification is additive: a high-confidence claim cannot override a wrong pin.

---

## Confidence

| Band | Meaning (stub) |
|------|----------------|
| `unknown` | No strong heuristic signal; soft suggestion only |
| `low` | Factual tokens (numbers / URLs / versions) or unhealthy identity context — **ground with tools** |
| `medium` | Identity claim grounded against active whoami (seal ok, not frozen) |
| `high` | Reserved for future hub / multi-source agreement (not emitted by heuristics today) |

Confidence is a **label**, not a calibrated probability. Hub may map bands to allow / require_tool / deny.

---

## Tool grounding

When `needs_tool` is true, the suggestion points at grounding actions:

- **Factual claims** (versions, URLs, quantities) → provider read tools (`*.list`, `*.get`, `*.status`, CLI under `locus exec`)
- **Identity claims** without pin → human `locus enter` / `locus pin`; agents use `locus_request_pin` / `locus_enter_hint`
- **Identity claims** with pin → attach whoami grounding; re-check with `locus_whoami` / `locus_heartbeat` if drift is possible

MCP never returns secrets. Grounding includes only aliases, tenant, binding id, seal/frozen flags.

---

## Surfaces

### CLI

```bash
locus verify claim --text "Error rate is 12% on https://api.example.com"
locus verify claim --text "We are pinned to acme" --json
locus verify claim --text "This always costs $500" --json
locus verify session --json   # doctor + whoami + safe_next pack for hub
```

**Claim result shape** (always; human mode also prints a compact summary):

```json
{
  "claim": "…",
  "confidence": "unknown" | "low" | "medium" | "high",
  "needs_tool": true,
  "suggestion": "…",
  "signals": ["url", "number", "percentage"],
  "grounding": {
    "kind": "whoami",
    "binding_alias": "acme",
    "tenant": "acme-corp",
    "binding_id": "bnd_…",
    "seal_ok": true,
    "frozen": false
  }
}
```

`grounding` is omitted when not identity-related or when unbound.

**Session pack** (`locus verify session`):

```json
{
  "kind": "session",
  "version": "0.x.y",
  "whoami": { "...": "…" },
  "doctor": { "verdict": "…", "ok": true, "findings": [] },
  "safe_next": { "action": "ready|enter|…", "ready": true, "message": "…" },
  "session_ok": true
}
```

Never includes secrets. `session_ok` is true only when doctor is ok **and** `safe_next.ready`. `locus verify session` emits the pack for inspection but exits nonzero whenever `session_ok` is false; there is no success-status inspection bypass.

### MCP

Control tool (available unpinned and pinned):

| Tool | Args | Result |
|------|------|--------|
| `locus_verify_claim` | `text` or `claim` (string) | Same JSON shape as CLI claim |

Session pack is CLI/hub today (`locus verify session --json`); agents can compose `locus_whoami` + doctor resource + `locus_safe_next` equivalently.

Audit op: `mcp.verify_claim` / `verify.claim` / `verify.session` — confidence, signals, truncated claim preview / verdict flags only (no secrets).

### Doctor (light)

When the recent audit tail has **many** detail blobs that look like ungrounded factual claims (numbers / URLs / versions), doctor adds a **WARN** finding:

- code: `ungrounded_claims`
- message points at `locus verify claim --text "…"`

Threshold: `DOCTOR_LOW_CONFIDENCE_AUDIT_THRESHOLD` (5) over the last `DOCTOR_LOW_CONFIDENCE_AUDIT_SCAN` (50) events. Never escalates to UNSAFE by itself.

---

## Heuristics (M5 stub)

Implemented in `locus_core::verify`:

1. **URL-like** tokens (`http://`, `https://`, `www.`, `://`) → signal `url`
2. **Version-like** tokens (`1.2`, `v0.1.1`) → signal `version`
3. **Percentages** (`12%`, `0.5 %`) → signal `percentage` (also counts as number)
4. **Currency** (`$1,200`, `USD 40`, `€99`) → signal `currency`
5. **Significant numbers** (2+ digits) → signal `number`
6. **Absolute language** (`always`, `never`, `impossible`, `guaranteed`, …) → signal `absolute_language` → **low** confidence
7. **Identity language** (pin, tenant, whoami, binding, acting as, wrong account, …) → signal `identity`
8. If (1–6) fire → `confidence=low`, `needs_tool=true` (suggestion names concrete grounding steps)
9. If identity + healthy pin → attach whoami grounding; `confidence=medium` when not also factual/absolute
10. Else → `confidence=unknown`

Hub extension points: re-score using `signals`, require tools when `needs_tool`, or demand multi-source agreement before allowing `high`.

---

## What this is not

- Not a substitute for seal / exclusive catalog / scrub / scope freeze
- Not Phantom — secrets still must not enter the model
- Not formal proof or model-based truth
- Not a hard block in CLI/MCP today (advisory structure for hub policy)

---

## Roadmap (from GOALS M5)

Still open under verification plane:

- Continuous whoami / watch as first-class hub heartbeat (session pack is the CLI primitive)
- Audit export → SIEM
- Adapter SDK + signed registry
- Harder sandbox (seccomp/VM); bounty on seal logic
- Streamable HTTP / remote multiplexor

Shipped as partial M5:

- [x] Architecture doc (this file)
- [x] `locus verify claim` + `locus_verify_claim` (currency / % / absolute language signals)
- [x] `locus verify session` — doctor + whoami + safe_next JSON pack
- [x] E2E + dogfood coverage: `scripts/e2e.sh` feature-detects `verify claim` / `verify session` (shape + no secret values); `scripts/dogfood.sh` hard-requires `session_ok` at the DOGFOOD READY gate
- [x] Doctor optional `ungrounded_claims` finding
- [x] Fail-closed macOS worker sandbox (`LOCUS_WORKER_SANDBOX=1` / `upstream.sandbox`); unsupported platforms refuse sandboxed spawn
- [x] Core module + unit tests (heuristics only)

---

## Code map

| Path | Role |
|------|------|
| `crates/locus-core/src/verify.rs` | Heuristics + `ClaimVerification` |
| `crates/locus-core/src/agent_report.rs` | `verify_session` → `SessionVerificationPack` |
| `crates/locus-core/src/workers/sandbox.rs` | Deny-by-default macOS Seatbelt; no PATH-only fallback |
| `crates/locus-cli` | `locus verify claim` · `locus verify session` |
| `crates/locus-mcp` | `locus_verify_claim` control tool |
| `crates/locus-core/src/doctor.rs` | Optional audit signal finding |
| `GOALS.md` | M5 checklist |

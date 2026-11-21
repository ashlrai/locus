# Verification plane

> **Locus** = certain identity. **Phantom** = certain secrets. **Verification** = certain *reasoning / action* gates.

Milestone **M5** seeds a lightweight plane so agents and hub can ask: *should this claim be grounded with a tool before we act?*

No ML models. Pure heuristics + a stable JSON shape for Phantom (and peers) to re-score or enforce later.

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

- **Factual claims** (versions, URLs, quantities) → policy-gated provider read tools (`*.list`, `*.get`, `*.status`)
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
locus watch --once --json     # one heartbeat tick (same pack, compact NDJSON)
locus watch --json --require-ok --interval 30s   # continuous hub stream; fail closed
```

**Claim result shape** (with `--json`; human mode prints a compact summary, then the same object as one JSON line):

```json
{
  "schema": "locus.verify.claim.v1",
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

`grounding` is omitted when not identity-related or when unbound. When it is attached, `signals` also includes `grounding_whoami`.

**Session pack** (`locus verify session`):

```json
{
  "schema": "locus.verify.session.v1",
  "kind": "session",
  "version": "0.x.y",
  "whoami": { "...": "…" },
  "doctor": { "verdict": "…", "ok": true, "findings": [] },
  "safe_next": { "action": "ready|enter|…", "ready": true, "message": "…" },
  "session_ok": true
}
```

Never includes secrets. `session_ok` is true only when doctor is ok **and** `safe_next.ready`. `locus verify session` emits the pack for inspection (human or `--json`) but exits nonzero whenever `session_ok` is false; there is no success-status inspection bypass. The CLI pack (like `locus watch`) also carries the operator-shell control-capability findings that `locus doctor` reports; the MCP tool's pack does not.

### Continuous watch (hub heartbeat stream)

`locus watch` re-runs the session pack each tick (not drift alone). Suitable for long agent sessions and hub heartbeats.

```bash
locus watch --once --json
locus watch --json --require-ok --interval 30s
```

**Tick shape** (`--json` → one NDJSON object per line):

```json
{
  "schema": "locus.watch.v1",
  "kind": "watch",
  "session_ok": true,
  "whoami": "acme",
  "doctor_verdict": "SAFE",
  "safe_next": "ready",
  "pinned": true,
  "frozen": false
}
```

| Flag | Behavior |
|------|----------|
| `--once` | Single tick then exit |
| `--require-ok` | Fail closed: non-zero exit whenever `session_ok` is false (including unpinned) |
| `--once` only | Non-zero when pin was present/expected **and** `session_ok` is false; unpinned soft-exits 0 |
| `--interval` | Poll period (`5s`, `30s`, `1m`, `2h`, or bare seconds such as `10`; default `5s`; minimum `1s`) |
| neither flag | Runs until interrupted; a tick with `session_ok` false is reported but does not exit |

`whoami` (the active binding alias) is omitted when there is none. Binding drift still freezes the pin (same as doctor / `verify_session`). Never includes secrets. Prefer `locus verify session --json` when hub needs the full doctor + whoami objects; use `watch` for continuous compact ticks.

### MCP

Control tools (available unpinned and pinned):

| Tool | Args | Result |
|------|------|--------|
| `locus_verify_claim` | `text` or `claim` (string) | Same JSON shape as CLI claim, without the `schema` key |
| `locus_verify_session` | none | Same pack as `locus verify session --json` without the `schema` key (`whoami?`, `doctor`, `safe_next`, `session_ok`); runs real external probes |

`locus_verify_session` differs from the CLI pack in two cases. A tenant-scoped grant (`locus-mcp --http --multi-tenant`) never sees the operator pack: it gets `scope: "tenant"`, `grant_id`, `whoami`, the grant's drift result as `runtime`, and `safe_next.action` `proceed` or `remint_grant` (with a `locus mcp mint --binding <alias>` command). When the MCP session is anchored to one identity, the body carries `mcp_anchor`; on the operator pack, if a *different* pin is now active, `session_ok` is forced false and `safe_next` becomes `reinitialize_client` with `locus enter <anchored alias>`. Either way `isError` stays false; gate on `session_ok`.

MCP tool payloads are not schema-stamped; only CLI `--json` output is (see [agent-contract.md](./agent-contract.md)).

Audit ops: `verify.claim`, `verify.session` and `watch.tick` (CLI); `mcp.verify_claim` and `mcp.verify_session` (MCP). They record confidence, signals, a truncated claim preview, or verdict flags only (no secrets).

### Doctor (light)

When the recent audit tail has **many** detail blobs that look like ungrounded factual claims (numbers / URLs / versions / currency / percentages / absolute language), doctor adds a **WARN** finding. Identity control ops are skipped because they are the grounding surface (`*whoami*`, `*heartbeat*`, `session.pin`, `session.leave`, `verify.*`):

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
8. If identity + pin → attach whoami grounding and signal `grounding_whoami` (also when 1–6 fire)
9. If (1–6) fire → `confidence=low`, `needs_tool=true` (suggestion names concrete grounding steps)
10. Else identity + healthy pin (seal ok, not frozen) → `confidence=medium`, `needs_tool=false`
11. Else identity with a bad seal or frozen pin, or with no pin → `confidence=low`, `needs_tool=true` (re-pin / doctor, or human `locus enter`)
12. Else → `confidence=unknown`, `needs_tool=false`

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

- Full team-tier SIEM (continuous remote append, chain verification beyond one-shot webhook)
- Signed release manifests and a published adapter registry root (see [registry-signing-status.md](./registry-signing-status.md))
- Full hard sandbox (seccomp/VM) beyond Linux bubblewrap partial; bounty on seal logic

Shipped as partial M5:

- [x] Architecture doc (this file)
- [x] `locus verify claim` + `locus_verify_claim` (currency / % / absolute language signals)
- [x] `locus verify session` — doctor + whoami + safe_next JSON pack
- [x] Continuous whoami / `locus watch` — each tick runs session pack; NDJSON heartbeat (`kind=watch`) + `--require-ok` fail-closed
- [x] E2E + dogfood coverage: `scripts/e2e.sh` feature-detects `verify claim` / `verify session` / optional `watch --once --json` (shape + no secret values); `scripts/dogfood.sh` hard-requires `session_ok` at the DOGFOOD READY gate
- [x] Doctor optional `ungrounded_claims` finding
- [x] Fail-closed macOS worker sandbox (`LOCUS_WORKER_SANDBOX=1` / `upstream.sandbox`)
- [x] Linux hard-sandbox **partial**: prefer bubblewrap (`LOCUS_WORKER_SANDBOX_BACKEND=bwrap`); fallback best-effort `path` (restricted PATH only — not kernel isolation); network allowed for MCP
- [x] Core module + unit tests (heuristics only)
- [x] Audit webhook sink — `locus events export --sink webhook` / `LOCUS_AUDIT_WEBHOOK_URL` (redacted POST; fail soft when unset)
- [x] Conformance pack in CI (`.github/workflows/conformance.yml`)
- [x] Adapter registry v0: manifest schema, `locus adapter list|verify` (`--require-signed`), ed25519 signatures and the `locus adapter trust` store
- [x] Streamable HTTP / remote multiplexor: streamable-HTTP-lite on `locus-mcp --http` (session resume, SSE) and `locus-mcp --http --multi-tenant` with operator-minted grants (`locus mcp mint|list|revoke`); see [mcp.md](./mcp.md)

---

## Code map

| Path | Role |
|------|------|
| `crates/locus-core/src/verify.rs` | Heuristics + `ClaimVerification` |
| `crates/locus-core/src/agent_report.rs` | `verify_session` → `SessionVerificationPack` |
| `crates/locus-core/src/workers/sandbox.rs` | macOS Seatbelt; Linux `bwrap` / best-effort `path` (tagged; not a VM) |
| `crates/locus-core/src/events.rs` | Fleet pulse / OTLP export + optional webhook sink |
| `crates/locus-cli` | `locus verify claim` · `locus verify session` · `locus watch` · `events export --sink webhook` |
| `crates/locus-mcp` | `locus_verify_claim` · `locus_verify_session` control tools |
| `crates/locus-core/src/doctor.rs` | Optional audit signal finding |
| `GOALS.md` | M5 checklist |

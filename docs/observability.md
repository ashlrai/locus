# Observability

Local audit, near-miss metrics, forensics packs, and optional OTLP / fleet-pulse export.

**Secrets never leave the machine via these tools** — exports contain ops, digests, aliases, and counts only.

---

## Audit log

Path: `$LOCUS_HOME/audit/events.jsonl` (default `~/.locus/audit/events.jsonl`).

Each line:

```json
{"ts":"2026-08-09T18:12:01Z","op":"mcp.scope_freeze","binding":"acme","detail":{"tool":"supabase.scope","args_digest":"…"}}
```

| Field | Notes |
|-------|--------|
| `ts` | RFC3339 UTC |
| `op` | e.g. `session.pin`, `mcp.scope_freeze`, `mcp.require_approval`, `approval.advisory` |
| `binding` | Binding alias (or empty for global ops) |
| `detail` | Optional JSON — digests and ids only, never raw tokens |

```bash
locus events --last 50
locus events --last 100 --op scope_freeze --binding acme
locus events --json
```

---

## Near-miss summary

A **near miss** is an audit event that shows the identity plane stopped a wrong-tenant or policy-blocked action:

| Op pattern | Meaning |
|------------|---------|
| `*scope_freeze*` | Model tried a selector outside frozen scope |
| `*require_approval*` | Destructive / gated tool blocked pending external authority |

`locus doctor` (and doctor JSON) include:

```json
"near_miss_count": 3
```

Counted over the **last 24 hours**. Human doctor pane also prints the count; WARN findings still fire for recent scope_freeze / deny in the audit tail.

Forensics packs embed a structured slice:

```json
"near_miss": {
  "window_hours": 24,
  "count": 3,
  "scope_freeze": 2,
  "require_approval": 1
}
```

---

## Forensics pack

Shareable incident bundle for support / postmortems:

```bash
locus forensics export
locus forensics export --binding acme --out pack.json
locus forensics export --binding acme --last 500 --json   # stdout when no --out
```

Contents (no secrets):

| Section | Contents |
|---------|----------|
| `pin` | Active session metadata (ids, tenant, seal_ok, frozen) |
| `bindings` | Summaries (alias, tenant, provider names) |
| `audit_events` | Last N events (optional binding filter) |
| `doctor` | Full doctor snapshot (SAFE/WARN/UNSAFE) |
| `pending_approvals` | Approval ids, tools, args_digest, grants |
| `near_miss` | 24h near-miss counters |
| `chain_tip` | SHA-256 of last event + optional seal HMAC |

```bash
# Shape check in tests / CI
locus forensics export --out /tmp/pack.json
jq '{pack_version, near_miss, chain_tip, verdict: .doctor.verdict}' /tmp/pack.json
```

---

## Events export (fleet pulse / OTLP)

### JSON lines (default)

One envelope per audit event — compatible with fleet pulse / log shippers:

```bash
locus events export --last 200
locus events export --binding acme --out acme.jsonl
```

Envelope schema `locus.audit.v1`:

```json
{
  "schema": "locus.audit.v1",
  "locus_version": "0.1.1",
  "exported_at": "…",
  "ts": "…",
  "op": "mcp.scope_freeze",
  "binding": "acme",
  "detail": { "tool": "…", "args_digest": "…" },
  "kind": "audit"
}
```

### OTLP-compatible logs JSON

```bash
locus events export --otlp --last 200 --out otlp-logs.json
```

Produces an OTLP **Logs** JSON body (`resourceLogs` → `scopeLogs` → `logRecords`) suitable for:

- OpenTelemetry Collector `otlphttp` receiver (POST body)
- Offline inspection / conversion

Attributes include `locus.op`, `locus.binding`, `service.name=locus`.  
Scope freeze / require_approval map to severity **WARN** (13).

Locus does **not** open a network connection; pipe or POST yourself:

```bash
locus events export --otlp --last 100 | \
  curl -sS -X POST -H 'Content-Type: application/json' \
    --data-binary @- http://localhost:4318/v1/logs
```

---

## Doctor as mission control

```bash
locus doctor
locus doctor --json | jq '{verdict, near_miss_count, audit, pending_approvals}'
```

Exit codes: SAFE=0, WARN=1, UNSAFE=2.

MCP resource `locus://doctor` exposes the same JSON for agents (still no secrets).

---

## What not to export

| Safe | Never |
|------|--------|
| Binding aliases, tenants | Resolved API keys / PATs |
| CredentialRef **names** (`phm:X`) | Phantom reveal output |
| args_digest, approval ids | Raw tool args that may hold secrets |
| Seal HMAC of digests | Daemon seal key bytes |

If a secret lands in audit history, rotate the credential and treat the log as sensitive.

---

## Related

| Doc | Topic |
|-----|--------|
| [architecture.md](./architecture.md) | Planes and gate |
| [policy.md](./policy.md) | require_approval / dual-control |
| [adapter-sdk.md](./adapter-sdk.md) | Freeze + audit of adapter blocks |
| [SECURITY.md](../SECURITY.md) | Threat model + reporting |

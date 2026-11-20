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
| `detail` | Optional JSON: sanitized details, such as digests and IDs |
| `source` | `executor` for broker-written records authenticated with an executor capability; absent on other records |

The broker constructs its own audit records and timestamps. An executor capability can request only `session.exec`, `session.exec.delegated`, `session.run`, and `ci.run` records; it cannot write control-plane operations through the broker. When the broker is unavailable, the local fallback appends directly.

```bash
locus events                                       # last 50 (--last / --limit)
locus events --last 100 --op scope_freeze --binding acme
locus events --json                                # JSON array
```

`--op` matches an exact op or a substring. A filter that matches nothing prints a hint naming what the log does hold.

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

Counted over the **last 24 hours**; events whose `ts` does not parse are left out. A non-zero count adds a `near_miss` WARN finding. Separately, `recent_scope_freeze` and `recent_deny` WARN findings fire for scope_freeze and deny events in the recent audit tail.

Forensics packs embed a structured slice (filtered to `--binding` when given):

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
locus forensics export                                 # writes ./pack.json
locus forensics export --binding acme --out pack.json
locus forensics export --binding acme --last 500 --json   # full pack on stdout when no --out
```

`--last` defaults to 200 events. With both `--json` and `--out`, the pack goes to the file and stdout gets a one-line summary (`path`, `pack_version`, `near_miss_count`, `audit_event_count`, `verdict`).

Contents (no secrets):

| Section | Contents |
|---------|----------|
| `pack_version`, `locus_version`, `exported_at`, `home` | Pack metadata (`filter_binding` too when `--binding` is set) |
| `pin` | Active session metadata (ids, tenant, principal, client, expiry, seal_ok, frozen, mode); absent when unpinned |
| `bindings` | Summaries (alias, tenant, provider names) |
| `audit_events`, `audit_event_count` | Last N events (optional binding filter); each `detail` is replaced by `{"redacted": true, "bytes": N}` |
| `doctor` | Full doctor snapshot (SAFE/WARN/UNSAFE); its audit tail `detail` is redacted the same way |
| `pending_approvals` | Approval ids, tools, args_digest, status, advisory grant counts |
| `near_miss` | 24h near-miss counters |
| `chain_tip` | Event count, SHA-256 of the last event, and its seal HMAC when the seal key is available |

```bash
# Shape check in tests / CI
locus forensics export --out /tmp/pack.json
jq '{pack_version, near_miss, chain_tip, verdict: .doctor.verdict}' /tmp/pack.json
```

---

## Events export (fleet pulse / SIEM / OTLP)

`--since <ts>` bounds the exported slice in every format — RFC3339
(`2026-08-09T00:00:00Z`, any offset) or a bare date (`2026-08-09` = start of
day UTC), inclusive. Events whose `ts` does not parse are dropped while a
since filter is active; an unparseable bound is an error, not a silent skip.

`--last` (default 200), `--op` and `--binding` filter the same way as `locus events`.

### JSON lines (default)

One envelope per audit event — compatible with fleet pulse / log shippers:

```bash
locus events export --last 200
locus events export --binding acme --out acme.jsonl
locus events export --since 2026-08-09T00:00:00Z
```

Envelope schema `locus.audit.v1` (versioned wire contract:
[`schema/locus-events.v1.schema.json`](../schema/locus-events.v1.schema.json),
`$defs.audit`; the same file also covers `locus.session_tick` and `watch`
ticks):

```json
{
  "schema": "locus.audit.v1",
  "locus_version": "0.7.2",
  "exported_at": "2026-10-27T12:05:00Z",
  "ts": "2026-10-27T12:00:00Z",
  "op": "mcp.scope_freeze",
  "binding": "acme",
  "detail": { "tool": "…", "args_digest": "…" },
  "kind": "audit"
}
```

Schema validation checks structure, not redaction or absence of sensitive data.
Audit `detail` is copied from the underlying record and can contain arbitrary
JSON; handle exports accordingly.

### SIEM NDJSON

`--format ndjson` emits one `locus.siem.v1` event per line with stable field
names, built for SIEM ingest:

```bash
locus events export --format ndjson --since 2026-08-09T00:00:00Z
locus events export --format ndjson --binding acme --out acme.siem.ndjson
```

```json
{"schema":"locus.siem.v1","ts":"…","op":"mcp.scope_freeze","binding":"acme","severity":"WARN","kind":"audit","detail":{"tool":"…","args_digest":"…"},"exported_at":"…","locus_version":"…"}
```

Beyond the store's write-time sanitization, the SIEM path deep-redacts
`detail` again at export: credential-shaped keys (`password`, `token`,
`credential_ref`, `*_key`, `authorization`, …) surface as
`<name>_redacted: "<redacted>"`, and strings carrying locator prefixes
(`phm:`/`env:`/`test:`) or known token prefixes become `<redacted>`. The
finished body must pass the same fail-closed secret scan as the webhook sink.

### OTLP-compatible logs JSON

```bash
locus events export --otlp --last 200 --out otlp-logs.json   # same as --format otlp
```

Produces an OTLP **Logs** JSON body (`resourceLogs` → `scopeLogs` → `logRecords`) suitable for:

- OpenTelemetry Collector `otlphttp` receiver (POST body)
- Offline inspection / conversion

Record attributes are `locus.op`, `locus.binding`, `locus.kind`, `service.name` (default `locus`; set with `--service-name`) and, when present, `locus.detail` (the detail JSON, clipped to 2048 bytes). The resource also carries `service.version`.

Severity follows the op, and SIEM NDJSON uses the same mapping:

| Op contains | Severity |
|-------------|----------|
| `scope_freeze`, `freeze`, `deny`, `require_approval` | **WARN** (13) |
| `error`, `fail` | **ERROR** (17) |
| anything else | **INFO** (9) |

You can still pipe or POST yourself:

```bash
locus events export --otlp --last 100 | \
  curl -sS -X POST -H 'Content-Type: application/json' \
    --data-binary @- http://localhost:4318/v1/logs
```

### Webhook sink (minimal SIEM / remote append)

Optional HTTP(S) POST of the same export body — team-tier primitive, not a full SIEM pipeline:

```bash
# Explicit URL
locus events export --sink webhook --url https://siem.example/ingest --last 200

# Or env (fail soft when unset — exit 0, no network)
export LOCUS_AUDIT_WEBHOOK_URL=https://siem.example/ingest
locus events export --sink webhook --last 200

# OTLP body to a collector path
locus events export --sink webhook --otlp --url http://127.0.0.1:4318/v1/logs
```

| Behavior | Detail |
|----------|--------|
| Unset URL | **Fail soft** — prints skip message to stderr, exit 0 |
| Body secret scan | **Fail closed** — refuse POST if body matches token prefixes / secret field names |
| URL scheme | `http://` or `https://` only |
| Response | Any non-2xx status is an error; 15 s timeout |
| Content-Type | `application/x-ndjson` (JSON lines, SIEM NDJSON) or `application/json` (OTLP) |
| Headers | `X-Locus-Export: audit`, `User-Agent: locus/<version>` |
| `--out` | Also writes the same body to a local file |
| Logs | Scheme and host only (`https://hooks.example.com`) — never full URL (query may hold tokens) |

Bodies are the same fleet-pulse / SIEM / OTLP payloads as file export: ops, digests, aliases — never resolved credentials. Pick `--format ndjson` when you want the extra detail redaction. Continuous chain-verified remote append remains a later team-tier item.

---

## Doctor as mission control

```bash
locus doctor
locus doctor --json | jq '{verdict, near_miss_count, audit, pending_approvals}'
```

Exit codes: SAFE=0, WARN=1, UNSAFE=2. A `WARN` whose findings are all setup advisories (such as `phantom_missing`) exits 0 while the report still says `WARN`; see [exit-codes.md](./exit-codes.md).

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

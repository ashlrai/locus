//! SIEM export — newline-delimited JSON (NDJSON) with stable field names.
//!
//! ```text
//! locus events export --format ndjson [--since <ts>] [--binding acme] [--last N]
//! ```
//!
//! One [`SiemEvent`] (`schema: locus.siem.v1`) per line — field names are
//! stable so downstream parsers can rely on them.
//!
//! Never a credential value:
//!
//! - `detail` is deep-redacted by [`redact_siem_value`]: credential-shaped keys
//!   (`password`, `token`, `credential_ref`, `*_key`, `authorization`, …) are
//!   emitted as `<name>_redacted: "<redacted>"`, and locator refs (`phm:` /
//!   `env:` / `test:`) or strings carrying known token prefixes become
//!   `<redacted>` — even if the raw audit line bypassed the store's own
//!   write-time sanitization.
//! - The finished body must pass
//!   [`crate::events::assert_export_body_no_secrets`] before it is returned
//!   (fail closed, same bar as the webhook sink).

use crate::error::LocusError;
use crate::events::{assert_export_body_no_secrets, severity_text_for_op, SECRET_VALUE_MARKERS};
use crate::store::AuditEvent;
use crate::VERSION;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Schema id carried by every SIEM line.
pub const SIEM_SCHEMA: &str = "locus.siem.v1";

/// Replacement text for redacted material.
const REDACTED: &str = "<redacted>";

/// One SIEM event line. Field names are the export contract — do not rename.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SiemEvent {
    /// Always [`SIEM_SCHEMA`].
    pub schema: String,
    /// Event timestamp (RFC3339, as written to the audit log).
    pub ts: String,
    /// Audit op, e.g. `session.pin`, `mcp.scope_freeze`.
    pub op: String,
    /// Binding alias (`""` for global ops).
    pub binding: String,
    /// `INFO` | `WARN` | `ERROR` — same mapping as the OTLP export.
    pub severity: String,
    /// Stable kind for dashboards: `audit`.
    pub kind: String,
    /// Redacted audit detail (`null` when the event has none).
    pub detail: Value,
    /// When this export ran (RFC3339).
    pub exported_at: String,
    pub locus_version: String,
}

impl SiemEvent {
    pub fn from_audit(ev: &AuditEvent, exported_at: &str) -> Self {
        Self {
            schema: SIEM_SCHEMA.into(),
            ts: ev.ts.clone(),
            op: ev.op.clone(),
            binding: ev.binding.clone(),
            severity: severity_text_for_op(&ev.op).into(),
            kind: "audit".into(),
            detail: ev
                .detail
                .as_ref()
                .map(redact_siem_value)
                .unwrap_or(Value::Null),
            exported_at: exported_at.into(),
            locus_version: VERSION.to_string(),
        }
    }
}

/// Options for [`export_siem_ndjson`].
#[derive(Debug, Clone, Default)]
pub struct SiemExportOptions {
    /// Lower bound on `ts` — RFC3339 or `YYYY-MM-DD` (parsed by
    /// [`parse_since`], inclusive).
    pub since: Option<String>,
    /// Max events from the end of the filtered log (default 200).
    pub last: Option<usize>,
    /// Filter by op (exact or substring).
    pub op: Option<String>,
    /// Filter by binding alias.
    pub binding: Option<String>,
}

/// Parse a `--since` bound: RFC3339 (`2026-08-09T12:00:00Z`, any offset) or a
/// bare date (`2026-08-09`, meaning start of day UTC). Anything else is an
/// error — a silently mis-parsed bound would ship the wrong slice to a SIEM.
pub fn parse_since(raw: &str) -> crate::Result<DateTime<Utc>> {
    let s = raw.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    if let Ok(day) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        if let Some(dt) = day.and_hms_opt(0, 0, 0) {
            return Ok(dt.and_utc());
        }
    }
    Err(LocusError::msg(format!(
        "invalid --since timestamp '{s}' — use RFC3339 (e.g. 2026-08-09T00:00:00Z) or a date (2026-08-09)"
    )))
}

/// Events whose `ts` parses as RFC3339 and is at or after `since` (inclusive).
///
/// Events with an unparseable `ts` are dropped while a since filter is active:
/// a SIEM consumer gets a provable lower bound, not a best-effort guess.
pub fn events_since(events: &[AuditEvent], since: &DateTime<Utc>) -> Vec<AuditEvent> {
    events
        .iter()
        .filter(|e| {
            DateTime::parse_from_rfc3339(&e.ts)
                .map(|ts| ts.with_timezone(&Utc) >= *since)
                .unwrap_or(false)
        })
        .cloned()
        .collect()
}

/// Filter + redact audit events into SIEM NDJSON (one [`SiemEvent`] per line).
pub fn export_siem_ndjson(
    events: &[AuditEvent],
    opts: &SiemExportOptions,
) -> crate::Result<String> {
    crate::events::export_events(
        events,
        &crate::events::EventsExportOptions {
            since: opts.since.clone(),
            last: opts.last,
            op: opts.op.clone(),
            binding: opts.binding.clone(),
            format: crate::events::EventsExportFormat::Siem,
            ..Default::default()
        },
    )
}

/// Serialize already-filtered events as SIEM NDJSON lines. The finished body
/// must pass the shared secret scan before it is returned (fail closed).
pub(crate) fn to_ndjson_lines(events: &[AuditEvent], exported_at: &str) -> crate::Result<String> {
    let mut out = String::new();
    for ev in events {
        let line = serde_json::to_string(&SiemEvent::from_audit(ev, exported_at))?;
        out.push_str(&line);
        out.push('\n');
    }
    assert_export_body_no_secrets(&out)?;
    Ok(out)
}

/// Detail keys (lowercase substring match) whose values are always redacted.
/// Covers credential material and bearer tokens by name; `*_key` suffix is
/// matched separately so `seal_key` / `private_key` / `session_key` are caught.
const SENSITIVE_KEY_MARKERS: &[&str] = &[
    "credential",
    "password",
    "passwd",
    "secret",
    "token",
    "authorization",
    "bearer",
    "api_key",
    "apikey",
    "api-key",
];

fn sensitive_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    k == "key" || k.ends_with("_key") || SENSITIVE_KEY_MARKERS.iter().any(|m| k.contains(m))
}

/// Credential locator prefixes — same rule as the store's write-time
/// sanitizer, applied again at export so raw seeded lines cannot leak refs.
const LOCATOR_PREFIXES: &[&str] = &["phm:", "env:", "test:"];

fn redact_string(s: &str) -> bool {
    if LOCATOR_PREFIXES.iter().any(|p| s.contains(p)) || s.contains("AKIA") {
        return true;
    }
    let lower = s.to_ascii_lowercase();
    SECRET_VALUE_MARKERS.iter().any(|m| lower.contains(m))
}

/// Deep-redact an audit detail value for SIEM output.
///
/// - Objects: credential-shaped keys ([`sensitive_key`]) keep a redaction
///   marker — `{name}_redacted: "<redacted>"` — so the stripped field is
///   visible without emitting the forbidden key literal; other values recurse.
/// - Strings carrying a locator prefix (`phm:` / `env:` / `test:`), a known
///   token prefix, or an `AKIA` marker become `"<redacted>"`.
/// - Numbers, bools and null pass through.
pub fn redact_siem_value(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| {
                    if sensitive_key(k) {
                        (format!("{k}_redacted"), Value::String(REDACTED.into()))
                    } else {
                        (k.clone(), redact_siem_value(v))
                    }
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(redact_siem_value).collect()),
        Value::String(s) if redact_string(s) => Value::String(REDACTED.into()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev(ts: &str, op: &str, binding: &str, detail: Option<Value>) -> AuditEvent {
        AuditEvent {
            ts: ts.into(),
            op: op.into(),
            binding: binding.into(),
            detail,
        }
    }

    fn sample() -> Vec<AuditEvent> {
        vec![
            ev(
                "2026-08-09T12:00:00Z",
                "session.pin",
                "acme",
                Some(json!({"session_id": "ses_1"})),
            ),
            ev(
                "2026-08-09T13:00:00Z",
                "mcp.scope_freeze",
                "acme",
                Some(json!({"tool": "supabase.scope", "args_digest": "abc"})),
            ),
            ev("2026-08-09T14:00:00Z", "binding.save", "personal", None),
        ]
    }

    #[test]
    fn parse_since_accepts_rfc3339_offsets_and_dates() {
        let z = parse_since("2026-08-09T12:00:00Z").unwrap();
        assert_eq!(z.to_rfc3339(), "2026-08-09T12:00:00+00:00");
        // A positive offset normalizes into UTC.
        let off = parse_since("2026-08-09T14:30:00+02:30").unwrap();
        assert_eq!(off.to_rfc3339(), "2026-08-09T12:00:00+00:00");
        // Bare date = start of day UTC.
        let day = parse_since(" 2026-08-09 ").unwrap();
        assert_eq!(day.to_rfc3339(), "2026-08-09T00:00:00+00:00");
        for bad in ["", "now", "2026-13-40", "12:00:00", "2026/08/09"] {
            assert!(parse_since(bad).is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn events_since_is_inclusive_and_drops_unparseable_ts() {
        let mut events = sample();
        events.push(ev("not-a-date", "audit.corrupt", "acme", None));
        let since = parse_since("2026-08-09T13:00:00Z").unwrap();
        let kept = events_since(&events, &since);
        assert_eq!(kept.len(), 2, "boundary event is inclusive");
        assert_eq!(kept[0].op, "mcp.scope_freeze");
        assert_eq!(kept[1].op, "binding.save");
        let epoch = parse_since("1970-01-01").unwrap();
        assert_eq!(events_since(&events, &epoch).len(), 3);
    }

    #[test]
    fn export_lines_parse_with_stable_fields() {
        let body = export_siem_ndjson(&sample(), &SiemExportOptions::default()).unwrap();
        let lines: Vec<&str> = body.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 3);
        for line in &lines {
            let v: Value = serde_json::from_str(line).expect("each line is a JSON object");
            assert_eq!(v["schema"], SIEM_SCHEMA);
            assert_eq!(v["kind"], "audit");
            for key in [
                "schema",
                "ts",
                "op",
                "binding",
                "severity",
                "kind",
                "detail",
                "exported_at",
                "locus_version",
            ] {
                assert!(v.get(key).is_some(), "missing stable field {key}: {line}");
            }
        }
        let parsed: Vec<SiemEvent> = lines
            .iter()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(parsed[1].severity, "WARN");
        assert_eq!(parsed[0].severity, "INFO");
        assert_eq!(parsed[2].detail, Value::Null);
    }

    #[test]
    fn export_since_last_op_and_binding_filters() {
        let all = export_siem_ndjson(
            &sample(),
            &SiemExportOptions {
                since: Some("2026-08-09T13:00:00Z".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(all.lines().filter(|l| !l.is_empty()).count(), 2);

        let one = export_siem_ndjson(
            &sample(),
            &SiemExportOptions {
                since: Some("2026-08-09".into()),
                last: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
        let v: Value = serde_json::from_str(one.lines().next().unwrap()).unwrap();
        assert_eq!(v["op"], "binding.save");

        let scoped = export_siem_ndjson(
            &sample(),
            &SiemExportOptions {
                op: Some("scope_freeze".into()),
                binding: Some("acme".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(scoped.lines().filter(|l| !l.is_empty()).count(), 1);

        assert!(export_siem_ndjson(
            &sample(),
            &SiemExportOptions {
                since: Some("garbage".into()),
                ..Default::default()
            },
        )
        .is_err());
    }

    /// Plainly fake fixture marker — nothing key-shaped is embedded in
    /// source; sensitive strings are assembled at runtime from the shared
    /// marker list (see `crate::events::SECRET_VALUE_MARKERS`).
    const CANARY: &str = "synthetic-siem-canary";

    #[test]
    fn seeded_credential_material_is_redacted_recursively() {
        // Every forbidden value marker glued to the canary: covers the whole
        // marker list plus the AKIA check and credential locators without
        // spelling one complete token in source.
        let leaks: Vec<Value> = SECRET_VALUE_MARKERS
            .iter()
            .map(|m| json!(format!("pasted {m}{CANARY}")))
            .chain([
                json!(format!("AKIA{CANARY}")),
                json!(format!("--ref=env:{CANARY}")),
                json!(format!("prefix test:{CANARY}")),
            ])
            .collect();
        let events = vec![ev(
            "2026-08-09T12:00:00Z",
            "probe",
            "acme",
            Some(json!({
                "credential_ref": format!("phm:{CANARY}"),
                "api_key": CANARY,
                "note": format!("see {}", leaks[0].as_str().unwrap()),
                "aws": format!("AKIA{CANARY}"),
                "session_key": CANARY,
                "nested": [
                    {"access_token": CANARY},
                    format!("--ref=env:{CANARY}"),
                ],
                "leaks": leaks,
                "safe": {"session_id": "ses_1", "args_digest": "sha256:abc"},
            })),
        )];
        let body = export_siem_ndjson(&events, &SiemExportOptions::default()).unwrap();
        assert!(!body.contains(CANARY), "seeded credential leaked: {body}");
        let v: Value = serde_json::from_str(body.lines().next().unwrap()).unwrap();
        // Safe fields survive; credential-shaped fields leave a named marker.
        assert_eq!(v["detail"]["safe"]["session_id"], "ses_1");
        assert_eq!(v["detail"]["api_key_redacted"], REDACTED);
        assert_eq!(v["detail"]["note"], REDACTED);
        assert_eq!(v["detail"]["aws"], REDACTED);
        assert!(
            v["detail"]["leaks"]
                .as_array()
                .unwrap()
                .iter()
                .all(|l| l.as_str() == Some(REDACTED)),
            "every marker-prefixed leak is redacted: {}",
            v["detail"]["leaks"]
        );
        assert!(v["detail"].get("api_key").is_none());
    }

    #[test]
    fn export_fails_closed_on_secret_shaped_envelope_fields() {
        // Redaction covers `detail`; a marker in an envelope field (binding
        // alias here) must still be caught by the final body scan.
        let marker = SECRET_VALUE_MARKERS
            .iter()
            .find(|m| m.starts_with("sk_"))
            .copied()
            .unwrap();
        let events = vec![ev(
            "2026-08-09T12:00:00Z",
            "probe",
            &format!("{marker}bindingalias"),
            None,
        )];
        assert!(export_siem_ndjson(&events, &SiemExportOptions::default()).is_err());
    }

    #[test]
    fn redact_siem_value_keeps_scalars_and_digests() {
        let v = redact_siem_value(&json!({
            "count": 3,
            "ok": true,
            "missing": null,
            "args_digest": "sha256:9f2c",
            "key": CANARY,
        }));
        assert_eq!(v["count"], 3);
        assert_eq!(v["ok"], true);
        assert_eq!(v["missing"], Value::Null);
        assert_eq!(v["args_digest"], "sha256:9f2c");
        assert_eq!(v["key_redacted"], REDACTED);
        assert!(v.get("key").is_none());
    }
}

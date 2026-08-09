//! Audit event export formats — JSON lines (fleet pulse) and OTLP-compatible logs.
//!
//! ```text
//! locus events export [--otlp] [--last N] [--binding acme] [--out file]
//! ```
//!
//! Never includes resolved secrets — only audit ops, digests, and metadata
//! already present in `$LOCUS_HOME/audit/events.jsonl`.

use crate::doctor::filter_audit_events;
use crate::store::AuditEvent;
use crate::VERSION;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Export format for audit events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EventsExportFormat {
    /// One JSON object per line (fleet / SIEM friendly).
    #[default]
    JsonLines,
    /// OTLP-compatible JSON body for Logs (partial Success-compatible shape).
    Otlp,
}

/// Options for [`export_events`].
#[derive(Debug, Clone, Default)]
pub struct EventsExportOptions {
    pub last: Option<usize>,
    pub op: Option<String>,
    pub binding: Option<String>,
    pub format: EventsExportFormat,
    /// Service name attribute (default: `locus`).
    pub service_name: Option<String>,
}

/// One fleet-pulse JSON line (envelope around an audit event).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FleetPulseEvent {
    /// Schema id for consumers.
    pub schema: String,
    pub locus_version: String,
    pub exported_at: String,
    pub ts: String,
    pub op: String,
    pub binding: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
    /// Stable kind for dashboards: `audit`.
    pub kind: String,
}

impl FleetPulseEvent {
    pub const SCHEMA: &'static str = "locus.audit.v1";

    pub fn from_audit(ev: &AuditEvent, exported_at: &str) -> Self {
        Self {
            schema: Self::SCHEMA.into(),
            locus_version: VERSION.to_string(),
            exported_at: exported_at.into(),
            ts: ev.ts.clone(),
            op: ev.op.clone(),
            binding: ev.binding.clone(),
            detail: ev.detail.clone(),
            kind: "audit".into(),
        }
    }
}

/// Filter + format audit events for export.
pub fn export_events(events: &[AuditEvent], opts: &EventsExportOptions) -> crate::Result<String> {
    let last = opts.last.unwrap_or(200).max(1);
    let filtered = filter_audit_events(events, last, opts.op.as_deref(), opts.binding.as_deref());
    let exported_at = Utc::now().to_rfc3339();
    match opts.format {
        EventsExportFormat::JsonLines => Ok(to_json_lines(&filtered, &exported_at)),
        EventsExportFormat::Otlp => Ok(to_otlp_json(
            &filtered,
            &exported_at,
            opts.service_name.as_deref().unwrap_or("locus"),
        )),
    }
}

fn to_json_lines(events: &[AuditEvent], exported_at: &str) -> String {
    let mut out = String::new();
    for ev in events {
        let pulse = FleetPulseEvent::from_audit(ev, exported_at);
        if let Ok(line) = serde_json::to_string(&pulse) {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

/// Build an OTLP JSON `resourceLogs` document (Logs data model, export request body).
///
/// Compatible with collectors that accept OTLP/HTTP JSON for logs. Does not
/// perform network I/O — callers POST the body themselves if desired.
fn to_otlp_json(events: &[AuditEvent], exported_at: &str, service_name: &str) -> String {
    let log_records: Vec<Value> = events
        .iter()
        .map(|ev| {
            let time_unix_nano = rfc3339_to_unix_nano(&ev.ts).unwrap_or(0);
            let body = format!("{} binding={}", ev.op, ev.binding);
            let mut attrs = vec![
                otlp_attr_str("locus.op", &ev.op),
                otlp_attr_str("locus.binding", &ev.binding),
                otlp_attr_str("locus.kind", "audit"),
                otlp_attr_str("service.name", service_name),
            ];
            if let Some(ref detail) = ev.detail {
                if let Ok(s) = serde_json::to_string(detail) {
                    // Truncate very large details to keep export bounded.
                    let clipped = if s.len() > 2048 {
                        format!("{}…", &s[..2048])
                    } else {
                        s
                    };
                    attrs.push(otlp_attr_str("locus.detail", &clipped));
                }
            }
            json!({
                "timeUnixNano": time_unix_nano.to_string(),
                "observedTimeUnixNano": rfc3339_to_unix_nano(exported_at)
                    .unwrap_or(time_unix_nano)
                    .to_string(),
                "severityNumber": severity_for_op(&ev.op),
                "severityText": severity_text_for_op(&ev.op),
                "body": { "stringValue": body },
                "attributes": attrs,
            })
        })
        .collect();

    let doc = json!({
        "resourceLogs": [{
            "resource": {
                "attributes": [
                    otlp_attr_str("service.name", service_name),
                    otlp_attr_str("service.version", VERSION),
                    otlp_attr_str("locus.export", "events"),
                ]
            },
            "scopeLogs": [{
                "scope": {
                    "name": "locus.audit",
                    "version": VERSION,
                },
                "logRecords": log_records,
            }]
        }]
    });
    serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".into())
}

fn otlp_attr_str(key: &str, value: &str) -> Value {
    json!({
        "key": key,
        "value": { "stringValue": value }
    })
}

fn severity_for_op(op: &str) -> u8 {
    // OTLP severity numbers: INFO=9, WARN=13, ERROR=17
    if op.contains("scope_freeze")
        || op.contains("deny")
        || op.contains("freeze")
        || op.contains("require_approval")
    {
        13
    } else if op.contains("error") || op.contains("fail") {
        17
    } else {
        9
    }
}

fn severity_text_for_op(op: &str) -> &'static str {
    match severity_for_op(op) {
        17 => "ERROR",
        13 => "WARN",
        _ => "INFO",
    }
}

fn rfc3339_to_unix_nano(ts: &str) -> Option<u64> {
    let dt = chrono::DateTime::parse_from_rfc3339(ts).ok()?;
    let secs = dt.timestamp();
    let nsecs = dt.timestamp_subsec_nanos();
    if secs < 0 {
        return None;
    }
    Some(
        (secs as u64)
            .saturating_mul(1_000_000_000)
            .saturating_add(u64::from(nsecs)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_events() -> Vec<AuditEvent> {
        vec![
            AuditEvent {
                ts: "2026-08-09T12:00:00Z".into(),
                op: "session.pin".into(),
                binding: "acme".into(),
                detail: Some(json!({"session_id": "ses_1"})),
            },
            AuditEvent {
                ts: "2026-08-09T12:01:00Z".into(),
                op: "mcp.scope_freeze".into(),
                binding: "acme".into(),
                detail: Some(json!({"tool": "supabase.scope", "args_digest": "abc"})),
            },
            AuditEvent {
                ts: "2026-08-09T12:02:00Z".into(),
                op: "pin".into(),
                binding: "personal".into(),
                detail: None,
            },
        ]
    }

    #[test]
    fn json_lines_fleet_pulse() {
        let out = export_events(
            &sample_events(),
            &EventsExportOptions {
                last: Some(10),
                format: EventsExportFormat::JsonLines,
                ..Default::default()
            },
        )
        .unwrap();
        let lines: Vec<&str> = out.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 3);
        for line in &lines {
            let v: Value = serde_json::from_str(line).unwrap();
            assert_eq!(v["schema"], FleetPulseEvent::SCHEMA);
            assert_eq!(v["kind"], "audit");
            assert!(v.get("op").is_some());
            assert!(v.get("binding").is_some());
            // No secret-looking keys in envelope
            assert!(v.get("token").is_none());
        }
        // Filter binding
        let out2 = export_events(
            &sample_events(),
            &EventsExportOptions {
                last: Some(10),
                binding: Some("acme".into()),
                format: EventsExportFormat::JsonLines,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(out2.lines().filter(|l| !l.is_empty()).count(), 2);
    }

    #[test]
    fn otlp_has_resource_logs() {
        let out = export_events(
            &sample_events(),
            &EventsExportOptions {
                last: Some(10),
                format: EventsExportFormat::Otlp,
                service_name: Some("locus-test".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        let records = &v["resourceLogs"][0]["scopeLogs"][0]["logRecords"];
        assert_eq!(records.as_array().unwrap().len(), 3);
        let first = &records[0];
        assert!(first.get("timeUnixNano").is_some());
        assert!(first.get("body").is_some());
        assert!(first.get("attributes").is_some());
        // service.name on resource
        let res_attrs = v["resourceLogs"][0]["resource"]["attributes"]
            .as_array()
            .unwrap();
        assert!(res_attrs
            .iter()
            .any(|a| { a["key"] == "service.name" && a["value"]["stringValue"] == "locus-test" }));
        // scope_freeze is WARN severity
        let freeze = records
            .as_array()
            .unwrap()
            .iter()
            .find(|r| {
                r["attributes"].as_array().unwrap().iter().any(|a| {
                    a["key"] == "locus.op" && a["value"]["stringValue"] == "mcp.scope_freeze"
                })
            })
            .unwrap();
        assert_eq!(freeze["severityNumber"], 13);
        assert_eq!(freeze["severityText"], "WARN");
    }

    #[test]
    fn last_n_trims() {
        let out = export_events(
            &sample_events(),
            &EventsExportOptions {
                last: Some(1),
                format: EventsExportFormat::JsonLines,
                ..Default::default()
            },
        )
        .unwrap();
        let lines: Vec<&str> = out.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 1);
        let v: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(v["binding"], "personal");
    }
}

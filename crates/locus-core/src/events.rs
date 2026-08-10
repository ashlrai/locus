//! Audit event export formats — JSON lines (fleet pulse) and OTLP-compatible logs.
//!
//! ```text
//! locus events export [--otlp] [--last N] [--binding acme] [--out file]
//! locus events export --sink webhook [--url URL]   # or LOCUS_AUDIT_WEBHOOK_URL
//! ```
//!
//! Never includes resolved secrets — only audit ops, digests, and metadata
//! already present in `$LOCUS_HOME/audit/events.jsonl`.
//!
//! Optional **webhook sink** (team SIEM primitive): POST the same redacted body
//! to a remote URL. Unset URL fails soft (no network). Bodies that look like
//! they contain secrets fail closed (refuse to POST).

use crate::doctor::filter_audit_events;
use crate::store::AuditEvent;
use crate::VERSION;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

/// Env var for optional audit webhook URL (`locus events export --sink webhook`).
pub const AUDIT_WEBHOOK_URL_ENV: &str = "LOCUS_AUDIT_WEBHOOK_URL";

/// Default HTTP timeout for webhook POSTs.
pub const AUDIT_WEBHOOK_TIMEOUT_SECS: u64 = 15;

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

/// Where to send an events export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EventsExportSink {
    /// Stdout or `--out` file (default).
    #[default]
    Local,
    /// HTTP(S) POST of the redacted export body.
    Webhook,
}

/// Result of a successful webhook POST (no secret material).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookPostResult {
    pub status: u16,
    pub bytes: usize,
    /// Host:port only — never full URL (may embed tokens in query).
    pub host: String,
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

/// Content-Type for an export body (for webhook POST or file handoff).
pub fn export_content_type(format: EventsExportFormat) -> &'static str {
    match format {
        EventsExportFormat::JsonLines => "application/x-ndjson",
        EventsExportFormat::Otlp => "application/json",
    }
}

/// Resolve webhook URL: explicit CLI arg wins, else [`AUDIT_WEBHOOK_URL_ENV`].
///
/// Returns `None` when unset/blank — callers must **fail soft** (skip POST, exit 0).
pub fn resolve_audit_webhook_url(explicit: Option<&str>) -> Option<String> {
    if let Some(u) = explicit {
        let t = u.trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }
    std::env::var(AUDIT_WEBHOOK_URL_ENV)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Safe label for logs: scheme + host[:port] only (strip userinfo / path / query).
pub fn webhook_url_safe_label(url: &str) -> String {
    let url = url.trim();
    let (scheme, rest) = match url.split_once("://") {
        Some((s, r)) => (s, r),
        None => return "invalid-url".into(),
    };
    let after_auth = rest.rsplit('@').next().unwrap_or(rest);
    let hostport = after_auth
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_auth);
    if hostport.is_empty() {
        return format!("{scheme}://");
    }
    format!("{scheme}://{hostport}")
}

/// Fail-closed secret scan on an export body. Empty issues = safe to ship.
///
/// Scans for known token prefixes and JSON keys that must never hold plaintext
/// secrets in audit exports. Digests / aliases / CredentialRef **names** are fine.
pub fn export_body_secret_issues(body: &str) -> Vec<String> {
    let mut issues = Vec::new();
    let lower = body.to_ascii_lowercase();

    for pat in [
        "sk_live_",
        "sk_test_",
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "github_pat_",
        "xoxb-",
        "xoxp-",
        "xoxa-",
        "xoxs-",
        "-----begin private",
        "-----begin rsa private",
        "-----begin openssh private",
        "-----begin ec private",
    ] {
        if lower.contains(pat) {
            issues.push(format!("forbidden secret pattern: {pat}"));
        }
    }

    if body.contains("AKIA") {
        issues.push("forbidden secret pattern: AKIA".into());
    }

    for key in [
        "\"password\"",
        "\"api_key\"",
        "\"apikey\"",
        "\"secret_key\"",
        "\"private_key\"",
        "\"access_token\"",
        "\"refresh_token\"",
        "\"authorization\"",
        "\"client_secret\"",
        "\"aws_secret_access_key\"",
    ] {
        if lower.contains(key) {
            issues.push(format!("forbidden secret field: {key}"));
        }
    }

    issues
}

/// Return `Ok(())` only if the body passes the secret scan (fail closed).
pub fn assert_export_body_no_secrets(body: &str) -> crate::Result<()> {
    let issues = export_body_secret_issues(body);
    if issues.is_empty() {
        Ok(())
    } else {
        Err(crate::LocusError::msg(format!(
            "refusing audit export: body failed secret scan ({})",
            issues.join("; ")
        )))
    }
}

/// POST a redacted audit export body to a webhook URL.
///
/// - Runs [`assert_export_body_no_secrets`] first (fail closed).
/// - Never logs the full URL (query may hold tokens).
/// - Expects HTTP 2xx; other statuses are errors.
pub fn post_audit_webhook(
    url: &str,
    body: &str,
    content_type: &str,
) -> crate::Result<WebhookPostResult> {
    assert_export_body_no_secrets(body)?;

    let host = webhook_url_safe_label(url);
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(crate::LocusError::msg(format!(
            "webhook URL must be http(s):// ({host})"
        )));
    }

    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(AUDIT_WEBHOOK_TIMEOUT_SECS))
        .user_agent(&format!("locus/{VERSION}"))
        .build();

    let resp = agent
        .post(url)
        .set("Content-Type", content_type)
        .set("X-Locus-Export", "audit")
        .send_string(body)
        .map_err(|e| crate::LocusError::msg(format!("webhook post to {host} failed: {e}")))?;

    let status = resp.status();
    if !(200..300).contains(&status) {
        return Err(crate::LocusError::msg(format!(
            "webhook post to {host} returned HTTP {status}"
        )));
    }

    Ok(WebhookPostResult {
        status,
        bytes: body.len(),
        host,
    })
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
            assert!(v.get("token").is_none());
        }
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
        let res_attrs = v["resourceLogs"][0]["resource"]["attributes"]
            .as_array()
            .unwrap();
        assert!(res_attrs
            .iter()
            .any(|a| { a["key"] == "service.name" && a["value"]["stringValue"] == "locus-test" }));
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

    #[test]
    fn resolve_webhook_url_explicit_and_env() {
        assert_eq!(
            resolve_audit_webhook_url(Some(" http://example.test/hook ")).as_deref(),
            Some("http://example.test/hook")
        );
        assert_eq!(resolve_audit_webhook_url(Some("  ")), None);
        assert_eq!(resolve_audit_webhook_url(Some("")), None);

        let prev = std::env::var_os(AUDIT_WEBHOOK_URL_ENV);
        std::env::remove_var(AUDIT_WEBHOOK_URL_ENV);
        assert_eq!(resolve_audit_webhook_url(None), None);
        std::env::set_var(AUDIT_WEBHOOK_URL_ENV, "https://siem.example/ingest");
        assert_eq!(
            resolve_audit_webhook_url(None).as_deref(),
            Some("https://siem.example/ingest")
        );
        std::env::set_var(AUDIT_WEBHOOK_URL_ENV, "   ");
        assert_eq!(resolve_audit_webhook_url(None), None);
        match prev {
            Some(v) => std::env::set_var(AUDIT_WEBHOOK_URL_ENV, v),
            None => std::env::remove_var(AUDIT_WEBHOOK_URL_ENV),
        }
    }

    #[test]
    fn webhook_url_safe_label_strips_secrets() {
        assert_eq!(
            webhook_url_safe_label("https://user:token@hooks.example.com/v1?key=secret"),
            "https://hooks.example.com"
        );
        assert_eq!(
            webhook_url_safe_label("http://127.0.0.1:9999/path"),
            "http://127.0.0.1:9999"
        );
    }

    #[test]
    fn export_body_secret_scan_fail_closed() {
        assert!(export_body_secret_issues(r#"{"op":"pin","binding":"a"}"#).is_empty());
        let clean = export_events(
            &sample_events(),
            &EventsExportOptions {
                last: Some(10),
                format: EventsExportFormat::JsonLines,
                ..Default::default()
            },
        )
        .unwrap();
        assert_export_body_no_secrets(&clean).expect("fleet pulse is clean");

        let dirty = r#"{"op":"x","detail":{"token":"sk_live_abc123SECRET"}}"#;
        let issues = export_body_secret_issues(dirty);
        assert!(
            issues.iter().any(|i| i.contains("sk_live_")),
            "expected sk_live_ issue, got {issues:?}"
        );
        assert!(assert_export_body_no_secrets(dirty).is_err());

        let keyed = r#"{"password":"hunter2","op":"x"}"#;
        assert!(assert_export_body_no_secrets(keyed).is_err());
    }

    #[test]
    fn webhook_post_refuses_secret_body_without_network() {
        let err = post_audit_webhook(
            "http://127.0.0.1:1/unused",
            r#"{"api_key":"supersecret"}"#,
            "application/json",
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("secret scan") || msg.contains("api_key"),
            "unexpected err: {msg}"
        );
    }

    #[test]
    fn webhook_posts_to_local_listener() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buf = vec![0u8; 65536];
            let mut total = 0usize;
            loop {
                let n = stream.read(&mut buf[total..]).expect("read");
                if n == 0 {
                    break;
                }
                total += n;
                let so_far = String::from_utf8_lossy(&buf[..total]);
                if so_far.contains("\r\n\r\n") {
                    if let Some(cl) = so_far
                        .lines()
                        .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
                    {
                        let len: usize = cl
                            .split(':')
                            .nth(1)
                            .and_then(|s| s.trim().parse().ok())
                            .unwrap_or(0);
                        if let Some(pos) = so_far.find("\r\n\r\n") {
                            let body_start = pos + 4;
                            if total.saturating_sub(body_start) >= len {
                                break;
                            }
                        }
                    } else {
                        break;
                    }
                }
                if total >= buf.len() {
                    break;
                }
            }
            let req = String::from_utf8_lossy(&buf[..total]).to_string();
            let _ = stream.write_all(
                b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            req
        });

        let body = export_events(
            &sample_events(),
            &EventsExportOptions {
                last: Some(10),
                format: EventsExportFormat::JsonLines,
                ..Default::default()
            },
        )
        .unwrap();
        assert_export_body_no_secrets(&body).unwrap();

        let url = format!("http://{addr}/locus/audit");
        let result = post_audit_webhook(
            &url,
            &body,
            export_content_type(EventsExportFormat::JsonLines),
        )
        .expect("post");
        assert_eq!(result.status, 204);
        assert_eq!(result.bytes, body.len());
        assert!(result.host.contains("127.0.0.1"));

        let req = server.join().expect("server");
        assert!(req.starts_with("POST "), "method: {req}");
        assert!(
            req.to_ascii_lowercase()
                .contains("content-type: application/x-ndjson"),
            "content-type missing: {req}"
        );
        assert!(req.contains("locus.audit.v1"), "body missing schema: {req}");
        assert!(req.contains("session.pin"), "body missing op: {req}");
        assert!(!req.to_ascii_lowercase().contains("sk_live_"));
        assert!(!req.to_ascii_lowercase().contains("\"password\""));
    }

    #[test]
    fn content_type_for_formats() {
        assert_eq!(
            export_content_type(EventsExportFormat::JsonLines),
            "application/x-ndjson"
        );
        assert_eq!(
            export_content_type(EventsExportFormat::Otlp),
            "application/json"
        );
    }
}

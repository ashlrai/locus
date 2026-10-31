//! Wire contract for the versioned locus event schema (events-schema bet).
//! Validates events actually emitted by `locus events export` so Phantom's
//! router can rely on `schema/locus-events.v1.schema.json` for structure.

use locus_core::events::{export_events, EventsExportFormat, EventsExportOptions, FleetPulseEvent};
use locus_core::store::AuditEvent;
use serde_json::{json, Value};

fn schema(fragment: &str) -> jsonschema::Validator {
    let mut schema: Value =
        serde_json::from_str(include_str!("../../../schema/locus-events.v1.schema.json")).unwrap();
    if !fragment.is_empty() {
        // Keep local definitions in scope without fetching the published URI.
        schema["$ref"] = json!(format!("#/$defs/{fragment}"));
        schema.as_object_mut().unwrap().remove("oneOf");
    }
    jsonschema::options()
        .should_validate_formats(true)
        .build(&schema)
        .unwrap()
}

fn audit_events() -> Vec<AuditEvent> {
    vec![
        AuditEvent {
            ts: "2026-10-27T12:00:00Z".into(),
            op: "session.pin".into(),
            binding: "acme".into(),
            detail: Some(json!({"session_id": "ses_1"})),
        },
        AuditEvent {
            ts: "2026-10-27T12:01:00Z".into(),
            op: "mcp.scope_freeze".into(),
            binding: "acme".into(),
            detail: Some(json!({"tool": "supabase.scope", "args_digest": "abc123"})),
        },
        AuditEvent {
            ts: "2026-10-27T12:02:00Z".into(),
            op: "pin".into(),
            binding: "personal".into(),
            detail: None,
        },
        AuditEvent {
            ts: "2026-10-27T12:03:00Z".into(),
            op: "diagnostic".into(),
            binding: String::new(), // global op: empty binding is legal
            detail: Some(json!({})),
        },
    ]
}

fn emitted_lines() -> Vec<Value> {
    let out = export_events(
        &audit_events(),
        &EventsExportOptions {
            last: Some(50),
            format: EventsExportFormat::JsonLines,
            ..Default::default()
        },
    )
    .unwrap();
    out.lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn emitted_audit_lines_validate() {
    let lines = emitted_lines();
    assert_eq!(lines.len(), 4);
    let any = schema("");
    let audit = schema("audit");
    for line in &lines {
        assert!(audit.is_valid(line), "{line}");
        assert!(any.is_valid(line), "{line}");
        assert_eq!(line["schema"], "locus.audit.v1");
        assert_eq!(line["kind"], "audit");
    }
    // Serialized struct directly (no export round-trip) also conforms.
    let pulse = serde_json::to_value(FleetPulseEvent::from_audit(
        &audit_events()[0],
        "2026-10-27T12:05:00Z",
    ))
    .unwrap();
    assert!(audit.is_valid(&pulse), "{pulse}");
}

#[test]
fn router_dispatch_is_unambiguous() {
    // Every valid event matches exactly one $defs member.
    for line in emitted_lines() {
        let hits = ["audit", "sessionTick", "watchTick"]
            .iter()
            .filter(|def| schema(def).is_valid(&line))
            .count();
        assert_eq!(hits, 1, "{line}");
    }
    // Heartbeat kinds do not collide with the audit envelope.
    assert!(!schema("audit").is_valid(&json!({
        "kind": "locus.session_tick", "session_ok": true, "whoami": "acme",
        "doctor_verdict": "SAFE", "safe_next": "ready", "pinned": true,
        "frozen": false, "version": "0.5.2"
    })));
    assert!(!schema("audit").is_valid(&json!({
        "kind": "watch", "session_ok": false, "doctor_verdict": "UNSAFE",
        "safe_next": "enter", "pinned": false, "frozen": false
    })));
}

#[test]
fn audit_envelope_fails_closed() {
    let good = emitted_lines().remove(0);
    let audit = schema("audit");
    // Discriminants are constants.
    for (key, bad) in [
        ("schema", json!("locus.audit.v0")),
        ("schema", json!("locus.audit.v2")),
        ("kind", json!("watch")),
    ] {
        let mut ev = good.clone();
        ev[key] = bad.clone();
        assert!(!audit.is_valid(&ev), "accepted {key}={bad}");
        assert!(!schema("").is_valid(&ev), "oneOf accepted {key}={bad}");
    }
    // Required fields cannot be dropped or mistyped.
    for key in [
        "schema",
        "locus_version",
        "exported_at",
        "ts",
        "op",
        "binding",
        "kind",
    ] {
        let mut missing = good.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert!(!audit.is_valid(&missing), "accepted missing {key}");
        let mut wrong = good.clone();
        wrong[key] = json!(7);
        assert!(!audit.is_valid(&wrong), "accepted numeric {key}");
    }
    // The exporter supplies an RFC 3339 export timestamp.
    let mut ev = good.clone();
    ev["exported_at"] = json!("tomorrow");
    assert!(!audit.is_valid(&ev));
    // Unknown envelope keys are rejected.
    let mut ev = good.clone();
    ev["token"] = json!("secret");
    assert!(!audit.is_valid(&ev), "accepted unknown key");
}

#[test]
fn audit_schema_accepts_exported_arbitrary_detail_and_record_strings() {
    let audit = schema("audit");
    for detail in [
        json!("synthetic note"),
        json!([1, "note"]),
        json!({"note": "example"}),
    ] {
        let event = AuditEvent {
            ts: "legacy timestamp".into(),
            op: "Legacy Operation".into(),
            binding: "acme".into(),
            detail: Some(detail.clone()),
        };
        let output = export_events(&[event], &EventsExportOptions::default()).unwrap();
        let value: Value = serde_json::from_str(output.trim()).unwrap();
        assert_eq!(value["detail"], detail);
        assert!(audit.is_valid(&value), "{value}");
        assert!(schema("").is_valid(&value), "{value}");
    }
}

#[test]
fn audit_schema_matches_documented_example() {
    let docs = include_str!("../../../docs/observability.md");
    let example = docs
        .split("One envelope per audit event")
        .nth(1)
        .unwrap()
        .split("```json\n")
        .nth(1)
        .unwrap()
        .split("\n```")
        .next()
        .unwrap();
    let value: Value = serde_json::from_str(example).unwrap();
    assert!(schema("audit").is_valid(&value), "{value}");
    assert!(schema("").is_valid(&value), "{value}");
}

#[test]
fn session_tick_schema_matches_documentation_and_scope_dependencies() {
    let docs = include_str!("../../../docs/mcp.md");
    let example = docs
        .split("Each tick is a JSON-RPC `notifications/message` with values-free `data`:")
        .nth(1)
        .unwrap()
        .split("```json\n")
        .nth(1)
        .unwrap()
        .split("\n```")
        .next()
        .unwrap();
    let operator: Value = serde_json::from_str(example).unwrap();
    let tick = schema("sessionTick");
    assert!(tick.is_valid(&operator), "{operator}");
    assert!(schema("").is_valid(&operator), "{operator}");

    let mut tenant = operator.clone();
    tenant["scope"] = json!("tenant");
    tenant["safe_next"] = json!("proceed");
    tenant["grant_id"] = json!("0123456789abcdef");
    tenant["issues"] = json!([]);
    assert!(tick.is_valid(&tenant), "{tenant}");
    assert!(schema("").is_valid(&tenant), "{tenant}");
    for key in ["scope", "grant_id", "issues"] {
        let mut missing = tenant.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert!(!tick.is_valid(&missing), "accepted missing {key}: {missing}");
    }
    let mut fallback = operator;
    fallback["whoami"] = Value::Null;
    fallback["session_ok"] = json!(false);
    fallback["doctor_verdict"] = json!("UNSAFE");
    fallback["safe_next"] = json!("init");
    fallback["pinned"] = json!(false);
    fallback["hint"] = json!("store unavailable under LOCUS_HOME");
    assert!(tick.is_valid(&fallback), "{fallback}");
}

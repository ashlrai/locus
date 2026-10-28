//! Wire contract for the versioned locus event schema (events-schema bet).
//! Validates events actually emitted by `locus events export` so Phantom's
//! router can rely on `schema/locus-events.v1.schema.json`. Values-free only.

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
    // Timestamps must be RFC 3339.
    for key in ["exported_at", "ts"] {
        let mut ev = good.clone();
        ev[key] = json!("tomorrow");
        assert!(!audit.is_valid(&ev), "accepted bad {key}");
    }
    // detail must be an object; unknown keys are rejected.
    let mut ev = good.clone();
    ev["detail"] = json!("freeform");
    assert!(!audit.is_valid(&ev));
    let mut ev = good.clone();
    ev["token"] = json!("secret");
    assert!(!audit.is_valid(&ev), "accepted unknown key");
}

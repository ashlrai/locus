//! Unit tests for the pure halves of `fleet_outcomes`: op classification and
//! per-record result derivation. File scanning is exercised by the CLI itself.
use super::*;
use serde_json::json;

fn event(op: &str, detail: Option<serde_json::Value>) -> AuditEvent {
    AuditEvent {
        ts: "2026-11-22T00:00:00Z".into(),
        op: op.into(),
        binding: "test-binding".into(),
        detail,
    }
}

#[test]
fn kind_maps_exec_and_ci_ops() {
    assert_eq!(kind("session.exec"), Some("exec"));
    assert_eq!(kind("ci.run"), Some("ci"));
}

#[test]
fn kind_matches_ops_exactly() {
    // A looser (prefix/suffix/case-insensitive) match would count unrelated
    // audit records as command outcomes.
    for op in ["session.exec.extra", "ci.run.x", "session", "CI.RUN", ""] {
        assert_eq!(kind(op), None, "op {op:?} must not be an outcome");
    }
}

#[test]
fn result_of_zero_exit_succeeds() {
    let e = event(
        "session.exec",
        Some(json!({"exit": 0, "spawn_error": null})),
    );
    assert_eq!(result_of(&e), (Result::Succeeded, Some(0)));
}

#[test]
fn result_of_nonzero_exit_fails_and_keeps_code() {
    let e = event("ci.run", Some(json!({"exit": 2})));
    assert_eq!(result_of(&e), (Result::Failed, Some(2)));
    let e = event("ci.run", Some(json!({"exit": -9})));
    assert_eq!(result_of(&e), (Result::Failed, Some(-9)));
}

#[test]
fn result_of_spawn_error_is_spawn_failed_even_with_exit_code() {
    // A recorded spawn error takes precedence over any exit value.
    let e = event(
        "session.exec",
        Some(json!({"spawn_error": "ENOENT", "exit": 0})),
    );
    assert_eq!(result_of(&e), (Result::SpawnFailed, Some(0)));
}

#[test]
fn result_of_spawn_error_without_exit_is_spawn_failed() {
    // Error path: the child never ran, so there is no exit code to report.
    let e = event("ci.run", Some(json!({"spawn_error": "EACCES"})));
    assert_eq!(result_of(&e), (Result::SpawnFailed, None));
}

#[test]
fn result_of_missing_or_unreadable_detail_is_unknown_not_guessed() {
    // Old records without detail, a null exit, and a string exit all stay
    // Unknown rather than being coerced into success or failure.
    assert_eq!(result_of(&event("ci.run", None)), (Result::Unknown, None));
    let e = event("ci.run", Some(json!({"exit": null})));
    assert_eq!(result_of(&e), (Result::Unknown, None));
    // A null spawn_error is not a spawn failure.
    let e = event(
        "session.exec",
        Some(json!({"spawn_error": null, "exit": "0"})),
    );
    assert_eq!(result_of(&e), (Result::Unknown, None));
}

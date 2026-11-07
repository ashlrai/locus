use serde_json::{json, Value};
use std::{fs, process::Command};

fn feed(home: &std::path::Path) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_locus"))
        .arg("fleet-outcomes")
        .env("HOME", home)
        .env("LOCUS_HOME", home)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(out.stderr.is_empty());
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn missing_log_is_explicit_and_read_only() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(
        feed(home.path()),
        json!({
            "schema": "locus.fleet_outcomes.v1",
            "log": "missing",
            "outcomes": [],
            "counts": {"succeeded": 0, "failed": 0, "spawn_failed": 0, "unknown": 0},
            "pending_approvals": 0,
        })
    );
    assert!(!home.path().join("audit").exists());
    assert!(!home.path().join("daemon.key").exists());
}

#[test]
fn outcomes_classify_without_leaking_values() {
    let home = tempfile::tempdir().unwrap();
    fs::create_dir(home.path().join("audit")).unwrap();
    let ev = |op: &str, detail: Value| {
        json!({"ts": "secret-ts", "op": op, "binding": "secret-binding", "detail": detail})
            .to_string()
    };
    let lines = [
        "malformed-secret".to_string(),
        ev(
            "session.exec",
            json!({"cmd": ["secret-cmd"], "exit": 0, "spawn_error": null}),
        ),
        ev("exec.deny", json!({})),
        ev(
            "session.exec",
            json!({"cmd": ["x"], "exit": 3, "spawn_error": null}),
        ),
        ev(
            "ci.run",
            json!({"exit": 127, "spawn_error": "secret-error"}),
        ),
        ev("session.exec", json!({"exit": null})),
        ev("ci.run", json!({"exit": 0})),
        ev("session.exec", Value::Null),
    ];
    fs::write(
        home.path().join("audit/events.jsonl"),
        lines.join("\n") + "\n",
    )
    .unwrap();
    let got = feed(home.path());
    assert_eq!(
        got["outcomes"],
        json!([
            {"position": 1, "kind": "exec", "result": "succeeded", "exit": 0},
            {"position": 3, "kind": "exec", "result": "failed", "exit": 3},
            {"position": 4, "kind": "ci", "result": "spawn_failed", "exit": 127},
            {"position": 5, "kind": "exec", "result": "unknown", "exit": null},
            {"position": 6, "kind": "ci", "result": "succeeded", "exit": 0},
            {"position": 7, "kind": "exec", "result": "unknown", "exit": null},
        ])
    );
    assert_eq!(got["log"], "present");
    assert_eq!(
        got["counts"],
        json!({"succeeded": 2, "failed": 1, "spawn_failed": 1, "unknown": 2})
    );
    assert!(!got.to_string().contains("secret"));
}

#[test]
fn only_the_recent_window_and_pending_approvals_are_reported() {
    let home = tempfile::tempdir().unwrap();
    fs::create_dir(home.path().join("audit")).unwrap();
    let line =
        json!({"ts": "t", "op": "session.exec", "binding": "b", "detail": {"exit": 0}}).to_string();
    fs::write(
        home.path().join("audit/events.jsonl"),
        vec![line; 101].join("\n") + "\n",
    )
    .unwrap();
    let rec = |id: &str, status: &str| {
        json!({
            "id": id, "tool": "t", "binding": "b", "args_digest": "d",
            "created_at": "2026-01-01T00:00:00Z", "status": status, "session_id": "s",
        })
        .to_string()
    };
    let dir = home.path().join("approvals");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("a.json"), rec("a", "pending")).unwrap();
    fs::write(dir.join("b.json"), rec("b", "approved")).unwrap();
    fs::write(dir.join("c.json"), "corrupt").unwrap();
    let got = feed(home.path());
    assert_eq!(got["outcomes"].as_array().unwrap().len(), 100);
    assert_eq!(got["outcomes"][0]["position"], 2);
    assert_eq!(got["pending_approvals"], 1);
}

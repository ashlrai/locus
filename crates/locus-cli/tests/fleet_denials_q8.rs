use serde_json::{json, Value};
use std::{fs, process::Command};

fn run(home: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_locus"))
        .arg("fleet-denials")
        .args(args)
        .env("HOME", home)
        .env("LOCUS_HOME", home)
        .output()
        .unwrap()
}

fn feed(home: &std::path::Path, args: &[&str]) -> Value {
    let out = run(home, args);
    assert!(out.status.success());
    assert!(out.stderr.is_empty());
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn empty_log_is_empty() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(
        feed(home.path(), &[]),
        json!({
            "schema": "locus.fleet_denials.v1",
            "denials": [],
            "next_cursor": 0,
            "has_more": false,
        })
    );
    // Read-only: the feed resolved the log without initializing LOCUS_HOME.
    assert!(!home.path().join("audit").exists());
    assert!(!home.path().join("daemon.key").exists());
}

#[test]
fn projection_cursor_and_recent_window_are_value_free() {
    let home = tempfile::tempdir().unwrap();
    fs::create_dir(home.path().join("audit")).unwrap();
    let event = |op| {
        json!({
            "ts": "secret-timestamp",
            "op": op,
            "binding": "secret-binding",
            "detail": {"token": "secret-token"},
        })
        .to_string()
    };
    let mut lines = vec![event("exec.allow"); 101];
    lines[0] = event("mcp.scope_freeze");
    lines[98] = event("mcp.require_approval");
    lines[99] = event("exec.deny");
    lines[100] = event("secret-prefix.denied");
    let path = home.path().join("audit/events.jsonl");
    fs::write(&path, format!("malformed-secret\n{}\n", lines.join("\n"))).unwrap();
    let first = feed(home.path(), &["--cursor", "0", "--json"]);
    assert_eq!(
        first,
        json!({
            "schema": "locus.fleet_denials.v1",
            "denials": [
                {"position": 1, "kind": "scope_freeze"},
                {"position": 99, "kind": "command_refused"},
                {"position": 100, "kind": "command_refused"},
            ],
            "next_cursor": 100,
            "has_more": true,
        })
    );
    let next = feed(home.path(), &["--cursor", "100"]);
    assert_eq!(
        next["denials"],
        json!([{"position": 101, "kind": "command_refused"}])
    );
    assert_eq!(next["next_cursor"], 101);
    assert_eq!(next["has_more"], false);
    assert_eq!(
        feed(home.path(), &["--cursor", "101"])["denials"],
        json!([])
    );
    let recent = feed(home.path(), &[]);
    assert_eq!(recent["denials"].as_array().unwrap().len(), 3);
    assert!(!recent.to_string().contains("secret"));
    assert!(!run(home.path(), &["--cursor", "102"]).status.success());
    lines.push(event("exec.refused"));
    fs::write(&path, lines.join("\n")).unwrap();
    assert_eq!(
        feed(home.path(), &["--cursor", "101"])["denials"],
        json!([{"position": 102, "kind": "command_refused"}])
    );
    fs::write(path, "").unwrap();
    assert_eq!(feed(home.path(), &[])["denials"], json!([]));
}

#[test]
fn unreadable_log_fails_without_echoing_paths() {
    let home = tempfile::Builder::new()
        .prefix("secret-path")
        .tempdir()
        .unwrap();
    fs::create_dir_all(home.path().join("audit/events.jsonl")).unwrap();
    let out = run(home.path(), &[]);
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    assert!(!String::from_utf8(out.stderr)
        .unwrap()
        .contains("secret-path"));
}

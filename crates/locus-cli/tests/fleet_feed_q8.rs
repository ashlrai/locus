//! End-to-end feed contract with a live, shell-owned authority broker.
use serde_json::{json, Value};
use std::process::Command;

fn feed(script: &str) -> Vec<Value> {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let out = Command::new("bash")
        .args(["--noprofile", "--norc", "-e", "-c", script])
        .env("LOCUS_BIN", env!("CARGO_BIN_EXE_locus"))
        .env("LOCUS_HOME", home.path())
        .env("HOME", home.path())
        .env_remove("LOCUS_SESSION_ID")
        .env_remove("LOCUS_CONTROL_CAPABILITY")
        .env_remove("LOCUS_EXECUTOR_CAPABILITY")
        .current_dir(cwd.path())
        .output()
        .unwrap();
    // Do not print captured setup output: it may contain authority material.
    assert!(out.status.success());
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn expected(state: &str) -> Value {
    // Exact allowlist ensures no session seal, capability, credential reference,
    // provider data or arbitrary error string can appear in the feed.
    json!({"schema": "locus.fleet_feed.v1", "kind": "locus_session",
        "state": state, "session_ok": state == "pinned"})
}

#[test]
fn pinned_unpinned_and_tampered_feed_contains_only_health() {
    let values = feed(
        r#"
"$LOCUS_BIN" init --with-samples >/dev/null
"$LOCUS_BIN" fleet-feed
 eval "$("$LOCUS_BIN" hook bash)"
"$LOCUS_BIN" pin personal >/dev/null
"$LOCUS_BIN" fleet-feed --json
(env -u LOCUS_CONTROL_CAPABILITY "$LOCUS_BIN" fleet-feed)
printf '%s' '{"seal":"synthetic-secret-sentinel"}' > "$LOCUS_HOME/sessions/active.json"
"$LOCUS_BIN" fleet-feed
"#,
    );
    assert_eq!(
        values,
        vec![
            expected("unpinned"),
            expected("pinned"),
            expected("unhealthy"),
            expected("unhealthy")
        ]
    );
}

#[test]
fn missing_selected_session_never_falls_back_to_unpinned() {
    let values = feed(
        r#"
"$LOCUS_BIN" init >/dev/null
LOCUS_SESSION_ID=missing "$LOCUS_BIN" fleet-feed
"#,
    );
    assert_eq!(values, vec![expected("unhealthy")]);
}

#[test]
fn expired_pin_is_unhealthy() {
    let values = feed(
        r#"
"$LOCUS_BIN" init --with-samples >/dev/null
 eval "$("$LOCUS_BIN" hook bash)"
sed 's/max_ttl = "12h"/max_ttl = "1s"/' "$LOCUS_HOME/bindings/personal.toml" > "$LOCUS_HOME/short.toml"
mv "$LOCUS_HOME/short.toml" "$LOCUS_HOME/bindings/personal.toml"
"$LOCUS_BIN" pin personal >/dev/null
sleep 2
"$LOCUS_BIN" fleet-feed
"#,
    );
    assert_eq!(values, vec![expected("unhealthy")]);
}

//! Synthetic credentials and a live shell-owned broker; no upstream calls.
#![cfg(unix)]
use serde_json::{json, Value};
use std::process::Command;

fn observe(script: &str) -> Vec<Value> {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let output = Command::new("bash")
        .args(["--noprofile", "--norc", "-e", "-c", script])
        .env("LOCUS_BIN", env!("CARGO_BIN_EXE_locus"))
        .env("LOCUS_HOME", home.path())
        .env("HOME", home.path())
        .env("SHELL", "/bin/bash")
        .env("LOCUS_GH_TOKEN_PERSONAL", "synthetic-secret-sentinel")
        .env("LOCUS_SUPABASE_PERSONAL", "synthetic-secret-sentinel")
        .env("LOCUS_VERCEL_TOKEN_PERSONAL", "synthetic-secret-sentinel")
        .env_remove("LOCUS_SESSION_ID")
        .env_remove("LOCUS_CONTROL_CAPABILITY")
        .env_remove("LOCUS_EXECUTOR_CAPABILITY")
        .current_dir(cwd.path())
        .output()
        .unwrap();
    assert!(output.status.success()); // Never dump captured authority material.
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains("synthetic-secret-sentinel"));
    stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn pinned_unpinned_expired_and_missing_authority() {
    let values = observe(
        r#"
"$LOCUS_BIN" init --with-samples >/dev/null 2>&1
"$LOCUS_BIN" fleet-preflight
 eval "$("$LOCUS_BIN" hook bash)"
mkdir "$HOME/bin"
printf '#!/bin/sh\nexit 99\n' > "$HOME/bin/gh"
chmod +x "$HOME/bin/gh"
export PATH="$HOME/bin:$PATH"
sed 's/max_ttl = "12h"/max_ttl = "3s"/' "$LOCUS_HOME/bindings/personal.toml" > "$HOME/short"
mv "$HOME/short" "$LOCUS_HOME/bindings/personal.toml"
"$LOCUS_BIN" pin personal >/dev/null 2>&1
cp "$LOCUS_HOME/sessions/active.json" "$HOME/before"
"$LOCUS_BIN" fleet-preflight --json
cmp "$HOME/before" "$LOCUS_HOME/sessions/active.json"
env -u LOCUS_CONTROL_CAPABILITY "$LOCUS_BIN" fleet-preflight
env -u LOCUS_GH_TOKEN_PERSONAL "$LOCUS_BIN" fleet-preflight
sleep 4
"$LOCUS_BIN" fleet-preflight
cmp "$HOME/before" "$LOCUS_HOME/sessions/active.json"
"#,
    );
    assert_eq!(values[0]["reason"], "unpinned");
    assert_eq!(
        values[1],
        json!({"schema":"locus.fleet_preflight.v1",
        "exec_ready":true,"reason":"ready","human_command":null})
    );
    assert_eq!(values[2]["exec_ready"], false);
    assert_eq!(values[3]["reason"], "credential_unavailable");
    assert_eq!(values[4]["reason"], "expired");
    assert_eq!(
        values[4]["human_command"],
        "eval \"$(locus hook bash)\"; locus leave --force; locus enter personal"
    );
    for value in values {
        assert_eq!(value.as_object().unwrap().len(), 4);
    }
}

#[test]
fn uninitialized_observation_creates_nothing() {
    let values = observe(
        r#"
"$LOCUS_BIN" fleet-preflight
test ! -e "$LOCUS_HOME/seal.key"
test -z "$(ls -A "$LOCUS_HOME")"
"#,
    );
    assert_eq!(values[0]["reason"], "uninitialized");
    assert_eq!(values[0]["exec_ready"], false);
}

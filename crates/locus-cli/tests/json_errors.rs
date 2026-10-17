//! Agent-facing CLI contracts: `--json` failures stay machine-readable, and
//! `locus workspace` names default/allowed aliases that have no binding yet.

use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};

fn locus(home: &Path, cwd: &Path, cap: Option<&str>, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_locus"));
    cmd.args(args)
        .env("LOCUS_HOME", home)
        .env("NO_COLOR", "1")
        .env_remove("LOCUS_SESSION_ID")
        .current_dir(cwd);
    match cap {
        Some(c) => cmd.env("LOCUS_CONTROL_CAPABILITY", c),
        None => cmd.env_remove("LOCUS_CONTROL_CAPABILITY"),
    };
    cmd.output().expect("locus binary runs")
}

fn assert_json_error(out: &Output, needle: &str) {
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "stderr: {stderr}");
    assert!(out.stdout.is_empty(), "stdout stays clean on failure");
    let v: Value = serde_json::from_str(stderr.trim())
        .unwrap_or_else(|e| panic!("--json failure must print JSON on stderr ({e}): {stderr:?}"));
    assert_eq!(v["ok"], false, "{v}");
    let msg = v["error"].as_str().expect("error string");
    assert!(msg.contains(needle), "{v}");
}

#[test]
fn json_flag_turns_command_errors_into_json() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    // Global flag before and after the subcommand.
    assert_json_error(
        &locus(home.path(), cwd.path(), None, &["--json", "whoami"]),
        "no active pin",
    );
    assert_json_error(
        &locus(home.path(), cwd.path(), None, &["whoami", "--json"]),
        "no active pin",
    );
    assert_json_error(
        &locus(
            home.path(),
            cwd.path(),
            None,
            &["--json", "binding", "show", "nope"],
        ),
        "binding not found",
    );
}

#[test]
fn without_json_flag_errors_stay_human() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let out = locus(home.path(), cwd.path(), None, &["whoami"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("error: no active pin"));
}

#[test]
fn workspace_warns_on_aliases_without_a_binding() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let init = locus(home.path(), cwd.path(), None, &["init", "--with-samples"]);
    assert!(init.status.success(), "{init:?}");
    let cap = std::fs::read_to_string(home.path().join("control_capability")).unwrap();
    let cap = cap.trim();

    let out = locus(
        home.path(),
        cwd.path(),
        Some(cap),
        &["workspace", "--default", "ghost", "--allow", "personal"],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "advisory only: {stderr}");
    assert!(cwd.path().join(".locus.toml").exists());
    assert!(
        stderr.contains("binding 'ghost' does not exist")
            && stderr.contains("locus binding add ghost"),
        "{stderr}"
    );
    assert!(!stderr.contains("'personal'"), "existing binding: {stderr}");
}

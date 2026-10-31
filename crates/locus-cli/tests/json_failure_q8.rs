//! Failed JSON commands must still give stdout consumers exactly one document.
use serde_json::Value;
use std::process::Command;

fn assert_failure(args: &[&str], blocked_store: bool) {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    if blocked_store {
        // A file cannot be opened as a store directory, even when run as root.
        std::fs::write(home.path().join(".locus"), b"blocked store").unwrap();
    }
    let out = Command::new(env!("CARGO_BIN_EXE_locus"))
        .args(args)
        .env_clear()
        .env("HOME", home.path())
        .current_dir(cwd.path())
        .output()
        .unwrap();
    assert!(!out.status.success(), "{args:?} must fail");
    // from_slice rejects empty output, prose, and trailing JSON documents.
    let value: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{args:?}: stdout must be one JSON document: {e}"));
    assert_eq!(value["schema"], "locus.error.v1", "{args:?}");
    assert_eq!(value["ok"], false);
    assert_eq!(value["exit_code"], out.status.code().unwrap());
    assert!(value["error"].is_string());
    let diagnostic: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(value, diagnostic);
}

#[test]
fn store_failures_keep_stdout_machine_readable() {
    for args in [
        &["doctor", "--json"][..],
        &["agent", "report", "--json"],
        &["agent", "doctor", "--json"],
        &["status", "--json"],
        &["whoami", "--json"],
        &["binding", "show", "missing", "--json"],
    ] {
        assert_failure(args, true);
    }
}

#[test]
fn init_dry_run_usage_failure_is_one_json_document() {
    // Dry-run intentionally does no store I/O; force a usage error instead.
    assert_failure(&["init", "--dry-run", "--json", "--invalid-q8"], false);
}

#[test]
fn leading_json_and_command_refusals_use_the_same_contract() {
    assert_failure(&["--json", "doctor"], true);
    assert_failure(&["whoami", "--json"], false);
    assert_failure(&["agent", "--json"], false);
}

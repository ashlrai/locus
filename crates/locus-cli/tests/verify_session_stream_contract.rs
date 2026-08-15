//! Stream contract for `locus verify session --json`.
//!
//! Hub gates and CI pipe stdout straight into a JSON parser, so the contract
//! is: stdout carries exactly one JSON document (even when `session_ok=false`),
//! every human/error line goes to stderr, and the exit code is nonzero when
//! the session is not ready.

use std::process::Command;

fn locus(home: &std::path::Path, cwd: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_locus"))
        .args(args)
        .env("LOCUS_HOME", home)
        .current_dir(cwd)
        .output()
        .expect("locus binary runs")
}

#[test]
fn verify_session_json_keeps_stdout_pure_json_and_errors_on_stderr() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();

    let init = locus(home.path(), cwd.path(), &["init"]);
    assert!(init.status.success(), "init failed: {init:?}");

    // Unpinned fresh store → session_ok=false → nonzero exit.
    let out = locus(home.path(), cwd.path(), &["verify", "session", "--json"]);
    assert!(
        !out.status.success(),
        "verify session must exit nonzero when session_ok=false"
    );

    // stdout is one parseable JSON document — nothing before or after it.
    let stdout = String::from_utf8(out.stdout).expect("stdout is UTF-8");
    let pack: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout must be pure JSON ({e}): {stdout:?}"));
    assert_eq!(pack["kind"], "session");
    assert_eq!(pack["session_ok"], false);
    assert!(
        !stdout.contains("error:"),
        "plain-text error leaked onto stdout: {stdout:?}"
    );

    // The failure explanation lives on stderr (exit-code semantics intact).
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("session_ok=false"),
        "stderr must carry the not-ready error, got: {stderr:?}"
    );
}

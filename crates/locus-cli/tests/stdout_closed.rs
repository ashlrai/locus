//! Regression: a closed stdout (`locus doctor | head -1`) must not panic with
//! "failed printing to stdout". The CLI exits 141 (128 + SIGPIPE) quietly.

use std::process::{Command, Stdio};

/// Run `locus <args>` with a stdout pipe whose read end is already closed, so
/// the first write deterministically fails with EPIPE.
fn run_with_closed_stdout(args: &[&str]) -> std::process::Output {
    let home = tempfile::tempdir().unwrap();
    let (reader, writer) = std::io::pipe().expect("pipe");
    drop(reader);
    let child = Command::new(env!("CARGO_BIN_EXE_locus"))
        .args(args)
        .env("LOCUS_HOME", home.path())
        .env_remove("LOCUS_CONTROL_CAPABILITY")
        .stdin(Stdio::null())
        .stdout(writer)
        .stderr(Stdio::piped())
        .spawn()
        .expect("locus binary runs");
    child.wait_with_output().expect("locus exits")
}

fn assert_quiet_sigpipe_exit(args: &[&str]) {
    let out = run_with_closed_stdout(args);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("panicked") && !stderr.contains("failed printing"),
        "locus {args:?} panicked on closed stdout: {stderr}"
    );
    assert_eq!(
        out.status.code(),
        Some(141),
        "locus {args:?} should exit 141 on closed stdout; stderr: {stderr}"
    );
}

#[test]
fn println_path_exits_quietly_on_closed_stdout() {
    assert_quiet_sigpipe_exit(&["hook", "bash"]);
}

#[test]
fn completion_path_exits_quietly_on_closed_stdout() {
    assert_quiet_sigpipe_exit(&["completion", "bash"]);
}

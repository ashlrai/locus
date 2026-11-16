//! Regression: a closed stdout (`locus doctor | head -1`) must not panic with
//! "failed printing to stdout". The CLI exits 141 (128 + SIGPIPE) quietly.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Run `locus <args>` with a stdout pipe whose read end is already closed, so
/// the first write deterministically fails with EPIPE.
fn run_with_closed_stdout(args: &[&str]) -> std::process::Output {
    let home = tempfile::tempdir().unwrap();
    let (reader, writer) = std::io::pipe().expect("pipe");
    drop(reader);
    let mut child = Command::new(env!("CARGO_BIN_EXE_locus"))
        .args(args)
        .env("LOCUS_HOME", home.path())
        .env_remove("LOCUS_CONTROL_CAPABILITY")
        .stdin(Stdio::null())
        .stdout(writer)
        .stderr(Stdio::piped())
        .spawn()
        .expect("locus binary runs");
    let budget = if std::env::var_os("CARGO_LLVM_COV").is_some()
        || std::env::var_os("LLVM_PROFILE_FILE").is_some()
    {
        Duration::from_secs(120)
    } else {
        Duration::from_secs(10)
    };
    let deadline = Instant::now() + budget;
    while child.try_wait().expect("check child status").is_none() {
        if Instant::now() >= deadline {
            child.kill().expect("kill hung child");
            child.wait().expect("reap hung child");
            panic!("locus {args:?} hung with closed stdout");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
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
fn doctor_exits_quietly_on_closed_stdout() {
    assert_quiet_sigpipe_exit(&["doctor"]);
}

#[test]
fn completion_path_exits_quietly_on_closed_stdout() {
    assert_quiet_sigpipe_exit(&["completion", "bash"]);
}

#[test]
fn json_error_path_exits_quietly_on_closed_stdout() {
    assert_quiet_sigpipe_exit(&["whoami", "--json"]);
}

#[test]
fn immediate_consumer_close_terminates_without_panic() {
    let home = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_locus"))
        .args(["hook", "bash"])
        .env("LOCUS_HOME", home.path())
        .env_remove("LOCUS_CONTROL_CAPABILITY")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("locus binary runs");
    drop(child.stdout.take().expect("piped stdout"));

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if child.try_wait().expect("check locus status").is_some() {
            break;
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill hung locus");
            child.wait().expect("reap hung locus");
            panic!("locus hung after its stdout consumer closed");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let out = child.wait_with_output().expect("collect locus output");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("panicked") && !stderr.contains("failed printing"),
        "locus panicked on closed stdout: {stderr}"
    );
    // The CLI can finish writing before the parent closes its pipe.
    assert!(
        matches!(out.status.code(), Some(0 | 141)),
        "unexpected exit status {}; stderr: {stderr}",
        out.status
    );
}

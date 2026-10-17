//! E2E: `locus exec` exit codes an agent can branch on. A command that never
//! started must not look like a child that ran and exited 1: not found is
//! 127 and not executable is 126 (shell convention), with an error naming
//! the next step. A child killed by a signal exits 128 + signal.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output};

struct Fixture {
    home: tempfile::TempDir,
    cwd: tempfile::TempDir,
    cap: String,
}

fn fixture() -> Fixture {
    let mut f = Fixture {
        home: tempfile::tempdir().unwrap(),
        cwd: tempfile::tempdir().unwrap(),
        cap: String::new(),
    };
    let qs = f.locus(&["quickstart"]);
    assert!(qs.status.success(), "quickstart failed: {qs:?}");
    f.cap = std::fs::read_to_string(f.home.path().join("control_capability"))
        .expect("quickstart persists the control capability")
        .trim()
        .to_string();
    f
}

impl Fixture {
    fn locus(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_locus"));
        cmd.args(args)
            .env("LOCUS_HOME", self.home.path())
            .env("NO_COLOR", "1")
            .env_remove("LOCUS_PHANTOM_PROJECT")
            .env_remove("LOCUS_CONTROL_CAPABILITY")
            .current_dir(self.cwd.path());
        if !self.cap.is_empty() {
            cmd.env("LOCUS_CONTROL_CAPABILITY", &self.cap);
        }
        cmd.output().expect("locus binary runs")
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn child_exit_code_passes_through() {
    let f = fixture();
    let out = f.locus(&["exec", "--", "sh", "-c", "exit 3"]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
}

#[test]
fn missing_command_exits_127_with_next_step() {
    let f = fixture();
    let out = f.locus(&["exec", "--", "locus-definitely-not-a-command"]);
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(127), "{err}");
    assert!(
        err.contains("spawn locus-definitely-not-a-command: command not found"),
        "{err}"
    );
    assert!(err.contains("absolute path"), "next step missing: {err}");

    // Same code under --json, and the error line is JSON.
    let out = f.locus(&["--json", "exec", "--", "locus-definitely-not-a-command"]);
    assert_eq!(out.status.code(), Some(127), "{}", stderr(&out));
    let last = stderr(&out).lines().last().unwrap_or_default().to_string();
    let v: serde_json::Value =
        serde_json::from_str(&last).unwrap_or_else(|e| panic!("json error line: {e}: {last}"));
    assert_eq!(v["ok"], false);
    assert!(v["error"].as_str().unwrap().contains("command not found"));
}

#[test]
fn non_executable_file_exits_126() {
    let f = fixture();
    let script = f.cwd.path().join("not-exec.sh");
    std::fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o644)).unwrap();
    let out = f.locus(&["exec", "--", script.to_str().unwrap()]);
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(126), "{err}");
    assert!(err.contains("permission denied"), "{err}");
}

#[test]
fn signal_killed_child_exits_128_plus_signal() {
    let f = fixture();
    let out = f.locus(&["exec", "--", "sh", "-c", "kill -TERM $$"]);
    assert_eq!(out.status.code(), Some(128 + 15), "{}", stderr(&out));
}

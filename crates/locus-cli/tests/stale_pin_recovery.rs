//! E2E: after the shell that pinned exits, its authority broker is gone and
//! the sealed pin can no longer be verified. Every surface must fail closed
//! AND name the human recovery:
//! `eval "$(locus hook bash)"; locus leave --force; locus enter <alias>`.

use std::path::Path;
use std::process::{Command, Output};

const RECOVERY_TAIL: &str = "locus leave --force; locus enter";

fn locus(home: &Path, cwd: &Path, cap: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_locus"))
        .args(args)
        .env("LOCUS_HOME", home)
        .env("LOCUS_CONTROL_CAPABILITY", cap)
        .env("SHELL", "/bin/bash")
        .env("NO_COLOR", "1")
        .current_dir(cwd)
        .output()
        .expect("locus binary runs")
}

/// Quickstart (which pins `personal`) under a short-lived `sh`, so the pin's
/// supervising process exits and takes the broker with it. Returns the
/// persisted control capability.
fn pin_in_exited_shell(home: &Path, cwd: &Path) -> String {
    // `; true` keeps sh from exec-ing locus, so sh (not this test) supervises.
    let status = Command::new("sh")
        .arg("-c")
        .arg("\"$0\" quickstart >/dev/null 2>&1; true")
        .arg(env!("CARGO_BIN_EXE_locus"))
        .env("LOCUS_HOME", home)
        .env_remove("LOCUS_CONTROL_CAPABILITY")
        .current_dir(cwd)
        .status()
        .expect("sh runs");
    assert!(status.success());
    let cap = std::fs::read_to_string(home.join("control_capability"))
        .expect("quickstart persists the control capability");
    let cap = cap.trim().to_string();
    // The broker notices its supervisor is gone; wait until whoami fails.
    for _ in 0..100 {
        if !locus(home, cwd, &cap, &["whoami"]).status.success() {
            return cap;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("pin stayed verifiable after its supervising shell exited");
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn stale_pin_surfaces_name_the_recovery() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let cap = pin_in_exited_shell(home.path(), cwd.path());

    let whoami = locus(home.path(), cwd.path(), &cap, &["whoami"]);
    assert!(!whoami.status.success());
    let err = stderr(&whoami);
    assert!(
        err.contains("eval \"$(locus hook bash)\"; locus leave --force; locus enter personal"),
        "whoami must name the recovery: {err}"
    );

    let enter = locus(home.path(), cwd.path(), &cap, &["enter", "acme"]);
    assert!(!enter.status.success(), "enter must still fail closed");
    let err = stderr(&enter);
    assert!(
        err.contains("locus leave --force; locus enter acme"),
        "enter must name the recovery: {err}"
    );

    // The named recovery works.
    let leave = locus(home.path(), cwd.path(), &cap, &["leave", "--force"]);
    assert!(leave.status.success(), "leave --force: {}", stderr(&leave));
    let enter = locus(home.path(), cwd.path(), &cap, &["enter", "acme"]);
    assert!(enter.status.success(), "re-enter: {}", stderr(&enter));
    let whoami = locus(home.path(), cwd.path(), &cap, &["whoami"]);
    assert!(whoami.status.success(), "whoami: {}", stderr(&whoami));
    assert!(!stderr(&whoami).contains(RECOVERY_TAIL));
}

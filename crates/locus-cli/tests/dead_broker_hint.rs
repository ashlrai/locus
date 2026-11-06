//! Regression: an intact pin whose broker exited can be replaced directly.
#![cfg(unix)]

use std::process::Command;
use std::time::Duration;

#[test]
fn dead_broker_surfaces_suggest_enter_before_manual_force_leave() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    // Keep the shell as supervisor, then let it exit along with its broker.
    let status = Command::new("sh")
        .arg("-c")
        .arg("\"$0\" quickstart >/dev/null 2>&1; true")
        .arg(env!("CARGO_BIN_EXE_locus"))
        .env("LOCUS_HOME", home.path())
        .env_remove("LOCUS_CONTROL_CAPABILITY")
        .current_dir(cwd.path())
        .status()
        .unwrap();
    assert!(status.success());
    let capability = std::fs::read_to_string(home.path().join("control_capability")).unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_locus"))
            .args(args)
            .env("LOCUS_HOME", home.path())
            .env("LOCUS_CONTROL_CAPABILITY", capability.trim())
            .env("SHELL", "/bin/bash")
            .env("NO_COLOR", "1")
            .current_dir(cwd.path())
            .output()
            .unwrap()
    };
    let mut dead = false;
    for _ in 0..100 {
        if !run(&["whoami"]).status.success() {
            dead = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(dead, "broker should stop after its supervisor exits");

    for args in [
        &["whoami"][..],
        &["status"][..],
        &["doctor"][..],
        &["agent", "report"][..],
        &["fleet-preflight"][..],
    ] {
        let output = run(args);
        let text = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let enter = text.find("locus enter personal").expect("enter hint");
        let force = text.find("locus leave --force").expect("manual fallback");
        assert!(enter < force, "{args:?} must suggest enter first");
        assert!(text.contains("# If enter refuses an invalid or unreadable pin"));
    }
}

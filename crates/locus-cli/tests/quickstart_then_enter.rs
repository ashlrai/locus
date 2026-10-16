//! E2E: `locus quickstart` followed by `locus enter acme` from the same
//! supervising process must succeed (regression for the stale session
//! authority error seen right after quickstart).

use std::process::Command;

fn locus(
    home: &std::path::Path,
    cwd: &std::path::Path,
    cap: Option<&str>,
    args: &[&str],
) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_locus"));
    cmd.args(args)
        .env("LOCUS_HOME", home)
        .env_remove("LOCUS_CONTROL_CAPABILITY")
        .current_dir(cwd);
    if let Some(cap) = cap {
        cmd.env("LOCUS_CONTROL_CAPABILITY", cap);
    }
    cmd.output().expect("locus binary runs")
}

#[test]
fn quickstart_then_enter_switches_binding() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();

    let qs = locus(home.path(), cwd.path(), None, &["quickstart"]);
    assert!(qs.status.success(), "quickstart failed: {qs:?}");

    let cap = std::fs::read_to_string(home.path().join("control_capability"))
        .expect("quickstart persists the control capability");
    let cap = cap.trim();

    let enter = locus(home.path(), cwd.path(), Some(cap), &["enter", "acme"]);
    let stderr = String::from_utf8_lossy(&enter.stderr);
    assert!(
        enter.status.success(),
        "enter acme failed after quickstart: {stderr}"
    );
    assert!(
        !stderr.contains("stale"),
        "unexpected stale authority: {stderr}"
    );

    let who = locus(home.path(), cwd.path(), Some(cap), &["whoami"]);
    let out = String::from_utf8_lossy(&who.stdout);
    assert!(who.status.success(), "whoami failed: {who:?}");
    assert!(out.contains("acme"), "whoami should report acme: {out}");
}

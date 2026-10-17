//! E2E: after the shell that pinned exits, its authority broker is gone and
//! the sealed pin can no longer be verified. Every surface must fail closed
//! AND name the human recovery:
//! `eval "$(locus hook bash)"; locus leave --force; locus enter <alias>`.

use std::path::Path;
use std::process::{Command, Output};

const RECOVERY_TAIL: &str = "locus leave --force; locus enter";

fn locus(home: &Path, cwd: &Path, cap: &str, args: &[&str]) -> Output {
    locus_cmd(home, cwd, cap, args)
        .output()
        .expect("locus binary runs")
}

/// For a control command that must fail closed on the stale pin. The first
/// such command's broker start never hands off an endpoint, so the CLI polls
/// for a current broker for the full default start timeout (10s) before
/// refusing. The documented override shortens only that wait; the
/// refusal and its recovery text are asserted unchanged.
fn locus_fail_closed(home: &Path, cwd: &Path, cap: &str, args: &[&str]) -> Output {
    locus_cmd(home, cwd, cap, args)
        .env("LOCUS_AUTHORITY_BROKER_START_TIMEOUT_MS", "500")
        .output()
        .expect("locus binary runs")
}

fn locus_cmd(home: &Path, cwd: &Path, cap: &str, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_locus"));
    cmd.args(args)
        .env("LOCUS_HOME", home)
        .env("LOCUS_CONTROL_CAPABILITY", cap)
        .env("SHELL", "/bin/bash")
        .env("NO_COLOR", "1")
        .current_dir(cwd);
    cmd
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

/// Render `_locus_prompt` from `locus hook bash` (raw, with color codes).
fn hook_prompt(home: &Path, cwd: &Path, cap: &str) -> String {
    let bin_dir = Path::new(env!("CARGO_BIN_EXE_locus")).parent().unwrap();
    let path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = Command::new("bash")
        .arg("-c")
        .arg("eval \"$(locus hook bash)\"; _locus_prompt")
        .env("PATH", path)
        .env("LOCUS_HOME", home)
        .env("LOCUS_CONTROL_CAPABILITY", cap)
        .current_dir(cwd)
        .output()
        .expect("bash runs");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
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

    // status must agree with whoami: not ok, same remedy, all three formats.
    let status = locus(home.path(), cwd.path(), &cap, &["status"]);
    assert_eq!(status.status.code(), Some(2), "stale status must exit 2");
    let out = String::from_utf8_lossy(&status.stdout);
    assert!(out.starts_with("INVALID personal"), "status: {out}");
    assert!(
        out.contains("eval \"$(locus hook bash)\"; locus leave --force; locus enter personal"),
        "status must name the recovery: {out}"
    );
    let oneline = locus(home.path(), cwd.path(), &cap, &["status", "--oneline"]);
    assert_eq!(oneline.status.code(), Some(2));
    assert_eq!(String::from_utf8_lossy(&oneline.stdout).trim(), "invalid");
    let json = locus(home.path(), cwd.path(), &cap, &["--json", "status"]);
    assert_eq!(json.status.code(), Some(2));
    let v: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["authority_verified"], false);
    assert!(v["error"]
        .as_str()
        .unwrap()
        .contains("human re-pin required"));
    assert!(v["recovery"]
        .as_str()
        .unwrap()
        .ends_with("locus leave --force; locus enter personal"));

    // agent report: human and JSON both carry the re-pin recovery.
    let report = locus(home.path(), cwd.path(), &cap, &["agent", "report"]);
    let out = String::from_utf8_lossy(&report.stdout);
    assert!(!report.status.success());
    assert!(
        out.contains("eval \"$(locus hook bash)\"; locus leave --force; locus enter personal"),
        "agent report must name the recovery: {out}"
    );
    let report = locus(
        home.path(),
        cwd.path(),
        &cap,
        &["agent", "report", "--json"],
    );
    let v: serde_json::Value = serde_json::from_slice(&report.stdout).unwrap();
    assert_eq!(v["ready"], false);
    let recovery = v["recovery"].as_str().expect("recovery in JSON");
    assert!(recovery.ends_with("locus leave --force; locus enter personal"));
    assert_eq!(v["next_steps"][0], recovery);

    // The bash prompt hook shows the invalid pin instead of "unpinned".
    let prompt = hook_prompt(home.path(), cwd.path(), &cap);
    assert!(prompt.contains("[locus:invalid]"), "prompt: {prompt}");

    let enter = locus_fail_closed(home.path(), cwd.path(), &cap, &["enter", "acme"]);
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
    let status = locus(home.path(), cwd.path(), &cap, &["--json", "status"]);
    assert!(status.status.success());
    let v: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["authority_verified"], true);
    assert!(v["recovery"].is_null());
    let report = locus(
        home.path(),
        cwd.path(),
        &cap,
        &["agent", "report", "--json"],
    );
    let v: serde_json::Value = serde_json::from_slice(&report.stdout).unwrap();
    assert!(v.get("recovery").is_none(), "live pin: no recovery");
}

#[test]
fn status_without_capability_is_unverified_not_ok() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    // Pin from this (live) test process so the broker stays up.
    let qs = Command::new(env!("CARGO_BIN_EXE_locus"))
        .arg("quickstart")
        .env("LOCUS_HOME", home.path())
        .env_remove("LOCUS_CONTROL_CAPABILITY")
        .current_dir(cwd.path())
        .output()
        .unwrap();
    assert!(qs.status.success(), "quickstart: {}", stderr(&qs));
    let status = Command::new(env!("CARGO_BIN_EXE_locus"))
        .arg("status")
        .env("LOCUS_HOME", home.path())
        .env_remove("LOCUS_CONTROL_CAPABILITY")
        .env("SHELL", "/bin/zsh")
        .env("NO_COLOR", "1")
        .current_dir(cwd.path())
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&status.stdout);
    assert!(status.status.success(), "{out}");
    assert!(out.starts_with("UNVERIFIED personal"), "status: {out}");
    assert!(out.contains("eval \"$(locus hook zsh)\""), "status: {out}");
}

/// Control commands outside the pin family (`workspace`, `binding add`,
/// `notify`) also fail closed on a stale pin; they must name the same
/// recovery instead of a bare "human re-pin required".
#[test]
fn stale_pin_control_commands_name_the_recovery() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let cap = pin_in_exited_shell(home.path(), cwd.path());

    let cases: &[&[&str]] = &[
        &["workspace", "--default", "personal"],
        &[
            "binding",
            "add",
            "stale-probe",
            "--tenant",
            "t",
            "--provider",
            "github",
            "--account",
            "x",
            "--credential-ref",
            "env:STALE_PROBE_TOKEN",
            "--non-interactive",
        ],
        &["notify", "on"],
    ];
    for args in cases {
        let out = locus_fail_closed(home.path(), cwd.path(), &cap, args);
        assert!(!out.status.success(), "{args:?} must fail closed");
        let err = stderr(&out);
        assert!(err.contains("human re-pin required"), "{args:?}: {err}");
        assert!(
            err.contains("eval \"$(locus hook bash)\"; locus leave --force; locus enter personal"),
            "{args:?} must name the recovery: {err}"
        );
        assert_eq!(
            err.matches(RECOVERY_TAIL).count(),
            1,
            "{args:?} names the recovery once: {err}"
        );
    }
    assert!(!cwd.path().join(".locus.toml").exists());
    assert!(!home.path().join("bindings/stale-probe.toml").exists());
}

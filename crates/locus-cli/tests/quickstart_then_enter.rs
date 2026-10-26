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

/// Next-step hints for exporting the control capability follow `$SHELL`
/// (fish needs `| source`, not `eval "$(…)"`); they used to always say zsh.
#[test]
fn capability_hints_follow_the_users_shell() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let run = |shell: &str, args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_locus"))
            .args(args)
            .env("LOCUS_HOME", home.path())
            .env_remove("LOCUS_CONTROL_CAPABILITY")
            .env("SHELL", shell)
            .env("NO_COLOR", "1")
            .current_dir(cwd.path())
            .output()
            .expect("locus binary runs")
    };

    let qs = run("/usr/bin/fish", &["quickstart"]);
    assert!(qs.status.success(), "quickstart failed: {qs:?}");
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&qs.stdout),
        String::from_utf8_lossy(&qs.stderr)
    );
    assert!(all.contains("locus hook fish | source"), "{all}");
    assert!(
        !all.contains("hook zsh"),
        "fish user told to use zsh: {all}"
    );

    // Persisted but not exported: doctor names the user's shell.
    let doctor = run("/bin/bash", &["doctor"]);
    let out = String::from_utf8_lossy(&doctor.stdout);
    assert!(
        out.contains("control_capability_not_exported"),
        "doctor: {out}"
    );
    assert!(out.contains("eval \"$(locus hook bash)\""), "doctor: {out}");
    assert!(
        !out.contains("hook zsh"),
        "bash user told to use zsh: {out}"
    );
}

/// Run `locus quickstart` under a short-lived `sh` in a fresh home, so the
/// pin's supervising shell exits and its broker generation dies with it (the
/// README first run when quickstart and enter happen in different shells).
fn quickstart_in_exited_shell(home: &std::path::Path, cwd: &std::path::Path) -> String {
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
    for _ in 0..100 {
        if !locus(home, cwd, Some(&cap), &["whoami"]).status.success() {
            return cap;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("pin stayed verifiable after its supervising shell exited");
}

/// P0 first-run regression: quickstart, then `enter personal` from another
/// process, then a passing `exec`, with no `leave --force` in between. The
/// dead pin is replaced only for the persisted operator capability.
#[test]
fn quickstart_then_enter_from_a_new_shell_reaches_exec() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let cap = quickstart_in_exited_shell(home.path(), cwd.path());

    let wrong = "ab".repeat(32);
    let refused = locus(
        home.path(),
        cwd.path(),
        Some(&wrong),
        &["enter", "personal"],
    );
    assert!(
        !refused.status.success(),
        "wrong capability must not replace the pin"
    );

    let enter = locus(home.path(), cwd.path(), Some(&cap), &["enter", "personal"]);
    let stderr = String::from_utf8_lossy(&enter.stderr);
    assert!(enter.status.success(), "enter after quickstart: {stderr}");

    let exec = locus(
        home.path(),
        cwd.path(),
        Some(&cap),
        &["exec", "--expect", "personal", "--", "true"],
    );
    assert!(exec.status.success(), "exec after enter: {exec:?}");

    let audit = std::fs::read_dir(home.path().join("audit"))
        .unwrap()
        .map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap())
        .collect::<String>();
    assert_eq!(audit.matches("session.replace_stale").count(), 1, "{audit}");
}

//! E2E: after the shell that pinned exits, its authority broker is gone and
//! the sealed pin can no longer be verified. Every surface must fail closed
//! AND name the human recovery. A dead broker with an intact seal is replaced
//! by `locus enter` directly (#156), so the hint is enter first:
//! `eval "$(locus hook bash)"; locus enter <alias>`, with a separate manual fallback.

use std::path::Path;
use std::process::{Command, Output};

const RECOVERY_TAIL: &str =
    "# If enter refuses an invalid or unreadable pin, run: locus leave --force && locus enter";
const DEAD_BROKER_RECOVERY: &str =
    "eval \"$(locus hook bash)\"; locus enter personal # If enter refuses an invalid or unreadable pin, run: locus leave --force && locus enter personal";

#[test]
fn changelog_dead_broker_recipes_match_the_current_hint() {
    let changelog = include_str!("../../../CHANGELOG.md");
    let current = locus_core::recovery::dead_broker_recovery_for_shell(None, "bash");
    let unreleased = changelog
        .split_once("## [Unreleased]")
        .unwrap()
        .1
        .split("## [0.5.2]")
        .next()
        .unwrap();
    let entry = unreleased
        .split_once("**Dead-broker recovery (")
        .expect("unreleased recovery entry exists")
        .1
        .split("\n- ")
        .next()
        .unwrap();
    assert!(entry.contains("recorded v0.5.1 first run"));
    assert!(entry.contains("incorrectly put `locus leave --force` before `locus enter <alias>`"));
    assert!(entry.contains("although entering directly succeeded"));
    assert!(entry.contains(&current), "entry must contain {current}");
    assert!(entry.contains("expired pin, invalid or legacy seal, or unreadable session"));
    assert!(entry.contains("`locus leave --force` and then `locus enter <alias>`"));
    assert!(entry.contains("Do not force leave for unrelated errors"));
    let automatic_fallback = "locus enter <alias> || { locus leave --force";
    assert!(
        !changelog.contains(automatic_fallback),
        "changelog must not recommend automatic forced teardown"
    );
}

/// Run the operator guide's and README's Bash commands against a dead broker
/// and a damaged seal, keeping both recovery recipes aligned with the CLI.
#[test]
fn documented_broker_recovery_commands_work_in_order() {
    let guide = include_str!("../../../docs/broker-recovery-q8.md");
    let readme = include_str!("../../../README.md");
    let commands = |document: &str, heading: &str| {
        document
            .split_once(heading)
            .expect("recovery section exists")
            .1
            .split_once("```bash\n")
            .expect("recovery commands exist")
            .1
            .split_once("\n```")
            .expect("recovery code block ends")
            .0
            .replace("<alias>", "personal")
    };
    let guide_enter = commands(guide, "## Enter first");
    let guide_fallback = commands(guide, "## If enter is refused");
    let readme_section = readme
        .split_once("### Recovering after the broker exits")
        .expect("README recovery section exists")
        .1
        .split_once("### Fresh quickstart doctor warning")
        .expect("README recovery section ends")
        .0;
    let readme_enter = commands(readme_section, "If a command reports")
        .replace("cd /path/to/workspace", "cd \"$RECOVERY_WORKSPACE\"");
    let readme_fallback = commands(readme_section, "If `enter` is refused");
    assert!(readme_enter.starts_with("cd \"$RECOVERY_WORKSPACE\"\n"));

    for (enter_first, fallback) in [
        (&guide_enter, &guide_fallback),
        (&readme_enter, &readme_fallback),
    ] {
        assert!(!enter_first.contains("leave --force"));
        assert!(enter_first.contains("locus enter personal"));
        assert!(
            fallback.find("locus leave --force").unwrap()
                < fallback.find("locus enter personal").unwrap()
        );
        for damaged in [false, true] {
            let home = tempfile::tempdir().unwrap();
            let cwd = tempfile::tempdir().unwrap();
            let _cap = pin_in_exited_shell(home.path(), cwd.path());
            if damaged {
                edit_active(home.path(), false, |s| s.seal = "deadbeef".into());
            }
            let bin_dir = Path::new(env!("CARGO_BIN_EXE_locus")).parent().unwrap();
            let path = format!(
                "{}:{}",
                bin_dir.display(),
                std::env::var("PATH").unwrap_or_default()
            );
            // Preserve the hooked shell between enter and the conditional manual
            // fallback, just as the guide instructs the operator to do.
            let script = if damaged {
                let (attempt, _) = enter_first.rsplit_once('\n').expect("whoami follows enter");
                format!("set -e\nif {attempt}\nthen exit 1; fi\n{fallback}\nexit $?")
            } else {
                format!("set -e\n{enter_first}\nexit $?")
            };
            let out = Command::new("bash")
                .arg("-c")
                .arg(script)
                .env("PATH", path)
                .env("LOCUS_HOME", home.path())
                .env("RECOVERY_WORKSPACE", cwd.path())
                .env("SHELL", "/bin/bash")
                .env_remove("LOCUS_CONTROL_CAPABILITY")
                .env_remove("LOCUS_SESSION_ID")
                .current_dir(cwd.path())
                .output()
                .expect("bash runs");
            assert!(
                out.status.success(),
                "documented recovery failed: {}",
                stderr(&out)
            );
            let audit = std::fs::read_to_string(home.path().join("audit/events.jsonl")).unwrap();
            assert_eq!(audit.contains("session.force_leave"), damaged);
            assert_eq!(replace_stale_audits(home.path()), usize::from(!damaged));
        }
    }
}

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
        err.contains(DEAD_BROKER_RECOVERY),
        "whoami must name the recovery: {err}"
    );

    // status must agree with whoami: not ok, same remedy, all three formats.
    let status = locus(home.path(), cwd.path(), &cap, &["status"]);
    assert_eq!(status.status.code(), Some(2), "stale status must exit 2");
    let out = String::from_utf8_lossy(&status.stdout);
    assert!(out.starts_with("INVALID personal"), "status: {out}");
    assert!(
        out.contains(DEAD_BROKER_RECOVERY),
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
    assert_eq!(v["recovery"].as_str(), Some(DEAD_BROKER_RECOVERY));

    // agent report: human and JSON both carry the re-pin recovery.
    let report = locus(home.path(), cwd.path(), &cap, &["agent", "report"]);
    let out = String::from_utf8_lossy(&report.stdout);
    assert!(!report.status.success());
    assert!(
        out.contains(DEAD_BROKER_RECOVERY),
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
    assert_eq!(recovery, DEAD_BROKER_RECOVERY);
    assert_eq!(v["next_steps"][0], recovery);

    // The bash prompt hook shows the invalid pin instead of "unpinned".
    let prompt = hook_prompt(home.path(), cwd.path(), &cap);
    assert!(prompt.contains("[locus:invalid]"), "prompt: {prompt}");

    // `enter` over the dead pin fails closed for a capability that does not
    // pass the `leave --force` verifier (see quickstart_then_enter.rs for the
    // operator path, which replaces the dead pin directly).
    let wrong = "ab".repeat(32);
    let enter = locus_fail_closed(home.path(), cwd.path(), &wrong, &["enter", "acme"]);
    assert!(!enter.status.success(), "enter with a wrong capability");
    assert!(stderr(&enter).contains("persisted operator capability"));

    // The forced fallback also works.
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
fn doctor_displays_enter_before_force_leave_for_dead_broker() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let cap = pin_in_exited_shell(home.path(), cwd.path());
    let doctor = locus(home.path(), cwd.path(), &cap, &["doctor"]);
    let output = String::from_utf8_lossy(&doctor.stdout);
    assert!(!doctor.status.success());
    let enter = output.find("locus enter personal").expect("enter hint");
    let force = output.find("locus leave --force").expect("force fallback");
    assert!(enter < force, "doctor must show enter first: {output}");
    assert!(output.contains(DEAD_BROKER_RECOVERY));
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
            err.contains(DEAD_BROKER_RECOVERY),
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

/// `locus pin nosuch` / `enter nosuch` / `switch nosuch` on a stale pin must
/// not recommend `locus enter nosuch` (a binding that does not exist, so the
/// recipe would fail at its last step). Fall back to the stale pin's own
/// binding, which does exist. `leave` names it too instead of `<alias>`.
#[test]
fn stale_pin_recovery_never_names_a_missing_binding() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let cap = pin_in_exited_shell(home.path(), cwd.path());

    // `enter`/`pin` may replace a dead pin, so for them a missing binding is
    // the only error; it must still never be suggested.
    for args in [&["pin", "nosuch"][..], &["enter", "nosuch"][..]] {
        let out = locus_fail_closed(home.path(), cwd.path(), &cap, args);
        assert!(!out.status.success(), "{args:?} must fail");
        assert!(
            !stderr(&out).contains("locus enter nosuch"),
            "{args:?} must not suggest a missing binding: {}",
            stderr(&out)
        );
    }
    for args in [&["--json", "switch", "nosuch"][..], &["leave"][..]] {
        let out = locus_fail_closed(home.path(), cwd.path(), &cap, args);
        assert!(!out.status.success(), "{args:?} must fail closed");
        let err = stderr(&out);
        assert!(
            !err.contains("locus enter nosuch"),
            "{args:?} must not suggest a missing binding: {err}"
        );
        assert!(
            err.contains(
                "locus enter personal # If enter refuses an invalid or unreadable pin, run: locus leave --force && locus enter personal"
            ),
            "{args:?} must name the stale pin's binding: {err}"
        );
    }
    assert!(!home.path().join("bindings/nosuch.toml").exists());
}

/// Follow the printed dead-broker hint verbatim in a fresh bash with no
/// capability exported: the hook exports it, `locus enter` replaces the dead
/// pin on its own (audited `session.replace_stale`), and the forced-leave
/// fallback remains a comment and never runs.
#[test]
fn printed_dead_broker_hint_works_verbatim() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let cap = pin_in_exited_shell(home.path(), cwd.path());
    let whoami = locus(home.path(), cwd.path(), &cap, &["whoami"]);
    assert!(!whoami.status.success());
    let err = stderr(&whoami);
    let hint = err
        .lines()
        .find_map(|l| l.split_once("recover (operator shell; control capability required): "))
        .map(|(_, h)| h.trim().to_string())
        .unwrap_or_else(|| panic!("no recovery line: {err}"));
    assert_eq!(hint, DEAD_BROKER_RECOVERY);
    let before = replace_stale_audits(home.path());

    let bin_dir = Path::new(env!("CARGO_BIN_EXE_locus")).parent().unwrap();
    let path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = Command::new("bash")
        .arg("-c")
        .arg(format!("set -e\n{hint}\nlocus whoami\nexit $?"))
        .env("PATH", path)
        .env("LOCUS_HOME", home.path())
        .env("SHELL", "/bin/bash")
        .env_remove("LOCUS_CONTROL_CAPABILITY")
        .env_remove("LOCUS_SESSION_ID")
        .current_dir(cwd.path())
        .output()
        .expect("bash runs");
    let all = format!("{}{}", String::from_utf8_lossy(&out.stdout), stderr(&out));
    assert!(out.status.success(), "verbatim hint failed: {all}");
    assert!(all.contains("personal"), "{all}");
    assert_eq!(
        replace_stale_audits(home.path()),
        before + 1,
        "enter must replace the dead pin itself: {all}"
    );
    let audit = std::fs::read_to_string(home.path().join("audit/events.jsonl")).unwrap_or_default();
    assert!(
        !audit.contains("operator forced leave"),
        "the --force fallback must not run when enter succeeds"
    );
}

/// An unrelated enter failure must not force-clear the old session when the
/// operator runs the printed line. In this case the binding disappears after
/// the hint was generated.
#[test]
fn printed_dead_broker_hint_does_not_force_leave_on_enter_failure() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let cap = pin_in_exited_shell(home.path(), cwd.path());
    let whoami = locus(home.path(), cwd.path(), &cap, &["whoami"]);
    let err = stderr(&whoami);
    let hint = err
        .lines()
        .find_map(|l| l.split_once("recover (operator shell; control capability required): "))
        .map(|(_, h)| h.trim())
        .expect("recovery line");
    assert_eq!(hint, DEAD_BROKER_RECOVERY);
    let active_path = home.path().join("sessions/active.json");
    let before = std::fs::read(&active_path).unwrap();
    std::fs::remove_file(home.path().join("bindings/personal.toml")).unwrap();
    let bin_dir = Path::new(env!("CARGO_BIN_EXE_locus")).parent().unwrap();
    let path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = Command::new("bash")
        .arg("-c")
        .arg(hint)
        .env("PATH", path)
        .env("LOCUS_HOME", home.path())
        .env("SHELL", "/bin/bash")
        .env_remove("LOCUS_CONTROL_CAPABILITY")
        .env_remove("LOCUS_SESSION_ID")
        .current_dir(cwd.path())
        .output()
        .expect("bash runs");
    assert!(!out.status.success(), "missing binding must fail");
    assert_eq!(
        std::fs::read(active_path).unwrap(),
        before,
        "failed enter must not clear the pin"
    );
    let audit = std::fs::read_to_string(home.path().join("audit/events.jsonl")).unwrap_or_default();
    assert!(!audit.contains("operator forced leave"));
}

/// `locus mcp --supervised` cannot grant executor capability over a dead
/// pin: it still serves control tools, and its own fallback line must name
/// the same enter-first recovery. That stderr line is printed by `mcp`
/// itself, so it never passes through the top-level recovery hatch.
#[test]
fn supervised_mcp_fallback_on_dead_pin_names_the_recovery() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let cap = pin_in_exited_shell(home.path(), cwd.path());

    // Stub locus-mcp that exits at once; only locus's own stderr is asserted.
    let stub = cwd.path().join("fake-locus-mcp");
    std::fs::write(&stub, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let out = locus_fail_closed(
        home.path(),
        cwd.path(),
        &cap,
        &["mcp", "--supervised", "--mcp-bin", stub.to_str().unwrap()],
    );
    let err = stderr(&out);
    assert!(out.status.success(), "fallback still serves: {err}");
    assert!(err.contains("serving control tools only"), "{err}");
    assert!(
        err.contains(DEAD_BROKER_RECOVERY),
        "mcp --supervised fallback must name the enter-first recovery: {err}"
    );
}

// ── Negative tests for the replace-stale branch of `enter` (#156) ─────────

/// Number of `session.replace_stale` audit records under `home`.
fn replace_stale_audits(home: &Path) -> usize {
    std::fs::read_dir(home.join("audit"))
        .map(|dir| {
            dir.map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap_or_default())
                .map(|s| s.matches("session.replace_stale").count())
                .sum()
        })
        .unwrap_or(0)
}

fn active_bytes(home: &Path) -> Vec<u8> {
    std::fs::read(home.join("sessions/active.json")).expect("active.json")
}

/// Wait until the pin under `home` can no longer be verified.
fn wait_dead(home: &Path, cwd: &Path, cap: &str) {
    for _ in 0..100 {
        if !locus(home, cwd, cap, &["whoami"]).status.success() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("pin stayed verifiable after its supervising shell exited");
}

/// Rewrite the dead active pin with `edit`, resealed with the store seal key
/// (an attacker who can read `daemon.key`), or not resealed.
fn edit_active(home: &Path, reseal: bool, edit: impl FnOnce(&mut locus_core::session::Session)) {
    let path = home.join("sessions/active.json");
    let mut s: locus_core::session::Session =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    edit(&mut s);
    if reseal {
        let key = std::fs::read_to_string(home.join("daemon.key")).unwrap();
        s.reseal(&locus_core::seal::SealKey::from_hex(&key).unwrap());
    }
    std::fs::write(&path, serde_json::to_vec_pretty(&s).unwrap()).unwrap();
}

/// Assert `enter acme` is refused, leaves active.json byte-identical and
/// writes no `session.replace_stale` record (item 5 for every case).
fn assert_refused_untouched(out: &Output, home: &Path, before: &[u8], case: &str) {
    assert!(!out.status.success(), "{case}: enter must be refused");
    assert_eq!(active_bytes(home), before, "{case}: active.json changed");
    assert_eq!(
        replace_stale_audits(home),
        0,
        "{case}: replace_stale audited"
    );
}

/// (1) Live pin + wrong capability: refused by the live broker
/// (authorize_control), active.json untouched; the right capability still
/// replaces the live pin, and that is not a stale replacement.
#[test]
fn live_pin_replacement_still_runs_authorize_control() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    // Pin from this live test process so the broker stays up.
    let qs = Command::new(env!("CARGO_BIN_EXE_locus"))
        .arg("quickstart")
        .env("LOCUS_HOME", home.path())
        .env_remove("LOCUS_CONTROL_CAPABILITY")
        .current_dir(cwd.path())
        .output()
        .unwrap();
    assert!(qs.status.success(), "quickstart: {}", stderr(&qs));
    let cap = std::fs::read_to_string(home.path().join("control_capability")).unwrap();
    let cap = cap.trim();
    assert!(locus(home.path(), cwd.path(), cap, &["whoami"])
        .status
        .success());
    let before = active_bytes(home.path());

    let wrong = "ab".repeat(32);
    let out = locus_fail_closed(home.path(), cwd.path(), &wrong, &["enter", "acme"]);
    assert_refused_untouched(&out, home.path(), &before, "live pin, wrong capability");
    let err = stderr(&out);
    assert!(
        err.contains("a live authority broker answered but refused this control capability"),
        "a live pin must be gated by the live broker's authentication: {err}"
    );

    let ok = locus(home.path(), cwd.path(), cap, &["enter", "acme"]);
    assert!(ok.status.success(), "right capability: {}", stderr(&ok));
    assert_ne!(active_bytes(home.path()), before);
    assert_eq!(
        replace_stale_audits(home.path()),
        0,
        "live replacement is not stale"
    );
}

/// (2) An explicit LOCUS_SESSION_ID keeps the strict check on a dead pin.
#[test]
fn explicit_session_id_on_dead_pin_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let cap = pin_in_exited_shell(home.path(), cwd.path());
    let before = active_bytes(home.path());
    let v: serde_json::Value = serde_json::from_slice(&before).unwrap();
    let id = v["session_id"].as_str().unwrap();

    let out = locus_cmd(home.path(), cwd.path(), &cap, &["enter", "acme"])
        .env("LOCUS_SESSION_ID", id)
        .env("LOCUS_AUTHORITY_BROKER_START_TIMEOUT_MS", "500")
        .output()
        .unwrap();
    assert_refused_untouched(&out, home.path(), &before, "explicit LOCUS_SESSION_ID");
}

/// (3a) A dead pin whose seal was tampered with is refused.
#[test]
fn dead_pin_with_tampered_seal_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let cap = pin_in_exited_shell(home.path(), cwd.path());
    edit_active(home.path(), false, |s| s.seal = "deadbeef".into());
    let before = active_bytes(home.path());

    let out = locus_fail_closed(home.path(), cwd.path(), &cap, &["enter", "acme"]);
    assert_refused_untouched(&out, home.path(), &before, "tampered seal");
    assert!(
        stderr(&out).contains("locus leave --force; locus enter"),
        "{}",
        stderr(&out)
    );
}

/// Expired pins require explicit teardown even with an authentic seal and
/// the correct operator capability. Backdate the fixture instead of sleeping.
#[test]
fn expired_pin_requires_force_before_a_fresh_pin() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let cap = pin_in_exited_shell(home.path(), cwd.path());
    edit_active(home.path(), true, |s| {
        let now = chrono::Utc::now();
        s.pinned_at = now - chrono::Duration::hours(2);
        s.expires_at = now - chrono::Duration::hours(1);
    });
    let before = active_bytes(home.path());
    let old: locus_core::session::Session = serde_json::from_slice(&before).unwrap();
    assert!(old.is_expired());
    assert!(!locus(home.path(), cwd.path(), &cap, &["whoami"])
        .status
        .success());
    let out = locus_fail_closed(home.path(), cwd.path(), &cap, &["enter", "personal"]);
    assert_refused_untouched(&out, home.path(), &before, "expired pin");
    assert!(stderr(&out).contains("session expired"), "{}", stderr(&out));
    let status = locus(home.path(), cwd.path(), &cap, &["status"]);
    let diagnostic = String::from_utf8_lossy(&status.stdout);
    assert!(
        diagnostic.contains("locus leave --force; locus enter personal"),
        "{diagnostic}"
    );
    let leave = locus(home.path(), cwd.path(), &cap, &["leave", "--force"]);
    assert!(leave.status.success(), "{}", stderr(&leave));
    let enter = locus(home.path(), cwd.path(), &cap, &["enter", "personal"]);
    assert!(enter.status.success(), "{}", stderr(&enter));
    let new: locus_core::session::Session =
        serde_json::from_slice(&active_bytes(home.path())).unwrap();
    assert_ne!(new.session_id, old.session_id);
    assert_eq!(new.binding_id, old.binding_id);
    assert!(!new.is_expired());
    assert!(locus(home.path(), cwd.path(), &cap, &["whoami"])
        .status
        .success());
    assert_eq!(replace_stale_audits(home.path()), 0);
    let audit = std::fs::read_to_string(home.path().join("audit/events.jsonl")).unwrap();
    assert!(audit.contains("session.force_leave"));
}

/// Expired pins must still refuse an invalid operator or seal;
/// it cannot replace delegated or explicitly selected sessions.
#[test]
fn expired_pin_replacement_keeps_authentication_gates() {
    for case in ["wrong capability", "tampered seal", "delegated", "explicit"] {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let cap = pin_in_exited_shell(home.path(), cwd.path());
        edit_active(home.path(), true, |s| {
            s.pinned_at = chrono::Utc::now() - chrono::Duration::hours(2);
            s.expires_at = chrono::Utc::now() - chrono::Duration::hours(1);
            if case == "delegated" {
                s.authority = locus_core::session::SessionAuthority::Delegated;
            }
        });
        if case == "tampered seal" {
            edit_active(home.path(), false, |s| s.seal = "deadbeef".into());
        }
        let before = active_bytes(home.path());
        let session: locus_core::session::Session = serde_json::from_slice(&before).unwrap();
        let presented = if case == "wrong capability" {
            "not-the-operator-capability"
        } else {
            &cap
        };
        let mut command = locus_cmd(home.path(), cwd.path(), presented, &["enter", "personal"]);
        if case == "explicit" {
            command.env("LOCUS_SESSION_ID", &session.session_id);
        }
        let out = command.output().unwrap();
        assert_refused_untouched(&out, home.path(), &before, case);
    }
}

/// (3b) A dead pin whose sealed authority is not local control is refused,
/// even when resealed with the store key.
#[test]
fn dead_non_local_control_pin_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let cap = pin_in_exited_shell(home.path(), cwd.path());
    edit_active(home.path(), true, |s| {
        s.authority = locus_core::session::SessionAuthority::Delegated
    });
    let before = active_bytes(home.path());

    let out = locus_fail_closed(home.path(), cwd.path(), &cap, &["enter", "acme"]);
    assert_refused_untouched(&out, home.path(), &before, "delegated pin");
    assert!(
        stderr(&out).contains("requires an authenticated local-control session"),
        "{}",
        stderr(&out)
    );
}

/// (4) Strict posture (`init --no-persist-capability`): no persisted
/// verifier exists once the broker is gone, so the dead pin is not replaced
/// (only `leave --force --no-verifier` can clear it).
#[test]
fn dead_pin_without_persisted_capability_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let cap = "cd".repeat(32);
    let status = Command::new("sh")
        .arg("-c")
        .arg("\"$0\" init --with-samples --no-persist-capability >/dev/null 2>&1 && \"$0\" enter personal >/dev/null 2>&1; true")
        .arg(env!("CARGO_BIN_EXE_locus"))
        .env("LOCUS_HOME", home.path())
        .env("LOCUS_CONTROL_CAPABILITY", &cap)
        .current_dir(cwd.path())
        .status()
        .unwrap();
    assert!(status.success());
    assert!(!home.path().join("control_capability").exists());
    let before = active_bytes(home.path());
    wait_dead(home.path(), cwd.path(), &cap);

    let out = locus_fail_closed(home.path(), cwd.path(), &cap, &["enter", "acme"]);
    assert_refused_untouched(&out, home.path(), &before, "no persisted capability");
    assert!(stderr(&out).contains("--no-verifier"), "{}", stderr(&out));
}

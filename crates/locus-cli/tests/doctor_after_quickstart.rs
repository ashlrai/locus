//! Issue #172: `locus doctor` right after `locus quickstart` reports the
//! expected fresh state as WARN advisories and exits 0; a state that is
//! actually broken still exits non-zero (1 for a WARN with findings the
//! advisory allowlist does not cover).

use serde_json::Value;
use std::process::{Command, Output};

/// Warn findings a fresh setup is expected to carry. Anything outside this
/// list means the state is broken and doctor must fail.
const FRESH_STATE_ADVISORIES: &[&str] = &[
    "approvals_missing",
    "phantom_missing",
    "control_capability_not_exported",
    "executor_authority_unavailable",
];

struct Operator {
    home: tempfile::TempDir,
    project: tempfile::TempDir,
}

impl Operator {
    fn new() -> Self {
        Self {
            home: tempfile::tempdir().unwrap(),
            project: tempfile::tempdir().unwrap(),
        }
    }

    fn locus_home(&self) -> std::path::PathBuf {
        self.home.path().join("locus")
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_locus"))
            .args(args)
            .env_clear()
            .env("HOME", self.home.path())
            .env("LOCUS_HOME", self.locus_home())
            // Make the optional Phantom installation independent of the host.
            .env("PATH", "/nonexistent")
            .env("NO_COLOR", "1")
            .current_dir(self.project.path())
            .output()
            .expect("locus runs")
    }

    fn quickstart(&self) {
        let out = self.run(&["quickstart"]);
        assert!(out.status.success(), "quickstart failed: {out:?}");
    }
}

#[test]
fn doctor_exits_zero_after_fresh_quickstart() {
    let operator = Operator::new();
    operator.quickstart();

    for args in [&["doctor"][..], &["doctor", "--json"][..]] {
        let out = operator.run(args);
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        if args.contains(&"--json") {
            let report: Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(report["verdict"], "WARN");
            // Exit 0 is only honest when every WARN finding is an expected
            // fresh-state advisory; anything else must keep doctor failing.
            for finding in report["findings"].as_array().unwrap() {
                if finding["severity"] == "warn" {
                    let code = finding["code"].as_str().unwrap();
                    assert!(
                        FRESH_STATE_ADVISORIES.contains(&code),
                        "unexpected non-advisory warning {code}: {report}"
                    );
                }
            }
        }
    }
}

#[test]
#[cfg(unix)]
fn doctor_exits_zero_after_quickstart_and_shell_hook() {
    let operator = Operator::new();
    let bin = operator.home.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    // The hook uses cat, but Phantom must remain absent regardless of the host.
    std::os::unix::fs::symlink("/bin/cat", bin.join("cat")).unwrap();
    // Keep the quickstart broker's parent shell alive, as in the documented
    // interactive setup, then export the persisted capability via the hook.
    let out = Command::new("/bin/bash")
        .arg("-c")
        .arg("\"$1\" quickstart >/dev/null && eval \"$(\"$1\" hook bash)\" && \"$1\" doctor --json")
        .arg("locus-test")
        .arg(env!("CARGO_BIN_EXE_locus"))
        .env_clear()
        .env("HOME", operator.home.path())
        .env("LOCUS_HOME", operator.locus_home())
        .env("PATH", &bin)
        .env("NO_COLOR", "1")
        .current_dir(operator.project.path())
        .output()
        .expect("setup shell runs");
    assert_eq!(out.status.code(), Some(0), "setup and doctor must succeed");
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["verdict"], "WARN");
    assert_eq!(report["runtime"]["authority_anchor_ok"], true);
    let findings = report["findings"].as_array().unwrap();
    assert!(findings.iter().any(|f| f["code"] == "phantom_missing"));
    assert!(!findings
        .iter()
        .any(|f| f["code"] == "control_capability_not_exported"));
}

#[test]
fn doctor_still_exits_one_for_a_broken_state() {
    let operator = Operator::new();
    operator.quickstart();
    // Break the state: remove the persisted control capability. Doctor must
    // surface `control_capability_missing`, which is not a fresh-state
    // advisory, so WARN goes back to failing.
    std::fs::remove_file(operator.locus_home().join("control_capability"))
        .expect("quickstart persisted a control capability");

    for args in [&["doctor"][..], &["doctor", "--json"][..]] {
        let out = operator.run(args);
        assert_eq!(out.status.code(), Some(1), "{out:?}");
        if args.contains(&"--json") {
            let report: Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(report["verdict"], "WARN");
            assert!(report["findings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| { f["code"] == "control_capability_missing" && f["severity"] == "warn" }));
        }
    }
}

//! A fresh quickstart has setup advice, not a failed diagnostic.

use serde_json::Value;
use std::process::{Command, Output};

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

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_locus"))
            .args(args)
            .env_clear()
            .env("HOME", self.home.path())
            .env("LOCUS_HOME", self.home.path().join("locus"))
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
fn quickstart_then_doctor_succeeds_with_visible_advisories() {
    let operator = Operator::new();
    operator.quickstart();

    for args in [&["doctor"][..], &["doctor", "--json"][..]] {
        let out = operator.run(args);
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        if args.contains(&"--json") {
            let report: Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(report["verdict"], "WARN");
            assert_eq!(report["status"], "warn");
            assert_eq!(report["ok"], false, "exit success does not claim readiness");
            for code in ["phantom_missing", "control_capability_not_exported"] {
                assert!(
                    report["findings"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|f| { f["code"] == code && f["severity"] == "warn" }),
                    "missing advisory {code}: {report}"
                );
            }
            let phantom_finding = report["findings"]
                .as_array()
                .unwrap()
                .iter()
                .find(|finding| finding["code"] == "phantom_missing")
                .unwrap();
            assert!(phantom_finding["message"]
                .as_str()
                .unwrap()
                .contains("phantom not on PATH"));
        } else {
            let text = String::from_utf8(out.stdout).unwrap();
            // Keep the README's stable output excerpt tied to a real fresh run.
            let readme = include_str!("../../../README.md");
            let excerpt = readme
                .split_once("<!-- fresh-doctor-output:begin -->")
                .unwrap()
                .1
                .split_once("<!-- fresh-doctor-output:end -->")
                .unwrap()
                .0;
            let lines: Vec<_> = excerpt
                .lines()
                .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with("```"))
                .collect();
            assert_eq!(lines.len(), 2, "README excerpt must include both lines");
            for line in lines {
                assert!(
                    text.lines().any(|actual| actual == line),
                    "missing documented line {line:?}: {text}"
                );
            }
        }
    }
}

#[test]
fn changelog_preserves_the_published_first_run_observation() {
    let changelog = include_str!("../../../CHANGELOG.md");
    let note = changelog
        .split_once("**Fresh quickstart and doctor WARN (")
        .expect("first-run issue has a changelog entry")
        .1
        .split_once("\n\n")
        .unwrap()
        .0;
    for detail in [
        "issues/172",
        "fresh home",
        "non-interactive stdin",
        "Phantom absent from PATH",
        "published npm 0.5.1",
        "`locus quickstart` exit 0",
        "`locus doctor` exiting 1",
        "`WARN`",
        "`phantom_missing`",
    ] {
        assert!(note.contains(detail), "missing first-run detail: {detail}");
    }
}

#[test]
fn quickstart_then_doctor_still_fails_for_invalid_workspace_policy() {
    let operator = Operator::new();
    operator.quickstart();
    std::fs::write(operator.project.path().join(".locus.toml"), "invalid = [").unwrap();

    let out = operator.run(&["doctor", "--json"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["verdict"], "UNSAFE");
    assert!(report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| { f["code"] == "workspace_policy_invalid" && f["severity"] == "unsafe" }));
}

#[test]
fn quickstart_then_doctor_still_fails_for_invalid_control_capability() {
    let operator = Operator::new();
    operator.quickstart();
    let out = Command::new(env!("CARGO_BIN_EXE_locus"))
        .args(["doctor", "--json"])
        .env_clear()
        .env("HOME", operator.home.path())
        .env("LOCUS_HOME", operator.home.path().join("locus"))
        .env("LOCUS_CONTROL_CAPABILITY", "invalid")
        .env("PATH", "/nonexistent")
        .current_dir(operator.project.path())
        .output()
        .unwrap();
    // An invalid supplied capability makes the active pin unverifiable.
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["verdict"], "UNSAFE");
    assert!(report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| { f["code"] == "control_capability_invalid" && f["severity"] == "warn" }));
}

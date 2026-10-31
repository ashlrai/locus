//! Regression pins for issues whose fixes landed before their issues were closed.
use std::process::{Command, Output};

struct Fixture {
    home: tempfile::TempDir,
    project: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self {
            home: tempfile::tempdir().unwrap(),
            project: tempfile::tempdir().unwrap(),
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_locus"));
        cmd.args(args)
            .env_clear()
            .env("HOME", self.home.path())
            .env("LOCUS_HOME", self.home.path().join("locus"))
            .env("PATH", "/nonexistent")
            .env("NO_COLOR", "1")
            .current_dir(self.project.path());
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("locus runs")
    }
}

#[test]
fn issue_172_fresh_doctor_is_advisory_but_broken_state_fails() {
    let f = Fixture::new();
    assert!(f.run(&["quickstart"]).status.success());
    let out = f.run(&["doctor", "--json"]);
    assert_eq!(out.status.code(), Some(0));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["verdict"], "WARN");
    assert!(report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["code"] == "phantom_missing"));
    std::fs::remove_file(f.home.path().join("locus/control_capability")).unwrap();
    let out = f.run(&["doctor", "--json"]);
    assert_eq!(out.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["code"] == "control_capability_missing"));
}

#[test]
fn issue_173_whoami_reports_presence_without_disclosing_credentials() {
    let f = Fixture::new();
    assert!(f.run(&["quickstart"]).status.success());
    let cap = std::fs::read_to_string(f.home.path().join("locus/control_capability")).unwrap();
    assert!(f
        .command(&["enter", "personal"])
        .env("LOCUS_CONTROL_CAPABILITY", cap.trim())
        .output()
        .unwrap()
        .status
        .success());
    for present in [false, true] {
        let mut cmd = f.command(&["whoami", "--json"]);
        cmd.env("LOCUS_CONTROL_CAPABILITY", cap.trim());
        if present {
            cmd.env("LOCUS_SUPABASE_PERSONAL", "synthetic-presence-canary");
        }
        let out = cmd.output().unwrap();
        assert!(out.status.success());
        let output = String::from_utf8(out.stdout).unwrap();
        let report: serde_json::Value = serde_json::from_str(&output).unwrap();
        let provider = report["providers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["provider"] == "supabase")
            .unwrap();
        assert_eq!(provider["credential"]["present"], present);
        assert_eq!(provider["credential"]["source"], "environment");
        assert_eq!(provider["credential"].as_object().unwrap().len(), 2);
        assert!(!output.contains("synthetic-presence-canary"));
        assert!(!output.contains("LOCUS_SUPABASE_PERSONAL"));
    }
}

#[test]
fn issue_177_older_npm_wrapper_preserves_newer_binary() {
    let home = tempfile::tempdir().unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let status = Command::new("node")
        .args([
            "--test",
            "--test-name-pattern",
            "an older wrapper preserves a newer executable",
            "npm/test/no-downgrade.test.js",
        ])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .current_dir(root)
        .output()
        .expect("Node.js is required to test the npm wrapper");
    assert!(status.status.success(), "npm downgrade regression failed");
    let stdout = String::from_utf8(status.stdout).unwrap();
    assert!(
        stdout.contains("# pass 1"),
        "the named npm regression must run exactly once"
    );
}

#[test]
fn issue_182_incomplete_canary_scan_fails_closed() {
    use super::fail_closed_unreadable::{scan, scan_with};
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("unreadable");
    std::fs::write(&file, "fixture-canary").unwrap();
    let err = scan_with(root.path(), "fixture-canary", &mut |_| {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "fixture denied",
        ))
    })
    .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(err.to_string().contains(&file.display().to_string()));
    assert!(scan(root.path(), "fixture-canary").unwrap());
    assert!(scan(&root.path().join("missing"), "fixture-canary").is_err());
}

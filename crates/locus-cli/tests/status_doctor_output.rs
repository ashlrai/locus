//! Snapshot the public CLI wire format and retain the human/shell forms.
use std::process::{Command, Output};

struct Sandbox {
    home: tempfile::TempDir,
    cwd: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        Self {
            home: tempfile::tempdir().unwrap(),
            cwd: tempfile::tempdir().unwrap(),
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_locus"))
            .args(args)
            .env_clear()
            .env("HOME", self.home.path())
            .env("LOCUS_HOME", self.home.path().join("locus"))
            .env("PATH", "")
            .env("NO_COLOR", "1")
            .current_dir(self.cwd.path())
            .output()
            .unwrap()
    }

    fn normalized(&self, bytes: &[u8]) -> String {
        String::from_utf8(bytes.to_vec())
            .unwrap()
            .replace(
                self.home.path().canonicalize().unwrap().to_str().unwrap(),
                "<HOME>",
            )
            .replace(
                self.cwd.path().canonicalize().unwrap().to_str().unwrap(),
                "<CWD>",
            )
            .replace(self.home.path().to_str().unwrap(), "<HOME>")
            .replace(self.cwd.path().to_str().unwrap(), "<CWD>")
    }
}

#[test]
fn status_json_and_text_snapshots() {
    let sandbox = Sandbox::new();
    for args in [["status", "--json"], ["--json", "status"]] {
        let out = sandbox.run(&args);
        assert!(out.status.success());
        assert!(out.stderr.is_empty());
        assert_eq!(
            sandbox.normalized(&out.stdout),
            include_str!("snapshots/status.json")
        );
    }
    let out = sandbox.run(&["status"]);
    assert!(out.status.success());
    assert_eq!(
        sandbox.normalized(&out.stdout),
        include_str!("snapshots/status.txt")
    );
    let out = sandbox.run(&["status", "--oneline"]);
    assert!(out.status.success());
    assert_eq!(out.stdout, b"unpinned\n");
}

#[test]
fn doctor_json_and_text_snapshots() {
    let sandbox = Sandbox::new();
    for args in [["doctor", "--json"], ["--json", "doctor"]] {
        let out = sandbox.run(&args);
        assert_eq!(out.status.code(), Some(1));
        assert!(out.stderr.is_empty());
        let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let actual = sandbox.normalized(serde_json::to_string(&value).unwrap().as_bytes());
        assert_eq!(actual, include_str!("snapshots/doctor.json").trim_end());
    }
    let out = sandbox.run(&["doctor"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stderr.is_empty());
    assert_eq!(
        sandbox.normalized(&out.stdout),
        include_str!("snapshots/doctor.txt")
    );
}

//! Matching adds must preserve operator edits and audit history without
//! bypassing control authorization or accepting changes to frozen scope.

use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

struct Fixture(tempfile::TempDir);

impl Fixture {
    fn new() -> Self {
        let fixture = Self(tempfile::tempdir().unwrap());
        assert!(fixture.run(&["init"], false).status.success());
        fixture
    }

    fn home(&self) -> std::path::PathBuf {
        self.0.path().join("locus")
    }

    fn binding(&self) -> std::path::PathBuf {
        self.home().join("bindings/acme.toml")
    }

    fn run(&self, args: &[&str], authorized: bool) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_locus"));
        command
            .env_clear()
            .env("HOME", self.0.path())
            .env("LOCUS_HOME", self.home())
            .env("NO_COLOR", "1")
            .current_dir(self.0.path())
            .args(args);
        if authorized {
            let capability = fs::read_to_string(self.home().join("control_capability")).unwrap();
            command.env("LOCUS_CONTROL_CAPABILITY", capability.trim());
        }
        command.output().unwrap()
    }

    fn add(&self, command: &str, extra: &[&str], authorized: bool) -> Output {
        let mut args = vec!["--json", command];
        args.extend("add acme --tenant acme --provider github --account acme --credential-ref env:BINDING_NOOP_FIXTURE --non-interactive".split_whitespace());
        args.extend_from_slice(extra);
        self.run(&args, authorized)
    }
}

fn json_success(output: Output) -> Value {
    assert!(output.status.success(), "binding add must succeed");
    assert!(output.stderr.is_empty());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], true);
    value
}

// Use assert! for comparisons so failures never dump snapshot contents.
fn snapshot(path: &Path) -> (Vec<u8>, SystemTime, fs::Permissions) {
    let metadata = fs::metadata(path).unwrap();
    (
        fs::read(path).unwrap(),
        metadata.modified().unwrap(),
        metadata.permissions(),
    )
}

#[test]
fn repeated_matching_adds_preserve_binding_metadata_and_audit_history() {
    let fixture = Fixture::new();
    assert_eq!(
        json_success(fixture.add("binding", &[], true))["changed"],
        true
    );
    let path = fixture.binding();
    let original = format!(
        "# Keep this operator comment.\n\n{}",
        fs::read_to_string(&path).unwrap()
    );
    fs::write(&path, original).unwrap();
    // An old timestamp makes even a fast rewrite observable without sleeps.
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(
            fs::FileTimes::new().set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
        )
        .unwrap();
    let binding_before = snapshot(&path);
    let audit_path = fixture.home().join("audit/events.jsonl");
    let audit_before = snapshot(&audit_path);

    for command in ["binding", "client", "binding"] {
        let result = json_success(fixture.add(command, &[], true));
        assert_eq!(result["changed"], false);
        assert_eq!(
            Path::new(result["path"].as_str().unwrap()),
            path.canonicalize().unwrap()
        );
        assert!(
            snapshot(&path) == binding_before,
            "matching add altered binding"
        );
        assert!(
            snapshot(&audit_path) == audit_before,
            "matching add altered audit history"
        );
    }
}

#[test]
fn scope_policy_and_description_changes_require_force() {
    let fixture = Fixture::new();
    json_success(fixture.add("binding", &[], true));
    let before = snapshot(&fixture.binding());
    let audit_path = fixture.home().join("audit/events.jsonl");
    let audit_before = snapshot(&audit_path);
    for change in [
        &["--org", "other-org"][..],
        &["--repos", "acme/other-repo"],
        &["--project-ref", "other-project"],
        &["--read-only"],
        &["--default-ttl", "1h"],
        &["--description", "operator change"],
    ] {
        let output = fixture.add("binding", change, true);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["ok"], false);
        assert!(error["error"].as_str().unwrap().contains("--force"));
        assert!(
            snapshot(&fixture.binding()) == before,
            "refused add altered binding"
        );
        assert!(
            snapshot(&audit_path) == audit_before,
            "refused add altered audit history"
        );
    }
    let result = json_success(fixture.add("binding", &["--org", "other-org", "--force"], true));
    assert_eq!(result["changed"], true);
    let binding =
        locus_core::Binding::parse_toml(&fs::read_to_string(fixture.binding()).unwrap()).unwrap();
    assert_eq!(binding.providers[0].scope.orgs, ["other-org"]);
    assert!(
        snapshot(&audit_path) != audit_before,
        "replacement must be audited"
    );
}

#[test]
fn matching_add_still_requires_control_authorization() {
    let fixture = Fixture::new();
    json_success(fixture.add("binding", &[], true));
    let before = snapshot(&fixture.binding());
    for command in ["binding", "client"] {
        let output = fixture.add(command, &[], false);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["ok"], false);
        assert!(error["error"].as_str().unwrap().contains("control"));
        assert!(
            snapshot(&fixture.binding()) == before,
            "unauthorized add altered binding"
        );
    }
}

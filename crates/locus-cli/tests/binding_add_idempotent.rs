//! Issue #175: `binding add` is an idempotent upsert — repeating it with
//! values that already match the saved binding is a true no-op (exit 0,
//! reports no change, binding bytes/mtime and audit history untouched), so
//! agents can add-then-retry safely. A changed add still refuses without
//! `--force`, and the no-op never bypasses control authorization.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

const CAP: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self {
            root: tempfile::tempdir().unwrap(),
        }
    }

    fn home(&self) -> PathBuf {
        self.root.path().join("locus")
    }

    fn binding(&self) -> PathBuf {
        self.home().join("bindings").join("same.toml")
    }

    fn audit(&self) -> PathBuf {
        self.home().join("audit").join("events.jsonl")
    }

    // env_clear: an inherited LOCUS_CONTROL_CAPABILITY or LOCUS_SESSION_ID
    // from the outer shell must not leak authority into the child.
    fn run(&self, args: &[&str], authorized: bool) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_locus"));
        cmd.env_clear()
            .env("HOME", self.root.path())
            .env("LOCUS_HOME", self.home())
            .env("NO_COLOR", "1")
            .current_dir(self.root.path())
            .args(args);
        if authorized {
            cmd.env("LOCUS_CONTROL_CAPABILITY", CAP);
        }
        cmd.output().expect("locus binary runs")
    }

    fn add(&self, command: &str, extra: &[&str], authorized: bool) -> Output {
        let mut args = vec!["--json", command];
        args.extend(
            "add same --tenant same --provider github --account acct --credential-ref env:SAME_TOKEN --non-interactive"
                .split_whitespace(),
        );
        args.extend_from_slice(extra);
        self.run(&args, authorized)
    }
}

fn json_success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "binding add must exit 0: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "stderr stays clean on success: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let v: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(v["ok"], true, "{v}");
    v
}

fn json_error(output: &Output, needle: &str) {
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(
        output.stdout, output.stderr,
        "stdout carries the JSON error"
    );
    let v: Value = serde_json::from_slice(&output.stderr)
        .unwrap_or_else(|e| panic!("--json failure must print JSON on stderr ({e})"));
    assert_eq!(v["ok"], false, "{v}");
    assert!(
        v["error"].as_str().unwrap().contains(needle),
        "{v} must mention {needle}"
    );
}

/// File state a true no-op must not disturb: bytes, mtime, permissions.
fn snapshot(path: &Path) -> (Vec<u8>, SystemTime, fs::Permissions) {
    let metadata = fs::metadata(path).unwrap();
    (
        fs::read(path).unwrap(),
        metadata.modified().unwrap(),
        metadata.permissions(),
    )
}

#[test]
fn repeated_matching_adds_are_a_true_noop() {
    let f = Fixture::new();
    assert_eq!(
        json_success(f.add("binding", &[], true))["changed"],
        true,
        "first add writes the binding"
    );
    let path = f.binding();
    // An operator comment survives the no-op: equivalence is semantic
    // (parsed TOML), so a canonicalizing rewrite would lose it.
    fs::write(
        &path,
        format!(
            "# Keep this operator comment.\n\n{}",
            fs::read_to_string(&path).unwrap()
        ),
    )
    .unwrap();
    // A deliberately old timestamp makes even a fast rewrite observable.
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(
            fs::FileTimes::new().set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
        )
        .unwrap();
    let binding_before = snapshot(&path);
    let audit_before = snapshot(&f.audit());

    // `binding add` and `client add` share the write path; each repeat must
    // report no change and leave file, mtime and audit history untouched.
    for command in ["binding", "client", "binding"] {
        let v = json_success(f.add(command, &[], true));
        assert_eq!(v["changed"], false, "{v}");
        assert_eq!(
            Path::new(v["path"].as_str().unwrap()),
            path.canonicalize().unwrap(),
            "{v}"
        );
        assert_eq!(snapshot(&path), binding_before, "matching add wrote");
        assert_eq!(snapshot(&f.audit()), audit_before, "matching add audited");
    }
    // Human output contract: json_errors.rs pins the "already matches" text.
}

#[test]
fn concurrent_identical_adds_write_once() {
    let f = Fixture::new();
    json_success(f.run(
        &[
            "--json",
            "binding",
            "add",
            "other",
            "--tenant",
            "other",
            "--provider",
            "github",
            "--account",
            "acct",
            "--credential-ref",
            "env:OTHER_TOKEN",
            "--non-interactive",
        ],
        true,
    ));
    let audit_before = fs::read_to_string(f.audit()).unwrap().lines().count();

    let (first, second) = std::thread::scope(|scope| {
        let first = scope.spawn(|| json_success(f.add("binding", &[], true)));
        let second = scope.spawn(|| json_success(f.add("binding", &[], true)));
        (first.join().unwrap(), second.join().unwrap())
    });
    assert_ne!(first["changed"], second["changed"]);
    assert_eq!(
        fs::read_to_string(f.audit()).unwrap().lines().count(),
        audit_before + 1
    );

    let before = snapshot(&f.binding());
    assert_eq!(json_success(f.add("binding", &[], true))["changed"], false);
    assert_eq!(snapshot(&f.binding()), before);
}

#[test]
fn a_different_add_still_requires_force() {
    let f = Fixture::new();
    json_success(f.add("binding", &[], true));
    let binding_before = snapshot(&f.binding());
    let audit_before = snapshot(&f.audit());

    // Frozen scope, policy and description changes are all "different":
    // each must refuse without --force and leave everything untouched.
    for change in [
        &["--org", "other-org"][..],
        &["--read-only"][..],
        &["--description", "operator change"][..],
    ] {
        let out = f.add("binding", change, true);
        json_error(&out, "--force");
        assert_eq!(snapshot(&f.binding()), binding_before, "refused add wrote");
        assert_eq!(snapshot(&f.audit()), audit_before, "refused add audited");
    }

    // --force replaces a genuinely different binding and audits the write.
    let v = json_success(f.add("binding", &["--org", "other-org", "--force"], true));
    assert_eq!(v["changed"], true, "{v}");
    assert!(fs::read_to_string(f.binding())
        .unwrap()
        .contains("other-org"));
    assert_ne!(
        snapshot(&f.audit()).0,
        audit_before.0,
        "a real replacement must be audited"
    );
}

#[test]
fn a_matching_add_still_requires_control_authority() {
    let f = Fixture::new();
    json_success(f.add("binding", &[], true));
    let binding_before = snapshot(&f.binding());

    // The no-op is still a control-plane operation: without the capability
    // it fails closed, even though nothing would have been written.
    for command in ["binding", "client"] {
        let out = f.add(command, &[], false);
        json_error(&out, "control");
        assert_eq!(
            snapshot(&f.binding()),
            binding_before,
            "unauthorized add wrote"
        );
    }
}

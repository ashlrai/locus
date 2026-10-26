//! First-run regressions #174 and #175. No real HOME, credentials or provider calls.
use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output};

struct Operator {
    root: tempfile::TempDir,
}

impl Operator {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("project")).unwrap();
        let operator = Self { root };
        assert!(operator.run(&["quickstart"]).status.success());
        operator
    }

    fn home(&self) -> PathBuf {
        self.root.path().join("locus")
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_locus"));
        cmd.args(args)
            .env_clear()
            .env("HOME", self.root.path())
            .env("LOCUS_HOME", self.home())
            .env("NO_COLOR", "1")
            .env("PATH", "/usr/bin:/bin")
            .current_dir(self.root.path().join("project"));
        if let Ok(cap) = std::fs::read_to_string(self.home().join("control_capability")) {
            cmd.env("LOCUS_CONTROL_CAPABILITY", cap.trim());
        }
        cmd
    }

    fn audit(&self) -> String {
        std::fs::read_dir(self.home().join("audit"))
            .unwrap()
            .map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap())
            .collect()
    }
}

fn add_args() -> Vec<&'static str> {
    vec![
        "binding",
        "add",
        "retry",
        "--tenant",
        "client",
        "--provider",
        "github",
        "--account",
        "client",
        "--credential-ref",
        "env:FIRSTRUN_CREDENTIAL",
        "--org",
        "client",
        "--read-only",
        "--non-interactive",
        "--json",
    ]
}

#[test]
fn issue_175_matching_binding_add_is_a_noop_but_differences_require_force() {
    let operator = Operator::new();
    let args = add_args();
    let first = operator.run(&args);
    assert!(first.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&first.stdout).unwrap()["unchanged"],
        false
    );
    let path = operator.home().join("bindings/retry.toml");
    let annotated = format!(
        "# Preserve operator comments\n{}",
        std::fs::read_to_string(&path).unwrap()
    );
    std::fs::write(&path, &annotated).unwrap();
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    let audit = operator.audit();

    let again = operator.run(&args);
    assert!(again.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&again.stdout).unwrap()["unchanged"],
        true
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), annotated);
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        modified
    );
    assert_eq!(operator.audit(), audit, "no duplicate binding.save receipt");

    let mut dry = args.clone();
    dry.push("--dry-run");
    let preview = operator.run(&dry);
    assert!(preview.status.success());
    let plan: Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(plan["unchanged"], true);
    assert!(plan["changes"].as_array().unwrap().is_empty());
    assert_eq!(operator.audit(), audit);

    // Every security-relevant selector participates in equality.
    for (flag, value) in [
        ("--tenant", "other"),
        ("--credential-ref", "env:OTHER_CREDENTIAL"),
        ("--org", "other"),
        ("--account", "other"),
    ] {
        let mut changed = args.clone();
        let index = changed.iter().position(|a| *a == flag).unwrap();
        changed[index + 1] = value;
        let refused = operator.run(&changed);
        assert!(
            !refused.status.success(),
            "different {flag} must require --force"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), annotated);
        assert_eq!(operator.audit(), audit);
    }
    let mut changed = args;
    let index = changed.iter().position(|a| *a == "--org").unwrap();
    changed[index + 1] = "other";
    changed.push("--force");
    assert!(operator.run(&changed).status.success());
    assert!(std::fs::read_to_string(&path).unwrap().contains("other"));
}

#[cfg(unix)]
#[test]
fn issue_174_sample_exec_has_one_setup_note_and_strict_creds_still_refuses() {
    let operator = Operator::new();
    // Assert absence inside the isolated child without printing any values.
    let script = "test -z \"${GH_TOKEN-}${GITHUB_TOKEN-}${SUPABASE_ACCESS_TOKEN-}${VERCEL_TOKEN-}\" && printf ok";
    let args = [
        "exec", "--expect", "personal", "--", "/bin/sh", "-c", script,
    ];
    let out = operator.run(&args);
    assert!(out.status.success());
    assert_eq!(out.stdout, b"ok");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert_eq!(stderr.matches("note: sample binding").count(), 1);
    assert!(!stderr.contains("warn"));
    assert!(!stderr.contains("env="));
    let events: Vec<Value> = operator
        .audit()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let execution = events
        .iter()
        .find(|event| event["op"] == "session.exec")
        .unwrap();
    let failures = execution["detail"]["secrets_failed"].as_array().unwrap();
    assert_eq!(
        failures.len(),
        3,
        "presentation must preserve failure evidence"
    );
    assert!(failures
        .iter()
        .all(|failure| failure["code"] == "unavailable"));

    let marker = operator.root.path().join("child-started");
    let command = format!("touch {}", marker.display());
    let strict = operator.run(&["exec", "--strict-creds", "--", "/bin/sh", "-c", &command]);
    assert!(!strict.status.success());
    assert!(!marker.exists(), "strict refusal precedes child spawn");
    assert!(String::from_utf8_lossy(&strict.stderr).contains("credential resolve failed"));

    // Edited samples are real bindings: failures stay visible individually.
    let path = operator.home().join("bindings/personal.toml");
    let edited = std::fs::read_to_string(&path).unwrap().replace(
        "Personal projects — sample; edit scopes before real use",
        "Configured operator binding",
    );
    std::fs::write(path, edited).unwrap();
    assert!(operator.run(&["enter", "personal"]).status.success());
    let out = operator.run(&args);
    assert!(out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert_eq!(stderr.matches("warn unresolved credential").count(), 3);
    assert!(!stderr.contains("note: sample binding"));
}

#[cfg(unix)]
#[test]
fn issue_175_matching_symlink_is_never_a_successful_noop() {
    use std::os::unix::fs::symlink;
    let operator = Operator::new();
    let args = add_args();
    assert!(operator.run(&args).status.success());
    let path = operator.home().join("bindings/retry.toml");
    let target = operator.home().join("bindings/copy.toml");
    std::fs::rename(&path, &target).unwrap();
    symlink(&target, &path).unwrap();
    for force in [false, true] {
        let mut args = args.clone();
        if force {
            args.push("--force");
        }
        assert!(!operator.run(&args).status.success());
        assert!(path.symlink_metadata().unwrap().file_type().is_symlink());
    }
}

#[test]
fn issue_175_concurrent_matching_adds_write_and_audit_once() {
    let operator = Operator::new();
    let mut children: Vec<_> = (0..6)
        .map(|_| {
            operator
                .command(&add_args())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let mut writes = 0;
    for child in children.drain(..) {
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "matching concurrent setup must succeed"
        );
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        if result["unchanged"] == false {
            writes += 1;
        }
    }
    assert_eq!(writes, 1);
    let receipts = operator
        .audit()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|event| event["op"] == "binding.save" && event["binding"] == "retry")
        .count();
    assert_eq!(receipts, 1);
}

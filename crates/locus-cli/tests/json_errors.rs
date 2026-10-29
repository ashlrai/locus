//! Agent-facing CLI contracts: `--json` failures stay machine-readable, and
//! `locus workspace` names default/allowed aliases that have no binding yet.

use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};

fn locus(home: &Path, cwd: &Path, cap: Option<&str>, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_locus"));
    cmd.args(args)
        .env("LOCUS_HOME", home)
        .env("NO_COLOR", "1")
        .env_remove("LOCUS_SESSION_ID")
        .current_dir(cwd);
    match cap {
        Some(c) => cmd.env("LOCUS_CONTROL_CAPABILITY", c),
        None => cmd.env_remove("LOCUS_CONTROL_CAPABILITY"),
    };
    cmd.output().expect("locus binary runs")
}

fn assert_json_error(out: &Output, needle: &str) {
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "stderr: {stderr}");
    assert!(out.stdout.is_empty(), "stdout stays clean on failure");
    let v: Value = serde_json::from_str(stderr.trim())
        .unwrap_or_else(|e| panic!("--json failure must print JSON on stderr ({e}): {stderr:?}"));
    assert_eq!(v["ok"], false, "{v}");
    let msg = v["error"].as_str().expect("error string");
    assert!(msg.contains(needle), "{v}");
}

#[test]
fn json_flag_turns_command_errors_into_json() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    // Global flag before and after the subcommand.
    assert_json_error(
        &locus(home.path(), cwd.path(), None, &["--json", "whoami"]),
        "no active pin",
    );
    assert_json_error(
        &locus(home.path(), cwd.path(), None, &["whoami", "--json"]),
        "no active pin",
    );
    assert_json_error(
        &locus(
            home.path(),
            cwd.path(),
            None,
            &["--json", "binding", "show", "nope"],
        ),
        "binding not found",
    );
}

#[test]
fn without_json_flag_errors_stay_human() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let out = locus(home.path(), cwd.path(), None, &["whoami"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("error: no active pin"));
}

#[test]
fn workspace_warns_on_aliases_without_a_binding() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let init = locus(home.path(), cwd.path(), None, &["init", "--with-samples"]);
    assert!(init.status.success(), "{init:?}");
    let cap = std::fs::read_to_string(home.path().join("control_capability")).unwrap();
    let cap = cap.trim();

    let out = locus(
        home.path(),
        cwd.path(),
        Some(cap),
        &["workspace", "--default", "ghost", "--allow", "personal"],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "advisory only: {stderr}");
    assert!(cwd.path().join(".locus.toml").exists());
    assert!(
        stderr.contains("binding 'ghost' does not exist")
            && stderr.contains("locus binding add ghost"),
        "{stderr}"
    );
    assert!(!stderr.contains("'personal'"), "existing binding: {stderr}");
}

const CAP: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[test]
fn binding_add_refuses_to_replace_an_existing_alias_without_force() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let add = |account: &str, extra: &[&str]| {
        let mut args = vec![
            "--json",
            "binding",
            "add",
            "dup",
            "--tenant",
            "dup",
            "--provider",
            "github",
            "--account",
            account,
            "--credential-ref",
            "env:DUP_TOKEN",
            "--non-interactive",
        ];
        args.extend_from_slice(extra);
        locus(home.path(), cwd.path(), Some(CAP), &args)
    };
    let toml = home.path().join("bindings").join("dup.toml");

    let first = add("first", &[]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let original = std::fs::read_to_string(&toml).unwrap();

    let second = add("second", &[]);
    assert_json_error(&second, "binding 'dup' already exists");
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        stderr.contains("locus binding show dup") && stderr.contains("--force"),
        "{stderr}"
    );
    assert_eq!(std::fs::read_to_string(&toml).unwrap(), original);

    // A dry run still previews without writing.
    let dry = add("second", &["--dry-run"]);
    assert!(dry.status.success());
    assert_eq!(std::fs::read_to_string(&toml).unwrap(), original);

    let forced = add("second", &["--force"]);
    assert!(
        forced.status.success(),
        "{}",
        String::from_utf8_lossy(&forced.stderr)
    );
    assert!(std::fs::read_to_string(&toml)
        .unwrap()
        .contains("\"second\""));

    // A mistyped alias on `binding show` names the real one.
    let show = locus(home.path(), cwd.path(), None, &["binding", "show", "dupp"]);
    let stderr = String::from_utf8_lossy(&show.stderr);
    assert!(!show.status.success());
    assert!(stderr.contains("did you mean `dup`?"), "{stderr}");
}

/// Issue #175: re-running `binding add` with the same values is a no-op that
/// exits 0 instead of refusing. The file is not rewritten (bytes and mtime
/// unchanged); `--json` reports `changed: false`.
#[test]
fn binding_add_is_a_noop_when_the_file_already_matches() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let add = |extra: &[&str]| {
        let mut args = vec![
            "--json",
            "binding",
            "add",
            "same",
            "--tenant",
            "same",
            "--provider",
            "github",
            "--account",
            "acct",
            "--credential-ref",
            "env:SAME_TOKEN",
            "--non-interactive",
        ];
        args.extend_from_slice(extra);
        locus(home.path(), cwd.path(), Some(CAP), &args)
    };
    let toml = home.path().join("bindings").join("same.toml");

    let first = add(&[]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&first.stdout)).unwrap();
    assert_eq!(v["changed"], true, "{v}");
    let original = std::fs::read_to_string(&toml).unwrap();
    let modified = std::fs::metadata(&toml).unwrap().modified().unwrap();

    let second = add(&[]);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&second.stdout)).unwrap();
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["changed"], false, "{v}");
    assert_eq!(std::fs::read_to_string(&toml).unwrap(), original);
    assert_eq!(
        std::fs::metadata(&toml).unwrap().modified().unwrap(),
        modified
    );

    // Human output says the same thing.
    let human = locus(
        home.path(),
        cwd.path(),
        Some(CAP),
        &[
            "binding",
            "add",
            "same",
            "--tenant",
            "same",
            "--provider",
            "github",
            "--account",
            "acct",
            "--credential-ref",
            "env:SAME_TOKEN",
            "--non-interactive",
        ],
    );
    assert!(
        human.status.success(),
        "{}",
        String::from_utf8_lossy(&human.stderr)
    );
    assert!(
        String::from_utf8_lossy(&human.stdout).contains("already matches; nothing written"),
        "{}",
        String::from_utf8_lossy(&human.stdout)
    );

    // A dry run plans zero changes and says why.
    let dry = add(&["--dry-run"]);
    assert!(
        dry.status.success(),
        "{}",
        String::from_utf8_lossy(&dry.stderr)
    );
    let plan: Value = serde_json::from_str(&String::from_utf8_lossy(&dry.stdout)).unwrap();
    assert_eq!(plan["changes"], serde_json::json!([]), "{plan}");
    assert!(
        plan["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n.as_str().unwrap().contains("already matches")),
        "{plan}"
    );
    assert_eq!(std::fs::read_to_string(&toml).unwrap(), original);
}

#[cfg(unix)]
#[test]
fn prompted_existing_alias_is_compared_before_replacement() {
    use std::io::Write;
    use std::os::fd::FromRawFd;
    use std::process::Stdio;

    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let flags = [
        "--tenant",
        "same",
        "--provider",
        "github",
        "--account",
        "acct",
        "--credential-ref",
        "env:SAME_TOKEN",
        "--org",
        "example",
        "--repos",
        "example/repo",
        "--read-only",
        "--default-ttl",
        "2h",
    ];
    let mut initial = vec!["binding", "add", "same", "--non-interactive"];
    initial.extend(flags);
    assert!(locus(home.path(), cwd.path(), Some(CAP), &initial)
        .status
        .success());
    let path = home.path().join("bindings/same.toml");
    let original = std::fs::read(&path).unwrap();
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();

    let prompted = |command: &str, account: &str, input: &[u8]| {
        let (mut master, mut slave) = (-1, -1);
        // Both descriptors are owned immediately after openpty succeeds.
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            },
            0
        );
        let mut master = unsafe { std::fs::File::from_raw_fd(master) };
        let slave = unsafe { std::fs::File::from_raw_fd(slave) };
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_locus"));
        cmd.args([
            command,
            "add",
            "--tenant",
            "same",
            "--provider",
            "github",
            "--account",
            account,
            "--credential-ref",
            "env:SAME_TOKEN",
            "--org",
            "example",
            "--repos",
            "example/repo",
            "--read-only",
            "--default-ttl",
            "2h",
        ])
        .env("LOCUS_HOME", home.path())
        .env("LOCUS_CONTROL_CAPABILITY", CAP)
        .env("NO_COLOR", "1")
        .env_remove("LOCUS_SESSION_ID")
        .env_remove("LOCUS_NON_INTERACTIVE")
        .current_dir(cwd.path())
        .stdin(Stdio::from(slave))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        let child = cmd.spawn().unwrap();
        master.write_all(input).unwrap();
        child.wait_with_output().unwrap()
    };

    for command in ["binding", "client"] {
        let out = prompted(command, "acct", b"same\n\x04");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(String::from_utf8_lossy(&out.stdout).contains("already matches; nothing written"));
    }
    let refused = prompted("binding", "other", b"same\n\x04");
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("already exists"));
    let different = prompted("client", "other", b"same\nn\n\x04");
    assert!(!different.status.success());
    assert!(String::from_utf8_lossy(&different.stdout).contains("replace it?"));
    assert!(String::from_utf8_lossy(&different.stderr).contains("aborted"));
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        modified
    );
}

#[cfg(unix)]
#[test]
fn binding_add_never_writes_through_a_dangling_symlink() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let victim = outside.path().join("victim.toml");
    let bindings = home.path().join("bindings");
    std::fs::create_dir_all(&bindings).unwrap();
    std::os::unix::fs::symlink(&victim, bindings.join("dang.toml")).unwrap();
    let add = |extra: &[&str]| {
        let mut args = vec![
            "--json",
            "binding",
            "add",
            "dang",
            "--tenant",
            "dang",
            "--provider",
            "github",
            "--account",
            "acct",
            "--credential-ref",
            "env:DANG_TOKEN",
            "--non-interactive",
        ];
        args.extend_from_slice(extra);
        locus(home.path(), cwd.path(), Some(CAP), &args)
    };
    // The link is an existing entry: refused like any existing alias.
    assert_json_error(&add(&[]), "binding 'dang' already exists");
    assert!(!victim.exists(), "nothing written outside LOCUS_HOME");
    // --force replaces a binding, never the file a link points at.
    assert_json_error(&add(&["--force"]), "symlink");
    assert!(!victim.exists(), "nothing written outside LOCUS_HOME");
}

#[test]
fn credential_errors_distinguish_unsupported_and_missing_without_exposing_values() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let canary = format!("SECRET_CANARY_{}", std::process::id());
    for (alias, reference, expected) in [
        (
            "phantom",
            format!("phm:{canary}"),
            "Phantom credential integration unsupported",
        ),
        (
            "missing",
            "env:LOCUS_DIAG_MISSING_VALUE".into(),
            "environment variable not set",
        ),
    ] {
        let add = locus(
            home.path(),
            cwd.path(),
            Some(CAP),
            &[
                "binding",
                "add",
                alias,
                "--tenant",
                alias,
                "--provider",
                "github",
                "--account",
                "acct",
                "--credential-ref",
                &reference,
                "--non-interactive",
            ],
        );
        assert!(add.status.success());
        for json in [false, true] {
            let mut cmd = Command::new(env!("CARGO_BIN_EXE_locus"));
            if json {
                cmd.arg("--json");
            }
            let out = cmd
                .args(["run", "-b", alias, "--strict-creds", "--", "/usr/bin/true"])
                .env("LOCUS_HOME", home.path())
                .env("LOCUS_CONTROL_CAPABILITY", CAP)
                .env("LOCUS_DIAG_CANARY", &canary)
                .env_remove("LOCUS_DIAG_MISSING_VALUE")
                .env_remove("LOCUS_SESSION_ID")
                .current_dir(cwd.path())
                .output()
                .unwrap();
            assert!(!out.status.success());
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(!stdout.contains(&canary));
            assert!(!stderr.contains(&canary));
            let message = if json {
                let value: Value = serde_json::from_str(stderr.trim()).unwrap();
                value["error"].as_str().unwrap().to_owned()
            } else {
                stderr.into_owned()
            };
            assert!(message.contains(expected));
            assert!(message.contains("env:VAR"));
        }
    }
}

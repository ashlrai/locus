//! Keep terminal stdin open without input: JSON commands must finish unaided.
#![cfg(unix)]

use serde_json::Value;
use std::fs::File;
use std::io::IsTerminal;
use std::os::fd::FromRawFd;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn run(home: &Path, args: &[&str], code: i32) -> Value {
    let (mut master, mut slave) = (-1, -1);
    // Successful openpty transfers both descriptors to the owning Files below.
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
    let _master = unsafe { File::from_raw_fd(master) };
    let slave = unsafe { File::from_raw_fd(slave) };
    assert!(slave.is_terminal());
    let mut child = Command::new(env!("CARGO_BIN_EXE_locus"))
        .args(args)
        .env_clear()
        .env("HOME", home)
        .env("LOCUS_HOME", home)
        // Synthetic capability only; no ambient identity or noninteractive override.
        .env("LOCUS_CONTROL_CAPABILITY", "01".repeat(32))
        .env("NO_COLOR", "1")
        // Never open a desktop dialog, even when testing the broken implementation.
        .env("LOCUS_TOUCHID_MOCK", "ok")
        .current_dir(home)
        .stdin(Stdio::from(slave))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("blocked waiting for terminal input: {args:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(code), "{args:?}: {out:?}");
    for stream in [&out.stdout, &out.stderr] {
        let text = String::from_utf8_lossy(stream);
        assert!(!text.contains("[y/N]") && !text.contains("[Y/n]"), "{text}");
    }
    if code == 0 {
        assert!(out.stderr.is_empty(), "{out:?}");
        serde_json::from_slice(&out.stdout).unwrap()
    } else {
        assert!(out.stdout.is_empty(), "{out:?}");
        let report: Value = serde_json::from_slice(&out.stderr).unwrap();
        assert_eq!(report["ok"], false);
        assert_eq!(report["schema"], "locus.error.v1");
        report
    }
}

#[test]
fn json_commands_never_prompt_on_a_tty() {
    let home = tempfile::tempdir().unwrap();
    run(
        home.path(),
        &[
            "binding",
            "add",
            "fixture",
            "--tenant",
            "fixture",
            "--provider",
            "github",
            "--account",
            "fixture",
            "--credential-ref",
            "env:UNSET_FIXTURE",
            "--json",
        ],
        0,
    );
    let binding = home.path().join("bindings/fixture.toml");
    let original = std::fs::read(&binding).unwrap();
    for leading_json in [true, false] {
        let check = |args: &[&str], code| {
            let args = if leading_json {
                [&["--json"][..], args].concat()
            } else {
                [args, &["--json"][..]].concat()
            };
            run(home.path(), &args, code)
        };
        let report = check(&["adapter", "uninstall", "fixture"], 2);
        assert!(report["error"].as_str().unwrap().contains("--yes"));
        let report = check(&["binding", "rm", "fixture"], 1);
        assert!(report["error"].as_str().unwrap().contains("--yes"));
        assert_eq!(std::fs::read(&binding).unwrap(), original);
        assert_eq!(check(&["leave"], 0)["left"], false);
        assert_eq!(check(&["leave", "--force"], 0)["left"], false);
        check(&["binding", "add"], 1);
        check(&["client", "add"], 1);
        assert_eq!(
            check(&["adapter", "uninstall", "fixture", "--yes"], 0)["removed"],
            false
        );
    }
    assert_eq!(
        run(
            home.path(),
            &["binding", "rm", "fixture", "--yes", "--json"],
            0
        )["removed"],
        "fixture"
    );
    assert!(!binding.exists());
}

#[test]
fn json_touchid_is_refused_before_confirmation_or_mutation() {
    let home = tempfile::tempdir().unwrap();
    let store = locus_core::Store::open(home.path()).unwrap();
    // Construct the fixture directly so notification opt-ins cannot launch a UI.
    let record = locus_core::ApprovalRecord {
        id: "appr_aabbccddeeff001122334455".into(),
        tool: "github.delete".into(),
        binding: "fixture".into(),
        args_digest: "synthetic".into(),
        created_at: chrono::Utc::now(),
        status: locus_core::ApprovalStatus::Pending,
        session_id: "fixture".into(),
        requester: "fixture".into(),
        grants: vec![],
        expires_at: None,
        granted_at: None,
    };
    let path = store.approvals_dir().join(format!("{}.json", record.id));
    let original = serde_json::to_vec(&record).unwrap();
    std::fs::write(&path, &original).unwrap();
    for leading_json in [true, false] {
        let args = [
            "approve",
            "grant",
            &record.id,
            "--as",
            "fixture",
            "--touchid",
        ];
        let args = if leading_json {
            [&["--json"][..], &args].concat()
        } else {
            [&args[..], &["--json"][..]].concat()
        };
        let report = run(home.path(), &args, 2);
        assert!(report["error"]
            .as_str()
            .unwrap()
            .contains("--touchid requires an interactive invocation"));
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }
}

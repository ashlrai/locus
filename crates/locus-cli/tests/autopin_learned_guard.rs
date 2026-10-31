//! Learned winners must pass every apply gate; a weaker match cannot replace one.
use locus_core::pin_history;
use std::path::Path;
use std::process::{Command, Output};

fn locus(home: &Path, cwd: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_locus"));
    cmd.current_dir(cwd)
        .env("LOCUS_HOME", home)
        .env("LOCUS_PIN_HISTORY", "1")
        .env_remove("LOCUS_CONTROL_CAPABILITY")
        .args(args);
    if args.contains(&"--apply") {
        let cap = std::fs::read_to_string(home.join("control_capability")).unwrap();
        cmd.env("LOCUS_CONTROL_CAPABILITY", cap.trim());
    }
    cmd.output().unwrap()
}

fn setup() -> (tempfile::TempDir, tempfile::TempDir) {
    let home = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let repo = parent.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    for args in [
        &["init", "-q"][..],
        &[
            "remote",
            "add",
            "origin",
            "https://example.invalid/widgets.git",
        ][..],
    ] {
        assert!(Command::new("git")
            .current_dir(&repo)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    assert!(locus(home.path(), &repo, &["init", "--with-samples"])
        .status
        .success());
    let key = pin_history::history_key(home.path()).unwrap();
    for (alias, count) in [("acme", 5), ("personal", 3)] {
        for _ in 0..count {
            let entry = pin_history::entry_for(&key, "pin", Some(alias), None, Some(&repo));
            pin_history::append(home.path(), &entry, 2000).unwrap();
        }
    }
    let output = locus(home.path(), &repo, &["autopin", "suggest", "--json"]);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["report"]["suggestion"], "acme");
    assert_eq!(value["report"]["candidates"][1]["score"], 3);
    (home, parent)
}

fn refused(home: &Path, cwd: &Path, reason: &str) {
    let history = std::fs::read(home.join(pin_history::FILE_NAME)).unwrap();
    for args in [
        &["autopin", "suggest", "--apply", "--dry-run", "--json"][..],
        &["autopin", "suggest", "--apply", "--json"][..],
    ] {
        let output = locus(home, cwd, args);
        assert!(!output.status.success(), "unsafe learned winner accepted");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(reason),
            "expected refusal: {reason}"
        );
        assert!(!home.join("sessions/active.json").exists());
        assert!(std::fs::read(home.join(pin_history::FILE_NAME)).unwrap() == history);
    }
}

#[test]
fn deleted_winner_refuses_instead_of_using_existing_runner_up() {
    let (home, parent) = setup();
    let repo = parent.path().join("repo");
    std::fs::remove_file(home.path().join("bindings/acme.toml")).unwrap();
    refused(home.path(), &repo, "binding not found: acme");
    assert!(!repo.join(".locus.toml").exists());
}

#[test]
fn blocked_winner_refuses_instead_of_using_allowed_runner_up() {
    let (home, parent) = setup();
    let repo = parent.path().join("repo");
    let policy =
        "default_binding = \"personal\"\nallowed_bindings = [\"personal\"]\nrequire_pin = true\n";
    std::fs::write(repo.join(".locus.toml"), policy).unwrap();
    refused(home.path(), &repo, "not allowed");
    assert_eq!(
        std::fs::read_to_string(repo.join(".locus.toml")).unwrap(),
        policy
    );
}

#[test]
fn shared_parent_policy_is_neither_written_nor_shadowed() {
    let (home, parent) = setup();
    let repo = parent.path().join("repo");
    let policy = "require_pin = true\n";
    let file = parent.path().join(".locus.toml");
    std::fs::write(&file, policy).unwrap();
    refused(home.path(), &repo, "outside this repo");
    assert_eq!(std::fs::read_to_string(file).unwrap(), policy);
    assert!(!repo.join(".locus.toml").exists());
}

#[cfg(unix)]
#[test]
fn symlink_policy_refuses_and_preserves_target_and_link() {
    let (home, parent) = setup();
    let repo = parent.path().join("repo");
    let target = parent.path().join("policy.toml");
    let link = repo.join(".locus.toml");
    std::fs::write(&target, "require_pin = true\n").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    refused(home.path(), &repo, "symlink");
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "require_pin = true\n"
    );
    assert_eq!(std::fs::read_link(&link).unwrap(), target);
    std::fs::remove_file(&target).unwrap();
    refused(home.path(), &repo, "broken or unreadable");
    assert!(!target.exists());
    assert_eq!(std::fs::read_link(&link).unwrap(), target);
}

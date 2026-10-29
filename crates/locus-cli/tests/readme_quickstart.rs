//! The README "30-second start" must keep working: the check script extracts
//! the documented commands and replays them verbatim in a throwaway HOME, so
//! a stale quickstart step fails here before a new user hits it. These tests
//! run the script and prove it refuses a stale or reshaped README.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn script() -> PathBuf {
    repo_root().join("scripts/check-readme-quickstart.sh")
}

fn run(extra_args: &[&str], readme: Option<&Path>) -> Output {
    let mut cmd = Command::new("bash");
    cmd.arg(script())
        .arg("--bin")
        .arg(env!("CARGO_BIN_EXE_locus"))
        .args(extra_args)
        .current_dir(repo_root())
        .env_remove("LOCUS_HOME")
        .env_remove("LOCUS_CONTROL_CAPABILITY")
        .env_remove("LOCUS_SESSION_ID")
        .env_remove("LOCUS_SEAL");
    if let Some(readme) = readme {
        cmd.arg("--readme").arg(readme);
    }
    cmd.output().expect("check script runs")
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The extracted README steps replay clean: quickstart, hook, enter, whoami,
/// the allowed and refused `exec --expect`, the control-capability export,
/// pin_mismatch with no child side effects, and the `--dry-run` plan surface.
#[test]
fn readme_quickstart_block_replays_clean() {
    let out = run(&[], None);
    assert!(
        out.status.success(),
        "readme quickstart check failed:\n{}",
        text(&out)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("readme quickstart PASS"),
        "missing pass summary: {stdout}"
    );
    for step in [
        "locus quickstart",
        "locus enter personal",
        "locus exec --expect personal",
        "locus exec --expect acme",
    ] {
        assert!(stdout.contains(step), "step not replayed: {step}\n{stdout}");
    }
}

/// `--dry-run` prints the extracted plan (steps + expectations) and runs
/// nothing, so it works on hosts without a built pin or control capability.
#[test]
fn readme_quickstart_dry_run_prints_plan() {
    let out = run(&["--dry-run"], None);
    assert!(out.status.success(), "dry run failed: {}", text(&out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("locus quickstart"), "{stdout}");
    assert!(stdout.contains("locus exec --expect acme"), "{stdout}");
    assert!(stdout.contains("expect exit=1"), "{stdout}");
}

/// A README whose quickstart section vanished or was renamed must fail the
/// check instead of silently passing with zero extracted steps.
#[test]
fn readme_quickstart_fails_when_section_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let mangled = dir.path().join("README.md");
    let body = std::fs::read_to_string(repo_root().join("README.md")).unwrap();
    std::fs::write(
        &mangled,
        body.replace("## 30-second start", "## Quickstart"),
    )
    .unwrap();

    let out = run(&[], Some(&mangled));
    assert!(
        !out.status.success(),
        "renamed quickstart section still passed:\n{}",
        text(&out)
    );
    assert!(
        text(&out).contains("extracted 0 locus step(s)"),
        "unhelpful failure: {}",
        text(&out)
    );
}

/// A stale command inside the block (renamed subcommand) must fail the check
/// naming the step and the documented expectation.
#[test]
fn readme_quickstart_fails_on_a_stale_step() {
    let dir = tempfile::tempdir().unwrap();
    let mangled = dir.path().join("README.md");
    let body = std::fs::read_to_string(repo_root().join("README.md")).unwrap();
    assert!(
        body.contains("\nlocus quickstart "),
        "README quickstart line shape changed; update this test"
    );
    std::fs::write(
        &mangled,
        body.replacen("\nlocus quickstart ", "\nlocus bogus-start ", 1),
    )
    .unwrap();

    let out = run(&[], Some(&mangled));
    assert!(
        !out.status.success(),
        "stale quickstart step still passed:\n{}",
        text(&out)
    );
    assert!(
        text(&out).contains("locus bogus-start"),
        "failure did not name the stale step: {}",
        text(&out)
    );
}

/// Missing sample credentials must make the warning-free smoke check fail.
#[test]
fn readme_quickstart_fails_when_sample_credentials_are_missing() {
    let dir = tempfile::tempdir().unwrap();
    let mangled = dir.path().join("README.md");
    let body = std::fs::read_to_string(repo_root().join("README.md")).unwrap();
    let assignment = "LOCUS_GH_TOKEN_PERSONAL=sample ";
    assert!(body.contains(assignment), "sample assignment missing");
    std::fs::write(&mangled, body.replacen(assignment, "", 1)).unwrap();

    let out = run(&[], Some(&mangled));
    assert!(!out.status.success(), "missing credential was accepted");
    assert!(text(&out).contains("sample exec emitted a warning"));
}

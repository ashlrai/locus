//! `scripts/hub-smoke.sh` must allocate its temporary home, project directory
//! and error file under a custom `TMPDIR` (including one that contains
//! spaces), fall back to `/tmp` when `TMPDIR` is unset or empty, and clean
//! them up.
//!
//! The script runs from a copy inside a fake repository root whose
//! `target/debug/locus` is a stub, so no build, broker or real binding is
//! needed. The stub records what the script allocated while it is running.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const STUB: &str = r#"#!/usr/bin/env bash
case "$1 $2" in
  "agent report")
    if [[ "${3:-}" == "--json" ]]; then
      {
        printf 'home=%s\n' "$LOCUS_HOME"
        printf 'cwd=%s\n' "$(pwd -P)"
        for f in "${TMPDIR:-/tmp}"/locus-hub-smoke-err.*; do
          [[ -f "$f" ]] && printf 'err=%s\n' "$f"
        done
      } >>"$STUB_LOG"
    fi
    echo '{}'
    ;;
  "mcp mint") exit 1 ;;
  "--version "*) echo "locus stub" ;;
esac
exit 0
"#;

fn script_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/hub-smoke.sh")
}

fn recorded(log: &str, key: &str) -> PathBuf {
    let prefix = format!("{key}=");
    let line = log
        .lines()
        .find_map(|l| l.strip_prefix(&prefix))
        .unwrap_or_else(|| panic!("stub did not record {key}; log:\n{log}"));
    PathBuf::from(line)
}

struct Run {
    stdout: String,
    stderr: String,
    code: Option<i32>,
    home: PathBuf,
    proj: PathBuf,
    err: PathBuf,
}

/// Runs the script copy against the stub with `TMPDIR` set to `tmpdir`
/// (`None` removes it from the environment).
fn run_smoke(tmpdir: Option<&Path>, fixture_root: &Path) -> Run {
    // Fake repo root: scripts/hub-smoke.sh + stub target/debug/locus.
    let root = fixture_root.join("repo root");
    std::fs::create_dir_all(root.join("scripts")).unwrap();
    std::fs::create_dir_all(root.join("target/debug")).unwrap();
    std::fs::copy(script_path(), root.join("scripts/hub-smoke.sh")).unwrap();
    let stub = root.join("target/debug/locus");
    std::fs::write(&stub, STUB).unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

    let log = fixture_root.join("stub.log");
    let home = fixture_root.join("home");
    std::fs::create_dir(&home).unwrap();

    let mut cmd = Command::new("bash");
    cmd.arg(root.join("scripts/hub-smoke.sh"))
        .env("STUB_LOG", &log)
        .env("HOME", &home)
        .env_remove("LOCUS_HOME")
        .env_remove("LOCUS_SESSION_ID");
    match tmpdir {
        Some(dir) => cmd.env("TMPDIR", dir),
        None => cmd.env_remove("TMPDIR"),
    };
    let out = cmd.output().expect("run hub-smoke.sh");
    let log = std::fs::read_to_string(&log).expect("stub log");
    Run {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        code: out.status.code(),
        home: recorded(&log, "home"),
        proj: recorded(&log, "cwd"),
        err: recorded(&log, "err"),
    }
}

/// Asserts the three temp paths sit directly under `tmpdir`, are named as
/// expected, and were removed by the script's exit trap.
fn assert_under_and_cleaned(run: &Run, tmpdir: &Path) {
    // Stub emits `{}`, so the contract checks fail by design; temp allocation
    // and the stubbed init/pin must still have succeeded.
    assert_eq!(
        run.code,
        Some(1),
        "stdout:\n{}\nstderr:\n{}",
        run.stdout,
        run.stderr
    );
    assert!(
        !run.stderr.contains("could not create temporary"),
        "stderr:\n{}",
        run.stderr
    );
    let want = tmpdir.canonicalize().unwrap();
    for (path, prefix) in [
        (&run.home, "locus-hub-smoke."),
        (&run.proj, "locus-hub-proj."),
        (&run.err, "locus-hub-smoke-err."),
    ] {
        let parent = path.parent().unwrap().canonicalize().unwrap();
        assert_eq!(parent, want, "{path:?} not directly under {tmpdir:?}");
        let name = path.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with(prefix), "unexpected name {name}");
    }
    assert!(run
        .stdout
        .contains(&format!("LOCUS_HOME={}", run.home.display())));
    for path in [&run.home, &run.proj, &run.err] {
        assert!(!path.exists(), "{path:?} survived cleanup");
    }
}

#[test]
fn hub_smoke_allocates_and_cleans_temp_paths_under_tmpdir_with_spaces() {
    let fixture = tempfile::tempdir().unwrap();
    let fixture_root = fixture.path().canonicalize().unwrap();
    let tmpdir = fixture_root.join("tmp dir with  spaces").join("nested dir");
    std::fs::create_dir_all(&tmpdir).unwrap();

    let run = run_smoke(Some(&tmpdir), &fixture_root);
    assert_under_and_cleaned(&run, &tmpdir);
    // Nothing else was left in TMPDIR.
    assert_eq!(std::fs::read_dir(&tmpdir).unwrap().count(), 0);
}

#[test]
fn hub_smoke_falls_back_to_slash_tmp_when_tmpdir_unset_or_empty() {
    for tmpdir in [None, Some(Path::new(""))] {
        let fixture = tempfile::tempdir().unwrap();
        let fixture_root = fixture.path().canonicalize().unwrap();
        let run = run_smoke(tmpdir, &fixture_root);
        assert_under_and_cleaned(&run, Path::new("/tmp"));
    }
}

//! `scripts/hub-smoke.sh` must allocate its temporary home, project directory
//! and error file under a `TMPDIR` that contains spaces, and clean them up.
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
        for f in "$TMPDIR"/locus-hub-smoke-err.*; do
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

#[test]
fn hub_smoke_allocates_and_cleans_temp_paths_under_tmpdir_with_spaces() {
    let fixture = tempfile::tempdir().unwrap();
    let fixture_root = fixture.path().canonicalize().unwrap();

    // Fake repo root: scripts/hub-smoke.sh + stub target/debug/locus.
    let root = fixture_root.join("repo root");
    std::fs::create_dir_all(root.join("scripts")).unwrap();
    std::fs::create_dir_all(root.join("target/debug")).unwrap();
    std::fs::copy(script_path(), root.join("scripts/hub-smoke.sh")).unwrap();
    let stub = root.join("target/debug/locus");
    std::fs::write(&stub, STUB).unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

    let tmpdir = fixture_root.join("tmp dir with  spaces").join("nested dir");
    std::fs::create_dir_all(&tmpdir).unwrap();
    let log = fixture_root.join("stub.log");
    let home = fixture_root.join("home");
    std::fs::create_dir(&home).unwrap();

    let out = Command::new("bash")
        .arg(root.join("scripts/hub-smoke.sh"))
        .env("TMPDIR", &tmpdir)
        .env("STUB_LOG", &log)
        .env("HOME", &home)
        .env_remove("LOCUS_HOME")
        .env_remove("LOCUS_SESSION_ID")
        .output()
        .expect("run hub-smoke.sh");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    // Stub emits `{}`, so the contract checks fail by design; temp allocation
    // and the stubbed init/pin must still have succeeded.
    assert_eq!(
        out.status.code(),
        Some(1),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("could not create temporary"),
        "stderr:\n{stderr}"
    );

    let log = std::fs::read_to_string(&log).expect("stub log");
    let smoke_home = recorded(&log, "home");
    let smoke_proj = recorded(&log, "cwd");
    let smoke_err = recorded(&log, "err");

    for (path, prefix) in [
        (&smoke_home, "locus-hub-smoke."),
        (&smoke_proj, "locus-hub-proj."),
        (&smoke_err, "locus-hub-smoke-err."),
    ] {
        assert_eq!(
            path.parent(),
            Some(tmpdir.as_path()),
            "{path:?} not directly under TMPDIR"
        );
        let name = path.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with(prefix), "unexpected name {name}");
    }
    assert!(stdout.contains(&format!("LOCUS_HOME={}", smoke_home.display())));

    // Trap removed all three, and nothing else was left in TMPDIR.
    for path in [&smoke_home, &smoke_proj, &smoke_err] {
        assert!(!path.exists(), "{path:?} survived cleanup");
    }
    assert_eq!(std::fs::read_dir(&tmpdir).unwrap().count(), 0);
}

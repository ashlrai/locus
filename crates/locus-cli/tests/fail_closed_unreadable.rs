//! Shared Phantom/Locus canary scanner and its always-on fail-closed regressions.
//! Included by the CLI harness, the Phantom integration, and locus-core's PR
//! sandbox test gate so these checks require neither PHANTOM_BIN nor secrets.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Attach the offending path to a canary-scan IO error.
fn scan_err(p: &Path, e: io::Error) -> io::Error {
    io::Error::new(e.kind(), format!("{}: {e}", p.display()))
}

/// Collect regular files under `d` for the canary scan. Fails closed: a path
/// that cannot be listed or statted is an error, never a silent skip — an
/// unreadable directory could be hiding the canary.
pub(crate) fn walk(d: &Path) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for e in fs::read_dir(d).map_err(|e| scan_err(d, e))? {
        let p = e.map_err(|e| scan_err(d, e))?.path();
        let m = fs::symlink_metadata(&p).map_err(|e| scan_err(&p, e))?;
        if m.is_dir() {
            out.extend(walk(&p)?);
        } else if m.is_file() {
            out.push(p);
        }
    }
    Ok(out)
}

/// Read every discovered file; an incomplete scan is an error.
pub(crate) fn scan(root: &Path, canary: &str) -> io::Result<bool> {
    scan_with(root, canary, &mut |path| fs::read(path))
}

pub(crate) fn scan_with(
    root: &Path,
    canary: &str,
    read: &mut impl FnMut(&Path) -> io::Result<Vec<u8>>,
) -> io::Result<bool> {
    let mut found = false;
    for path in walk(root)? {
        let bytes = read(&path).map_err(|e| scan_err(&path, e))?;
        found |= String::from_utf8_lossy(&bytes).contains(canary);
    }
    Ok(found)
}

pub(crate) fn assert_canary_absent(root: &Path, result: io::Result<bool>) {
    assert!(
        !result.unwrap_or_else(|e| panic!("canary scan failed: {e}")),
        "canary found under {}",
        root.display()
    );
}

#[test]
fn readable_tree_is_scanned() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir(tmp.path().join("nested")).unwrap();
    fs::write(tmp.path().join("nested/file"), b"ordinary contents").unwrap();
    assert_canary_absent(tmp.path(), scan(tmp.path(), "fixture-canary"));
    fs::write(tmp.path().join("nested/file"), b"fixture-canary").unwrap();
    assert!(scan(tmp.path(), "fixture-canary").unwrap());
}

#[test]
fn unlistable_path_is_denied() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("not-a-directory");
    fs::write(&file, b"ordinary contents").unwrap();
    for root in [file, tmp.path().join("missing")] {
        let err = scan(&root, "fixture-canary").unwrap_err();
        assert!(err.to_string().contains(&root.display().to_string()));
    }
}

#[cfg(unix)]
#[test]
fn unreadable_scan_exits_nonzero() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    const CHILD_PATH: &str = "LOCUS_UNREADABLE_SCAN_FIXTURE";
    if let Some(path) = std::env::var_os(CHILD_PATH).map(PathBuf::from) {
        let root = path.parent().unwrap();
        // Root/capability-bearing runners can bypass mode bits. Inject EACCES
        // only there, exercising the same read-error path without skipping.
        let result = if fs::read(&path).is_err() {
            scan(root, "fixture-canary")
        } else {
            scan_with(root, "fixture-canary", &mut |p| {
                assert_eq!(p, path);
                Err(io::Error::from_raw_os_error(libc::EACCES))
            })
        };
        assert_canary_absent(root, result);
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("unreadable");
    fs::write(&file, b"ordinary contents").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
    // The shared module has different prefixes in each harness.
    let name = concat!(module_path!(), "::unreadable_scan_exits_nonzero");
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name.split_once("::").unwrap().1, "--nocapture"])
        .env_clear()
        .env("HOME", tmp.path())
        .env(CHILD_PATH, &file)
        .output()
        .unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(!output.status.success(), "incomplete scan succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("canary scan failed"), "{stderr}");
    assert!(stderr.contains(&file.display().to_string()), "{stderr}");
    assert!(stderr.contains("Permission denied"), "{stderr}");
    assert_canary_absent(tmp.path(), scan(tmp.path(), "fixture-canary"));
}

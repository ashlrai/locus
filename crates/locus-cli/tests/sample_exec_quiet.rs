#![cfg(unix)]

use std::path::Path;
use std::process::Command;

fn has_credential_warning(output: &str) -> bool {
    let output = output.to_lowercase();
    ["warn", "unset", "not set", "unresolved credential"]
        .iter()
        .any(|needle| output.contains(needle))
}

/// Replay the documented sample without ambient credentials; the shell that
/// created the pin must remain alive to supervise its broker.
#[test]
fn documented_sample_exec_succeeds_without_unset_warnings() {
    let guide = include_str!("../../../examples/sample-exec-q8.md");
    let block = guide
        .split_once("```bash\n")
        .expect("sample Bash block exists")
        .1
        .split_once("\n```")
        .expect("sample Bash block ends")
        .0;
    let (setup, exec) = block.rsplit_once('\n').expect("sample has an exec step");
    assert!(exec.starts_with("locus exec --expect personal --no-resolve -- "));

    let dir = tempfile::tempdir().unwrap();
    let bin_dir = Path::new(env!("CARGO_BIN_EXE_locus")).parent().unwrap();
    let mut paths = vec![bin_dir.to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let path = std::env::join_paths(paths).unwrap();
    let script = format!(
        "{setup}\n{exec} >\"$1\" 2>\"$2\"\n\
         locus exec --no-resolve -- true 2>\"$3\"\n\
         locus exec -- true 2>\"$4\"\n\
         if locus exec --strict-creds -- touch \"$6\" 2>\"$5\"; then exit 1; fi\n"
    );
    let stdout = dir.path().join("exec.stdout");
    let stderr = dir.path().join("exec.stderr");
    let hint_stderr = dir.path().join("hint.stderr");
    let resolved_stderr = dir.path().join("resolved.stderr");
    let strict_stderr = dir.path().join("strict.stderr");
    let marker = dir.path().join("child-started");
    let output = Command::new("bash")
        .args(["--noprofile", "--norc", "-c"])
        .arg(script)
        .arg("sample-exec")
        .arg(&stdout)
        .arg(&stderr)
        .arg(&hint_stderr)
        .arg(&resolved_stderr)
        .arg(&strict_stderr)
        .arg(&marker)
        .env_clear()
        .env("PATH", path)
        .env("HOME", dir.path())
        .env("TMPDIR", dir.path())
        .env("SHELL", "/bin/bash")
        .env("NO_COLOR", "1")
        .current_dir(dir.path())
        .output()
        .expect("bash runs the documented sample");
    assert!(output.status.success(), "sample Bash block failed");
    assert!(String::from_utf8_lossy(&output.stdout).contains("locus exec --no-resolve -- true"));
    assert_eq!(
        std::fs::read_to_string(stdout).unwrap().trim(),
        "sample exec succeeded"
    );
    let stderr = std::fs::read_to_string(stderr).unwrap();
    assert!(
        !has_credential_warning(&stderr),
        "sample exec emitted warning output"
    );
    let hint = std::fs::read_to_string(hint_stderr).unwrap();
    assert!(!has_credential_warning(&hint));

    // Explicit resolution still diagnoses missing credentials, and strict
    // mode must refuse to start the child rather than hiding the failures.
    let resolved = std::fs::read_to_string(resolved_stderr).unwrap();
    assert_eq!(resolved.matches("unresolved credential").count(), 3);
    let strict = std::fs::read_to_string(strict_stderr).unwrap();
    assert!(strict.contains("credential resolve failed (--strict-creds)"));
    assert!(!marker.exists());
}

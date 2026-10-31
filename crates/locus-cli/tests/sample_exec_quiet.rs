#![cfg(unix)]

use std::path::Path;
use std::process::Command;

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
    let script = format!("{setup}\n{exec} >\"$1\" 2>\"$2\"\n");
    let stdout = dir.path().join("exec.stdout");
    let stderr = dir.path().join("exec.stderr");
    let output = Command::new("bash")
        .args(["--noprofile", "--norc", "-c"])
        .arg(script)
        .arg("sample-exec")
        .arg(&stdout)
        .arg(&stderr)
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
    assert_eq!(
        std::fs::read_to_string(stdout).unwrap().trim(),
        "sample exec succeeded"
    );
    let stderr = std::fs::read_to_string(stderr).unwrap();
    let warnings: Vec<_> = stderr
        .lines()
        .filter(|line| {
            let line = line.to_lowercase();
            line.contains("warn") || line.contains("unset")
        })
        .collect();
    assert!(warnings.is_empty(), "sample exec emitted warning output");
}

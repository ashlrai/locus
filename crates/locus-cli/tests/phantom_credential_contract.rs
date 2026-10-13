//! Credential compatibility fixtures: synthetic stores, no provider/vault access.
use locus_core::{Binding, Store};
use std::path::Path;
use std::process::{Command, Output};

fn command(home: &Path, cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_locus"))
        .args(args)
        .env_clear()
        .env("LOCUS_HOME", home)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .current_dir(cwd)
        .output()
        .expect("locus runs")
}

fn store_with_phantom_binding(home: &Path) {
    let store = Store::open(home).unwrap();
    let binding = Binding::parse_toml(
        r#"
id = "bnd_contract"
alias = "contract"
tenant = "synthetic"

[[providers]]
provider = "github"
account = "synthetic"
credential_ref = "phm:REFERENCE_NAME_CANARY"
"#,
    )
    .unwrap();
    std::fs::write(
        store.bindings_dir().join("contract.toml"),
        binding.to_toml().unwrap(),
    )
    .unwrap();
}

#[test]
fn unsupported_phantom_run_and_ci_run_fail_before_child_or_session_effects() {
    for args in [
        vec!["run", "-b", "contract", "--", "unavailable-child-canary"],
        vec![
            "ci",
            "run",
            "-b",
            "contract",
            "--",
            "unavailable-child-canary",
        ],
    ] {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        store_with_phantom_binding(home.path());
        let output = command(home.path(), cwd.path(), &args);
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("Phantom credential integration unsupported"),
            "{stderr}"
        );
        assert!(stderr.contains("env:VAR"), "{stderr}");
        assert!(
            stderr.contains("before child or session effects"),
            "{stderr}"
        );
        assert!(!stderr.contains("REFERENCE_NAME_CANARY"), "{stderr}");
        assert!(
            !stderr.contains("spawn unavailable-child-canary"),
            "{stderr}"
        );
        assert!(output.stdout.is_empty());
        assert!(!home.path().join("active.json").exists());
        for directory in ["sessions", "workers", "runtime"] {
            assert!(std::fs::read_dir(home.path().join(directory))
                .unwrap()
                .next()
                .is_none());
        }
    }
}

#[cfg(unix)]
#[test]
fn unsupported_launch_does_not_invoke_a_phantom_executable_on_path() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    store_with_phantom_binding(home.path());
    let bin = cwd.path().join("phantom");
    let marker = cwd.path().join("phantom-invoked");
    std::fs::write(&bin, "#!/bin/sh\nprintf invoked > phantom-invoked\n").unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_locus"))
        .args(["run", "-b", "contract", "--", "unavailable-child-canary"])
        .env_clear()
        .env("LOCUS_HOME", home.path())
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("PATH", cwd.path())
        .current_dir(cwd.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("Phantom credential integration unsupported"));
    assert!(
        !marker.exists(),
        "credential preflight must precede every Phantom invocation"
    );
}

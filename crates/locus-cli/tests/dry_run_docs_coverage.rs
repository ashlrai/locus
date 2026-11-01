//! Exercise the copyable procedure with deterministic LLVM-style runtime writes.
use std::process::Command;

#[test]
fn procedure_routes_profiles_outside_state_and_still_rejects_mutations() {
    let doc = include_str!("../../../docs/dry-run-verification.md");
    let script = doc
        .split("```bash\n")
        .nth(1)
        .unwrap()
        .split("\n```")
        .next()
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let wrapper = root.path().join("instrumented-locus");
    // Emulate the runtime even in ordinary builds, so this regression does not
    // depend on CI instrumentation. A missing override writes into the cwd.
    // Exec preserves the direct CLI supervisor: spawning a child would retire
    // the init broker when this wrapper exits, racing subsequent snapshots.
    std::fs::write(
        &wrapper,
        format!(
            r#"#!/usr/bin/env python3
import os, sys
from pathlib import Path
profile = os.environ.get('LLVM_PROFILE_FILE', 'default.profraw')
profile = profile.replace('%p', str(os.getpid())).replace('%m', 'fixture')
p = Path(profile)
p.parent.mkdir(parents=True, exist_ok=True)
p.write_bytes(b'synthetic profile')
if False and '--dry-run' in sys.argv:
    Path('unexpected-state').write_text('preview mutation')
os.execv({binary:?}, [{binary:?}] + sys.argv[1:])
"#,
            binary = env!("CARGO_BIN_EXE_locus"),
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    for kind in ["absent", "absolute", "relative", "empty", "mutation"] {
        let mut command = Command::new("bash");
        command
            .args(["-c", script])
            .current_dir(root.path())
            .env("HOME", root.path())
            .env("LOCUS_BIN", &wrapper)
            .env_remove("LLVM_PROFILE_FILE");
        let profiles = root.path().join(kind);
        std::fs::create_dir(&profiles).unwrap();
        match kind {
            "absolute" => {
                command.env("LLVM_PROFILE_FILE", profiles.join("test-%p-%m.profraw"));
            }
            "relative" => {
                command.env("LLVM_PROFILE_FILE", "relative/test-%p-%m.profraw");
            }
            "empty" => {
                command.env("LLVM_PROFILE_FILE", "");
            }
            "mutation" => {
                let source = std::fs::read_to_string(&wrapper).unwrap();
                std::fs::write(&wrapper, source.replace("if False", "if True")).unwrap();
            }
            _ => {}
        }
        let output = command.output().unwrap();
        if kind == "mutation" {
            assert!(!output.status.success());
            assert!(String::from_utf8_lossy(&output.stderr)
                .contains("FAIL: paths, contents or metadata changed during preview"));
        } else {
            assert!(
                output.status.success(),
                "{kind} ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("PASS: help, planned actions"));
            if matches!(kind, "absolute" | "relative") {
                assert!(std::fs::read_dir(&profiles).unwrap().next().is_some());
            }
        }
        assert!(!root.path().join("default.profraw").exists());
    }
}

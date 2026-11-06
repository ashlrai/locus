//! Hermetic CLI children must retain coverage output outside their fixture.
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

fn profile_path(inherited: Option<OsString>, fallback: &Path) -> PathBuf {
    let path = inherited
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| fallback.join("locus-%p-%m.profraw"));
    // Resolve before changing the child's cwd. Never silently fall back to a
    // relative path, which would put runtime writes inside the fixture.
    std::path::absolute(path).expect("absolute coverage output path")
}

pub(super) fn preserve_profile_output(command: &mut Command, fallback: &Path) {
    command.env(
        "LLVM_PROFILE_FILE",
        profile_path(std::env::var_os("LLVM_PROFILE_FILE"), fallback),
    );
}

#[test]
fn preview_keeps_home_and_project_pristine_with_coverage() {
    let cwd = std::env::current_dir().unwrap();
    for profile_kind in ["absent", "absolute", "relative"] {
        // A relative inherited profile must be resolved against the parent's
        // cwd, even though the CLI runs in a different project directory.
        let root = tempfile::tempdir_in(&cwd).unwrap();
        let home = root.path().join("user");
        let project = root.path().join("project");
        let profiles = root.path().join("profiles");
        for path in [&home, &project, &profiles] {
            std::fs::create_dir(path).unwrap();
        }
        let before = [&home, &project].map(|p| std::fs::metadata(p).unwrap().modified().unwrap());
        let inherited_path = profiles.join("inherited-%p-%m.profraw");
        let inherited = match profile_kind {
            "absolute" => Some(inherited_path.clone().into()),
            "relative" => Some(inherited_path.strip_prefix(&cwd).unwrap().into()),
            _ => None,
        };
        let profile = profile_path(inherited, &profiles);
        let output = Command::new(env!("CARGO_BIN_EXE_locus"))
            .env_clear()
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            // Deliberately omit LOCUS_HOME: exercise the HOME fallback too.
            .env("LLVM_PROFILE_FILE", profile)
            .current_dir(&project)
            .args(["--dry-run", "init", "--with-samples", "--json"])
            .output()
            .unwrap();
        assert!(output.status.success(), "preview status: {}", output.status);
        let plan: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(plan["schema"], "locus.plan.v1");
        assert_eq!(plan["dry_run"], true);
        assert_eq!(
            plan["changes"][0]["target"],
            home.join(".locus").to_str().unwrap()
        );
        for (path, modified) in [&home, &project].into_iter().zip(before) {
            assert!(std::fs::read_dir(path).unwrap().next().is_none());
            assert_eq!(
                std::fs::metadata(path).unwrap().modified().unwrap(),
                modified
            );
        }
        if std::env::var_os("CARGO_LLVM_COV").is_some() {
            assert!(std::fs::read_dir(&profiles).unwrap().next().is_some());
        }
    }
}

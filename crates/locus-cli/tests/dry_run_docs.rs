//! Keep the documented dry-run command forms discoverable in CLI help and
//! exercise the first copyable preview in an uninitialized home.

use std::path::Path;
use std::process::{Command, Output};

fn locus(args: &[&str], home: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_locus"))
        .args(args)
        .env("HOME", home)
        .env("LOCUS_HOME", home.join("locus"))
        .env("NO_COLOR", "1")
        .env_remove("LOCUS_CONTROL_CAPABILITY")
        .current_dir(home)
        .output()
        .expect("run locus")
}

#[test]
fn documented_plan_commands_expose_dry_run_in_help() {
    let doc = include_str!("../../../docs/README.md");
    let table = doc
        .split("| Area | Command forms |")
        .nth(1)
        .expect("dry-run command table")
        .split("\n\n")
        .next()
        .unwrap();
    let home = tempfile::tempdir().unwrap();
    let mut count = 0;
    for line in table.lines().filter(|line| line.starts_with('|')) {
        for form in line.split('`').skip(1).step_by(2) {
            let mut args: Vec<_> = form.split_whitespace().collect();
            args.push("--help");
            let output = locus(&args, home.path());
            let help = String::from_utf8_lossy(&output.stdout);
            assert!(output.status.success(), "help failed for {form}");
            assert!(
                help.contains("--dry-run"),
                "{form} does not advertise --dry-run"
            );
            count += 1;
        }
    }
    assert!(count >= 35, "dry-run table is missing command forms");
}

#[test]
fn documented_init_preview_never_creates_home() {
    let home = tempfile::tempdir().unwrap();
    let example = include_str!("../../../docs/README.md")
        .lines()
        .find_map(|line| line.strip_prefix("locus init "))
        .expect("documented init preview");
    let mut args = vec!["init"];
    args.extend(example.split_whitespace());
    let output = locus(&args, home.path());
    assert!(output.status.success(), "documented init preview failed");
    let plan: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(plan["schema"], "locus.plan.v1");
    assert_eq!(plan["dry_run"], true);
    assert!(!home.path().join("locus").exists());
}

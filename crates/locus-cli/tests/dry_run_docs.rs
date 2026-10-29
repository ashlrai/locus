//! Keep the documented dry-run command forms discoverable in CLI help and
//! exercise the first copyable preview in an uninitialized home.

use std::collections::BTreeSet;
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

fn forms(table: &str, prefix: &str) -> BTreeSet<String> {
    table
        .lines()
        .filter(|line| line.starts_with(prefix))
        .flat_map(|line| line.split('`').skip(1).step_by(2))
        .map(str::to_owned)
        .collect()
}

#[test]
fn command_matrix_matches_the_documented_plan_forms() {
    let overview = include_str!("../../../docs/README.md")
        .split("| Area | Command forms |")
        .nth(1)
        .unwrap()
        .split("\n\n")
        .next()
        .unwrap();
    let matrix = include_str!("../../../docs/dry-run-matrix-q8.md");
    let listed = forms(overview, "|");
    assert!(!listed.is_empty());
    assert_eq!(listed, forms(matrix, "| Plan |"));
}

#[test]
fn changelog_issue_171_matches_the_cli_and_plan_matrix() {
    let changelog = include_str!("../../../CHANGELOG.md");
    let note = changelog
        .split("- **Dry-run scope ([#171]")
        .nth(1)
        .expect("issue 171 changelog note")
        .split("\n\n")
        .next()
        .unwrap();
    let start = note.find("`exec`").expect("unsupported command list");
    let end = note.find(". Provider operations").expect("server list end");
    let unsupported = &note[start..end];
    let documented: BTreeSet<_> = unsupported
        .split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect();
    let home = tempfile::tempdir().unwrap();
    let output = locus(&["topic", "agent-contract", "--json"], home.path());
    assert!(output.status.success());
    let contract: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let actual: BTreeSet<_> = contract["dry_run"]["unsupported"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(documented, actual);

    let matrix = forms(
        include_str!("../../../docs/dry-run-matrix-q8.md"),
        "| Plan |",
    );
    for form in [
        "init",
        "pin",
        "binding add",
        "agent setup",
        "adapter install",
        "events export",
        "mcp mint",
        "mcp revoke",
    ] {
        assert!(note.contains(&format!("`{form}`")), "missing {form}");
        assert!(matrix.contains(form), "{form} is not in the plan matrix");
    }
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

#[test]
fn documented_print_only_setup_forms_keep_their_output() {
    let home = tempfile::tempdir().unwrap();
    for args in [
        &["--json", "setup", "--client", "generic"][..],
        &["--json", "setup", "--client", "claude", "--print"][..],
    ] {
        let normal = locus(args, home.path());
        let mut preview_args = args.to_vec();
        preview_args.push("--dry-run");
        let preview = locus(&preview_args, home.path());
        assert!(normal.status.success() && preview.status.success());
        let value: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
        assert_eq!(
            value,
            serde_json::from_slice::<serde_json::Value>(&normal.stdout).unwrap()
        );
        assert_ne!(value["schema"], "locus.plan.v1");
        assert!(!home.path().join("locus").exists());
        assert!(!home.path().join(".mcp.json").exists());
    }
}

#[test]
fn unsupported_exec_preview_does_not_start_the_child() {
    let home = tempfile::tempdir().unwrap();
    let marker = home.path().join("child-ran");
    let output = locus(
        &["exec", "--dry-run", "--", "touch", marker.to_str().unwrap()],
        home.path(),
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(!marker.exists());
}

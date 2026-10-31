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

    let readme = include_str!("../../../README.md")
        .split("### Previewing mutations with `--dry-run`")
        .nth(1)
        .expect("README dry-run section")
        .split("\n---")
        .next()
        .unwrap();
    assert_eq!(forms(readme, "|"), listed);
    assert!(readme.contains("Leave persistent state unchanged"));
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
fn readme_init_preview_produces_a_plan_without_creating_home() {
    let home = tempfile::tempdir().unwrap();
    let section = include_str!("../../../README.md")
        .split("### Previewing mutations\n")
        .nth(1)
        .expect("README dry-run section");
    let example = section
        .lines()
        .find_map(|line| line.strip_prefix("locus init "))
        .expect("README init preview");
    let mut args = vec!["init"];
    args.extend(example.split_whitespace());
    let output = locus(&args, home.path());
    assert!(output.status.success(), "README init preview failed");
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

#[test]
fn readme_identifies_unplannable_commands_that_reject_the_flag() {
    let readme = include_str!("../../../README.md")
        .split("### Previewing mutations with `--dry-run`")
        .nth(1)
        .expect("README dry-run section")
        .split("\n---")
        .next()
        .unwrap();
    let matrix = include_str!("../../../docs/dry-run-matrix-q8.md")
        .split("## Unsupported cases")
        .nth(1)
        .expect("unsupported cases section")
        .split("\n\n")
        .nth(1)
        .expect("unsupported command table");
    let cases: [(&str, &[&str]); 6] = [
        ("exec", &["exec", "--", "true"]),
        ("run", &["run", "-b", "example", "--", "true"]),
        ("ci run", &["ci", "run", "-b", "example", "--", "true"]),
        ("mcp", &["mcp"]),
        ("serve", &["serve"]),
        ("dashboard", &["dashboard", "--no-open"]),
    ];
    let expected: BTreeSet<_> = cases.iter().map(|(name, _)| (*name).to_owned()).collect();
    assert_eq!(forms(matrix, "| `"), expected);
    let home = tempfile::tempdir().unwrap();
    for (name, args) in cases {
        assert!(readme.contains(&format!("`{name}`")), "missing {name}");
        let mut help_args: Vec<_> = name.split_whitespace().collect();
        help_args.push("--help");
        let help = locus(&help_args, home.path());
        assert!(help.status.success(), "help failed for {name}");
        assert!(
            !String::from_utf8_lossy(&help.stdout).contains("--dry-run"),
            "{name} advertises --dry-run"
        );
        let mut preview = vec!["--dry-run"];
        preview.extend_from_slice(args);
        let output = locus(&preview, home.path());
        assert_eq!(output.status.code(), Some(2), "{name} accepted --dry-run");
        assert!(String::from_utf8_lossy(&output.stderr).contains("unexpected argument '--dry-run'"));
    }
}

#[test]
fn fleet_read_only_commands_are_in_the_full_matrix_and_help() {
    let matrix = include_str!("../../../docs/dry-run-matrix-q8.md");
    let read_only = forms(matrix, "| Accepted, ignored |");
    let home = tempfile::tempdir().unwrap();
    for name in ["fleet-preflight", "fleet-feed", "fleet-denials"] {
        assert!(read_only.contains(name), "missing {name}");
        let output = locus(&[name, "--help"], home.path());
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("--dry-run"));
    }
}

#[test]
fn mutating_inventory_tracks_matrix_and_examples() {
    let inventory = include_str!("../../../docs/dry-run-command-inventory.md");
    let plans = inventory
        .split("## Commands with plans")
        .nth(1)
        .unwrap()
        .split("\n\n- ")
        .next()
        .unwrap();
    let unsupported = inventory
        .split("## No Locus preview")
        .nth(1)
        .unwrap()
        .split("\n\nThese forms")
        .next()
        .unwrap();
    let matrix = include_str!("../../../docs/dry-run-matrix-q8.md");
    let command_column = |table: &str| -> BTreeSet<String> {
        table
            .lines()
            .filter(|line| line.starts_with("| `"))
            .flat_map(|line| {
                line.split('|')
                    .nth(1)
                    .unwrap()
                    .split('`')
                    .skip(1)
                    .step_by(2)
            })
            .map(str::to_owned)
            .collect()
    };
    assert_eq!(command_column(plans), forms(matrix, "| Plan |"));
    let matrix_unsupported = matrix.split("## Unsupported cases").nth(1).unwrap();
    assert_eq!(
        command_column(unsupported),
        forms(matrix_unsupported, "| `")
    );
    assert!(include_str!("../../../docs/README.md").contains("dry-run-command-inventory.md"));

    let home = tempfile::tempdir().unwrap();
    for args in [&["binding", "rm", "--help"][..], &["exec", "--help"]] {
        let command = format!("locus {}", args.join(" "));
        assert!(inventory.lines().any(|line| line == command));
        assert!(locus(args, home.path()).status.success());
    }
    assert!(inventory.contains("LOCUS_HOME=\"$scratch/locus\" locus init --dry-run --json"));
    let output = locus(&["init", "--dry-run", "--json"], home.path());
    assert!(output.status.success());
    let plan: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(plan["schema"], "locus.plan.v1");
    assert_eq!(plan["dry_run"], true);
    assert!(!home.path().join("locus").exists());
    assert!(inventory.contains("locus --dry-run exec -- true; test \"$?\" -eq 2"));
    let output = locus(&["--dry-run", "exec", "--", "true"], home.path());
    assert_eq!(output.status.code(), Some(2));
    assert!(!home.path().join("locus").exists());
}

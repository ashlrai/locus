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
fn manual_verification_procedure_runs_from_the_checkout() {
    let doc = include_str!("../../../docs/dry-run-verification.md");
    let script = doc
        .split("```bash\n")
        .nth(1)
        .expect("manual verification script")
        .split("\n```")
        .next()
        .unwrap();
    let home = tempfile::tempdir().unwrap();
    let output = Command::new("bash")
        .arg("-c")
        .arg(script)
        .env("LOCUS_BIN", env!("CARGO_BIN_EXE_locus"))
        .env("HOME", home.path())
        .output()
        .expect("run manual verification");
    assert!(
        output.status.success(),
        "manual verification failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("PASS: help, planned actions"));
}

#[test]
fn acceptance_checklist_is_discoverable_and_cites_existing_docs() {
    let overview = include_str!("../../../docs/README.md");
    assert!(overview.contains("(dry-run-acceptance.md)"));
    let checklist = include_str!("../../../docs/dry-run-acceptance.md");
    for section in [
        "## Scope and mutation avoidance",
        "## Useful output",
        "## Error handling",
        "## Evidence for review",
    ] {
        assert!(checklist.contains(section), "missing {section}");
    }
    let docs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs");
    for source in [
        "dry-run-matrix-q8.md",
        "dry-run-verification.md",
        "agent-contract.md",
        "exit-codes.md",
        "README.md",
    ] {
        assert!(checklist.contains(source), "missing citation: {source}");
        assert!(docs.join(source).is_file(), "missing source: {source}");
    }
}

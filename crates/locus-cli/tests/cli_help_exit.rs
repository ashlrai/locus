//! E2E smoke: `locus --help` must exit 0 and print usage text naming
//! at least one subcommand so agents can discover what to do next.
//!
//! This test exists at the crate level (not `#[path]`-included) so CI
//! treats it as a standalone integration test binary with its own
//! `cargo build --test` compilation boundary.

use std::process::Command;

#[test]
fn locus_help_exits_zero_and_names_subcommands() {
    let output = Command::new(env!("CARGO_BIN_EXE_locus"))
        .arg("--help")
        .output()
        .expect("failed to execute locus");

    assert!(
        output.status.success(),
        "locus --help exited non-zero: {}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let text = String::from_utf8_lossy(&output.stdout);

    // Verify the output contains subcommand names from the CLI.
    // clap outputs them under "Commands:" or "SUBCOMMANDS:" sections.
    let known_subcommands = [
        "init",
        "quickstart",
        "enter",
        "pin",
        "leave",
        "whoami",
        "exec",
        "run",
        "status",
        "doctor",
        "binding",
        "onboard",
        "offboard",
        "mcp",
        "completion",
        "version",
        "approve",
        "events",
    ];

    let found: Vec<&str> = known_subcommands
        .iter()
        .copied()
        .filter(|name| text.contains(name))
        .collect();

    assert!(
        !found.is_empty(),
        "locus --help output did not name any subcommand.\nstdout:\n{text}"
    );
}

#[test]
fn failure_codes_are_stable_across_commands() {
    let home = tempfile::tempdir().unwrap();
    for args in [
        &["whoami", "--bad"][..],
        &["status", "--bad"][..],
        &["doctor", "--bad"][..],
        &["binding", "--bad"][..],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_locus"))
            .args(args)
            .env("HOME", home.path())
            .env("LOCUS_HOME", home.path().join("locus"))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
    }
}

#[test]
fn readme_exit_table_matches_cli_catalog() {
    let output = Command::new(env!("CARGO_BIN_EXE_locus"))
        .args(["topic", "agent-contract"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    let start = text.find("| Code | Name | Meaning |").unwrap();
    let table = text[start..].split("\n\n").next().unwrap();
    let readme = include_str!("../../../README.md");
    assert!(
        readme.contains(table),
        "README exit table differs from CLI catalog"
    );
}

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
        "init", "quickstart", "enter", "pin", "leave", "whoami", "exec",
        "run", "status", "doctor", "binding", "onboard", "offboard",
        "mcp", "completion", "version", "approve", "events",
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

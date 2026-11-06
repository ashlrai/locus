use std::process::Command;

#[test]
fn cli_package_builds_both_executable_entrypoints() {
    for binary in [env!("CARGO_BIN_EXE_locus"), env!("CARGO_BIN_EXE_locus-mcp")] {
        let output = Command::new(binary).arg("--help").output().unwrap();
        assert!(output.status.success(), "{binary} --help failed");
        assert!(
            !output.stdout.is_empty() || !output.stderr.is_empty(),
            "{binary} --help was empty"
        );
    }
}

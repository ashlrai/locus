use std::process::Command;

#[test]
fn unknown_command_exits_nonzero() {
    let output = Command::new(env!("CARGO_BIN_EXE_locus"))
        .arg("this-command-does-not-exist")
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "unknown command exited successfully"
    );
}

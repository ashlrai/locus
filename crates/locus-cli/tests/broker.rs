use std::process::Command;

fn command(home: &std::path::Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_locus"));
    c.env("LOCUS_HOME", home).env("LOCUS_ENGINE_ID", "worker");
    c
}

#[test]
fn check_allow_deny_unknown_and_exec_gate() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("broker.toml"),
        "[engines.worker]\ncli = ['gh']\nmcp = ['github']",
    )
    .unwrap();
    for (engine, tool, allowed) in [
        ("worker", "cli:gh", true),
        ("worker", "mcp:github", true),
        ("worker", "cli:sh", false),
        ("unknown", "cli:gh", false),
    ] {
        let output = command(home.path())
            .args(["broker", "check", engine, tool, "--json"])
            .output()
            .unwrap();
        assert_eq!(output.status.success(), allowed);
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["allowed"], allowed);
    }
    let output = command(home.path())
        .args(["exec", "--", "never-launch", "argument-secret"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("broker denied"));
    let audit = std::fs::read_to_string(home.path().join("audit/events.jsonl")).unwrap();
    assert!(audit.contains("broker.deny"));
    assert!(!audit.contains("argument-secret"));
}

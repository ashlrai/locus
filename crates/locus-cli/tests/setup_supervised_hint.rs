//! E2E: `locus setup --client claude` registers plain `locus-mcp` (control
//! tools only) and must say how to get provider tools: `locus mcp --supervised`.

use std::process::Command;

fn locus(
    home: &std::path::Path,
    cwd: &std::path::Path,
    cap: Option<&str>,
    args: &[&str],
) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_locus"));
    cmd.args(args)
        .env("LOCUS_HOME", home)
        .env("HOME", home)
        .env("NO_COLOR", "1")
        .env_remove("LOCUS_CONTROL_CAPABILITY")
        .current_dir(cwd);
    if let Some(cap) = cap {
        cmd.env("LOCUS_CONTROL_CAPABILITY", cap);
    }
    cmd.output().expect("locus binary runs")
}

#[test]
fn setup_claude_points_at_supervised_mcp() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();

    let qs = locus(home.path(), cwd.path(), None, &["quickstart"]);
    assert!(qs.status.success(), "quickstart failed: {qs:?}");
    let cap = std::fs::read_to_string(home.path().join("control_capability"))
        .expect("quickstart persists the control capability");

    let setup = locus(
        home.path(),
        cwd.path(),
        Some(cap.trim()),
        &["setup", "--client", "claude"],
    );
    let out = String::from_utf8_lossy(&setup.stdout);
    assert!(setup.status.success(), "setup failed: {setup:?}");
    assert!(
        out.contains("mcp --supervised"),
        "no supervised hint: {out}"
    );

    // The registration itself is unchanged: plain locus-mcp, no args.
    let mcp: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(cwd.path().join(".mcp.json")).unwrap())
            .unwrap();
    let entry = &mcp["mcpServers"]["locus"];
    assert!(
        entry["command"].as_str().unwrap().ends_with("locus-mcp"),
        "{entry}"
    );
    assert_eq!(entry["args"], serde_json::json!([]));
}

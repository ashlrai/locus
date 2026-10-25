//! E2E: `locus setup --client gemini` writes the project `.gemini/settings.json`
//! (`mcpServers.locus`), preserves unrelated keys, is idempotent, refuses to
//! touch malformed JSON, and the readiness probe sees it.

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

fn capability(home: &std::path::Path, cwd: &std::path::Path) -> String {
    let qs = locus(home, cwd, None, &["quickstart"]);
    assert!(qs.status.success(), "quickstart failed: {qs:?}");
    std::fs::read_to_string(home.join("control_capability"))
        .expect("quickstart persists the control capability")
        .trim()
        .to_string()
}

#[test]
fn setup_gemini_writes_merges_and_is_idempotent() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let cap = capability(home.path(), cwd.path());

    // Pre-existing project settings with an unrelated key and another server.
    let dir = cwd.path().join(".gemini");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("settings.json");
    std::fs::write(
        &file,
        r#"{"theme":"dark","mcpServers":{"other":{"command":"o"}}}"#,
    )
    .unwrap();

    for _ in 0..2 {
        let out = locus(
            home.path(),
            cwd.path(),
            Some(&cap),
            &["setup", "--client", "gemini"],
        );
        assert!(out.status.success(), "setup failed: {out:?}");
    }
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(v["theme"], "dark", "unrelated keys preserved");
    assert_eq!(v["mcpServers"]["other"]["command"], "o");
    let entry = &v["mcpServers"]["locus"];
    assert!(entry["command"].as_str().unwrap().ends_with("locus-mcp"));
    assert_eq!(entry["env"]["LOCUS_CLIENT"], "gemini");
    assert_eq!(entry["env"]["LOCUS_AUTO_PIN"], "cwd");
    assert!(entry["env"].get("LOCUS_NOTIFY").is_none());
    assert_eq!(
        v["mcpServers"].as_object().unwrap().len(),
        2,
        "no duplicate"
    );
}

#[test]
fn setup_gemini_refuses_malformed_json_untouched() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let cap = capability(home.path(), cwd.path());
    let dir = cwd.path().join(".gemini");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("settings.json");
    std::fs::write(&file, "{not json").unwrap();

    let out = locus(
        home.path(),
        cwd.path(),
        Some(&cap),
        &["setup", "--client", "gemini"],
    );
    assert!(!out.status.success());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "{not json");
}

#[test]
fn setup_gemini_print_writes_nothing() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let out = locus(
        home.path(),
        cwd.path(),
        None,
        &["setup", "--client", "gemini", "--print"],
    );
    assert!(out.status.success(), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("mcpServers"));
    assert!(!cwd.path().join(".gemini").exists());
}

//! Agent-facing CLI input contracts: out-of-range mint TTLs fail before
//! anything is minted, a `--provider` typo is named on stderr, and
//! `locus --json setup` emits JSON instead of ignoring the flag.

use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};

fn locus(home: &Path, cwd: &Path, cap: Option<&str>, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_locus"));
    cmd.args(args)
        .env("LOCUS_HOME", home)
        .env("HOME", home)
        .env("NO_COLOR", "1")
        .env_remove("LOCUS_SESSION_ID")
        .env_remove("LOCUS_EXECUTOR_CAPABILITY")
        .current_dir(cwd);
    match cap {
        Some(c) => cmd.env("LOCUS_CONTROL_CAPABILITY", c),
        None => cmd.env_remove("LOCUS_CONTROL_CAPABILITY"),
    };
    cmd.output().expect("locus binary runs")
}

/// `init --with-samples` in a fresh home; returns the persisted capability.
fn init(home: &Path, cwd: &Path) -> String {
    let out = locus(home, cwd, None, &["init", "--with-samples"]);
    assert!(out.status.success(), "{out:?}");
    std::fs::read_to_string(home.join("control_capability"))
        .unwrap()
        .trim()
        .to_string()
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[test]
fn mint_ttl_out_of_range_fails_before_minting() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let cap = init(home.path(), cwd.path());
    let run = |args: &[&str]| locus(home.path(), cwd.path(), Some(&cap), args);

    for (args, needle) in [
        (
            vec!["ci", "mint", "--binding", "personal", "--ttl", "0s"],
            "too short",
        ),
        (
            vec!["ci", "mint", "--binding", "personal", "--ttl", "48h"],
            "too long",
        ),
        (
            vec!["ci", "env", "--binding", "personal", "--ttl", "0m"],
            "too short",
        ),
        (
            vec!["mcp", "mint", "--binding", "personal", "--ttl=-5m"],
            "too short",
        ),
        (
            vec!["mcp", "mint", "--binding", "personal", "--ttl", "abc"],
            "use 30m / 2h / 1d",
        ),
    ] {
        let out = run(&args);
        let stderr = text(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{args:?}: {stderr}");
        assert!(stderr.contains(needle), "{args:?}: {stderr}");
        let stdout = text(&out.stdout);
        assert!(
            !stdout.contains("lmt_") && !stdout.contains("LOCUS_SEAL"),
            "{args:?} minted anyway: {stdout}"
        );
    }

    // ci run never starts the child under a zero TTL.
    let out = run(&[
        "ci",
        "run",
        "--binding",
        "personal",
        "--no-resolve",
        "--ttl",
        "0s",
        "--",
        "sh",
        "-c",
        "echo child-ran",
    ]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(!text(&out.stdout).contains("child-ran"), "{out:?}");
    assert!(text(&out.stderr).contains("too short"), "{out:?}");

    // In-range TTLs still mint, including second-scale expiry probes.
    let out = run(&["ci", "mint", "--binding", "personal", "--ttl", "1s"]);
    assert!(out.status.success(), "{out:?}");
    let out = run(&["ci", "mint", "--binding", "personal", "--ttl", "15m"]);
    assert!(out.status.success(), "{out:?}");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["binding"], "personal");
}

#[test]
fn binding_add_names_a_provider_without_an_adapter() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let cap = init(home.path(), cwd.path());
    let add = |alias: &str, provider: &str, json: bool| {
        let mut args = vec![];
        if json {
            args.push("--json");
        }
        args.extend([
            "binding",
            "add",
            alias,
            "--tenant",
            "t",
            "--provider",
            provider,
            "--account",
            "a",
            "--credential-ref",
            "phm:X",
            "--non-interactive",
        ]);
        locus(home.path(), cwd.path(), Some(&cap), &args)
    };

    let out = add("typo", "githb", false);
    let stderr = text(&out.stderr);
    assert!(out.status.success(), "still written: {stderr}");
    assert!(
        stderr.contains("no built-in adapter for 'githb'") && stderr.contains("github"),
        "{stderr}"
    );
    assert!(home.path().join("bindings/typo.toml").exists());

    // --json: stdout stays one JSON document, the warning goes to stderr.
    let out = add("typo2", "githb", true);
    assert!(out.status.success(), "{out:?}");
    let v: Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
    assert_eq!(v["ok"], true);
    assert!(text(&out.stderr).contains("no built-in adapter"), "{out:?}");

    // Built-in providers (any case) stay quiet.
    let out = add("real", "GitHub", false);
    assert!(out.status.success(), "{out:?}");
    assert!(!text(&out.stderr).contains("warning"), "{out:?}");
}

#[test]
fn setup_honors_json() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let cap = init(home.path(), cwd.path());
    let run = |args: &[&str]| locus(home.path(), cwd.path(), Some(&cap), args);
    let json_of = |out: &Output| -> Value {
        assert!(out.status.success(), "{out:?}");
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("--json setup must print JSON ({e}): {out:?}"))
    };

    let v = json_of(&run(&["--json", "setup", "--client", "cursor"]));
    assert_eq!(v["ok"], true);
    assert_eq!(v["client"], "cursor");
    let written = v["written"].as_array().unwrap();
    assert_eq!(written.len(), 1, "{v}");
    assert!(written[0].as_str().unwrap().ends_with(".cursor/mcp.json"));
    assert!(cwd.path().join(".cursor/mcp.json").exists());
    assert!(v["supervised_command"]
        .as_str()
        .is_some_and(|c| c.ends_with("mcp --supervised")));

    let v = json_of(&run(&["--json", "setup", "--client", "codex", "--print"]));
    assert_eq!(v["written"], serde_json::json!([]));
    assert!(v["entry_toml"].as_str().is_some_and(
        |t| t.contains("[mcp_servers.locus]") && t.contains("LOCUS_CLIENT = \"codex\"")
    ));
    assert!(!home.path().join(".codex/config.toml").exists());

    let v = json_of(&run(&["--json", "setup", "--client", "generic"]));
    assert!(
        v["entry"]["mcpServers"]["locus"]["command"].is_string(),
        "{v}"
    );
    assert!(v["entry_toml"].is_string(), "{v}");

    // Human output is unchanged.
    let out = run(&["setup", "--client", "claude"]);
    assert!(out.status.success(), "{out:?}");
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains("wrote/merged") && stdout.contains("mcp --supervised"),
        "{stdout}"
    );
}

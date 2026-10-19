//! E2E parity matrix for `locus setup --client <c>` across every on-disk
//! client shape (ecosystem plan, Bet 1):
//!
//! | client | file                         | format | table                    |
//! |--------|------------------------------|--------|--------------------------|
//! | claude | `<project>/.mcp.json`        | JSON   | `mcpServers.locus`       |
//! | cursor | `<project>/.cursor/mcp.json` | JSON   | `mcpServers.locus`       |
//! | gemini | `<project>/.gemini/settings.json` | JSON | `mcpServers.locus`     |
//! | codex  | `~/.codex/config.toml`       | TOML   | `[mcp_servers.locus]`    |
//! | grok   | `~/.grok/config.toml`        | TOML   | `[mcp_servers.locus]`    |
//!
//! For each: the entry is written with `LOCUS_CLIENT=<c>` and
//! `LOCUS_AUTO_PIN=cwd`, an unrelated server and key are kept, a second run
//! leaves the file byte for byte unchanged, and a malformed file is refused
//! and left untouched.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn locus(home: &Path, cwd: &Path, cap: Option<&str>, args: &[&str]) -> Output {
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

fn capability(home: &Path, cwd: &Path) -> String {
    let qs = locus(home, cwd, None, &["quickstart"]);
    assert!(qs.status.success(), "quickstart failed: {qs:?}");
    std::fs::read_to_string(home.join("control_capability"))
        .expect("quickstart persists the control capability")
        .trim()
        .to_string()
}

#[derive(Clone, Copy)]
enum Format {
    Json,
    Toml,
}

struct Shape {
    client: &'static str,
    format: Format,
    /// Path of the config file, given (home, project).
    path: fn(&Path, &Path) -> PathBuf,
}

const SHAPES: &[Shape] = &[
    Shape {
        client: "claude",
        format: Format::Json,
        path: |_, p| p.join(".mcp.json"),
    },
    Shape {
        client: "cursor",
        format: Format::Json,
        path: |_, p| p.join(".cursor").join("mcp.json"),
    },
    Shape {
        client: "gemini",
        format: Format::Json,
        path: |_, p| p.join(".gemini").join("settings.json"),
    },
    Shape {
        client: "codex",
        format: Format::Toml,
        path: |h, _| h.join(".codex").join("config.toml"),
    },
    Shape {
        client: "grok",
        format: Format::Toml,
        path: |h, _| h.join(".grok").join("config.toml"),
    },
];

fn seed(format: Format) -> &'static str {
    match format {
        Format::Json => r#"{"keepMe":true,"mcpServers":{"other":{"command":"o"}}}"#,
        Format::Toml => "keep_me = true\n\n[mcp_servers.other]\ncommand = \"o\"\n",
    }
}

fn malformed(format: Format) -> &'static str {
    match format {
        Format::Json => "{ not json",
        Format::Toml => "[mcp_servers.other\ncommand = ",
    }
}

/// (command, LOCUS_CLIENT, LOCUS_AUTO_PIN, other server kept, unrelated key kept, server count)
fn read_entry(format: Format, text: &str) -> (String, String, String, bool, bool, usize) {
    match format {
        Format::Json => {
            let v: serde_json::Value = serde_json::from_str(text).expect("valid JSON");
            let e = &v["mcpServers"]["locus"];
            (
                e["command"].as_str().unwrap_or_default().to_string(),
                e["env"]["LOCUS_CLIENT"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                e["env"]["LOCUS_AUTO_PIN"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                v["mcpServers"]["other"]["command"] == "o",
                v["keepMe"] == true,
                v["mcpServers"].as_object().map_or(0, |m| m.len()),
            )
        }
        Format::Toml => {
            let v: toml::Value = text.parse().expect("valid TOML");
            let servers = &v["mcp_servers"];
            let e = &servers["locus"];
            let env = |k: &str| {
                e.get("env")
                    .and_then(|t| t.get(k))
                    .and_then(|s| s.as_str())
                    .unwrap_or_default()
                    .to_string()
            };
            (
                e.get("command")
                    .and_then(|s| s.as_str())
                    .unwrap_or_default()
                    .to_string(),
                env("LOCUS_CLIENT"),
                env("LOCUS_AUTO_PIN"),
                servers
                    .get("other")
                    .and_then(|o| o.get("command"))
                    .and_then(|s| s.as_str())
                    == Some("o"),
                v.get("keep_me").and_then(|b| b.as_bool()) == Some(true),
                servers.as_table().map_or(0, |t| t.len()),
            )
        }
    }
}

#[test]
fn every_client_shape_is_written_merged_and_idempotent() {
    for shape in SHAPES {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let cap = capability(home.path(), cwd.path());
        let file = (shape.path)(home.path(), cwd.path());
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, seed(shape.format)).unwrap();

        let run = || {
            locus(
                home.path(),
                cwd.path(),
                Some(&cap),
                &["setup", "--client", shape.client],
            )
        };
        let first = run();
        assert!(
            first.status.success(),
            "{}: setup failed: {first:?}",
            shape.client
        );
        let after_first = std::fs::read(&file).unwrap();
        let second = run();
        assert!(
            second.status.success(),
            "{}: re-run failed: {second:?}",
            shape.client
        );
        assert_eq!(
            std::fs::read(&file).unwrap(),
            after_first,
            "{}: a second run must leave the file byte for byte unchanged",
            shape.client
        );

        let text = String::from_utf8(after_first).unwrap();
        let (command, client, pin, other, unrelated, count) = read_entry(shape.format, &text);
        assert!(
            command.ends_with("locus-mcp"),
            "{}: command {command}",
            shape.client
        );
        assert_eq!(client, shape.client, "{}: LOCUS_CLIENT", shape.client);
        assert_eq!(pin, "cwd", "{}: LOCUS_AUTO_PIN", shape.client);
        assert!(other, "{}: other server kept", shape.client);
        assert!(unrelated, "{}: unrelated key kept", shape.client);
        assert_eq!(
            count, 2,
            "{}: exactly locus and other, no duplicate",
            shape.client
        );
    }
}

#[test]
fn every_client_shape_refuses_a_malformed_config_untouched() {
    for shape in SHAPES {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let cap = capability(home.path(), cwd.path());
        let file = (shape.path)(home.path(), cwd.path());
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, malformed(shape.format)).unwrap();
        let out = locus(
            home.path(),
            cwd.path(),
            Some(&cap),
            &["setup", "--client", shape.client],
        );
        assert!(
            !out.status.success(),
            "{}: malformed config must fail setup",
            shape.client
        );
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            malformed(shape.format),
            "{}: malformed config left untouched",
            shape.client
        );
    }
}

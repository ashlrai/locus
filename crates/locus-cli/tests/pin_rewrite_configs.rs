#![cfg(unix)]

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

const CONTROL: &str = "0b5e7a1c9d2f4e6a8b0c1d3e5f7a9b1c2d4e6f8a0b1c3d5e7f9a1b2c4d6e8f0a";

fn run(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_locus"))
        .args(args)
        .env("HOME", home)
        .env("LOCUS_HOME", home.join("locus-store"))
        .env("LOCUS_CONTROL_CAPABILITY", CONTROL)
        .env_remove("LOCUS_SESSION_ID")
        .env_remove("LOCUS_EXECUTOR_CAPABILITY")
        .current_dir(home)
        .output()
        .unwrap()
}

fn write(home: &Path, path: &str, data: &str) {
    let path = home.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, data).unwrap();
}

fn backups(home: &Path, relative: &str) -> Vec<std::path::PathBuf> {
    let path = home.join(relative);
    let prefix = format!("{}.", path.file_name().unwrap().to_str().unwrap());
    fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with(&prefix)
                && p.extension().is_some_and(|e| e == "bak")
        })
        .collect()
}

#[test]
fn pin_rewrites_only_locus_servers_and_preserves_backups_and_auth() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let fixtures = [
        (".codex/config.toml", "# keep this comment\nmodel = 'example'\n[mcp_servers.locus]\ncommand = 'npx' # keep\nargs = ['-y', '@ashlrai/locus-mcp', '--http']\n[mcp_servers.other]\ncommand = 'npx'\nargs = ['-y', 'other']\n"),
        (".claude.json", "{\n  \"other\": {\"x\":1},\n  \"mcpServers\": {\"locus\": {\"command\": \"npx\", \"args\": [\"-y\",\"@ashlrai/locus-mcp\",\"--http\"], \"env\": {\"KEEP\": \"value\"}}, \"other\": {\"command\": \"npx\", \"args\": [\"other\"]}},\n  \"projects\": {\"/workspace\": {\"mcpServers\": {\"locus2\": {\"command\": \"npx\", \"args\": [\"locus-mcp\"]}}}}\n}\n"),
        (".cursor/mcp.json", "{\"mcpServers\":{\"locus\":{\"command\":\"npx\",\"args\":[\"--yes\",\"@ashlrai/locus-mcp\"]},\"other\":{\"command\":\"npx\",\"args\":[\"other\"]}}}\n"),
    ];
    for (path, text) in fixtures {
        write(home, path, text);
    }
    let canary = "{\n  \"untouched\": \"auth-canary\"\n}\n";
    write(home, ".codex/auth.json", canary);
    assert!(
        run(home, &["init", "--with-samples", "--no-persist-capability"])
            .status
            .success()
    );
    // A normal pin does not modify configs.
    assert!(run(home, &["pin", "personal"]).status.success());
    for (path, original) in fixtures {
        assert_eq!(fs::read_to_string(home.join(path)).unwrap(), original);
    }
    let out = run(home, &["pin", "personal", "--rewrite-configs"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    for (path, original) in fixtures {
        let backup = backups(home, path);
        assert_eq!(backup.len(), 1);
        assert_eq!(fs::read_to_string(&backup[0]).unwrap(), original);
        assert!(fs::read_to_string(home.join(path))
            .unwrap()
            .contains("locus-mcp"));
    }
    let codex = fs::read_to_string(home.join(".codex/config.toml")).unwrap();
    assert!(codex.contains("# keep this comment") && codex.contains("# keep"));
    let parsed: toml::Value = toml::from_str(&codex).unwrap();
    assert_eq!(
        parsed["mcp_servers"]["locus"]["args"].as_array().unwrap()[0].as_str(),
        Some("--http")
    );
    assert!(codex.contains("args = ['-y', 'other']"));
    let claude = fs::read_to_string(home.join(".claude.json")).unwrap();
    assert!(claude.contains("\"other\": {\"x\":1}"));
    let v: serde_json::Value = serde_json::from_str(&claude).unwrap();
    assert_eq!(
        v["mcpServers"]["locus"]["args"],
        serde_json::json!(["--http"])
    );
    assert_eq!(v["mcpServers"]["other"]["command"], "npx");
    assert!(
        v["projects"]["/workspace"]["mcpServers"]["locus2"]["command"]
            .as_str()
            .unwrap()
            .ends_with("locus-mcp")
    );
    let after: Vec<_> = fixtures
        .iter()
        .map(|(p, _)| fs::read(home.join(p)).unwrap())
        .collect();
    assert!(run(home, &["pin", "personal", "--rewrite-configs"])
        .status
        .success());
    for ((path, _), data) in fixtures.iter().zip(after) {
        assert_eq!(fs::read(home.join(path)).unwrap(), data);
        assert_eq!(backups(home, path).len(), 1);
    }
    assert_eq!(
        fs::read(home.join(".codex/auth.json")).unwrap(),
        canary.as_bytes()
    );
}

#[test]
fn malformed_config_fails_before_pin_or_file_changes() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let original = "[mcp_servers.locus]\ncommand = 'npx'\nargs = ['locus-mcp']\n";
    write(home, ".codex/config.toml", original);
    write(home, ".cursor/mcp.json", "{");
    assert!(
        run(home, &["init", "--with-samples", "--no-persist-capability"])
            .status
            .success()
    );
    assert!(!run(home, &["pin", "personal", "--rewrite-configs"])
        .status
        .success());
    assert_eq!(
        fs::read_to_string(home.join(".codex/config.toml")).unwrap(),
        original
    );
    assert!(backups(home, ".codex/config.toml").is_empty());
    assert!(!home.join("locus-store/sessions/active.json").exists());
}

#[test]
fn config_symlink_to_auth_is_rejected_without_changes() {
    use std::os::unix::fs::symlink;
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let canary = "auth canary";
    write(home, ".codex/auth.json", canary);
    symlink("auth.json", home.join(".codex/config.toml")).unwrap();
    assert!(
        run(home, &["init", "--with-samples", "--no-persist-capability"])
            .status
            .success()
    );
    assert!(!run(home, &["pin", "personal", "--rewrite-configs"])
        .status
        .success());
    assert_eq!(
        fs::read(home.join(".codex/auth.json")).unwrap(),
        canary.as_bytes()
    );
    assert!(backups(home, ".codex/config.toml").is_empty());
}

#[test]
fn config_hardlink_to_auth_is_rejected_without_changes() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let canary = "auth canary";
    write(home, ".codex/auth.json", canary);
    fs::hard_link(
        home.join(".codex/auth.json"),
        home.join(".codex/config.toml"),
    )
    .unwrap();
    assert!(
        run(home, &["init", "--with-samples", "--no-persist-capability"])
            .status
            .success()
    );
    assert!(!run(home, &["pin", "personal", "--rewrite-configs"])
        .status
        .success());
    assert_eq!(
        fs::read(home.join(".codex/auth.json")).unwrap(),
        canary.as_bytes()
    );
    assert!(backups(home, ".codex/config.toml").is_empty());
}

//! Mutation previews must preserve the entire filesystem, including runtime
//! state, empty directories, symlinks, permissions and modification times.
//! Fixtures never start a broker, so no background writer needs an exception.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::SystemTime;

use locus_core::pin_history::{self, HistoryEntry};
use serde_json::Value;

const PRIVATE_MARKER: &str = "DRY_RUN_EXISTING_CONFIG_SENTINEL";

#[derive(PartialEq)]
enum Contents {
    Directory,
    File(Vec<u8>),
    Symlink(PathBuf),
}

#[derive(PartialEq)]
struct Entry {
    contents: Contents,
    modified: SystemTime,
    permissions: fs::Permissions,
}

type Snapshot = BTreeMap<PathBuf, Entry>;

fn snapshot(root: &Path) -> Snapshot {
    fn visit(root: &Path, path: &Path, entries: &mut Snapshot) {
        let metadata = fs::symlink_metadata(path).unwrap();
        let contents = if metadata.is_dir() {
            Contents::Directory
        } else if metadata.file_type().is_symlink() {
            Contents::Symlink(fs::read_link(path).unwrap())
        } else {
            assert!(
                metadata.is_file(),
                "unexpected fixture type: {}",
                path.display()
            );
            Contents::File(fs::read(path).unwrap())
        };
        entries.insert(
            path.strip_prefix(root).unwrap().into(),
            Entry {
                contents,
                modified: metadata.modified().unwrap(),
                permissions: metadata.permissions(),
            },
        );
        if metadata.is_dir() {
            for child in fs::read_dir(path).unwrap() {
                visit(root, &child.unwrap().path(), entries);
            }
        }
    }
    let mut entries = BTreeMap::new();
    visit(root, root, &mut entries);
    entries
}

fn assert_unchanged(before: &Snapshot, after: &Snapshot) {
    for path in before.keys() {
        assert!(
            after.contains_key(path),
            "dry-run removed {}",
            path.display()
        );
    }
    for path in after.keys() {
        assert!(
            before.contains_key(path),
            "dry-run created {}",
            path.display()
        );
    }
    for (path, entry) in before {
        // Never dump bytes on failure: the fixture store contains a seal key.
        assert!(
            after.get(path) == Some(entry),
            "dry-run changed contents or metadata: {}",
            path.display()
        );
    }
}

struct Env {
    _temp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    user: PathBuf,
    project: PathBuf,
}

impl Env {
    fn new(initialized: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let env = Self {
            home: root.join("locus"),
            user: root.join("user"),
            project: root.join("project"),
            root,
            _temp: temp,
        };
        fs::create_dir(&env.user).unwrap();
        fs::create_dir(&env.project).unwrap();
        if initialized {
            locus_core::Store::open(&env.home).unwrap();
            env.binding();
        }
        env
    }

    fn binding(&self) {
        fs::write(
            self.home.join("bindings/acme.toml"),
            r#"[binding]
id = "bnd_acme"
alias = "acme"
tenant = "acme"
[[binding.providers]]
provider = "github"
account = "acme"
credential_ref = "env:DRY_RUN_FIXTURE_GH"
"#,
        )
        .unwrap();
    }

    fn command(&self, bin: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(bin);
        command
            .env_clear()
            .env("HOME", &self.user)
            .env("LOCUS_HOME", &self.home)
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("NO_COLOR", "1")
            .env("LOCUS_NON_INTERACTIVE", "1")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .current_dir(&self.project);
        command
    }

    fn run(&self, args: &[&str], json: bool) -> Output {
        let mut command = self.command(env!("CARGO_BIN_EXE_locus"));
        command.args(args).arg("--dry-run");
        if json {
            command.arg("--json");
        }
        command.output().unwrap()
    }

    fn plan(&self, args: &[&str]) -> Value {
        let before = snapshot(&self.root);
        let output = self.run(args, true);
        assert_unchanged(&before, &snapshot(&self.root));
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let plan: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(plan["schema"], "locus.plan.v1");
        assert_eq!(plan["dry_run"], true);
        assert_eq!(plan["requires_control_capability"], true);
        assert_eq!(plan["control_capability_in_env"], false);

        let output = self.run(args, false);
        assert_unchanged(&before, &snapshot(&self.root));
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let human = String::from_utf8(output.stdout).unwrap();
        assert!(human.contains("nothing written"), "{human}");
        for change in plan["changes"].as_array().unwrap() {
            assert!(
                human.contains(change["target"].as_str().unwrap()),
                "{human}"
            );
        }
        plan
    }

    fn refusal(&self, args: &[&str], message: &str) {
        let before = snapshot(&self.root);
        for json in [true, false] {
            let output = self.run(args, json);
            assert_unchanged(&before, &snapshot(&self.root));
            assert!(!output.status.success(), "dry-run should refuse {args:?}");
            assert!(output.stdout.is_empty());
            let error = String::from_utf8_lossy(&output.stderr);
            assert!(error.contains(message), "{error}");
            assert!(
                !error.contains(PRIVATE_MARKER),
                "error exposed existing config contents"
            );
        }
    }

    fn history(&self) {
        const REMOTE: &str = "https://github.com/dry-run-fixture/widgets.git";
        for args in [&["init", "-q"][..], &["remote", "add", "origin", REMOTE]] {
            assert!(self.command("git").args(args).status().unwrap().success());
        }
        let key = pin_history::history_key(&self.home).unwrap();
        let hash =
            pin_history::keyed_hash(&key, &locus_core::autopin::normalize_remote_url(REMOTE));
        for i in 0..3 {
            pin_history::append(
                &self.home,
                &HistoryEntry {
                    ts: format!("2026-10-25T00:00:0{i}Z"),
                    event: "pin".into(),
                    alias: Some("acme".into()),
                    expected: None,
                    workspace_hash: None,
                    remote_hashes: vec![hash.clone()],
                },
                100,
            )
            .unwrap();
        }
    }
}

fn assert_change(plan: &Value, action: &str, target: &Path) {
    assert!(
        plan["changes"].as_array().unwrap().iter().any(|change| {
            change["action"] == action && change["target"] == target.to_str().unwrap()
        }),
        "missing {action} {} in {plan}",
        target.display()
    );
}

const ADD: &[&str] = &[
    "binding",
    "add",
    "acme",
    "--tenant",
    "acme",
    "--provider",
    "github",
    "--account",
    "acme",
    "--credential-ref",
    "env:DRY_RUN_FIXTURE_GH",
    "--non-interactive",
];
const AUTOPIN: &[&str] = &["autopin", "suggest", "--apply"];

#[test]
fn binding_add_dry_run_preserves_fresh_and_existing_filesystems() {
    for initialized in [false, true] {
        let env = Env::new(initialized);
        let plan = env.plan(ADD);
        assert_change(
            &plan,
            if initialized { "overwrite" } else { "create" },
            &env.home.join("bindings/acme.toml"),
        );
        let mut forced = ADD.to_vec();
        forced.push("--force");
        env.plan(&forced);
    }
}

#[test]
fn binding_remove_dry_run_keeps_binding_and_audit_untouched() {
    let env = Env::new(true);
    let plan = env.plan(&["binding", "rm", "acme", "--yes"]);
    assert_change(&plan, "delete", &env.home.join("bindings/acme.toml"));
    env.refusal(&["binding", "rm", "missing", "--yes"], "missing");
}

#[test]
fn setup_dry_run_preserves_each_clients_new_and_existing_configs() {
    for client in ["claude", "cursor", "gemini", "codex", "grok"] {
        let env = Env::new(false);
        let args = [
            "setup",
            "--client",
            client,
            "--mcp-bin",
            "/fixture/locus-mcp",
        ];
        let local = match client {
            "claude" => env.project.join(".mcp.json"),
            "cursor" => env.project.join(".cursor/mcp.json"),
            "gemini" => env.project.join(".gemini/settings.json"),
            _ => env.user.join(format!(".{client}/config.toml")),
        };
        let plan = env.plan(&args);
        assert_change(&plan, "create", &local);
        fs::create_dir_all(local.parent().unwrap()).unwrap();
        fs::write(
            &local,
            if matches!(client, "codex" | "grok") {
                "# existing config\n[features]\nfixture = true\n"
            } else {
                "{\"fixture\": true}\n"
            },
        )
        .unwrap();
        let global = match client {
            "cursor" => Some(env.user.join(".cursor/mcp.json")),
            "gemini" => Some(env.user.join(".gemini/settings.json")),
            _ => None,
        };
        if let Some(path) = &global {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "{\"fixture\": \"global\"}\n").unwrap();
        }
        let plan = env.plan(&args);
        assert_change(&plan, "overwrite", &local);
        if let Some(path) = global {
            assert_change(&plan, "overwrite", &path);
        }
        assert!(!env.home.exists());
    }
}

#[test]
fn setup_dry_run_refuses_malformed_json_without_writes() {
    for client in ["claude", "cursor", "gemini"] {
        for contents in ["{ invalid JSON", "[]", "{\"mcpServers\": []}"] {
            let env = Env::new(false);
            let relative = match client {
                "claude" => ".mcp.json",
                "cursor" => ".cursor/mcp.json",
                _ => ".gemini/settings.json",
            };
            let path = env.project.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
            env.refusal(&["setup", "--client", client], "refusing to modify");
        }
    }
}

#[test]
fn setup_dry_run_refuses_malformed_global_json_without_creating_local_config() {
    for (client, relative) in [
        ("cursor", ".cursor/mcp.json"),
        ("gemini", ".gemini/settings.json"),
    ] {
        let env = Env::new(false);
        let global = env.user.join(relative);
        fs::create_dir_all(global.parent().unwrap()).unwrap();
        fs::write(global, "{ invalid global JSON").unwrap();
        env.refusal(&["setup", "--client", client], "refusing to modify");
        assert!(!env.project.join(relative).exists());
    }
}

#[test]
fn setup_dry_run_refuses_invalid_toml_and_server_tables_without_writes() {
    for client in ["codex", "grok"] {
        for contents in [
            "[DRY_RUN_EXISTING_CONFIG_SENTINEL broken TOML",
            "mcp_servers = 1\n",
            "[mcp_servers]\nlocus = 1\n",
            "[mcp_servers.locus]\nenv = 1\n",
        ] {
            let env = Env::new(false);
            let path = env.user.join(format!(".{client}/config.toml"));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
            env.refusal(&["setup", "--client", client], "refusing to modify");
        }
    }
}

#[test]
fn autopin_apply_dry_run_preserves_history_and_workspace() {
    let env = Env::new(true);
    env.history();
    let target = env.project.join(".locus.toml");
    let plan = env.plan(AUTOPIN);
    assert_eq!(plan["suggestion"], "acme");
    assert_change(&plan, "create", &target);
    fs::write(&target, "version = 1\nrequire_pin = true\n").unwrap();
    assert_change(&env.plan(AUTOPIN), "overwrite", &target);
}

#[test]
fn autopin_apply_dry_run_from_subdirectory_targets_git_root() {
    let mut env = Env::new(true);
    env.history();
    let target = env.project.join(".locus.toml");
    env.project = env.project.join("nested");
    fs::create_dir(&env.project).unwrap();
    assert_change(&env.plan(AUTOPIN), "create", &target);
}

#[test]
fn autopin_apply_dry_run_refuses_stale_binding_without_writes() {
    let env = Env::new(true);
    env.history();
    fs::remove_file(env.home.join("bindings/acme.toml")).unwrap();
    env.refusal(AUTOPIN, "acme");
}

#[test]
fn autopin_apply_dry_run_refuses_invalid_binding_without_writes_or_contents() {
    let env = Env::new(true);
    env.history();
    fs::write(
        env.home.join("bindings/acme.toml"),
        "[DRY_RUN_EXISTING_CONFIG_SENTINEL invalid binding",
    )
    .unwrap();
    env.refusal(AUTOPIN, "malformed");
}

#[cfg(unix)]
#[test]
fn autopin_apply_dry_run_refuses_binding_symlink_outside_store() {
    let env = Env::new(true);
    env.history();
    let binding = env.home.join("bindings/acme.toml");
    let outside = env.user.join("outside.toml");
    fs::rename(&binding, &outside).unwrap();
    std::os::unix::fs::symlink(outside, binding).unwrap();
    env.refusal(AUTOPIN, "outside");
}

#[test]
fn autopin_apply_dry_run_refuses_shared_parent_without_writes() {
    let env = Env::new(true);
    env.history();
    fs::write(env.root.join(".locus.toml"), "version = 1\n").unwrap();
    env.refusal(AUTOPIN, "outside this repo");
}

#[cfg(unix)]
#[test]
fn autopin_apply_dry_run_refuses_existing_and_dangling_symlinks() {
    for dangling in [false, true] {
        let env = Env::new(true);
        env.history();
        let external = env.user.join("policy.toml");
        if !dangling {
            fs::write(&external, "version = 1\n").unwrap();
        }
        std::os::unix::fs::symlink(&external, env.project.join(".locus.toml")).unwrap();
        env.refusal(
            AUTOPIN,
            if dangling {
                "link is broken"
            } else {
                "symlink"
            },
        );
    }
}

#[test]
fn leave_force_dry_run_preserves_wedged_session_and_worker_files() {
    let env = Env::new(true);
    let session = env.home.join("sessions/active.json");
    fs::write(&session, "{ malformed session").unwrap();
    fs::create_dir_all(env.home.join("workers/fixture")).unwrap();
    fs::write(env.home.join("workers/fixture/state"), "worker fixture").unwrap();
    fs::write(
        env.home.join("audit/events.jsonl"),
        "existing audit fixture\n",
    )
    .unwrap();
    let plan = env.plan(&["leave", "--force"]);
    assert_change(&plan, "delete", &session);
    env.plan(&["leave", "--force", "--no-verifier"]);
    env.refusal(&["leave"], "locus leave --force");
}

#[test]
fn leave_force_dry_run_without_pin_preserves_filesystem() {
    let env = Env::new(true);
    let plan = env.plan(&["leave", "--force"]);
    assert!(plan["changes"].as_array().unwrap().is_empty());
}

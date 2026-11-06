//! `locus offboard <alias>`: the default (and `--dry-run`) is a read-only
//! `locus.plan.v1` plan; `--apply` removes only the client's records.
//!
//! Every fixture value is synthetic — a preview or apply must never echo a
//! credential reference, a grant seal or a capability anywhere.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Synthetic control capability: 64 lowercase hex, matches no real operator.
const CAP: &str = "cafecafecafecafecafecafecafecafecafecafecafecafecafecafecafecafe";
/// Distinctive fixture strings that must never appear on stdout/stderr.
const SECRET_LIKE: &[&str] = &["OFFBOARD_FIXTURE", "f1xture", CAP];

fn binding_toml(alias: &str) -> String {
    format!(
        r#"
[binding]
id = "bnd_{alias}"
alias = "{alias}"
tenant = "{alias}-co"
[[binding.providers]]
provider = "github"
account = "{alias}"
credential_ref = "env:OFFBOARD_FIXTURE_{}"
"#,
        alias.to_uppercase()
    )
}

fn write_approval(home: &Path, id: &str, binding: &str) -> PathBuf {
    let rec = serde_json::json!({
        "id": id,
        "tool": "github.delete_repo",
        "binding": binding,
        "args_digest": "00".repeat(32),
        "created_at": "2024-01-01T00:00:00Z",
        "status": "pending",
        "session_id": "ses_fixture",
    });
    let dir = home.join("approvals");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{id}.json"));
    fs::write(&path, serde_json::to_string_pretty(&rec).unwrap()).unwrap();
    path
}

fn write_session(store: &locus_core::Store, alias: &str, file: &str) -> PathBuf {
    let worker = store.home().join("workers").join(format!("w-{file}"));
    fs::create_dir_all(&worker).unwrap();
    let session = locus_core::session::Session::new(
        &format!("bnd_{alias}"),
        alias,
        &format!("{alias}-co"),
        None,
        locus_core::session::PinSource::Run,
        None,
        chrono::Duration::hours(1),
        worker.display().to_string(),
        &store.seal_key().unwrap(),
    );
    let path = store.home().join("sessions").join(file);
    fs::write(&path, serde_json::to_string(&session).unwrap()).unwrap();
    path
}

fn write_grant(home: &Path, grant_id: &str, alias: &str) -> PathBuf {
    let grant = serde_json::json!({
        "v": 1,
        "grant_id": grant_id,
        "token_seal": "f1xtureseal",
        "session_id": "ses_fixture",
        "binding_alias": alias,
        "tenant": format!("{alias}-co"),
        "created_at": "2024-01-01T00:00:00Z",
        "expires_at": "2999-01-01T00:00:00Z",
        "revoked": false,
    });
    let dir = home.join("mcp-grants");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{grant_id}.json"));
    fs::write(&path, serde_json::to_string_pretty(&grant).unwrap()).unwrap();
    path
}

/// A two-client home: `acme` owns every record class; `beta` owns a binding,
/// an approval, a session, a grant and a history entry that must survive.
struct Fixture {
    root: tempfile::TempDir,
    home: PathBuf,
}

fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("locus");
    fs::create_dir(root.path().join("user")).unwrap();
    fs::create_dir(root.path().join("project")).unwrap();
    let store = locus_core::Store::open(&home).unwrap();
    for alias in ["acme", "beta"] {
        fs::write(
            store.bindings_dir().join(format!("{alias}.toml")),
            binding_toml(alias),
        )
        .unwrap();
    }
    write_approval(&home, "apr_acme01", "acme");
    write_approval(&home, "apr_beta01", "beta");
    write_session(&store, "acme", "run-acme01.json");
    write_session(&store, "beta", "run-beta01.json");
    write_grant(&home, "aaaaaaaabbbbcccc", "acme");
    write_grant(&home, "ccccccccbbbbaaaa", "beta");
    fs::write(
        store.engagements_dir().join("acme.json"),
        serde_json::to_string(&locus_core::engagement::EngagementMeta::open(
            "acme", "acme-co",
        ))
        .unwrap(),
    )
    .unwrap();
    fs::create_dir_all(store.archives_dir()).unwrap();
    fs::write(store.archives_dir().join("acme-2024-01.jsonl"), "{}\n").unwrap();

    let key = locus_core::pin_history::history_key(&home).unwrap();
    for (event, alias) in [("pin", "acme"), ("leave", "acme"), ("pin", "beta")] {
        let e = locus_core::pin_history::entry_for(&key, event, Some(alias), None, None);
        locus_core::pin_history::append(&home, &e, 100).unwrap();
    }
    // A malformed line is not a record "for" anyone; it must survive a rewrite.
    let history = locus_core::pin_history::path(&home);
    let mut raw = fs::read_to_string(&history).unwrap();
    raw.push_str("{not-json\n");
    fs::write(&history, raw).unwrap();

    Fixture { root, home }
}

#[derive(PartialEq)]
struct Entry {
    contents: Vec<u8>,
    modified: std::time::SystemTime,
    permissions: fs::Permissions,
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Entry> {
    fn visit(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, Entry>) {
        let meta = fs::symlink_metadata(path).unwrap();
        let contents = if meta.is_dir() {
            b"directory".to_vec()
        } else if meta.file_type().is_symlink() {
            fs::read_link(path)
                .unwrap()
                .as_os_str()
                .as_encoded_bytes()
                .to_vec()
        } else {
            assert!(meta.is_file(), "unexpected fixture type");
            fs::read(path).unwrap()
        };
        out.insert(
            path.strip_prefix(root).unwrap().into(),
            Entry {
                contents,
                modified: meta.modified().unwrap(),
                permissions: meta.permissions(),
            },
        );
        if meta.is_dir() {
            for child in fs::read_dir(path).unwrap() {
                visit(root, &child.unwrap().path(), out);
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}

fn assert_unchanged(before: &BTreeMap<PathBuf, Entry>, after: &BTreeMap<PathBuf, Entry>) {
    let changed = before
        .keys()
        .chain(after.keys())
        .find(|p| before.get(*p) != after.get(*p));
    assert!(changed.is_none(), "unexpected file change at {changed:?}");
}

fn locus(fx: &Fixture, args: &[&str], cap: Option<&str>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_locus"));
    cmd.env_clear()
        .env("HOME", fx.root.path().join("user"))
        .env("LOCUS_HOME", &fx.home)
        .env("SHELL", "/bin/bash")
        .env("NO_COLOR", "1")
        .env("LOCUS_NON_INTERACTIVE", "1")
        .current_dir(fx.root.path().join("project"))
        .args(args);
    if let Some(cap) = cap {
        cmd.env("LOCUS_CONTROL_CAPABILITY", cap);
    }
    cmd.output().unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Preview or apply output must never carry a credential ref, a grant seal
/// or the capability value.
fn assert_no_secret_like(out: &Output) {
    let all = format!("{}{}", stdout(out), stderr(out));
    for leak in SECRET_LIKE {
        assert!(!all.contains(leak), "output leaked {leak}: {all}");
    }
}

#[test]
fn plan_is_the_default_and_leaves_every_byte() {
    let fx = fixture();
    // Bare `offboard`, `--dry-run`, and `--json --dry-run` all preview only.
    for extra in [
        vec![],
        vec!["--dry-run"],
        vec!["--dry-run", "--json"],
        vec!["--json"],
    ] {
        let before = snapshot(&fx.home);
        let mut args = vec!["offboard", "acme"];
        args.extend(extra.iter().copied());
        let out = locus(&fx, &args, None);
        assert_unchanged(&before, &snapshot(&fx.home));
        assert!(out.status.success(), "{args:?} failed: {}", stderr(&out));
        assert_no_secret_like(&out);
        if extra.contains(&"--json") {
            let plan: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
            assert_eq!(plan["schema"], "locus.plan.v1");
            assert_eq!(plan["dry_run"], true);
            assert_eq!(plan["command"], "locus offboard acme --apply");
            assert_eq!(plan["requires_control_capability"], true);
            let changes = plan["changes"].as_array().unwrap();
            let targets: Vec<String> = changes
                .iter()
                .map(|c| format!("{} {}", c["action"], c["target"]))
                .collect();
            for needle in [
                "delete",
                "bindings/acme.toml",
                "approvals/apr_acme01.json",
                "run-acme01.json",
                "aaaaaaaabbbbcccc.json",
                "engagements/acme.json",
                "update",
                "pin-history.jsonl",
            ] {
                assert!(
                    targets.iter().any(|t| t.contains(needle)),
                    "plan change missing {needle}: {targets:?}"
                );
            }
            // The other client is never in the plan.
            assert!(targets.iter().all(|t| !t.contains("beta")));
        } else {
            let text = stdout(&out);
            assert!(text.contains("planned change"), "{text}");
            assert!(text.contains("nothing written"), "{text}");
        }
    }
}

#[test]
fn apply_removes_the_client_and_keeps_the_other() {
    let fx = fixture();
    let beta_binding = fs::read(fx.home.join("bindings/beta.toml")).unwrap();
    let beta_session = fs::read(fx.home.join("sessions/run-beta01.json")).unwrap();
    let beta_grant = fs::read(fx.home.join("mcp-grants/ccccccccbbbbaaaa.json")).unwrap();
    let archive = fx.home.join("archives/acme-2024-01.jsonl");
    let archive_bytes = fs::read(&archive).unwrap();

    let out = locus(&fx, &["offboard", "acme", "--apply", "--json"], Some(CAP));
    assert!(out.status.success(), "apply failed: {}", stderr(&out));
    assert_no_secret_like(&out);
    let body: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(body["schema"], "locus.offboard.v1");
    let outcome = &body["outcome"];
    assert_eq!(outcome["removed_binding"], true);
    assert_eq!(outcome["removed_approvals"], 1);
    assert_eq!(outcome["removed_sessions"], 1);
    assert_eq!(outcome["revoked_mcp_grants"], 1);
    assert_eq!(outcome["removed_engagement_meta"], true);
    assert_eq!(outcome["dropped_pin_history_entries"], 2);

    // Every acme-owned record is gone.
    for gone in [
        "bindings/acme.toml",
        "approvals/apr_acme01.json",
        "sessions/run-acme01.json",
        "mcp-grants/aaaaaaaabbbbcccc.json",
        "engagements/acme.json",
    ] {
        assert!(!fx.home.join(gone).exists(), "{gone} still exists");
    }
    // The session's worker home is cleaned only under the workers/ root.
    assert!(!fx.home.join("workers/w-run-acme01.json").exists());
    // Pin history kept the other client and the malformed line, byte-for-byte.
    let history = fs::read_to_string(locus_core::pin_history::path(&fx.home)).unwrap();
    assert!(history.contains("\"beta\""), "{history}");
    assert!(!history.contains("\"acme\""), "{history}");
    assert!(history.contains("{not-json"), "{history}");
    // The other client is byte-identical; the audit archive stays as evidence.
    assert_eq!(
        fs::read(fx.home.join("bindings/beta.toml")).unwrap(),
        beta_binding
    );
    assert_eq!(
        fs::read(fx.home.join("sessions/run-beta01.json")).unwrap(),
        beta_session
    );
    assert_eq!(
        fs::read(fx.home.join("mcp-grants/ccccccccbbbbaaaa.json")).unwrap(),
        beta_grant
    );
    assert!(fx.home.join("approvals/apr_beta01.json").exists());
    assert!(fx.home.join("workers/w-run-beta01.json").exists());
    assert_eq!(fs::read(&archive).unwrap(), archive_bytes);
    // The offboard itself is audited — evidence is kept, operable state is not.
    let audit = fs::read_to_string(fx.home.join("audit/events.jsonl")).unwrap();
    assert!(audit.contains("client.offboard"), "{audit}");
}

#[test]
fn apply_without_the_capability_changes_nothing() {
    let fx = fixture();
    let before = snapshot(&fx.home);
    let out = locus(&fx, &["offboard", "acme", "--apply"], None);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("LOCUS_CONTROL_CAPABILITY"));
    assert_unchanged(&before, &snapshot(&fx.home));
    assert!(fx.home.join("bindings/acme.toml").exists());
}

#[test]
fn apply_and_dry_run_is_a_usage_error() {
    let fx = fixture();
    let before = snapshot(&fx.home);
    let out = locus(
        &fx,
        &["offboard", "acme", "--apply", "--dry-run"],
        Some(CAP),
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr(&out).contains("cannot be combined"),
        "{}",
        stderr(&out)
    );
    assert_unchanged(&before, &snapshot(&fx.home));
}

#[test]
fn an_unknown_alias_fails_without_writes() {
    let fx = fixture();
    let before = snapshot(&fx.home);
    let out = locus(&fx, &["offboard", "ghost"], None);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("binding not found: ghost"),
        "{}",
        stderr(&out)
    );
    assert_unchanged(&before, &snapshot(&fx.home));

    let out = locus(&fx, &["offboard", "ghost", "--apply"], Some(CAP));
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("binding not found: ghost"),
        "{}",
        stderr(&out)
    );
}

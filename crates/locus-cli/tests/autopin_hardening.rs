//! Issue #181 regression coverage: the `locus autopin suggest` hardening
//! follow-ups from the #163 review. Each test pins a behavior the pre-hardening
//! code lacked, so it fails against the pre-#226 tree:
//! - `--apply` must refuse a winner outside a non-empty `allowed_bindings`
//!   (matched by alias or id); it used to write the unusable default anyway.
//! - the atomic `.locus.toml` rewrite must keep the existing file's mode on
//!   the temp file; it used to create the replacement at a fixed 0644.
//! - the rename must be followed by a parent-directory fsync (the durable-half
//!   is pinned at the core seam; here we pin the atomic-write contract).
//! - a read-only `suggest` must not create `.pin-history.lock`; the locked
//!   reader used to create it on every call.
//! - `git rev-parse --show-toplevel` may lose only its line ending and the
//!   reported root must contain cwd; `trim()` used to corrupt whitespace
//!   roots and an outside root was written without a containment check.
//! - the README CLI inventory must name `locus autopin suggest
//!   [--json] [--apply]` exactly once.

use std::path::Path;
use std::process::{Command, Output};

use locus_core::autopin::normalize_remote_url;
use locus_core::pin_history::{self, HistoryEntry};

const URL: &str = "https://github.com/acme-co/widgets.git";

fn locus(
    home: &Path,
    cwd: &Path,
    cap: Option<&str>,
    env: &[(&str, &str)],
    args: &[&str],
) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_locus"));
    cmd.args(args)
        .env("LOCUS_HOME", home)
        .env_remove("LOCUS_CONTROL_CAPABILITY")
        .env_remove("LOCUS_PIN_HISTORY")
        .env("NO_COLOR", "1")
        .current_dir(cwd);
    if let Some(cap) = cap {
        cmd.env("LOCUS_CONTROL_CAPABILITY", cap);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("locus binary runs")
}

fn init_git_repo(dir: &Path) {
    for args in [&["init", "-q"][..], &["remote", "add", "origin", URL][..]] {
        assert!(Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
}

/// Sample bindings, a control capability, and `pin` history entries keyed to
/// `URL`; five pins of one alias is enough to win (score >= MIN_SCORE).
fn setup(pins: &[(&str, usize)]) -> (tempfile::TempDir, tempfile::TempDir, String) {
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    init_git_repo(repo.path());
    let init = locus(
        home.path(),
        repo.path(),
        None,
        &[],
        &["init", "--with-samples"],
    );
    assert!(init.status.success(), "init failed: {init:?}");
    let cap = std::fs::read_to_string(home.path().join("control_capability"))
        .unwrap()
        .trim()
        .to_string();
    let key = pin_history::history_key(home.path()).unwrap();
    let hash = pin_history::keyed_hash(&key, &normalize_remote_url(URL));
    let mut n = 0;
    for (alias, count) in pins {
        for _ in 0..*count {
            n += 1;
            pin_history::append(
                home.path(),
                &HistoryEntry {
                    ts: format!("2026-10-25T00:00:{n:02}Z"),
                    event: "pin".into(),
                    alias: Some((*alias).into()),
                    expected: None,
                    workspace_hash: None,
                    remote_hashes: vec![hash.clone()],
                },
                2000,
            )
            .unwrap();
        }
    }
    (home, repo, cap)
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn json(o: &Output) -> serde_json::Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("bad json {e}: {o:?}"))
}

fn binding_id(home: &Path, alias: &str) -> String {
    let binding: toml::Table = toml::from_str(
        &std::fs::read_to_string(home.join("bindings").join(format!("{alias}.toml"))).unwrap(),
    )
    .unwrap();
    binding["binding"]["id"].as_str().unwrap().to_string()
}

/// A suggestion outside a non-empty `allowed_bindings` is refused by both the
/// plan (`--dry-run`) and the write (`--apply`); the file is never touched.
/// The same winner is accepted when the allowlist names it by alias or by id.
#[test]
fn apply_respects_a_nonempty_allowlist_matched_by_alias_or_id() {
    let (home, repo, cap) = setup(&[("acme", 5)]);
    let file = repo.path().join(".locus.toml");

    let blocked = "version = 1\nallowed_bindings = [\"personal\"]\nrequire_pin = true\n";
    std::fs::write(&file, blocked).unwrap();
    for args in [
        &["autopin", "suggest", "--apply", "--dry-run", "--json"][..],
        &["autopin", "suggest", "--apply", "--json"][..],
    ] {
        let o = locus(home.path(), repo.path(), Some(&cap), &[], args);
        assert!(!o.status.success(), "blocked winner accepted: {o:?}");
        assert!(text(&o).contains("allowed_bindings"), "{o:?}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), blocked);
    }

    let id = binding_id(home.path(), "acme");
    for allowed in ["acme", id.as_str()] {
        std::fs::write(
            &file,
            format!("version = 1\nallowed_bindings = [\"{allowed}\"]\nrequire_pin = true\n"),
        )
        .unwrap();
        let o = locus(
            home.path(),
            repo.path(),
            Some(&cap),
            &[],
            &["autopin", "suggest", "--apply", "--json"],
        );
        assert!(o.status.success(), "allowlisted winner refused: {o:?}");
        let written: toml::Table =
            toml::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(written["default_binding"].as_str(), Some("acme"));
    }
}

/// The temp file renamed over `.locus.toml` carries the existing file's mode,
/// so a 0600 or group-readable policy does not come back at a fixed 0644.
#[cfg(unix)]
#[test]
fn apply_carries_the_existing_locus_toml_mode() {
    use std::os::unix::fs::PermissionsExt;

    let (home, repo, cap) = setup(&[("acme", 5)]);
    let file = repo.path().join(".locus.toml");
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    for m in [0o400, 0o600, 0o640] {
        if file.exists() {
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        std::fs::write(&file, "version = 1\nrequire_pin = true\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(m)).unwrap();
        let o = locus(
            home.path(),
            repo.path(),
            Some(&cap),
            &[],
            &["autopin", "suggest", "--apply"],
        );
        assert!(o.status.success(), "{o:?}");
        assert_eq!(mode(&file), m, "mode {m:o} not carried onto the temp file");
    }

    // A fresh `.locus.toml` is created no wider than the default 0644.
    std::fs::remove_file(&file).unwrap();
    let o = locus(
        home.path(),
        repo.path(),
        Some(&cap),
        &[],
        &["autopin", "suggest", "--apply"],
    );
    assert!(o.status.success(), "{o:?}");
    assert_eq!(mode(&file) & !0o644, 0, "new file wider than 0644");
}

/// The write commits by `create_new` temp + rename, then fsyncs the parent
/// directory so the new directory entry is durable. The fsync half is pinned
/// at the `default_binding_syncs_parent_after_rename_and_reports_sync_failure`
/// unit seam in locus-core (a failing-before test: the seam did not exist);
/// here we pin the observable end of the contract.
#[test]
fn apply_commits_the_replacement_with_no_staging_debris() {
    let (home, repo, cap) = setup(&[("acme", 5)]);
    let file = repo.path().join(".locus.toml");
    std::fs::write(&file, "version = 1\nrequire_pin = true\n").unwrap();
    let o = locus(
        home.path(),
        repo.path(),
        Some(&cap),
        &[],
        &["autopin", "suggest", "--apply", "--json"],
    );
    assert!(o.status.success(), "{o:?}");
    let v = json(&o);
    assert_eq!(v["applied"], true);
    assert_eq!(
        v["written"],
        file.canonicalize().unwrap().display().to_string()
    );
    let written: toml::Table = toml::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(written["default_binding"].as_str(), Some("acme"));
    assert_eq!(written["require_pin"].as_bool(), Some(true));
    let debris: Vec<_> = std::fs::read_dir(repo.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(debris.is_empty(), "staging temp left behind: {debris:?}");
}

/// A read-only `suggest` must not create `.pin-history.lock` or rewrite the
/// log; the pre-hardening reader opened the lock (O_CREAT) on every call.
#[test]
fn suggest_reads_history_without_creating_the_lock() {
    let (home, repo, _) = setup(&[("acme", 5)]);
    let lock = home.path().join(".pin-history.lock");
    std::fs::remove_file(&lock).unwrap();
    let history = std::fs::read(home.path().join(pin_history::FILE_NAME)).unwrap();

    for args in [
        &["autopin", "suggest"][..],
        &["autopin", "suggest", "--json"][..],
    ] {
        let o = locus(home.path(), repo.path(), None, &[], args);
        assert!(o.status.success(), "{o:?}");
        assert!(!lock.exists(), "read-only suggest created the history lock");
        assert_eq!(
            std::fs::read(home.path().join(pin_history::FILE_NAME)).unwrap(),
            history
        );
    }
    assert_eq!(
        json(&locus(
            home.path(),
            repo.path(),
            None,
            &[],
            &["autopin", "suggest", "--json"]
        ))["report"]["suggestion"],
        "acme"
    );
}

/// `rev-parse --show-toplevel` may lose only its trailing line ending: a root
/// whose name ends in real whitespace still gets its `.locus.toml`, where
/// `trim()` used to select a different (missing or wrong) directory.
#[cfg(unix)]
#[test]
fn apply_writes_a_git_root_named_with_trailing_whitespace() {
    let (home, _seed, cap) = setup(&[("acme", 5)]);
    let parent = tempfile::tempdir().unwrap();
    let repo = parent.path().join("repo \t\n");
    std::fs::create_dir(&repo).unwrap();
    init_git_repo(&repo);
    let subdir = repo.join("src");
    std::fs::create_dir(&subdir).unwrap();

    let o = locus(
        home.path(),
        &subdir,
        Some(&cap),
        &[],
        &["autopin", "suggest", "--apply", "--json"],
    );
    assert!(o.status.success(), "whitespace root refused: {o:?}");
    let file = repo.canonicalize().unwrap().join(".locus.toml");
    assert_eq!(json(&o)["written"], file.display().to_string());
    let written: toml::Table = toml::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(written["default_binding"].as_str(), Some("acme"));
    assert!(!subdir.join(".locus.toml").exists());
}

/// A toplevel that does not contain cwd (a `GIT_WORK_TREE` pointing outside
/// the repo) is refused; the pre-hardening target trusted it and wrote the
/// default into the foreign tree.
#[test]
fn apply_refuses_a_toplevel_that_does_not_contain_cwd() {
    let (home, repo, cap) = setup(&[("acme", 5)]);
    let outside = tempfile::tempdir().unwrap();
    let git_env = [
        (
            "GIT_DIR",
            repo.path().join(".git").to_str().unwrap().to_string(),
        ),
        (
            "GIT_WORK_TREE",
            outside.path().to_str().unwrap().to_string(),
        ),
    ];
    let git = Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["rev-parse", "--show-toplevel"])
        .envs(git_env.iter().map(|(k, v)| (*k, v.as_str())))
        .output()
        .unwrap();
    assert!(git.status.success());
    for args in [
        &["autopin", "suggest", "--apply", "--dry-run", "--json"][..],
        &["autopin", "suggest", "--apply", "--json"][..],
    ] {
        let env: Vec<(&str, &str)> = git_env.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let o = locus(home.path(), repo.path(), Some(&cap), &env, args);
        assert!(!o.status.success(), "outside toplevel accepted: {o:?}");
        assert!(!repo.path().join(".locus.toml").exists());
        assert!(!outside.path().join(".locus.toml").exists());
    }
}

/// The README CLI inventory names `locus autopin suggest [--json] [--apply]`
/// exactly once.
#[test]
fn readme_cli_inventory_lists_autopin_suggest_with_json_and_apply() {
    let readme =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../README.md"))
            .unwrap();
    let block = readme
        .split("## CLI")
        .nth(1)
        .and_then(|s| s.split("```").nth(1))
        .expect("README ## CLI fenced block");
    let lines: Vec<&str> = block
        .lines()
        .filter(|l| l.trim_start().starts_with("locus autopin suggest"))
        .collect();
    assert_eq!(
        lines.len(),
        1,
        "expected exactly one `locus autopin suggest` line in the CLI inventory, found {lines:?}"
    );
    assert!(
        lines[0].contains("[--json]") && lines[0].contains("[--apply]"),
        "inventory line must show `locus autopin suggest [--json] [--apply]`: {}",
        lines[0]
    );
}

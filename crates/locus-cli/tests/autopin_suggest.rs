//! E2E for `locus autopin suggest` (L2): suggestion from keyed pin history,
//! the history off switch, the control-capability gate on `--apply`, the
//! atomic `.locus.toml` write, and that the raw remote URL never leaks.

use std::path::Path;
use std::process::{Command, Output};

use locus_core::autopin::normalize_remote_url;
use locus_core::pin_history::{self, HistoryEntry};

const URL: &str = "https://octo:tok3n-SECRET@github.com/acme-co/widgets.git";
const RAW_BITS: [&str; 3] = ["tok3n-SECRET", "acme-co/widgets", "octo:"];

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

fn git_repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    for args in [&["init", "-q"][..], &["remote", "add", "origin", URL][..]] {
        assert!(Command::new("git")
            .arg("-C")
            .arg(d.path())
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    d
}

/// Quickstart (bindings acme + personal), then seed history for the repo.
fn setup(pins: &[(&str, usize)]) -> (tempfile::TempDir, tempfile::TempDir, String) {
    let home = tempfile::tempdir().unwrap();
    let repo = git_repo();
    // Quickstart outside the repo so its own pin is not history for the repo.
    let elsewhere = tempfile::tempdir().unwrap();
    let qs = locus(home.path(), elsewhere.path(), None, &[], &["quickstart"]);
    assert!(qs.status.success(), "quickstart failed: {qs:?}");
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
            let e = HistoryEntry {
                ts: format!("2026-10-25T00:00:{n:02}Z"),
                event: "pin".into(),
                alias: Some((*alias).into()),
                expected: None,
                workspace_hash: None,
                remote_hashes: vec![hash.clone()],
            };
            pin_history::append(home.path(), &e, 2000).unwrap();
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

fn assert_no_raw_url(s: &str) {
    for bit in RAW_BITS {
        assert!(!s.contains(bit), "raw remote leaked ({bit}): {s}");
    }
}

fn json(o: &Output) -> serde_json::Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("bad json {e}: {o:?}"))
}

/// Include directories as well as file bytes so read-only commands cannot
/// silently create a lock file or initialize an otherwise empty home.
fn snapshot(root: &Path) -> std::collections::BTreeMap<std::path::PathBuf, Option<Vec<u8>>> {
    fn walk(
        root: &Path,
        dir: &Path,
        result: &mut std::collections::BTreeMap<std::path::PathBuf, Option<Vec<u8>>>,
    ) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let metadata = entry.metadata().unwrap();
            let bytes = metadata.is_file().then(|| std::fs::read(&path).unwrap());
            result.insert(path.strip_prefix(root).unwrap().to_path_buf(), bytes);
            if metadata.is_dir() {
                walk(root, &path, result);
            }
        }
    }
    let mut result = std::collections::BTreeMap::new();
    walk(root, root, &mut result);
    result
}

#[test]
fn suggests_from_history_with_stable_json_and_no_raw_url() {
    let (home, repo, _) = setup(&[("acme", 4), ("personal", 1)]);
    let o = locus(
        home.path(),
        repo.path(),
        None,
        &[],
        &["autopin", "suggest", "--json"],
    );
    assert!(o.status.success(), "{o:?}");
    assert_no_raw_url(&text(&o));
    let v = json(&o);
    let r = &v["report"];
    assert_eq!(r["schema"], "locus.autopin.suggest.v1");
    assert_eq!(r["suggestion"], "acme");
    assert_eq!(r["reason"], "suggested");
    assert_eq!(r["candidates"][0]["pins"], 4);
    assert_eq!(r["candidates"][0]["last_seen"], "2026-10-25T00:00:04Z");
    assert_eq!(v["applied"], false);
    // Human output too, and nothing written.
    let h = locus(home.path(), repo.path(), None, &[], &["autopin", "suggest"]);
    assert!(text(&h).contains("suggested binding: acme"), "{h:?}");
    assert_no_raw_url(&text(&h));
    assert!(!repo.path().join(".locus.toml").exists());
    // History file holds only hashes.
    let hist = std::fs::read_to_string(home.path().join("pin-history.jsonl")).unwrap();
    assert_no_raw_url(&hist);
    assert!(!hist.contains("github.com"));
}

#[test]
fn thin_and_tied_evidence_suggest_nothing() {
    let (home, repo, _) = setup(&[("acme", 2)]);
    let v = json(&locus(
        home.path(),
        repo.path(),
        None,
        &[],
        &["autopin", "suggest", "--json"],
    ));
    assert_eq!(v["report"]["reason"], "thin_evidence");
    assert!(v["report"]["suggestion"].is_null());

    let (home, repo, _) = setup(&[("acme", 3), ("personal", 3)]);
    let v = json(&locus(
        home.path(),
        repo.path(),
        None,
        &[],
        &["autopin", "suggest", "--json"],
    ));
    assert_eq!(v["report"]["reason"], "tied");
    assert!(v["report"]["suggestion"].is_null());
}

#[test]
fn disabled_history_suggests_nothing() {
    let (home, repo, _) = setup(&[("acme", 5)]);
    let o = locus(
        home.path(),
        repo.path(),
        None,
        &[("LOCUS_PIN_HISTORY", "0")],
        &["autopin", "suggest"],
    );
    assert!(o.status.success());
    assert!(text(&o).contains("pin history is disabled"), "{o:?}");
    let v = json(&locus(
        home.path(),
        repo.path(),
        None,
        &[("LOCUS_PIN_HISTORY", "0")],
        &["autopin", "suggest", "--json"],
    ));
    assert_eq!(v["report"]["history_enabled"], false);
    assert!(v["report"]["suggestion"].is_null());
}

#[test]
fn apply_is_refused_without_capability() {
    let (home, repo, _) = setup(&[("acme", 5)]);
    let o = locus(
        home.path(),
        repo.path(),
        None,
        &[],
        &["autopin", "suggest", "--apply"],
    );
    assert!(
        !o.status.success(),
        "apply must fail without capability: {o:?}"
    );
    assert!(text(&o).contains("LOCUS_CONTROL_CAPABILITY"), "{o:?}");
    assert_no_raw_url(&text(&o));
    assert!(!repo.path().join(".locus.toml").exists());
}

#[test]
fn apply_with_capability_writes_locus_toml_atomically() {
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
    assert_no_raw_url(&text(&o));
    assert_eq!(json(&o)["applied"], true);
    let written = std::fs::read_to_string(&file).unwrap();
    assert_no_raw_url(&written);
    let t: toml::Table = toml::from_str(&written).unwrap();
    assert_eq!(t["default_binding"].as_str(), Some("acme"));
    assert_eq!(t["require_pin"].as_bool(), Some(true), "other keys kept");
    let leftovers: Vec<_> = std::fs::read_dir(repo.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "temp file left behind");
}

#[cfg(unix)]
#[test]
fn apply_refuses_symlinked_locus_toml() {
    let (home, repo, cap) = setup(&[("acme", 5)]);
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("victim.toml");
    std::fs::write(&target, "version = 1\n").unwrap();
    std::os::unix::fs::symlink(&target, repo.path().join(".locus.toml")).unwrap();
    for args in [
        &["autopin", "suggest", "--apply"][..],
        &["autopin", "suggest", "--apply", "--dry-run", "--json"][..],
    ] {
        let o = locus(home.path(), repo.path(), Some(&cap), &[], args);
        assert!(!o.status.success(), "{o:?}");
        assert!(text(&o).contains("symlink"), "{o:?}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "version = 1\n");
        assert!(std::fs::symlink_metadata(repo.path().join(".locus.toml"))
            .unwrap()
            .file_type()
            .is_symlink());
    }
}

#[test]
fn suggest_without_history_key_writes_nothing() {
    let home = tempfile::tempdir().unwrap();
    let repo = git_repo();
    let elsewhere = tempfile::tempdir().unwrap();
    let qs = locus(
        home.path(),
        elsewhere.path(),
        None,
        &[("LOCUS_PIN_HISTORY", "0")],
        &["quickstart"],
    );
    assert!(qs.status.success(), "{qs:?}");
    let key = home.path().join(pin_history::KEY_FILE);
    let _ = std::fs::remove_file(&key);
    let before: Vec<_> = std::fs::read_dir(home.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    let v = json(&locus(
        home.path(),
        repo.path(),
        None,
        &[],
        &["autopin", "suggest", "--json"],
    ));
    assert_eq!(v["report"]["reason"], "no_history");
    assert!(!key.exists(), "read-only suggest created history.key");
    let after: Vec<_> = std::fs::read_dir(home.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(before, after, "suggest wrote to LOCUS_HOME");
    assert!(!repo.path().join(".locus.toml").exists());
}

#[test]
fn apply_refuses_parent_locus_toml_above_git_toplevel() {
    let (home, _repo, cap) = setup(&[("acme", 5)]);
    let parent = tempfile::tempdir().unwrap();
    let shared = parent.path().join(".locus.toml");
    std::fs::write(&shared, "version = 1\n").unwrap();
    let repo = parent.path().join("child");
    std::fs::create_dir(&repo).unwrap();
    for args in [&["init", "-q"][..], &["remote", "add", "origin", URL][..]] {
        assert!(Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    for args in [
        &["autopin", "suggest", "--apply"][..],
        &["autopin", "suggest", "--apply", "--dry-run", "--json"][..],
    ] {
        let o = locus(home.path(), &repo, Some(&cap), &[], args);
        assert!(!o.status.success(), "{o:?}");
        assert!(text(&o).contains("outside this repo"), "{o:?}");
        assert_eq!(std::fs::read_to_string(&shared).unwrap(), "version = 1\n");
        assert!(!repo.join(".locus.toml").exists());
    }
}

#[test]
fn dry_run_and_apply_target_git_root_from_a_subdirectory() {
    let (home, repo, cap) = setup(&[("acme", 5)]);
    let nested = repo.path().join("src/nested");
    std::fs::create_dir_all(&nested).unwrap();
    let planned = locus(
        home.path(),
        &nested,
        None,
        &[],
        &["autopin", "suggest", "--apply", "--dry-run", "--json"],
    );
    assert!(planned.status.success(), "{planned:?}");
    let plan = json(&planned);
    let expected = repo.path().canonicalize().unwrap().join(".locus.toml");
    assert_eq!(plan["changes"][0]["target"], expected.to_str().unwrap());
    assert_eq!(plan["changes"][0]["action"], "create");
    assert_eq!(plan["requires_control_capability"], true);
    assert!(!expected.exists());

    let applied = locus(
        home.path(),
        &nested,
        Some(&cap),
        &[],
        &["autopin", "suggest", "--apply", "--json"],
    );
    assert!(applied.status.success(), "{applied:?}");
    assert_eq!(json(&applied)["written"], plan["changes"][0]["target"]);
    assert_eq!(
        locus_core::workspace::WorkspaceConfig::parse(&std::fs::read_to_string(expected).unwrap())
            .unwrap()
            .default_binding
            .as_deref(),
        Some("acme")
    );
    assert!(!nested.join(".locus.toml").exists());
}

#[test]
fn dry_run_and_apply_preserve_nearest_child_workspace_policy() {
    let (home, repo, cap) = setup(&[("acme", 5)]);
    let child = repo.path().join("packages/app");
    let nested = child.join("src");
    std::fs::create_dir_all(&nested).unwrap();
    let root_file = repo.path().join(".locus.toml");
    let root_policy = "version = 1\nallowed_bindings = [\"personal\"]\nrequire_pin = true\n";
    std::fs::write(&root_file, root_policy).unwrap();
    let child_file = child.join(".locus.toml");
    let child_policy =
        "version = 1\nallowed_bindings = [\"acme\"]\nrequire_pin = true\ncustom = \"preserved\"\n";
    std::fs::write(&child_file, child_policy).unwrap();
    let planned = locus(
        home.path(),
        &nested,
        None,
        &[],
        &["autopin", "suggest", "--apply", "--dry-run", "--json"],
    );
    assert!(planned.status.success(), "{planned:?}");
    let target = child_file.canonicalize().unwrap();
    assert_eq!(
        json(&planned)["changes"][0]["target"],
        target.to_str().unwrap()
    );
    assert_eq!(std::fs::read_to_string(&child_file).unwrap(), child_policy);
    let applied = locus(
        home.path(),
        &nested,
        Some(&cap),
        &[],
        &["autopin", "suggest", "--apply", "--json"],
    );
    assert!(applied.status.success(), "{applied:?}");
    assert_eq!(json(&applied)["written"], target.to_str().unwrap());
    let written: toml::Table =
        toml::from_str(&std::fs::read_to_string(child_file).unwrap()).unwrap();
    assert_eq!(written["default_binding"].as_str(), Some("acme"));
    assert_eq!(
        written["allowed_bindings"].as_array().unwrap()[0].as_str(),
        Some("acme")
    );
    assert_eq!(written["require_pin"].as_bool(), Some(true));
    assert_eq!(written["custom"].as_str(), Some("preserved"));
    assert_eq!(std::fs::read_to_string(root_file).unwrap(), root_policy);
}

#[test]
fn unknown_suggested_binding_refuses_apply_and_dry_run() {
    let (home, repo, cap) = setup(&[("missing", 5)]);
    for args in [
        &["autopin", "suggest", "--apply"][..],
        &["autopin", "suggest", "--apply", "--dry-run", "--json"][..],
    ] {
        let o = locus(home.path(), repo.path(), Some(&cap), &[], args);
        assert!(!o.status.success(), "{o:?}");
        assert!(text(&o).contains("binding not found: missing"), "{o:?}");
        assert!(!repo.path().join(".locus.toml").exists());
    }
}

#[test]
fn blocked_winner_refuses_apply_and_dry_run_without_falling_back() {
    let (home, repo, cap) = setup(&[("acme", 5), ("personal", 3)]);
    let file = repo.path().join(".locus.toml");
    let policy = "version = 1\ndefault_binding = \"personal\"\nallowed_bindings = [\"personal\"]\nrequire_pin = true\n";
    std::fs::write(&file, policy).unwrap();
    for args in [
        &["autopin", "suggest", "--apply"][..],
        &["autopin", "suggest", "--apply", "--dry-run", "--json"][..],
    ] {
        let o = locus(home.path(), repo.path(), Some(&cap), &[], args);
        assert!(!o.status.success(), "{o:?}");
        assert!(text(&o).contains("not allowed"), "{o:?}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), policy);
        assert_no_raw_url(&text(&o));
    }
}

#[test]
fn workspace_allowlist_accepts_binding_id_for_apply_and_dry_run() {
    let (home, repo, cap) = setup(&[("acme", 5)]);
    let binding = locus_core::binding::Binding::parse_toml(
        &std::fs::read_to_string(home.path().join("bindings/acme.toml")).unwrap(),
    )
    .unwrap();
    assert_ne!(binding.id, binding.alias);
    let file = repo.path().join(".locus.toml");
    std::fs::write(
        &file,
        format!("version = 1\nallowed_bindings = [\"{}\"]\n", binding.id),
    )
    .unwrap();
    for args in [
        &["autopin", "suggest", "--apply", "--dry-run", "--json"][..],
        &["autopin", "suggest", "--apply", "--json"][..],
    ] {
        let o = locus(home.path(), repo.path(), Some(&cap), &[], args);
        assert!(o.status.success(), "{o:?}");
    }
    let written = std::fs::read_to_string(file).unwrap();
    let config = locus_core::workspace::WorkspaceConfig::parse(&written).unwrap();
    assert_eq!(config.default_binding.as_deref(), Some("acme"));
    assert_eq!(config.allowed_bindings, vec![binding.id]);
}

#[test]
fn suggest_and_dry_run_create_no_files_even_without_history_lock() {
    let (home, repo, _) = setup(&[("acme", 5)]);
    std::fs::remove_file(home.path().join(".pin-history.lock")).unwrap();
    let home_before = snapshot(home.path());
    let repo_before = snapshot(repo.path());
    for args in [
        &["autopin", "suggest", "--json"][..],
        &["autopin", "suggest", "--apply", "--dry-run", "--json"][..],
    ] {
        let o = locus(home.path(), repo.path(), None, &[], args);
        assert!(o.status.success(), "{o:?}");
        assert_no_raw_url(&text(&o));
        // Snapshots include temporary capability/key files; never dump bytes
        // into test diagnostics if a read-only operation unexpectedly writes.
        assert!(snapshot(home.path()) == home_before, "home changed");
        assert!(snapshot(repo.path()) == repo_before, "repo changed");
    }
}

#[test]
fn suggest_with_uninitialized_home_creates_nothing() {
    let parent = tempfile::tempdir().unwrap();
    let home = parent.path().join("not-initialized");
    let repo = git_repo();
    let repo_before = snapshot(repo.path());
    let o = locus(
        &home,
        repo.path(),
        None,
        &[],
        &["autopin", "suggest", "--json"],
    );
    assert!(o.status.success(), "{o:?}");
    assert_eq!(json(&o)["report"]["reason"], "no_history");
    assert_no_raw_url(&text(&o));
    assert!(!home.exists());
    assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 0);
    assert!(snapshot(repo.path()) == repo_before, "repo changed");
}

#[test]
fn dry_run_refuses_missing_binding_lock_without_creating_it() {
    let (home, repo, _) = setup(&[("acme", 5)]);
    std::fs::remove_file(home.path().join("bindings/.credential-migrations.lock")).unwrap();
    let home_before = snapshot(home.path());
    let repo_before = snapshot(repo.path());
    let o = locus(
        home.path(),
        repo.path(),
        None,
        &[],
        &["autopin", "suggest", "--apply", "--dry-run", "--json"],
    );
    assert!(!o.status.success(), "{o:?}");
    assert!(text(&o).contains("credential migration lock"), "{o:?}");
    assert!(snapshot(home.path()) == home_before, "home changed");
    assert!(snapshot(repo.path()) == repo_before, "repo changed");
}

#[cfg(unix)]
#[test]
fn symlinked_history_is_an_error_for_suggest_and_apply_preview() {
    let (home, repo, _) = setup(&[("acme", 5)]);
    let history = home.path().join(pin_history::FILE_NAME);
    let target = home.path().join("saved-history.jsonl");
    std::fs::rename(&history, &target).unwrap();
    std::os::unix::fs::symlink(&target, &history).unwrap();
    let before = std::fs::read(&target).unwrap();
    for args in [
        &["autopin", "suggest", "--json"][..],
        &["autopin", "suggest", "--apply", "--dry-run", "--json"][..],
    ] {
        let o = locus(home.path(), repo.path(), None, &[], args);
        assert!(!o.status.success(), "{o:?}");
        assert_no_raw_url(&text(&o));
        assert_eq!(std::fs::read(&target).unwrap(), before);
        assert!(!repo.path().join(".locus.toml").exists());
    }
}

#[test]
fn spoofed_non_origin_remote_does_not_match() {
    let (home, repo, _) = setup(&[("acme", 5)]);
    for args in [
        &["remote", "rename", "origin", "upstream"][..],
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/someone/fork.git",
        ][..],
    ] {
        assert!(Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    let v = json(&locus(
        home.path(),
        repo.path(),
        None,
        &[],
        &["autopin", "suggest", "--json"],
    ));
    assert_eq!(v["report"]["reason"], "no_history");
    assert!(v["report"]["suggestion"].is_null());
}

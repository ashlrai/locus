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
    init_git_repo(d.path());
    d
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

/// Initialize bindings acme + personal, then seed history for the repo.
fn setup(pins: &[(&str, usize)]) -> (tempfile::TempDir, tempfile::TempDir, String) {
    let home = tempfile::tempdir().unwrap();
    let repo = git_repo();
    // These tests need bindings and control authority, not an active pin.
    // Avoid quickstart's unrelated broker/doctor round trip in every fixture.
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

#[test]
fn apply_and_dry_run_refuse_suggestion_outside_workspace_allowlist() {
    let (home, repo, cap) = setup(&[("acme", 5)]);
    let file = repo.path().join(".locus.toml");
    let original = "version = 1\nallowed_bindings = [\"personal\"]\nrequire_pin = true\n";
    std::fs::write(&file, original).unwrap();
    for args in [
        &["autopin", "suggest", "--apply", "--dry-run", "--json"][..],
        &["autopin", "suggest", "--apply", "--json"][..],
    ] {
        let o = locus(home.path(), repo.path(), Some(&cap), &[], args);
        assert!(!o.status.success(), "disallowed suggestion accepted: {o:?}");
        assert!(text(&o).contains("allowed_bindings"), "{o:?}");
        assert_no_raw_url(&text(&o));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), original);
    }
}

#[test]
fn apply_accepts_binding_alias_or_id_in_workspace_allowlist() {
    let (home, repo, cap) = setup(&[("acme", 5)]);
    let binding: toml::Table =
        toml::from_str(&std::fs::read_to_string(home.path().join("bindings/acme.toml")).unwrap())
            .unwrap();
    let id = binding["binding"]["id"].as_str().unwrap();
    let file = repo.path().join(".locus.toml");
    for allowed in ["acme", id] {
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
        assert!(o.status.success(), "allowed binding refused: {o:?}");
        let written: toml::Table =
            toml::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(written["default_binding"].as_str(), Some("acme"));
        assert_eq!(written["allowed_bindings"][0].as_str(), Some(allowed));
        assert_eq!(written["require_pin"].as_bool(), Some(true));
    }
}

#[cfg(unix)]
#[test]
fn apply_preserves_existing_workspace_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let (home, repo, cap) = setup(&[("acme", 5)]);
    let file = repo.path().join(".locus.toml");
    for mode in [0o600, 0o640] {
        std::fs::write(&file, "version = 1\nrequire_pin = true\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
        let o = locus(
            home.path(),
            repo.path(),
            Some(&cap),
            &[],
            &["autopin", "suggest", "--apply"],
        );
        assert!(o.status.success(), "{o:?}");
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            mode
        );
    }
}

#[test]
fn apply_and_dry_run_choose_git_root_from_a_subdirectory() {
    let (home, repo, cap) = setup(&[("acme", 5)]);
    let subdir = repo.path().join("src/nested");
    std::fs::create_dir_all(&subdir).unwrap();
    let file = repo.path().canonicalize().unwrap().join(".locus.toml");
    let preview = locus(
        home.path(),
        &subdir,
        None,
        &[],
        &["autopin", "suggest", "--apply", "--dry-run", "--json"],
    );
    assert!(preview.status.success(), "{preview:?}");
    assert_eq!(
        json(&preview)["changes"][0]["target"],
        file.display().to_string()
    );
    assert!(!file.exists(), "dry-run wrote its target");
    let applied = locus(
        home.path(),
        &subdir,
        Some(&cap),
        &[],
        &["autopin", "suggest", "--apply", "--json"],
    );
    assert!(applied.status.success(), "{applied:?}");
    assert_eq!(json(&applied)["written"], file.display().to_string());
    assert!(file.is_file());
    assert!(!subdir.join(".locus.toml").exists());
}

#[cfg(unix)]
#[test]
fn apply_keeps_trailing_whitespace_in_git_root() {
    let (home, _repo, cap) = setup(&[("acme", 5)]);
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
    assert!(o.status.success(), "{o:?}");
    let file = repo.canonicalize().unwrap().join(".locus.toml");
    assert_eq!(json(&o)["written"], file.display().to_string());
    assert!(file.is_file());
    assert!(!subdir.join(".locus.toml").exists());
}

#[test]
fn apply_and_dry_run_refuse_git_root_outside_current_directory() {
    let (home, repo, cap) = setup(&[("acme", 5)]);
    let outside = tempfile::tempdir().unwrap();
    let git_dir = repo.path().join(".git");
    let git_env = [
        ("GIT_DIR", git_dir.to_str().unwrap()),
        ("GIT_WORK_TREE", outside.path().to_str().unwrap()),
    ];
    // Git succeeds here, but its claimed work tree does not contain cwd.
    let git = Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["rev-parse", "--show-toplevel"])
        .envs(git_env)
        .output()
        .unwrap();
    assert!(git.status.success());
    for args in [
        &["autopin", "suggest", "--apply", "--dry-run", "--json"][..],
        &["autopin", "suggest", "--apply", "--json"][..],
    ] {
        let o = locus(home.path(), repo.path(), Some(&cap), &git_env, args);
        assert!(!o.status.success(), "outside git root accepted: {o:?}");
        assert!(!repo.path().join(".locus.toml").exists());
        assert!(!outside.path().join(".locus.toml").exists());
    }
}

#[test]
fn dry_run_refuses_a_binding_deleted_since_history_was_recorded() {
    let (home, repo, _) = setup(&[("acme", 5)]);
    std::fs::remove_file(home.path().join("bindings/acme.toml")).unwrap();
    let o = locus(
        home.path(),
        repo.path(),
        None,
        &[],
        &["autopin", "suggest", "--apply", "--dry-run", "--json"],
    );
    assert!(!o.status.success(), "deleted binding accepted: {o:?}");
    assert!(!repo.path().join(".locus.toml").exists());
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
        &["autopin", "suggest", "--apply", "--dry-run"][..],
        &["autopin", "suggest", "--apply"][..],
    ] {
        let o = locus(home.path(), repo.path(), Some(&cap), &[], args);
        assert!(!o.status.success(), "{o:?}");
        assert!(text(&o).contains("symlink"), "{o:?}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "version = 1\n");
    }
}

#[test]
fn suggest_with_history_key_does_not_create_a_missing_lock() {
    let (home, repo, _) = setup(&[("acme", 5)]);
    let lock = home.path().join(".pin-history.lock");
    std::fs::remove_file(&lock).unwrap();
    let history_before = std::fs::read(home.path().join(pin_history::FILE_NAME)).unwrap();
    let o = locus(
        home.path(),
        repo.path(),
        None,
        &[],
        &["autopin", "suggest", "--json"],
    );
    assert!(o.status.success(), "{o:?}");
    assert_eq!(json(&o)["report"]["suggestion"], "acme");
    assert!(!lock.exists(), "read-only suggest created history lock");
    assert_eq!(
        std::fs::read(home.path().join(pin_history::FILE_NAME)).unwrap(),
        history_before
    );
}

#[test]
fn suggest_with_absent_locus_home_writes_nothing() {
    let parent = tempfile::tempdir().unwrap();
    let home = parent.path().join("uninitialized");
    let repo = git_repo();
    let o = locus(
        &home,
        repo.path(),
        None,
        &[],
        &["autopin", "suggest", "--json"],
    );
    assert!(o.status.success(), "{o:?}");
    assert_eq!(json(&o)["report"]["reason"], "no_history");
    assert!(!home.exists(), "suggest initialized LOCUS_HOME");
    assert!(!repo.path().join(".locus.toml").exists());
}

#[test]
fn suggest_without_history_key_writes_nothing() {
    let home = tempfile::tempdir().unwrap();
    let repo = git_repo();
    let init = locus(
        home.path(),
        repo.path(),
        None,
        &[("LOCUS_PIN_HISTORY", "0")],
        &["init", "--with-samples"],
    );
    assert!(init.status.success(), "{init:?}");
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
        &["autopin", "suggest", "--apply", "--dry-run"][..],
        &["autopin", "suggest", "--apply"][..],
    ] {
        let o = locus(home.path(), &repo, Some(&cap), &[], args);
        assert!(!o.status.success(), "{o:?}");
        assert!(text(&o).contains("outside this repo"), "{o:?}");
        assert_eq!(std::fs::read_to_string(&shared).unwrap(), "version = 1\n");
        assert!(!repo.join(".locus.toml").exists());
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

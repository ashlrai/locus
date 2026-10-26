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
    let o = locus(
        home.path(),
        repo.path(),
        Some(&cap),
        &[],
        &["autopin", "suggest", "--apply"],
    );
    assert!(!o.status.success(), "{o:?}");
    assert!(text(&o).contains("symlink"), "{o:?}");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "version = 1\n");
}

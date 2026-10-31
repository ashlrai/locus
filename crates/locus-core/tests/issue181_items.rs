//! Issue #181: exercise the production writer without exposing a public or
//! unauthenticated Store mutation API. CLI coverage checks the control gate.
use locus_core::{binding, error, workspace, Binding, BindingBody, Policy, Store};
use std::{fs, path::Path, process::Command};

#[path = "../src/autopin_write.rs"]
mod autopin_write;

fn binding() -> Binding {
    Binding::from_body(BindingBody {
        id: "bnd_acme".into(),
        alias: "acme".into(),
        tenant: "acme".into(),
        principal: None,
        description: None,
        policy: Policy::default(),
        providers: vec![locus_core::ProviderBinding {
            provider: "github".into(),
            account: "acme".into(),
            credential_ref: "env:ISSUE181_UNUSED".into(),
            scope: locus_core::Scope::default(),
            upstream: None,
        }],
    })
}

fn seed(home: &Path, repo: &Path) -> Store {
    use locus_core::{autopin::normalize_remote_url, pin_history};
    let url = "https://github.com/example/issue181.git";
    for args in [vec!["init", "-q"], vec!["remote", "add", "origin", url]] {
        assert!(Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    }
    let store = Store::open(home).unwrap();
    store.save_binding(&binding()).unwrap();
    let key = pin_history::history_key(home).unwrap();
    let hash = pin_history::keyed_hash(&key, &normalize_remote_url(url));
    for _ in 0..3 {
        pin_history::append(
            home,
            &pin_history::HistoryEntry {
                ts: "2026-10-25T00:00:00Z".into(),
                event: "pin".into(),
                alias: Some("acme".into()),
                expected: None,
                workspace_hash: None,
                remote_hashes: vec![hash.clone()],
            },
            2000,
        )
        .unwrap();
    }
    store
}

#[test]
fn item1_refuses_suggestions_outside_nonempty_allowlist() {
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let store = seed(home.path(), repo.path());
    let file = repo.path().join(".locus.toml");
    let policy = "allowed_bindings = [\"personal\"]\nrequire_pin = true\n";
    fs::write(&file, policy).unwrap();
    assert!(store.autopin_suggestion_plan(repo.path()).is_err());
    // The writer independently rechecks the allowlist at the write boundary.
    assert!(autopin_write::write_default_binding(&file, &binding()).is_err());
    assert_eq!(fs::read_to_string(&file).unwrap(), policy);
    for allowed in ["acme", "bnd_acme", ""] {
        let list = if allowed.is_empty() {
            "[]".into()
        } else {
            format!("[\"{allowed}\"]")
        };
        fs::write(&file, format!("allowed_bindings = {list}\n")).unwrap();
        assert!(store.autopin_suggestion_plan(repo.path()).is_ok());
        autopin_write::write_default_binding(&file, &binding()).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn item2_preserves_mode_across_atomic_rename() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let repo = tempfile::tempdir().unwrap();
    let file = repo.path().join(".locus.toml");
    for mode in [0o400, 0o600, 0o640] {
        fs::write(&file, "require_pin = true\n").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(mode)).unwrap();
        let before = fs::metadata(&file).unwrap().ino();
        autopin_write::write_default_binding(&file, &binding()).unwrap();
        let after = fs::metadata(&file).unwrap();
        assert_ne!(before, after.ino(), "must replace the file atomically");
        assert_eq!(after.permissions().mode() & 0o777, mode);
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn item3_syncs_parent_after_rename_and_propagates_failure() {
    let repo = tempfile::tempdir().unwrap();
    let file = repo.path().join(".locus.toml");
    let mut synced = false;
    autopin_write::write_default_binding_with_sync(&file, &binding(), |parent| {
        assert_eq!(parent, repo.path());
        let cfg = workspace::WorkspaceConfig::parse(&fs::read_to_string(&file)?).unwrap();
        assert_eq!(cfg.default_binding.as_deref(), Some("acme"));
        assert_eq!(fs::read_dir(parent)?.count(), 1, "rename precedes sync");
        fs::File::open(parent)?.sync_all()?;
        synced = true;
        Ok(())
    })
    .unwrap();
    assert!(synced);
    let err = autopin_write::write_default_binding_with_sync(&file, &binding(), |_| {
        Err(std::io::Error::other("parent sync failed"))
    })
    .unwrap_err();
    assert!(err.to_string().contains("parent sync failed"));
    assert_eq!(fs::read_dir(repo.path()).unwrap().count(), 1);
}

#[test]
fn item4_history_read_never_creates_lock() {
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    seed(home.path(), repo.path());
    fs::remove_file(home.path().join(".pin-history.lock")).unwrap();
    let before = fs::read(home.path().join(locus_core::pin_history::FILE_NAME)).unwrap();
    let report = locus_core::autopin_suggest::suggest(home.path(), repo.path()).unwrap();
    assert_eq!(report.suggestion.as_deref(), Some("acme"));
    assert!(!home.path().join(".pin-history.lock").exists());
    assert_eq!(
        fs::read(home.path().join(locus_core::pin_history::FILE_NAME)).unwrap(),
        before
    );
}

#[cfg(unix)]
#[test]
fn item5_strips_only_git_toplevel_line_ending() {
    let home = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    // Include leading whitespace and trailing CR/LF, all valid path bytes.
    let repo = parent.path().join(" repo \t\r\n");
    fs::create_dir(&repo).unwrap();
    let store = seed(home.path(), &repo);
    let subdir = repo.join("src");
    fs::create_dir(&subdir).unwrap();
    let (_, file) = store.autopin_suggestion_plan(&subdir).unwrap();
    assert_eq!(file, repo.canonicalize().unwrap().join(".locus.toml"));
    assert!(!file.exists(), "planning must remain read-only");
}

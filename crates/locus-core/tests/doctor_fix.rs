//! Integration: `doctor --fix` planning (dry run) and the explicit apply step.

use locus_core::doctor_fix::{apply_fixes, plan_fixes, FixPlan, PlannedFix, LAST_ENTRY_MESSAGE};
use locus_core::{Binding, BindingBody, Policy, ProviderBinding, Scope, Store};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::{tempdir, TempDir};

const LOCATOR: &str = "DOCTOR_FIX_LOCATOR_CANARY";

fn binding(alias: &str, credential_ref: &str) -> Binding {
    Binding::from_body(BindingBody {
        id: format!("bnd_{alias}"),
        alias: alias.into(),
        tenant: alias.into(),
        principal: None,
        description: None,
        policy: Policy::default(),
        providers: vec![ProviderBinding {
            provider: "github".into(),
            account: format!("{alias}-gh"),
            credential_ref: credential_ref.into(),
            scope: Scope::default(),
            upstream: None,
        }],
    })
}

struct Fixture {
    _home: TempDir,
    _proj: TempDir,
    store: Store,
    project: PathBuf,
}

fn fixture(workspace: &str) -> Fixture {
    let home = tempdir().unwrap();
    let proj = tempdir().unwrap();
    let store = Store::open(home.path()).unwrap();
    store
        .save_binding(&binding("acme", &format!("phm:{LOCATOR}")))
        .unwrap();
    store
        .save_binding(&binding("personal", "env:LOCUS_DOCTOR_FIX_TOKEN"))
        .unwrap();
    fs::write(proj.path().join(".locus.toml"), workspace).unwrap();
    let project = proj.path().to_path_buf();
    Fixture {
        _home: home,
        _proj: proj,
        store,
        project,
    }
}

/// Write a binding file directly, bypassing save-time validation (legacy or malformed refs).
fn write_raw_binding(store: &Store, alias: &str, credential_ref: &str) {
    let text = binding(alias, "phm:PLACEHOLDER")
        .to_toml()
        .unwrap()
        .replace("phm:PLACEHOLDER", credential_ref);
    fs::write(store.bindings_dir().join(format!("{alias}.toml")), text).unwrap();
}

fn snapshot(dirs: &[&Path]) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    for d in dirs {
        for e in walk(d) {
            out.insert(e.clone(), fs::read(&e).unwrap());
        }
    }
    out
}

fn walk(d: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(d) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

fn codes(plan: &FixPlan) -> Vec<String> {
    plan.report_only.iter().map(|r| r.code.clone()).collect()
}

#[test]
fn dry_run_plans_and_writes_nothing() {
    let f = fixture("allowed_bindings = [\"acme\", \"gone\"]\ndefault_binding = \"old\"\n");
    write_raw_binding(&f.store, "legacy", "LEGACY_NAME");
    write_raw_binding(&f.store, "broken", "phm:");
    let before = snapshot(&[f.store.home(), &f.project]);
    let plan = plan_fixes(&f.store, &f.project, None).unwrap();
    assert_eq!(snapshot(&[f.store.home(), &f.project]), before);

    let ws = f.project.join(".locus.toml");
    assert!(plan.fixes.contains(&PlannedFix::StaleAllowlistEntry {
        file: ws.clone(),
        entry: "gone".into()
    }));
    assert!(plan.fixes.contains(&PlannedFix::StaleDefaultBinding {
        file: ws,
        entry: "old".into()
    }));
    assert!(plan.fixes.contains(&PlannedFix::LegacyPhantomRef {
        alias: "legacy".into(),
        providers: vec!["github".into()]
    }));
    let c = codes(&plan);
    assert!(c.contains(&"malformed_ref".to_string()));
    // Phantom names unknown: every valid phm ref is reported unverified (fail closed).
    assert!(c.contains(&"phm_unavailable".to_string()));
    assert_eq!(plan.fixes.len(), 3);
}

#[test]
fn apply_removes_stale_entries_with_a_backup_and_keeps_unknown_keys() {
    let original = "allowed_bindings = [\"acme\", \"gone\", \"personal\"]\ndefault_binding = \"old\"\nrequire_pin = true\nfuture_key = \"kept\"\n";
    let f = fixture(original);
    let ws = f.project.join(".locus.toml");
    let plan = plan_fixes(&f.store, &f.project, None).unwrap();
    let out = apply_fixes(&f.store, &plan).unwrap();
    assert!(out.all_applied(), "{out:?}");

    let now: toml::Table = fs::read_to_string(&ws).unwrap().parse().unwrap();
    let allowed: Vec<&str> = now["allowed_bindings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(allowed, vec!["acme", "personal"]);
    assert!(now.get("default_binding").is_none());
    assert_eq!(now["require_pin"].as_bool(), Some(true));
    assert_eq!(now["future_key"].as_str(), Some("kept"));

    let backups: BTreeSet<PathBuf> = out
        .results
        .iter()
        .filter_map(|r| r.backup.clone())
        .collect();
    assert_eq!(backups.len(), 1, "one backup per file");
    let backup = backups.into_iter().next().unwrap();
    assert_eq!(fs::read_to_string(&backup).unwrap(), original);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    // Nothing left to fix.
    assert!(plan_fixes(&f.store, &f.project, None)
        .unwrap()
        .fixes
        .is_empty());
}

#[test]
fn the_last_allowlist_entry_is_never_removed() {
    let f = fixture("allowed_bindings = [\"gone1\", \"gone2\"]\n");
    let ws = f.project.join(".locus.toml");
    let plan = plan_fixes(&f.store, &f.project, None).unwrap();
    assert_eq!(
        plan.fixes,
        vec![PlannedFix::StaleAllowlistEntry {
            file: ws.clone(),
            entry: "gone1".into()
        }]
    );
    let last = plan
        .report_only
        .iter()
        .find(|r| r.code == "allowlist_last_entry")
        .expect("last entry reported");
    assert!(last.message.contains(LAST_ENTRY_MESSAGE));
    assert!(last.message.contains("gone2"));
    apply_fixes(&f.store, &plan).unwrap();
    let now: toml::Table = fs::read_to_string(&ws).unwrap().parse().unwrap();
    assert_eq!(now["allowed_bindings"].as_array().unwrap().len(), 1);

    // A hand-made plan that would empty the list is refused at apply time,
    // with no backup and no change.
    let before = fs::read(&ws).unwrap();
    let forced = FixPlan {
        fixes: vec![PlannedFix::StaleAllowlistEntry {
            file: ws.clone(),
            entry: "gone2".into(),
        }],
        report_only: vec![],
    };
    let out = apply_fixes(&f.store, &forced).unwrap();
    assert!(!out.results[0].applied);
    assert_eq!(out.results[0].message.as_deref(), Some(LAST_ENTRY_MESSAGE));
    assert!(out.results[0].backup.is_none());
    assert_eq!(fs::read(&ws).unwrap(), before);
}

#[test]
fn an_entry_whose_binding_exists_again_is_kept() {
    let f = fixture("allowed_bindings = [\"acme\", \"later\"]\n");
    let plan = plan_fixes(&f.store, &f.project, None).unwrap();
    assert_eq!(plan.fixes.len(), 1);
    f.store
        .save_binding(&binding("later", "env:LOCUS_DOCTOR_FIX_TOKEN"))
        .unwrap();
    let ws = f.project.join(".locus.toml");
    let before = fs::read(&ws).unwrap();
    let out = apply_fixes(&f.store, &plan).unwrap();
    assert!(!out.results[0].applied);
    assert_eq!(fs::read(&ws).unwrap(), before);
}

#[test]
fn legacy_refs_migrate_with_a_backup_and_malformed_refs_stay_untouched() {
    let f = fixture("");
    write_raw_binding(&f.store, "legacy", "LEGACY_NAME");
    write_raw_binding(&f.store, "broken", "env:NOT-A-VAR");
    let legacy_file = f.store.bindings_dir().join("legacy.toml");
    let broken_file = f.store.bindings_dir().join("broken.toml");
    let legacy_before = fs::read(&legacy_file).unwrap();
    let broken_before = fs::read(&broken_file).unwrap();

    let plan = plan_fixes(&f.store, &f.project, None).unwrap();
    let out = apply_fixes(&f.store, &plan).unwrap();
    let r = out
        .results
        .iter()
        .find(|r| matches!(r.fix, PlannedFix::LegacyPhantomRef { .. }))
        .unwrap();
    assert!(r.applied, "{r:?}");
    let backup = r.backup.clone().unwrap();
    assert!(backup.starts_with(f.store.home().join("backups")));
    assert_eq!(fs::read(&backup).unwrap(), legacy_before);
    let migrated = Binding::parse_toml(&fs::read_to_string(&legacy_file).unwrap()).unwrap();
    assert_eq!(migrated.providers[0].credential_ref, "phm:LEGACY_NAME");

    assert_eq!(
        fs::read(&broken_file).unwrap(),
        broken_before,
        "malformed ref is report only"
    );
    let again = plan_fixes(&f.store, &f.project, None).unwrap();
    assert!(again.fixes.is_empty());
    assert!(codes(&again).contains(&"malformed_ref".to_string()));
}

#[test]
fn phm_refs_are_checked_against_names_only() {
    let f = fixture("");
    let none = plan_fixes(&f.store, &f.project, None).unwrap();
    assert_eq!(codes(&none), vec!["phm_unavailable"]);
    let listed: BTreeSet<String> = [LOCATOR.to_string()].into();
    assert!(plan_fixes(&f.store, &f.project, Some(&listed))
        .unwrap()
        .is_empty());
    let other: BTreeSet<String> = ["SOMETHING_ELSE".to_string()].into();
    let dangling = plan_fixes(&f.store, &f.project, Some(&other)).unwrap();
    assert_eq!(codes(&dangling), vec!["dangling_phm_ref"]);
    assert!(dangling.fixes.is_empty(), "dangling refs are report only");
}

#[test]
fn plans_and_outcomes_never_carry_refs_or_locator_names() {
    let f = fixture("allowed_bindings = [\"acme\", \"gone\"]\n");
    write_raw_binding(&f.store, "legacy", "LEGACY_LOCATOR_CANARY");
    write_raw_binding(&f.store, "broken", "phm:bad name CANARY_RAW");
    let other: BTreeSet<String> = ["X".to_string()].into();
    let plan = plan_fixes(&f.store, &f.project, Some(&other)).unwrap();
    let mut text = serde_json::to_string(&plan).unwrap();
    text += &plan
        .fixes
        .iter()
        .map(|f| f.describe())
        .collect::<Vec<_>>()
        .join("\n");
    let out = apply_fixes(&f.store, &plan).unwrap();
    text += &serde_json::to_string(&out).unwrap();
    for needle in [
        LOCATOR,
        "LEGACY_LOCATOR_CANARY",
        "CANARY_RAW",
        "LOCUS_DOCTOR_FIX_TOKEN",
        "phm:bad",
        "phm:LEGACY",
        "phm:DOCTOR",
    ] {
        assert!(!text.contains(needle), "{needle} leaked: {text}");
    }
}

#[cfg(unix)]
#[test]
fn phantom_is_never_started() {
    use std::os::unix::fs::PermissionsExt;
    let bin = tempdir().unwrap();
    let marker = bin.path().join("invoked");
    let fake = bin.path().join("phantom");
    fs::write(
        &fake,
        format!("#!/bin/sh\necho \"$@\" >> {}\nexit 1\n", marker.display()),
    )
    .unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    let old = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![bin.path().to_path_buf()];
    paths.extend(std::env::split_paths(&old));
    std::env::set_var("PATH", std::env::join_paths(paths).unwrap());

    let f = fixture("allowed_bindings = [\"acme\", \"gone\"]\n");
    write_raw_binding(&f.store, "legacy", "LEGACY_NAME");
    let plan = plan_fixes(&f.store, &f.project, None).unwrap();
    apply_fixes(&f.store, &plan).unwrap();
    std::env::set_var("PATH", old);
    assert!(
        !marker.exists(),
        "phantom was invoked: {:?}",
        fs::read_to_string(&marker)
    );
}

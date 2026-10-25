//! E2E: `locus doctor --fix` previews repairs and writes nothing;
//! `--fix --apply` makes them with a backup. Output never carries a
//! credential reference string or locator name.

use locus_core::{Binding, BindingBody, Policy, ProviderBinding, Scope, Store};
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::tempdir;

const LOCATOR: &str = "DOCTOR_FIX_CLI_LOCATOR_CANARY";

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

fn locus_with_cap(home: &Path, cwd: &Path, cap: Option<&str>, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_locus"));
    cmd.args(args)
        .env("LOCUS_HOME", home)
        .env_remove("LOCUS_CONTROL_CAPABILITY")
        .env("NO_COLOR", "1")
        .current_dir(cwd);
    if let Some(c) = cap {
        cmd.env("LOCUS_CONTROL_CAPABILITY", c);
    }
    cmd.output().expect("locus binary runs")
}

struct Env {
    home: tempfile::TempDir,
    proj: tempfile::TempDir,
    cap: String,
}

impl Env {
    fn locus(&self, args: &[&str]) -> Output {
        locus_with_cap(self.home.path(), self.proj.path(), Some(&self.cap), args)
    }
}

/// Quickstart (this test process supervises the broker, so control commands
/// work), one extra binding whose phm locator must never be printed, and a
/// workspace naming a binding that does not exist.
fn setup(workspace: &str) -> Env {
    let home = tempdir().unwrap();
    let proj = tempdir().unwrap();
    let qs = locus_with_cap(home.path(), proj.path(), None, &["quickstart"]);
    assert!(qs.status.success(), "quickstart failed: {qs:?}");
    let cap = fs::read_to_string(home.path().join("control_capability"))
        .expect("quickstart persists the control capability")
        .trim()
        .to_string();
    // Written directly: an in-process store write would need its own broker.
    let store = Store::open(home.path()).unwrap();
    fs::write(
        store.bindings_dir().join("vault.toml"),
        binding("vault", &format!("phm:{LOCATOR}"))
            .to_toml()
            .unwrap(),
    )
    .unwrap();
    fs::write(proj.path().join(".locus.toml"), workspace).unwrap();
    Env { home, proj, cap }
}

const WS: &str = "allowed_bindings = [\"acme\", \"gone\"]\ndefault_binding = \"old\"\n";

#[test]
fn fix_previews_and_writes_nothing() {
    let e = setup(WS);
    let ws = e.proj.path().join(".locus.toml");
    let out = e.locus(&["doctor", "--fix"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("dry run, nothing written"), "{stdout}");
    assert!(
        stdout.contains("remove allowed_bindings entry 'gone'"),
        "{stdout}"
    );
    assert!(stdout.contains("clear default_binding 'old'"), "{stdout}");
    assert!(stdout.contains("locus doctor --fix --apply"), "{stdout}");
    assert!(!stdout.contains(LOCATOR), "{stdout}");
    assert_eq!(fs::read_to_string(&ws).unwrap(), WS);
    let backups: Vec<_> = fs::read_dir(e.proj.path())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains(".bak-"))
        .collect();
    assert!(backups.is_empty());
}

#[test]
fn fix_apply_repairs_with_backup_and_json_is_value_free() {
    let e = setup(WS);
    let ws = e.proj.path().join(".locus.toml");
    let out = e.locus(&["--json", "doctor", "--fix", "--apply"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!stdout.contains(LOCATOR), "{stdout}");
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["mode"], "apply");
    let results = v["outcome"]["results"].as_array().unwrap();
    assert_eq!(results.len(), 2, "{stdout}");
    assert!(results.iter().all(|r| r["applied"] == true), "{stdout}");

    let now: toml::Table = fs::read_to_string(&ws).unwrap().parse().unwrap();
    let allowed: Vec<&str> = now["allowed_bindings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(allowed, vec!["acme"]);
    assert!(!now.contains_key("default_binding"));
    let backup = fs::read_dir(e.proj.path())
        .unwrap()
        .flatten()
        .find(|e| e.file_name().to_string_lossy().contains(".bak-"))
        .expect("backup written");
    assert_eq!(fs::read_to_string(backup.path()).unwrap(), WS);

    // Second run: nothing left to repair.
    let again = e.locus(&["doctor", "--fix"]);
    assert!(again.status.success());
    assert!(String::from_utf8_lossy(&again.stdout).contains("nothing to repair"));
}

#[test]
fn apply_requires_fix() {
    let e = setup(WS);
    let out = e.locus(&["doctor", "--apply"]);
    assert!(!out.status.success());
    assert_eq!(
        fs::read_to_string(e.proj.path().join(".locus.toml")).unwrap(),
        WS
    );
}

#[test]
fn apply_without_control_capability_fails_closed() {
    let e = setup(WS);
    let out = locus_with_cap(
        e.home.path(),
        e.proj.path(),
        None,
        &["doctor", "--fix", "--apply"],
    );
    assert!(!out.status.success());
    assert_eq!(
        fs::read_to_string(e.proj.path().join(".locus.toml")).unwrap(),
        WS
    );
}

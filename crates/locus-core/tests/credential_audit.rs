//! Integration: the opt-in credential injection audit at the env builder.
//!
//! One test function, because it sets LOCUS_HOME for this test binary.

use chrono::Duration;
use locus_core::config::{CredentialAuditConfig, LocusConfig};
use locus_core::credential_audit::{log_path, verify, InjectionRecord};
use locus_core::seal::SealKey;
use locus_core::session::{PinSource, Session};
use locus_core::{
    build_isolated_env_opts, Binding, BindingBody, Policy, ProviderBinding, Scope, Store,
};
use std::fs;
use tempfile::tempdir;

const VALUE: &str = "audit-test-credential-value-123";

fn binding() -> Binding {
    Binding::from_body(BindingBody {
        id: "bnd_example".into(),
        alias: "example".into(),
        tenant: "example".into(),
        principal: None,
        description: None,
        policy: Policy::default(),
        providers: vec![ProviderBinding {
            provider: "supabase".into(),
            account: "example".into(),
            credential_ref: "env:LOCUS_CRED_AUDIT_TEST_TOKEN".into(),
            scope: Scope {
                project_ref: Some("proj_example".into()),
                ..Scope::default()
            },
            upstream: None,
        }],
    })
}

fn session(b: &Binding) -> Session {
    Session::new(
        &b.id,
        &b.alias,
        &b.tenant,
        None,
        PinSource::Explicit,
        None,
        Duration::hours(1),
        "/tmp/locus-cred-audit-test".into(),
        &SealKey::generate(),
    )
}

#[test]
fn credential_injection_audit_end_to_end() {
    let home = tempdir().unwrap();
    std::env::set_var("LOCUS_HOME", home.path());
    std::env::set_var("LOCUS_CRED_AUDIT_TEST_TOKEN", VALUE);
    let store = Store::open(home.path()).unwrap();
    let b = binding();
    let s = session(&b);

    // 1. Off by default: value injected as before, nothing logged, no key made.
    let iso = build_isolated_env_opts(&s, &b, true);
    assert_eq!(
        iso.vars.get("SUPABASE_ACCESS_TOKEN").map(String::as_str),
        Some(VALUE)
    );
    assert!(iso.secrets_failed.is_empty());
    assert!(!log_path(home.path()).exists());
    assert!(!home.path().join("credential-audit.key").exists());

    // 2. Enabled: same injection, plus one signed value-free record.
    store
        .save_config(&LocusConfig {
            credential_audit: CredentialAuditConfig { enabled: true },
            ..Default::default()
        })
        .unwrap();
    let iso = build_isolated_env_opts(&s, &b, true);
    assert_eq!(
        iso.vars.get("SUPABASE_ACCESS_TOKEN").map(String::as_str),
        Some(VALUE)
    );
    let raw = fs::read_to_string(log_path(home.path())).unwrap();
    assert!(!raw.contains(VALUE), "the log must never hold the value");
    let rec: InjectionRecord = serde_json::from_str(raw.trim()).unwrap();
    assert_eq!(rec.provider, "supabase");
    assert_eq!(rec.target, "child-env");
    assert_eq!(rec.credential_ref, "env:LOCUS_CRED_AUDIT_TEST_TOKEN");
    assert_eq!(rec.env_keys, vec!["SUPABASE_ACCESS_TOKEN"]);
    assert_eq!(rec.session_id, s.session_id);
    assert!(verify(home.path()).unwrap().ok());

    // 3. Enabled but the log cannot be written: the credential is withheld.
    fs::remove_file(log_path(home.path())).unwrap();
    fs::create_dir_all(log_path(home.path())).unwrap(); // a directory, not a file
    let iso = build_isolated_env_opts(&s, &b, true);
    assert!(!iso.vars.contains_key("SUPABASE_ACCESS_TOKEN"));
    assert_eq!(
        iso.vars
            .get("LOCUS_SUPABASE_CREDENTIAL_RESOLVED")
            .map(String::as_str),
        Some("0")
    );
    assert!(iso
        .secrets_failed
        .iter()
        .any(|i| i.code == "audit_write_failed" && i.source == "credential_audit"));
    assert_eq!(iso.secrets_resolved, 0);
}

//! Integration: exclusive pin isolation — acme vs personal credential refs.
//!
//! A session pinned to one binding must never surface the sibling binding's
//! credential refs in agent-facing output, project refs, or ambient provider env.

use locus_core::{
    build_isolated_env, build_isolated_env_opts, Binding, BindingBody, Policy, ProviderBinding,
    Scope, Store,
};
use tempfile::tempdir;

fn binding(
    alias: &str,
    tenant: &str,
    gh_ref: &str,
    vercel_ref: &str,
    sb_ref: &str,
    project: &str,
    team: &str,
) -> Binding {
    Binding::from_body(BindingBody {
        id: format!("bnd_{alias}"),
        alias: alias.into(),
        tenant: tenant.into(),
        principal: None,
        description: None,
        policy: Policy {
            require_approval: vec!["*.delete*".into(), "vercel.deploy.prod".into()],
            max_ttl: Some("1h".into()),
            ..Policy::default()
        },
        providers: vec![
            ProviderBinding {
                provider: "github".into(),
                account: format!("{alias}-gh"),
                credential_ref: gh_ref.into(),
                scope: Scope {
                    orgs: vec![tenant.into()],
                    ..Scope::default()
                },
                upstream: None,
            },
            ProviderBinding {
                provider: "vercel".into(),
                account: format!("{alias}-vc"),
                credential_ref: vercel_ref.into(),
                scope: Scope {
                    team_id: Some(team.into()),
                    projects: vec![format!("{alias}-web")],
                    ..Scope::default()
                },
                upstream: None,
            },
            ProviderBinding {
                provider: "supabase".into(),
                account: format!("{alias}-db"),
                credential_ref: sb_ref.into(),
                scope: Scope {
                    project_ref: Some(project.into()),
                    read_only: Some(true),
                    ..Scope::default()
                },
                upstream: None,
            },
        ],
    })
}

#[test]
fn pin_acme_vs_personal_exclusive_credential_refs() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();

    let acme = binding(
        "acme",
        "acme-corp",
        "phm:GH_TOKEN_ACME",
        "phm:VERCEL_TOKEN_ACME",
        "phm:SUPABASE_ACME",
        "proj_acme",
        "team_acme",
    );
    let personal = binding(
        "personal",
        "personal",
        "phm:GH_TOKEN_PERSONAL",
        "phm:VERCEL_TOKEN_PERSONAL",
        "phm:SUPABASE_PERSONAL",
        "proj_me",
        "team_personal",
    );
    store.save_binding(&acme).unwrap();
    store.save_binding(&personal).unwrap();

    // ── pin acme ──────────────────────────────────────────────────────────
    let s_acme = store
        .pin("acme", dir.path(), Some("test".into()), false)
        .unwrap();
    let w = store.whoami().unwrap();
    assert_eq!(w.binding_alias, "acme");
    assert_eq!(w.tenant, "acme-corp");

    assert!(w.providers.iter().all(|p| p.credential.present));
    assert!(w.providers.iter().all(|p| p.credential.source == "phantom"));
    let whoami_json = serde_json::to_string(&w).unwrap();
    assert!(!whoami_json.contains("GH_TOKEN_ACME"));
    assert!(!whoami_json.contains("PERSONAL"));
    assert!(!whoami_json.contains("credential_ref"));
    assert!(w
        .providers
        .iter()
        .all(|p| p.project_ref.as_deref() != Some("proj_me")));

    let iso = build_isolated_env(&s_acme, &acme);
    assert!(!iso.vars.values().any(|v| v.contains("PERSONAL")));
    assert!(!iso.vars.values().any(|v| v.contains("proj_me")));
    assert!(!iso.vars.keys().any(|key| key.contains("CREDENTIAL_REF")));
    assert!(!iso
        .vars
        .values()
        .any(|value| value.contains("GH_TOKEN_ACME")));

    // ── switch to personal — exclusive ────────────────────────────────────
    let s_personal = store
        .pin("personal", dir.path(), Some("test".into()), false)
        .unwrap();
    let w2 = store.whoami().unwrap();
    assert_eq!(w2.binding_alias, "personal");
    for p in &w2.providers {
        assert!(p.credential.present);
        assert_eq!(p.credential.source, "phantom");
    }
    let whoami_json = serde_json::to_string(&w2).unwrap();
    assert!(!whoami_json.contains("GH_TOKEN_PERSONAL"));
    assert!(!whoami_json.contains("ACME"));
    assert!(!whoami_json.contains("credential_ref"));
    assert!(w2
        .providers
        .iter()
        .all(|p| p.project_ref.as_deref() != Some("proj_acme")));

    let iso2 = build_isolated_env(&s_personal, &personal);
    assert!(!iso2.vars.values().any(|v| v.contains("ACME")));
    assert!(!iso2.vars.values().any(|v| v.contains("proj_acme")));
    assert!(!iso2.vars.values().any(|v| v.contains("team_acme")));
    assert!(!iso2.vars.keys().any(|key| key.contains("CREDENTIAL_REF")));
    assert!(!iso2
        .vars
        .values()
        .any(|value| value.contains("GH_TOKEN_PERSONAL")));
}

#[test]
fn production_library_rejects_test_credentials_even_with_env_opt_in() {
    std::env::set_var("LOCUS_ALLOW_TEST_CREDS", "1");
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let binding = binding(
        "release-probe",
        "release-probe",
        "test:RELEASE_CANARY",
        "phm:VERCEL_RELEASE",
        "phm:SUPABASE_RELEASE",
        "proj_release",
        "team_release",
    );

    let error = store.save_binding(&binding).unwrap_err().to_string();
    std::env::remove_var("LOCUS_ALLOW_TEST_CREDS");
    assert!(error.contains("invalid credential_ref"));
    assert!(!error.contains("RELEASE_CANARY"));
}

#[test]
fn resolution_failures_and_child_env_never_expose_locator_names() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let binding = binding(
        "failure-probe",
        "failure-probe",
        "env:LOCUS_MISSING_LOCATOR_CANARY",
        "phm:VERCEL_FAILURE",
        "phm:SUPABASE_FAILURE",
        "proj_failure",
        "team_failure",
    );
    store.save_binding(&binding).unwrap();
    let session = store.pin("failure-probe", dir.path(), None, false).unwrap();

    let env = build_isolated_env_opts(&session, &binding, true);
    let serialized = serde_json::to_string(&env.secrets_failed).unwrap();
    assert!(!serialized.contains("LOCUS_MISSING_LOCATOR_CANARY"));
    assert!(!env.vars.keys().any(|key| key.contains("CREDENTIAL_REF")));
    assert!(!env
        .vars
        .values()
        .any(|value| value.contains("LOCUS_MISSING_LOCATOR_CANARY")));
}

#[test]
fn call_tool_github_and_vercel_scope_under_pin() {
    use locus_core::call_tool;
    use serde_json::json;

    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let acme = binding(
        "acme",
        "acme-corp",
        "phm:GH_TOKEN_ACME",
        "phm:VERCEL_TOKEN_ACME",
        "phm:SUPABASE_ACME",
        "proj_acme",
        "team_acme",
    );
    store.save_binding(&acme).unwrap();
    store.pin("acme", dir.path(), None, false).unwrap();
    let binding = store.load_binding("acme").unwrap();

    let gh = call_tool(&binding, "github.scope", &json!({})).unwrap();
    assert!(gh.ok, "github.scope failed: {:?}", gh.content);
    assert!(gh.content.get("credential_ref").is_none());
    assert_eq!(gh.content["credential"]["present"], true);
    assert_eq!(gh.content["credential"]["source"], "phantom");
    assert!(!serde_json::to_string(&gh.content)
        .unwrap()
        .contains("GH_TOKEN_ACME"));
    assert_eq!(
        gh.content.get("tenant").and_then(|v| v.as_str()),
        Some("acme-corp")
    );

    let vc = call_tool(&binding, "vercel.scope", &json!({})).unwrap();
    assert!(vc.ok, "vercel.scope failed: {:?}", vc.content);
    assert_eq!(
        vc.content.get("team_id").and_then(|v| v.as_str()),
        Some("team_acme")
    );

    // freeze deny on wrong team_id
    let denied = call_tool(&binding, "vercel.scope", &json!({ "team_id": "team_evil" }));
    assert!(denied.is_err(), "expected freeze deny, got {denied:?}");
    let msg = denied.unwrap_err().to_string();
    assert!(
        msg.contains("scope freeze") || msg.contains("team_evil"),
        "unexpected: {msg}"
    );
}

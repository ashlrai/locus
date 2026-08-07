//! Integration: exclusive pin isolation — acme vs personal credential_refs.
//!
//! A session pinned to one binding must never surface the sibling binding's
//! credential refs, project refs, or ambient provider env.

use locus_core::{
    build_isolated_env, visible_credential_refs, Binding, BindingBody, Policy, ProviderBinding,
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

    let refs: Vec<&str> = w
        .providers
        .iter()
        .map(|p| p.credential_ref.as_str())
        .collect();
    assert!(refs.contains(&"phm:GH_TOKEN_ACME"));
    assert!(refs.contains(&"phm:VERCEL_TOKEN_ACME"));
    assert!(refs.contains(&"phm:SUPABASE_ACME"));
    for r in &refs {
        assert!(
            !r.to_uppercase().contains("PERSONAL"),
            "acme pin leaked personal ref: {r}"
        );
    }
    assert!(w
        .providers
        .iter()
        .all(|p| p.project_ref.as_deref() != Some("proj_me")));

    let iso = build_isolated_env(&s_acme, &acme);
    let visible = visible_credential_refs(&acme);
    assert_eq!(visible.len(), 3);
    assert!(visible.iter().all(|r| r.contains("ACME")));
    assert!(!iso.vars.values().any(|v| v.contains("PERSONAL")));
    assert!(!iso.vars.values().any(|v| v.contains("proj_me")));
    assert_eq!(
        iso.vars
            .get("LOCUS_GITHUB_CREDENTIAL_REF")
            .map(String::as_str),
        Some("phm:GH_TOKEN_ACME")
    );
    assert_eq!(
        iso.vars
            .get("LOCUS_VERCEL_CREDENTIAL_REF")
            .map(String::as_str),
        Some("phm:VERCEL_TOKEN_ACME")
    );

    // ── switch to personal — exclusive ────────────────────────────────────
    let s_personal = store
        .pin("personal", dir.path(), Some("test".into()), false)
        .unwrap();
    let w2 = store.whoami().unwrap();
    assert_eq!(w2.binding_alias, "personal");
    for p in &w2.providers {
        assert!(
            p.credential_ref.to_uppercase().contains("PERSONAL"),
            "personal pin leaked non-personal ref: {}",
            p.credential_ref
        );
        assert!(!p.credential_ref.to_uppercase().contains("ACME"));
    }
    assert!(w2
        .providers
        .iter()
        .all(|p| p.project_ref.as_deref() != Some("proj_acme")));

    let iso2 = build_isolated_env(&s_personal, &personal);
    assert!(!iso2.vars.values().any(|v| v.contains("ACME")));
    assert!(!iso2.vars.values().any(|v| v.contains("proj_acme")));
    assert!(!iso2.vars.values().any(|v| v.contains("team_acme")));
    assert_eq!(
        iso2.vars
            .get("LOCUS_GITHUB_CREDENTIAL_REF")
            .map(String::as_str),
        Some("phm:GH_TOKEN_PERSONAL")
    );
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
    assert_eq!(
        gh.content.get("credential_ref").and_then(|v| v.as_str()),
        Some("phm:GH_TOKEN_ACME")
    );
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

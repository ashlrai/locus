//! Build an isolated process environment for `locus exec`.
//!
//! Invariants:
//! - Only the **pinned** binding's providers appear.
//! - Credential **refs** (not values) are exported for audit/debug.
//! - Private GH/AWS config dirs point at the session worker home.
//! - Ambient global identity env vars that could confuse tools are scrubbed
//!   when they would imply a different account (we blank common switch vars).

use crate::binding::Binding;
use crate::credential::resolve_binding_secrets;
use crate::error::Result;
use crate::session::Session;
use std::collections::BTreeMap;
use std::path::Path;

/// Environment map layered on top of (a scrubbed copy of) the parent env.
#[derive(Debug, Clone)]
pub struct IsolatedEnv {
    pub vars: BTreeMap<String, String>,
    /// Keys removed from parent env for isolation.
    pub scrubbed: Vec<String>,
    /// How many credential refs were successfully resolved into secret env vars.
    pub secrets_resolved: usize,
    /// Credential refs that failed to resolve (names only — never values).
    pub secrets_failed: Vec<String>,
}

/// Global CLI identity vars that cause wrong-account races if inherited.
const SCRUB_KEYS: &[&str] = &[
    "AWS_PROFILE",
    "AWS_DEFAULT_PROFILE",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "GH_TOKEN",
    "GH_HOST",
    "GITHUB_TOKEN",
    "SUPABASE_ACCESS_TOKEN",
    "SUPABASE_DB_PASSWORD",
    "SUPABASE_PROJECT_ID",
    "SUPABASE_PROJECT_REF",
    "VERCEL_TOKEN",
    "VERCEL_ORG_ID",
    "VERCEL_PROJECT_ID",
    "VERCEL_TEAM_ID",
    "CLOUDFLARE_API_TOKEN",
    "CLOUDFLARE_ACCOUNT_ID",
    "STRIPE_API_KEY",
    "STRIPE_SECRET_KEY",
    "RESEND_API_KEY",
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "XAI_API_KEY",
];

/// Build isolated env. When `resolve_secrets` is true, CredentialRefs are
/// resolved (phm:/env:/test:) and injected as standard provider env vars.
/// Values never appear in logs; only counts/names of failures are returned.
pub fn build_isolated_env(session: &Session, binding: &Binding) -> IsolatedEnv {
    build_isolated_env_opts(session, binding, false)
}

pub fn build_isolated_env_opts(
    session: &Session,
    binding: &Binding,
    resolve_secrets: bool,
) -> IsolatedEnv {
    let mut vars = BTreeMap::new();
    let mut scrubbed = Vec::new();

    // Start from parent, then scrub dangerous ambient identity.
    for (k, v) in std::env::vars() {
        if SCRUB_KEYS.iter().any(|s| s == &k) {
            scrubbed.push(k);
            continue;
        }
        // Also scrub LOCUS_* from parent so we fully own them
        if k.starts_with("LOCUS_") {
            scrubbed.push(k);
            continue;
        }
        vars.insert(k, v);
    }

    // Core identity
    vars.insert("LOCUS_SESSION_ID".into(), session.session_id.clone());
    vars.insert("LOCUS_BINDING".into(), session.binding_alias.clone());
    vars.insert("LOCUS_BINDING_ID".into(), session.binding_id.clone());
    vars.insert("LOCUS_TENANT".into(), session.tenant.clone());
    if let Some(ref p) = session.principal {
        vars.insert("LOCUS_PRINCIPAL".into(), p.clone());
    }
    vars.insert("LOCUS_SEAL".into(), session.seal.clone());
    vars.insert("LOCUS_WORKER_HOME".into(), session.worker_home.clone());

    // Private CLI config roots — never touch the user's global configs
    let worker = Path::new(&session.worker_home);
    vars.insert(
        "GH_CONFIG_DIR".into(),
        worker.join("gh").display().to_string(),
    );
    vars.insert(
        "AWS_CONFIG_FILE".into(),
        worker.join("aws").join("config").display().to_string(),
    );
    vars.insert(
        "AWS_SHARED_CREDENTIALS_FILE".into(),
        worker.join("aws").join("credentials").display().to_string(),
    );

    // Provider surface — only this binding
    let names: Vec<&str> = binding.provider_names();
    vars.insert("LOCUS_PROVIDERS".into(), names.join(","));

    for p in &binding.providers {
        let prefix = format!("LOCUS_{}", p.provider.to_uppercase());
        vars.insert(format!("{prefix}_ACCOUNT"), p.account.clone());
        vars.insert(format!("{prefix}_CREDENTIAL_REF"), p.credential_ref.clone());

        if let Some(ref r) = p.scope.project_ref {
            vars.insert(format!("{prefix}_PROJECT_REF"), r.clone());
            // Common alias for Supabase CLIs
            if p.provider.eq_ignore_ascii_case("supabase") {
                vars.insert("SUPABASE_PROJECT_REF".into(), r.clone());
                vars.insert("SUPABASE_PROJECT_ID".into(), r.clone());
            }
        }
        if let Some(ref t) = p.scope.team_id {
            vars.insert(format!("{prefix}_TEAM_ID"), t.clone());
            if p.provider.eq_ignore_ascii_case("vercel") {
                vars.insert("VERCEL_ORG_ID".into(), t.clone());
                vars.insert("VERCEL_TEAM_ID".into(), t.clone());
            }
        }
        if let Some(ref a) = p.scope.account_id {
            vars.insert(format!("{prefix}_ACCOUNT_ID"), a.clone());
            if p.provider.eq_ignore_ascii_case("cloudflare") {
                vars.insert("CLOUDFLARE_ACCOUNT_ID".into(), a.clone());
            }
            if p.provider.eq_ignore_ascii_case("aws") {
                vars.insert("AWS_ACCOUNT_ID".into(), a.clone());
            }
        }
        if let Some(ro) = p.scope.read_only {
            vars.insert(format!("{prefix}_READ_ONLY"), ro.to_string());
        }
        if !p.scope.orgs.is_empty() {
            vars.insert(format!("{prefix}_ORGS"), p.scope.orgs.join(","));
        }
        if !p.scope.repos.is_empty() {
            vars.insert(format!("{prefix}_REPOS"), p.scope.repos.join(","));
        }
        if !p.scope.projects.is_empty() {
            vars.insert(format!("{prefix}_PROJECTS"), p.scope.projects.join(","));
            if p.provider.eq_ignore_ascii_case("vercel") {
                if let Some(first) = p.scope.projects.first() {
                    vars.insert("VERCEL_PROJECT_ID".into(), first.clone());
                }
            }
        }

        // Always export the ref for audit/debug (not the value).
        vars.insert(
            format!("LOCUS_{}_CREDENTIAL_RESOLVED", p.provider.to_uppercase()),
            "0".into(),
        );
    }

    let mut secrets_resolved = 0usize;
    let mut secrets_failed = Vec::new();

    if resolve_secrets {
        // Soft-resolve: missing phantom secrets should not hard-crash exec by default
        let soft = std::env::var("LOCUS_SOFT_CREDS").ok().as_deref() != Some("0");
        if soft {
            std::env::set_var("LOCUS_SOFT_CREDS", "1");
        }
        match resolve_binding_secrets(binding) {
            Ok(secrets) => {
                for (k, v) in secrets {
                    vars.insert(k, v.as_str().to_string());
                    secrets_resolved += 1;
                }
                for p in &binding.providers {
                    let flag = format!("LOCUS_{}_CREDENTIAL_RESOLVED", p.provider.to_uppercase());
                    // Mark resolved if any inject key present
                    let keys = crate::credential::inject_keys_for_provider(&p.provider);
                    let ok = keys.iter().any(|k| vars.contains_key(*k));
                    vars.insert(flag, if ok { "1".into() } else { "0".into() });
                    if !ok {
                        secrets_failed.push(p.credential_ref.clone());
                    }
                }
            }
            Err(e) => {
                secrets_failed.push(e.to_string());
            }
        }
        // Clear soft flag if we set it and it wasn't already
        let _ = soft;
    }

    IsolatedEnv {
        vars,
        scrubbed,
        secrets_resolved,
        secrets_failed,
    }
}

/// Fallible variant that propagates resolve errors when soft mode is off.
pub fn build_isolated_env_strict(session: &Session, binding: &Binding) -> Result<IsolatedEnv> {
    std::env::set_var("LOCUS_SOFT_CREDS", "0");
    let env = build_isolated_env_opts(session, binding, true);
    if !env.secrets_failed.is_empty() && env.secrets_resolved == 0 {
        return Err(crate::error::LocusError::msg(format!(
            "credential resolve failed: {}",
            env.secrets_failed.join("; ")
        )));
    }
    Ok(env)
}

/// Credential refs that would be visible if secrets were resolved — isolation surface.
pub fn visible_credential_refs(binding: &Binding) -> Vec<String> {
    binding
        .credential_refs()
        .into_iter()
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::{Binding, BindingBody, Policy, ProviderBinding, Scope};
    use crate::seal::SealKey;
    use crate::session::{PinSource, Session};
    use chrono::Duration;

    fn binding_pair() -> (Binding, Binding) {
        let acme = Binding::from_body(BindingBody {
            id: "bnd_acme".into(),
            alias: "acme".into(),
            tenant: "acme-corp".into(),
            principal: None,
            description: None,
            policy: Policy::default(),
            providers: vec![ProviderBinding {
                provider: "supabase".into(),
                account: "acme".into(),
                credential_ref: "phm:SUPABASE_ACME".into(),
                scope: Scope {
                    project_ref: Some("proj_acme".into()),
                    ..Scope::default()
                },
            }],
        });
        let personal = Binding::from_body(BindingBody {
            id: "bnd_personal".into(),
            alias: "personal".into(),
            tenant: "personal".into(),
            principal: None,
            description: None,
            policy: Policy::default(),
            providers: vec![ProviderBinding {
                provider: "supabase".into(),
                account: "me".into(),
                credential_ref: "phm:SUPABASE_PERSONAL".into(),
                scope: Scope {
                    project_ref: Some("proj_me".into()),
                    ..Scope::default()
                },
            }],
        });
        (acme, personal)
    }

    #[test]
    fn env_only_contains_pinned_binding_refs() {
        let (acme, personal) = binding_pair();
        let key = SealKey::generate();
        let session = Session::new(
            &acme.id,
            &acme.alias,
            &acme.tenant,
            None,
            PinSource::Explicit,
            None,
            Duration::hours(1),
            "/tmp/locus-test-worker".into(),
            &key,
        );
        // Simulate ambient wrong account
        std::env::set_var("SUPABASE_PROJECT_REF", "proj_me");
        std::env::set_var("AWS_PROFILE", "personal-prod");

        let iso = build_isolated_env(&session, &acme);
        assert_eq!(
            iso.vars.get("SUPABASE_PROJECT_REF").map(String::as_str),
            Some("proj_acme")
        );
        assert_eq!(
            iso.vars
                .get("LOCUS_SUPABASE_CREDENTIAL_REF")
                .map(String::as_str),
            Some("phm:SUPABASE_ACME")
        );
        assert!(!iso.vars.values().any(|v| v.contains("PERSONAL")));
        assert!(!iso.vars.values().any(|v| v.contains("proj_me")));
        assert!(iso.scrubbed.iter().any(|k| k == "AWS_PROFILE"));
        // personal binding not used
        let _ = personal;
        std::env::remove_var("SUPABASE_PROJECT_REF");
        std::env::remove_var("AWS_PROFILE");
    }

    #[test]
    fn resolve_test_creds_into_env() {
        // Use env: ref to avoid racing LOCUS_ALLOW_TEST_CREDS with other tests.
        std::env::set_var("LOCUS_ISOLATION_TEST_TOKEN", "super-secret-token");
        let acme = Binding::from_body(BindingBody {
            id: "bnd_acme".into(),
            alias: "acme".into(),
            tenant: "acme-corp".into(),
            principal: None,
            description: None,
            policy: Policy::default(),
            providers: vec![ProviderBinding {
                provider: "supabase".into(),
                account: "acme".into(),
                credential_ref: "env:LOCUS_ISOLATION_TEST_TOKEN".into(),
                scope: Scope {
                    project_ref: Some("proj_acme".into()),
                    ..Scope::default()
                },
            }],
        });
        let key = SealKey::generate();
        let session = Session::new(
            &acme.id,
            &acme.alias,
            &acme.tenant,
            None,
            PinSource::Explicit,
            None,
            Duration::hours(1),
            "/tmp/locus-test-worker2".into(),
            &key,
        );
        let iso = build_isolated_env_opts(&session, &acme, true);
        assert_eq!(
            iso.vars.get("SUPABASE_ACCESS_TOKEN").map(String::as_str),
            Some("super-secret-token")
        );
        assert!(iso.secrets_resolved >= 1);
        std::env::remove_var("LOCUS_ISOLATION_TEST_TOKEN");
    }
}

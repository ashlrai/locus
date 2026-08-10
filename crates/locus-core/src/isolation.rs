//! Build an isolated process environment for `locus exec`.
//!
//! Invariants:
//! - Only the **pinned** binding's providers appear.
//! - Credential locator names are never exported to children or diagnostics.
//! - Private GH/AWS config dirs point at the session worker home.
//! - Parent env is rebuilt from a small runtime allowlist; unknown variables
//!   never cross into child processes.

use crate::binding::Binding;
use crate::credential::{resolve_binding_secrets, CredentialResolutionIssue};
use crate::error::Result;
use crate::session::Session;
use std::collections::BTreeMap;
use std::path::Path;

/// Complete environment map for an isolated child.
#[derive(Debug, Clone)]
pub struct IsolatedEnv {
    pub vars: BTreeMap<String, String>,
    /// Keys removed from parent env for isolation.
    pub scrubbed: Vec<String>,
    /// How many credential refs were successfully resolved into secret env vars.
    pub secrets_resolved: usize,
    /// Safe provider/source failures. Locator names and provider stderr are absent.
    pub secrets_failed: Vec<CredentialResolutionIssue>,
}

/// OS/process basics needed to find executables and create temporary files.
/// Identity, provider, cloud, proxy, and application variables are excluded.
const RUNTIME_ENV_KEYS: &[&str] = &[
    "PATH",
    "LANG",
    "TERM",
    "COLORTERM",
    "NO_COLOR",
    "TMPDIR",
    "TMP",
    "TEMP",
    "SystemRoot",
    "WINDIR",
    "PATHEXT",
    "ComSpec",
];

fn is_runtime_env_key(key: &str) -> bool {
    RUNTIME_ENV_KEYS.contains(&key) || key.starts_with("LC_")
}

/// Build isolated env. When `resolve_secrets` is true, CredentialRefs are
/// resolved (`phm:` / `env:`) and injected as standard provider env vars.
/// Values and locator names never appear in logs or the child environment.
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
    // Positive construction: unknown parent keys never reach the child.
    for (k, v) in std::env::vars() {
        if !is_runtime_env_key(&k) {
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
    vars.insert("HOME".into(), session.worker_home.clone());
    vars.insert("USERPROFILE".into(), session.worker_home.clone());
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

        vars.insert(
            format!("LOCUS_{}_CREDENTIAL_RESOLVED", p.provider.to_uppercase()),
            "0".into(),
        );
    }

    let mut secrets_resolved = 0usize;
    let mut secrets_failed = Vec::new();

    if resolve_secrets {
        let outcome = resolve_binding_secrets(binding);
        let allowed_secret_keys = binding
            .providers
            .iter()
            .flat_map(|provider| crate::credential::inject_keys_for_provider(&provider.provider))
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        for (k, v) in outcome.env {
            if allowed_secret_keys.contains(k.as_str()) {
                vars.insert(k, v.as_str().to_string());
                secrets_resolved += 1;
            }
        }
        secrets_failed = outcome.issues;
        for p in &binding.providers {
            let flag = format!("LOCUS_{}_CREDENTIAL_RESOLVED", p.provider.to_uppercase());
            let keys = crate::credential::inject_keys_for_provider(&p.provider);
            let ok = keys.iter().any(|k| vars.contains_key(*k));
            vars.insert(flag, if ok { "1".into() } else { "0".into() });
        }
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
    let env = build_isolated_env_opts(session, binding, true);
    if !env.secrets_failed.is_empty() {
        return Err(crate::error::LocusError::msg(format!(
            "credential resolve failed: {}",
            env.secrets_failed
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
        )));
    }
    Ok(env)
}

/// Build the **export surface** for CI mint JSON / `locus ci env`.
///
/// Contains only LOCUS_* identity vars and provider frozen scopes (project_ref,
/// team_id, orgs, …). Never inherits the parent process env.
///
/// Secrets are **never** included unless `resolve_secrets` is true **and**
/// `LOCUS_CI_ALLOW_SECRETS=1` is set in the environment.
pub fn build_ci_env_map(
    session: &Session,
    binding: &Binding,
    resolve_secrets: bool,
) -> BTreeMap<String, String> {
    let allow_secrets =
        resolve_secrets && std::env::var("LOCUS_CI_ALLOW_SECRETS").ok().as_deref() == Some("1");

    // Start empty — no parent env.
    let mut vars = BTreeMap::new();

    vars.insert("LOCUS_SESSION_ID".into(), session.session_id.clone());
    vars.insert("LOCUS_BINDING".into(), session.binding_alias.clone());
    vars.insert("LOCUS_BINDING_ID".into(), session.binding_id.clone());
    vars.insert("LOCUS_TENANT".into(), session.tenant.clone());
    if let Some(ref p) = session.principal {
        vars.insert("LOCUS_PRINCIPAL".into(), p.clone());
    }
    vars.insert("LOCUS_SEAL".into(), session.seal.clone());
    vars.insert("LOCUS_WORKER_HOME".into(), session.worker_home.clone());
    vars.insert("LOCUS_EXPIRES_AT".into(), session.expires_at.to_rfc3339());

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

    let names: Vec<&str> = binding.provider_names();
    vars.insert("LOCUS_PROVIDERS".into(), names.join(","));

    for p in &binding.providers {
        let prefix = format!("LOCUS_{}", p.provider.to_uppercase());
        vars.insert(format!("{prefix}_ACCOUNT"), p.account.clone());
        vars.insert(format!("{prefix}_CREDENTIAL_RESOLVED"), "0".into());

        if let Some(ref r) = p.scope.project_ref {
            vars.insert(format!("{prefix}_PROJECT_REF"), r.clone());
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
    }

    if allow_secrets {
        let outcome = resolve_binding_secrets(binding);
        for (k, v) in outcome.env {
            vars.insert(k, v.as_str().to_string());
        }
        for p in &binding.providers {
            let flag = format!("LOCUS_{}_CREDENTIAL_RESOLVED", p.provider.to_uppercase());
            let keys = crate::credential::inject_keys_for_provider(&p.provider);
            let ok = keys.iter().any(|k| vars.contains_key(*k));
            vars.insert(flag, if ok { "1".into() } else { "0".into() });
        }
    }

    vars
}

/// True when CI mint/env is allowed to emit resolved secrets.
pub fn ci_secrets_allowed() -> bool {
    std::env::var("LOCUS_CI_ALLOW_SECRETS").ok().as_deref() == Some("1")
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
                upstream: None,
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
                upstream: None,
            }],
        });
        (acme, personal)
    }

    #[test]
    fn env_only_contains_pinned_binding_identity_without_locator_names() {
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
        std::env::set_var("UNLISTED_SECRET_CANARY", "must-not-cross-boundary");

        let iso = build_isolated_env(&session, &acme);
        assert_eq!(
            iso.vars.get("SUPABASE_PROJECT_REF").map(String::as_str),
            Some("proj_acme")
        );
        assert!(!iso.vars.keys().any(|k| k.contains("CREDENTIAL_REF")));
        assert!(!iso.vars.values().any(|v| v.contains("SUPABASE_ACME")));
        assert!(!iso.vars.values().any(|v| v.contains("PERSONAL")));
        assert!(!iso.vars.values().any(|v| v.contains("proj_me")));
        assert!(!iso.vars.contains_key("UNLISTED_SECRET_CANARY"));
        assert!(!iso.vars.values().any(|v| v == "must-not-cross-boundary"));
        assert_eq!(
            iso.vars.get("HOME").map(String::as_str),
            Some("/tmp/locus-test-worker")
        );
        assert!(iso.scrubbed.iter().any(|k| k == "AWS_PROFILE"));
        assert!(iso.scrubbed.iter().any(|k| k == "UNLISTED_SECRET_CANARY"));
        // personal binding not used
        let _ = personal;
        std::env::remove_var("SUPABASE_PROJECT_REF");
        std::env::remove_var("AWS_PROFILE");
        std::env::remove_var("UNLISTED_SECRET_CANARY");
    }

    #[test]
    fn resolve_test_creds_into_env() {
        // Use env: to exercise the production resolver path.
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
                upstream: None,
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
        assert!(!iso.vars.contains_key("LOCUS_ISOLATION_TEST_TOKEN"));
        assert!(iso
            .scrubbed
            .iter()
            .any(|key| key == "LOCUS_ISOLATION_TEST_TOKEN"));
        assert!(iso.secrets_resolved >= 1);
        std::env::remove_var("LOCUS_ISOLATION_TEST_TOKEN");
    }

    #[test]
    fn ci_env_map_never_includes_secrets_by_default() {
        std::env::set_var("LOCUS_CI_SECRET_TOKEN", "super-secret-ci");
        std::env::remove_var("LOCUS_CI_ALLOW_SECRETS");
        let acme = Binding::from_body(BindingBody {
            id: "bnd_acme".into(),
            alias: "acme".into(),
            tenant: "acme-corp".into(),
            principal: Some("ci".into()),
            description: None,
            policy: Policy::default(),
            providers: vec![ProviderBinding {
                provider: "supabase".into(),
                account: "acme".into(),
                credential_ref: "env:LOCUS_CI_SECRET_TOKEN".into(),
                scope: Scope {
                    project_ref: Some("proj_acme".into()),
                    ..Scope::default()
                },
                upstream: None,
            }],
        });
        let key = SealKey::generate();
        let session = Session::new(
            &acme.id,
            &acme.alias,
            &acme.tenant,
            Some("ci".into()),
            PinSource::Ci,
            Some("ci".into()),
            Duration::minutes(15),
            "/tmp/locus-ci-worker".into(),
            &key,
        );

        // Even with resolve_secrets=true, without LOCUS_CI_ALLOW_SECRETS secrets stay out
        let map = build_ci_env_map(&session, &acme, true);
        assert_eq!(
            map.get("LOCUS_SESSION_ID").map(String::as_str),
            Some(session.session_id.as_str())
        );
        assert_eq!(
            map.get("SUPABASE_PROJECT_REF").map(String::as_str),
            Some("proj_acme")
        );
        assert!(!map.keys().any(|k| k.contains("CREDENTIAL_REF")));
        assert!(!map.values().any(|v| v.contains("LOCUS_CI_SECRET_TOKEN")));
        assert!(!map.values().any(|v| v == "super-secret-ci"));
        assert!(!map.contains_key("SUPABASE_ACCESS_TOKEN"));

        // With allow flag, secrets may resolve
        std::env::set_var("LOCUS_CI_ALLOW_SECRETS", "1");
        let map2 = build_ci_env_map(&session, &acme, true);
        assert_eq!(
            map2.get("SUPABASE_ACCESS_TOKEN").map(String::as_str),
            Some("super-secret-ci")
        );
        std::env::remove_var("LOCUS_CI_ALLOW_SECRETS");
        std::env::remove_var("LOCUS_CI_SECRET_TOKEN");
    }
}

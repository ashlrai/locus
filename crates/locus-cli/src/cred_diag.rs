//! Visible diagnostics for credential refs that do not resolve.
//!
//! `phantom list` only proves a locator exists; it does not prove Locus can
//! read it. Current Phantom Secrets refuses the non-interactive
//! `phantom reveal --yes NAME` that Locus uses, so a listed `phm:` ref can
//! still leave the child without its variable. This module:
//!
//! - probes the real resolution path for listed `phm:` refs so `locus doctor`
//!   (and every CLI surface built on the same external facts) flags a refused
//!   or failed reveal instead of reporting "credentials ok";
//! - turns `CredentialResolutionIssue`s into operator lines naming the binding,
//!   the env vars left unset and the cause.
//!
//! Values never leave [`locus_core::resolve`]'s `Zeroizing` buffer here, and
//! locator names never appear in any output (only provider/source/code).

use locus_core::{CredentialRef, CredentialResolutionIssue, Store};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// `phantom reveal` exited non-zero, printed nothing, or could not spawn.
pub(crate) const REVEAL_FAILED: &str = "reveal-failed";
/// `phantom reveal` outlived the core deadline and was killed.
pub(crate) const REVEAL_TIMEOUT: &str = "reveal-timeout";

/// Long-lived CLI loops (`locus watch`) gather doctor facts every tick; cache
/// each probe outcome so a vault is not asked to reveal on every tick.
const PROBE_CACHE_TTL: Duration = Duration::from_secs(60);

/// Probe the real resolution path for every `phm:` ref whose provider is not
/// already in `already_unresolved` (the `phantom list` check). Returns extra
/// issues (`reveal-failed` / `reveal-timeout`) to append. No probe runs when
/// Phantom is off PATH: every `phm:` ref is already flagged in that case.
pub(crate) fn probe_phm_reveal(
    store: &Store,
    phantom_on_path: bool,
    already_unresolved: &[CredentialResolutionIssue],
) -> Vec<CredentialResolutionIssue> {
    if !phantom_on_path {
        return Vec::new();
    }
    let needed = phm_refs_needing_probe(store, already_unresolved);
    probe_with(needed, cached_reveal_failure)
}

/// `(provider label, phantom name)` pairs for listed-but-unprobed `phm:` refs.
fn phm_refs_needing_probe(
    store: &Store,
    already_unresolved: &[CredentialResolutionIssue],
) -> Vec<(String, String)> {
    let Ok(summaries) = store.list_bindings() else {
        return Vec::new();
    };
    let mut needed: Vec<(String, String)> = Vec::new();
    for sum in summaries {
        let Ok(binding) = store.load_binding(&sum.alias) else {
            continue;
        };
        for p in &binding.providers {
            let CredentialRef::Phantom { name } = CredentialRef::parse(&p.credential_ref) else {
                continue;
            };
            let provider = provider_label(&p.provider);
            let flagged = already_unresolved
                .iter()
                .any(|i| i.source == "phantom" && i.provider == provider);
            if !flagged && !needed.iter().any(|(pp, n)| pp == &provider && n == &name) {
                needed.push((provider, name));
            }
        }
    }
    needed
}

/// Run `reveal` for each name once; one issue per failing provider.
fn probe_with(
    needed: Vec<(String, String)>,
    reveal: impl Fn(&str) -> Option<&'static str>,
) -> Vec<CredentialResolutionIssue> {
    let mut outcomes: HashMap<String, Option<&'static str>> = HashMap::new();
    let mut issues: Vec<CredentialResolutionIssue> = Vec::new();
    for (provider, name) in needed {
        let failure = *outcomes
            .entry(name.clone())
            .or_insert_with(|| reveal(&name));
        if let Some(code) = failure {
            if !issues.iter().any(|i| i.provider == provider) {
                issues.push(CredentialResolutionIssue {
                    provider,
                    source: "phantom".into(),
                    code: code.into(),
                });
            }
        }
    }
    issues.sort_by(|a, b| a.provider.cmp(&b.provider));
    issues
}

/// `None` when the name resolves to a non-empty value; otherwise the failure
/// code. The value is dropped (zeroized) immediately.
fn reveal_failure(name: &str) -> Option<&'static str> {
    match locus_core::resolve(&CredentialRef::Phantom { name: name.into() }) {
        Ok(value) if !value.is_empty() => None,
        Ok(_) => Some(REVEAL_FAILED),
        // The core error carries only a static label (never the name or child
        // output); "timed out" is the deadline runner's wording.
        Err(e) if e.to_string().contains("timed out") => Some(REVEAL_TIMEOUT),
        Err(_) => Some(REVEAL_FAILED),
    }
}

type ProbeCache = HashMap<(String, Option<String>), (Instant, Option<&'static str>)>;

fn cached_reveal_failure(name: &str) -> Option<&'static str> {
    static CACHE: OnceLock<Mutex<ProbeCache>> = OnceLock::new();
    let key = (
        name.to_string(),
        std::env::var("LOCUS_PHANTOM_PROJECT").ok(),
    );
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = match cache.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some((at, outcome)) = map.get(&key) {
        if at.elapsed() < PROBE_CACHE_TTL {
            return *outcome;
        }
    }
    let outcome = reveal_failure(name);
    map.insert(key, (Instant::now(), outcome));
    outcome
}

/// Same normalization core applies to `CredentialResolutionIssue::provider`.
fn provider_label(provider: &str) -> String {
    if !provider.is_empty()
        && provider.len() <= 64
        && provider
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    {
        provider.to_ascii_lowercase()
    } else {
        "unknown".into()
    }
}

/// Human cause for one issue. `phantom_on_path` disambiguates the generic
/// `unavailable` code that the `phantom list` check and exec resolution share.
pub(crate) fn cause(issue: &CredentialResolutionIssue, phantom_on_path: bool) -> &'static str {
    match (issue.source.as_str(), issue.code.as_str()) {
        (_, REVEAL_FAILED) => {
            "phantom refused or failed `reveal` (current Phantom Secrets refuses non-interactive reveal)"
        }
        (_, REVEAL_TIMEOUT) => "phantom reveal timed out",
        ("phantom", "unavailable") if !phantom_on_path => "phantom not on PATH",
        ("phantom", "unavailable") => {
            "name not listed by `phantom list` (absent from the vault, or list failed)"
        }
        ("environment", "unavailable") => "environment variable not set",
        (_, "unsupported-mapping") => "no env var mapping for this provider",
        (_, "invalid-adapter-contract") => "community adapter contract rejected",
        _ => "credential unavailable",
    }
}

/// Refine exec-time issues: core reports every failed `phm:` resolve as
/// `unavailable`. Re-check with `phantom list` (failure path only) so a listed
/// name that still failed is reported as a refused/failed reveal.
pub(crate) fn refine_exec_issues(
    store: &Store,
    issues: &[CredentialResolutionIssue],
) -> (Vec<CredentialResolutionIssue>, bool) {
    let has_phm = issues
        .iter()
        .any(|i| i.source == "phantom" && i.code == "unavailable");
    if !has_phm {
        return (issues.to_vec(), true);
    }
    let phantom = locus_core::phantom_on_path();
    if !phantom {
        return (issues.to_vec(), false);
    }
    let listed_unresolved =
        locus_core::collect_unresolved_phm_refs(store, phantom).unwrap_or_default();
    let refined = issues
        .iter()
        .map(|i| {
            let mut i = i.clone();
            if i.source == "phantom"
                && i.code == "unavailable"
                && !listed_unresolved.iter().any(|l| l.provider == i.provider)
            {
                i.code = REVEAL_FAILED.into();
            }
            i
        })
        .collect();
    (refined, phantom)
}

/// One line per issue: binding, provider, env vars left unset, source, cause.
/// Never includes locator names or values.
pub(crate) fn describe_unresolved(
    binding: &locus_core::Binding,
    issues: &[CredentialResolutionIssue],
    phantom_on_path: bool,
) -> Vec<String> {
    issues
        .iter()
        .map(|issue| {
            let mut vars: Vec<String> = Vec::new();
            for p in binding
                .providers
                .iter()
                .filter(|p| provider_label(&p.provider) == issue.provider)
            {
                for key in
                    locus_core::credential::inject_keys_for_binding_provider(p).unwrap_or_default()
                {
                    if !vars.contains(&key) {
                        vars.push(key);
                    }
                }
            }
            let env = if vars.is_empty() {
                "-".to_string()
            } else {
                vars.join(",")
            };
            format!(
                "binding={} provider={} source={} code={} env={}: {}",
                binding.alias,
                issue.provider,
                issue.source,
                issue.code,
                env,
                cause(issue, phantom_on_path)
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn issue(provider: &str, source: &str, code: &str) -> CredentialResolutionIssue {
        CredentialResolutionIssue {
            provider: provider.into(),
            source: source.into(),
            code: code.into(),
        }
    }

    #[test]
    fn probe_flags_refused_reveal_once_per_provider_and_name() {
        let calls = Cell::new(0);
        let issues = probe_with(
            vec![
                ("github".into(), "GH".into()),
                ("vercel".into(), "VC".into()),
                ("github".into(), "GH".into()),
                ("supabase".into(), "GH".into()),
            ],
            |name| {
                calls.set(calls.get() + 1);
                (name == "GH").then_some(REVEAL_FAILED)
            },
        );
        assert_eq!(calls.get(), 2, "each name revealed once");
        assert_eq!(
            issues,
            vec![
                issue("github", "phantom", REVEAL_FAILED),
                issue("supabase", "phantom", REVEAL_FAILED),
            ]
        );
    }

    #[test]
    fn probe_is_empty_when_every_reveal_succeeds() {
        let issues = probe_with(vec![("github".into(), "GH".into())], |_| None);
        assert!(issues.is_empty());
    }

    #[test]
    fn causes_are_distinct_and_value_free() {
        assert!(cause(&issue("github", "phantom", REVEAL_FAILED), true).contains("reveal"));
        assert_eq!(
            cause(&issue("github", "phantom", "unavailable"), false),
            "phantom not on PATH"
        );
        assert!(cause(&issue("github", "phantom", "unavailable"), true).contains("phantom list"));
        assert_eq!(
            cause(&issue("github", "environment", "unavailable"), true),
            "environment variable not set"
        );
    }

    #[test]
    fn describe_names_binding_and_env_vars_not_locator() {
        let binding = locus_core::Binding::parse_toml(
            r#"
[binding]
id = "bnd_acme"
alias = "acme"
tenant = "acme"

[[binding.providers]]
provider = "github"
account = "acme"
credential_ref = "phm:GH_TOKEN_SECRET_LOCATOR"
"#,
        )
        .unwrap();
        let lines =
            describe_unresolved(&binding, &[issue("github", "phantom", REVEAL_FAILED)], true);
        assert_eq!(lines.len(), 1);
        let line = &lines[0];
        assert!(line.contains("binding=acme"), "{line}");
        assert!(
            line.contains("env=GH_TOKEN,GITHUB_TOKEN,GITHUB_PERSONAL_ACCESS_TOKEN"),
            "{line}"
        );
        assert!(line.contains("code=reveal-failed"), "{line}");
        assert!(!line.contains("GH_TOKEN_SECRET_LOCATOR"), "{line}");
    }
}

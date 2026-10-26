//! Value-free launch diagnostics. First-run samples explain setup once;
//! real or edited bindings retain individual credential failure diagnostics.

use colored::Colorize;
use locus_core::{Binding, CredentialResolutionIssue};

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

pub(crate) fn cause(issue: &CredentialResolutionIssue) -> &'static str {
    match (issue.source.as_str(), issue.code.as_str()) {
        ("phantom", "unsupported-integration") => "Phantom credential integration unsupported; use an explicitly supplied env:VAR reference",
        ("environment", "unavailable") => "environment variable not set",
        (_, "unsupported-mapping") => "no env var mapping for this provider",
        (_, "invalid-adapter-contract") => "community adapter contract rejected",
        _ => "credential unavailable",
    }
}

/// Diagnostics are presentation only: callers enforce --strict-creds before
/// this point, and resolution failures remain in the execution audit record.
pub(crate) fn print_launch_diagnostics(
    binding: &Binding,
    issues: &[CredentialResolutionIssue],
    lines: &[String],
) {
    if issues.is_empty() {
        return;
    }
    if issues
        .iter()
        .all(|i| i.source == "environment" && i.code == "unavailable")
        && crate::samples::is_sample(binding)
    {
        eprintln!("note: sample binding; provider credentials are not configured. Edit the binding before real use; --strict-creds requires credentials.");
        return;
    }
    for line in lines {
        eprintln!(
            "{} unresolved credential, child runs without it (--strict-creds to fail): {}",
            "warn".yellow(),
            line
        );
    }
}

/// One line per issue: binding, provider, env vars left unset, source, cause.
/// Never includes locator names or values.
pub(crate) fn describe_unresolved(
    binding: &locus_core::Binding,
    issues: &[CredentialResolutionIssue],
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
                cause(issue)
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_matching_checks_the_whole_binding() {
        let original = Binding::parse_toml(crate::samples::PERSONAL).unwrap();
        assert!(crate::samples::is_sample(&original));
        let mut edited = original.clone();
        edited.providers[0].credential_ref = "env:REAL_PROVIDER_CREDENTIAL".into();
        assert!(!crate::samples::is_sample(&edited));
        edited = original;
        edited.policy.require_approval.push("*.delete".into());
        assert!(!crate::samples::is_sample(&edited));
    }

    #[test]
    fn diagnostics_include_safe_context_without_reference_names() {
        let binding = Binding::parse_toml(crate::samples::PERSONAL).unwrap();
        let issue = CredentialResolutionIssue {
            provider: "github".into(),
            source: "environment".into(),
            code: "unavailable".into(),
        };
        let lines = describe_unresolved(&binding, &[issue]);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("binding=personal provider=github"));
        assert!(lines[0].contains("environment variable not set"));
        assert!(!lines[0].contains("LOCUS_GH_TOKEN_PERSONAL"));
    }
}

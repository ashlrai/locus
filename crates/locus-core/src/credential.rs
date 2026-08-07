//! CredentialRef resolution.
//!
//! Formats:
//! - `phm:NAME`     — resolve via `phantom reveal --yes NAME` (value never logged)
//! - `env:VAR`      — read from process environment
//! - `keychain:…`   — reserved (phase 2)
//! - `test:VALUE`   — only when `LOCUS_ALLOW_TEST_CREDS=1` (unit tests)
//!
//! Values are held in `Zeroizing<String>` and must only be injected into
//! worker env maps — never returned over MCP or printed to agent-facing stdout.

use crate::error::{LocusError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::process::Command;
use zeroize::Zeroizing;

/// Parsed credential reference (no secret material).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CredentialRef {
    /// Phantom vault secret name.
    Phantom { name: String },
    /// Environment variable in the parent process.
    Env { var: String },
    /// Test-only plaintext (gated).
    Test { value: String },
    /// Unrecognized / unsupported scheme.
    Unknown { raw: String },
}

impl CredentialRef {
    pub fn parse(raw: &str) -> Self {
        let raw = raw.trim();
        if let Some(rest) = raw.strip_prefix("phm:") {
            return Self::Phantom {
                name: rest.to_string(),
            };
        }
        if let Some(rest) = raw.strip_prefix("env:") {
            return Self::Env {
                var: rest.to_string(),
            };
        }
        if let Some(rest) = raw.strip_prefix("test:") {
            return Self::Test {
                value: rest.to_string(),
            };
        }
        // Bare names default to Phantom for DX (`SUPABASE_ACME` ≈ `phm:SUPABASE_ACME`)
        if !raw.contains(':') && !raw.is_empty() {
            return Self::Phantom {
                name: raw.to_string(),
            };
        }
        Self::Unknown {
            raw: raw.to_string(),
        }
    }

    pub fn display(&self) -> String {
        match self {
            Self::Phantom { name } => format!("phm:{name}"),
            Self::Env { var } => format!("env:{var}"),
            Self::Test { .. } => "test:***".into(),
            Self::Unknown { raw } => raw.clone(),
        }
    }
}

/// Resolve a credential ref to a secret value.
///
/// # Safety
/// Caller must not log or serialize the returned value into agent context.
pub fn resolve(cred: &CredentialRef) -> Result<Zeroizing<String>> {
    match cred {
        CredentialRef::Phantom { name } => resolve_phantom(name),
        CredentialRef::Env { var } => {
            let v = std::env::var(var).map_err(|_| {
                LocusError::msg(format!("env credential not set: {var}"))
            })?;
            Ok(Zeroizing::new(v))
        }
        CredentialRef::Test { value } => {
            if std::env::var("LOCUS_ALLOW_TEST_CREDS").ok().as_deref() != Some("1") {
                return Err(LocusError::msg(
                    "test: credentials require LOCUS_ALLOW_TEST_CREDS=1",
                ));
            }
            Ok(Zeroizing::new(value.clone()))
        }
        CredentialRef::Unknown { raw } => Err(LocusError::msg(format!(
            "unsupported credential_ref: {raw} (use phm:NAME or env:VAR)"
        ))),
    }
}

fn resolve_phantom(name: &str) -> Result<Zeroizing<String>> {
    // Optional project directory for multi-vault machines
    let mut cmd = Command::new("phantom");
    cmd.arg("reveal").arg("--yes").arg(name);
    if let Ok(dir) = std::env::var("LOCUS_PHANTOM_PROJECT") {
        cmd.current_dir(dir);
    }
    let output = cmd.output().map_err(|e| {
        LocusError::msg(format!(
            "failed to run `phantom reveal` (is phantom installed?): {e}"
        ))
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Never include stdout (may have partial secrets) in the error if failed
        return Err(LocusError::msg(format!(
            "phantom reveal failed for secret '{name}': {stderr}"
        )));
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() {
        return Err(LocusError::msg(format!(
            "phantom reveal returned empty value for '{name}'"
        )));
    }
    Ok(Zeroizing::new(value))
}

/// Map provider → standard env var names that receive the resolved secret.
pub fn inject_keys_for_provider(provider: &str) -> &'static [&'static str] {
    match provider.to_ascii_lowercase().as_str() {
        "supabase" => &["SUPABASE_ACCESS_TOKEN"],
        "github" => &["GH_TOKEN", "GITHUB_TOKEN", "GITHUB_PERSONAL_ACCESS_TOKEN"],
        "vercel" => &["VERCEL_TOKEN"],
        "cloudflare" => &["CLOUDFLARE_API_TOKEN"],
        "aws" => &["AWS_SECRET_ACCESS_KEY"], // incomplete without key id — phase 2
        "resend" => &["RESEND_API_KEY"],
        "stripe" => &["STRIPE_API_KEY", "STRIPE_SECRET_KEY"],
        "openai" => &["OPENAI_API_KEY"],
        "anthropic" => &["ANTHROPIC_API_KEY"],
        "xai" => &["XAI_API_KEY"],
        _ => &[],
    }
}

/// Resolve all provider credentials for a binding into env var injections.
/// Returns map of env_key → secret. Values must not be logged.
pub fn resolve_binding_secrets(
    binding: &crate::binding::Binding,
) -> Result<BTreeMap<String, Zeroizing<String>>> {
    let mut out = BTreeMap::new();
    for p in &binding.providers {
        let cred = CredentialRef::parse(&p.credential_ref);
        let value = match resolve(&cred) {
            Ok(v) => v,
            Err(e) => {
                // Soft-fail individual providers when LOCUS_SOFT_CREDS=1
                if std::env::var("LOCUS_SOFT_CREDS").ok().as_deref() == Some("1") {
                    continue;
                }
                return Err(LocusError::msg(format!(
                    "resolve {} ({}): {e}",
                    p.provider,
                    cred.display()
                )));
            }
        };
        for key in inject_keys_for_provider(&p.provider) {
            out.insert((*key).to_string(), Zeroizing::new(value.as_str().to_string()));
        }
        // Also set LOCUS_<PROVIDER>_RESOLVED=1 (not the secret) for debugging
        let flag = format!("LOCUS_{}_CREDENTIAL_RESOLVED", p.provider.to_uppercase());
        // Don't put secrets in out under flag — use empty marker via separate path
        let _ = flag;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_formats() {
        assert_eq!(
            CredentialRef::parse("phm:FOO"),
            CredentialRef::Phantom {
                name: "FOO".into()
            }
        );
        assert_eq!(
            CredentialRef::parse("env:BAR"),
            CredentialRef::Env {
                var: "BAR".into()
            }
        );
        assert_eq!(
            CredentialRef::parse("BARE"),
            CredentialRef::Phantom {
                name: "BARE".into()
            }
        );
    }

    #[test]
    fn resolve_env_and_test() {
        std::env::set_var("LOCUS_TEST_SECRET_XYZ", "s3cret");
        let v = resolve(&CredentialRef::Env {
            var: "LOCUS_TEST_SECRET_XYZ".into(),
        })
        .unwrap();
        assert_eq!(v.as_str(), "s3cret");
        std::env::remove_var("LOCUS_TEST_SECRET_XYZ");

        std::env::set_var("LOCUS_ALLOW_TEST_CREDS", "1");
        let v = resolve(&CredentialRef::Test {
            value: "tval".into(),
        })
        .unwrap();
        assert_eq!(v.as_str(), "tval");
        std::env::remove_var("LOCUS_ALLOW_TEST_CREDS");
        assert!(resolve(&CredentialRef::Test {
            value: "tval".into()
        })
        .is_err());
    }

    #[test]
    fn inject_keys() {
        assert!(inject_keys_for_provider("supabase").contains(&"SUPABASE_ACCESS_TOKEN"));
        assert!(inject_keys_for_provider("github").contains(&"GH_TOKEN"));
    }
}

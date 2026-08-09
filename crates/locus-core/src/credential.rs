//! CredentialRef resolution.
//!
//! Formats:
//! - `phm:NAME`     — resolve via `phantom reveal --yes NAME` (value never logged)
//! - `env:VAR`      — read from process environment
//! - `test:VALUE`   — compiled unit tests only; release binaries always reject it
//!
//! Values are held in `Zeroizing<String>` and must only be injected into
//! worker env maps — never returned over MCP or printed to agent-facing stdout.

use crate::error::{LocusError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use zeroize::Zeroizing;

/// Process-lifetime cache of `phantom --version` success.
///
/// Doctor, agent report, forensics, and the dashboard all need to know whether
/// Phantom is on PATH. Shelling out on every probe is slow (and dashboard polls
/// `/api/doctor` often). Probe once per process; result is sticky for the life
/// of the binary.
pub fn phantom_on_path() -> bool {
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(probe_phantom_version)
}

fn probe_phantom_version() -> bool {
    Command::new("phantom")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|st| st.success())
        .unwrap_or(false)
}

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
        Self::Unknown {
            raw: raw.to_string(),
        }
    }

    /// Parse and validate a reference accepted in a binding.
    ///
    /// Only explicit supported schemes are accepted. `test:` is compiled out
    /// of production acceptance and cannot be enabled by environment.
    pub fn validate(raw: &str) -> Result<Self> {
        if raw != raw.trim() {
            return Err(invalid_ref());
        }
        let parsed = Self::parse(raw);
        match &parsed {
            Self::Phantom { name } if valid_phantom_name(name) => Ok(parsed),
            Self::Env { var } if valid_env_name(var) => Ok(parsed),
            Self::Test { value } if value.is_empty() => Err(invalid_ref()),
            Self::Test { .. } if cfg!(test) => Ok(parsed),
            _ => Err(invalid_ref()),
        }
    }

    /// Safe source label for agent-facing metadata. Never includes the ref name.
    pub fn source(&self) -> &'static str {
        match self {
            Self::Phantom { .. } => "phantom",
            Self::Env { .. } => "environment",
            Self::Test { .. } => "test",
            Self::Unknown { .. } => "unsupported",
        }
    }
}

/// Agent-safe credential metadata. The reference name/value is intentionally absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialMetadata {
    pub present: bool,
    pub source: String,
}

pub fn credential_metadata(raw: &str) -> CredentialMetadata {
    let parsed = CredentialRef::parse(raw);
    CredentialMetadata {
        present: !raw.trim().is_empty(),
        source: parsed.source().to_string(),
    }
}

/// Safe resolution failure metadata. Locator names and provider stderr are absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialResolutionIssue {
    pub provider: String,
    pub source: String,
    pub code: String,
}

impl fmt::Display for CredentialResolutionIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "provider={} source={} code={}",
            self.provider, self.source, self.code
        )
    }
}

#[derive(Debug)]
pub struct ResolvedBindingSecrets {
    pub env: BTreeMap<String, Zeroizing<String>>,
    pub issues: Vec<CredentialResolutionIssue>,
}

fn safe_provider_label(provider: &str) -> String {
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

fn valid_phantom_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphanumeric())
        && chars.all(|c| c == '_' || c == '-' || c == '.' || c.is_ascii_alphanumeric())
}

fn valid_env_name(var: &str) -> bool {
    let mut chars = var.chars();
    matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn invalid_ref() -> LocusError {
    LocusError::msg("invalid credential_ref: use explicit phm:NAME or env:VAR")
}

/// Convert only a conservative legacy bare Phantom name. Unsafe input is never returned.
pub fn migrate_legacy_phantom_ref(raw: &str) -> Option<String> {
    let mut chars = raw.chars();
    let conservative_name = matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_uppercase())
        && chars.all(|c| {
            c == '_' || c == '-' || c == '.' || c.is_ascii_uppercase() || c.is_ascii_digit()
        });
    if raw == raw.trim() && !raw.contains(':') && conservative_name {
        Some(format!("phm:{raw}"))
    } else {
        None
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
            let v = std::env::var(var)
                .map_err(|_| LocusError::msg("credential unavailable (source=environment)"))?;
            Ok(Zeroizing::new(v))
        }
        CredentialRef::Test { value } => {
            if !cfg!(test) {
                return Err(LocusError::msg("credential source unsupported"));
            }
            Ok(Zeroizing::new(value.clone()))
        }
        CredentialRef::Unknown { .. } => Err(invalid_ref()),
    }
}

fn resolve_phantom(name: &str) -> Result<Zeroizing<String>> {
    // Optional project directory for multi-vault machines
    let mut cmd = Command::new("phantom");
    cmd.arg("reveal").arg("--yes").arg(name);
    if let Ok(dir) = std::env::var("LOCUS_PHANTOM_PROJECT") {
        cmd.current_dir(dir);
    }
    let output = cmd
        .output()
        .map_err(|_| LocusError::msg("credential unavailable (source=phantom)"))?;
    if !output.status.success() {
        // Both streams are untrusted and may contain locator names or secret material.
        return Err(LocusError::msg("credential unavailable (source=phantom)"));
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() {
        return Err(LocusError::msg("credential unavailable (source=phantom)"));
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
pub fn resolve_binding_secrets(binding: &crate::binding::Binding) -> ResolvedBindingSecrets {
    let mut out = BTreeMap::new();
    let mut issues = Vec::new();
    for p in &binding.providers {
        let cred = CredentialRef::parse(&p.credential_ref);
        let value = match resolve(&cred) {
            Ok(v) => v,
            Err(_) => {
                issues.push(CredentialResolutionIssue {
                    provider: safe_provider_label(&p.provider),
                    source: cred.source().into(),
                    code: "unavailable".into(),
                });
                continue;
            }
        };
        for key in inject_keys_for_provider(&p.provider) {
            out.insert(
                (*key).to_string(),
                Zeroizing::new(value.as_str().to_string()),
            );
        }
        // Also set LOCUS_<PROVIDER>_RESOLVED=1 (not the secret) for debugging
        let flag = format!("LOCUS_{}_CREDENTIAL_RESOLVED", p.provider.to_uppercase());
        // Don't put secrets in out under flag — use empty marker via separate path
        let _ = flag;
    }
    ResolvedBindingSecrets { env: out, issues }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_formats() {
        assert_eq!(
            CredentialRef::parse("phm:FOO"),
            CredentialRef::Phantom { name: "FOO".into() }
        );
        assert_eq!(
            CredentialRef::parse("env:BAR"),
            CredentialRef::Env { var: "BAR".into() }
        );
        assert!(matches!(
            CredentialRef::parse("BARE"),
            CredentialRef::Unknown { .. }
        ));
    }

    #[test]
    fn validation_rejects_bare_tokens_and_bad_explicit_refs_without_echoing_them() {
        for raw in ["sk_live_canary_secret", "ghp_canary_secret", "oauth:token"] {
            let err = CredentialRef::validate(raw).unwrap_err().to_string();
            assert!(!err.contains(raw), "validation error leaked candidate ref");
        }
        for raw in ["phm:", "env:NOT-A-VAR", " env:GOOD"] {
            assert!(CredentialRef::validate(raw).is_err());
        }
    }

    #[test]
    fn metadata_never_contains_reference_name_or_value() {
        let metadata = credential_metadata("phm:TOP_SECRET_CANARY");
        let json = serde_json::to_string(&metadata).unwrap();
        assert_eq!(metadata.source, "phantom");
        assert!(metadata.present);
        assert!(!json.contains("TOP_SECRET_CANARY"));
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

        assert!(CredentialRef::validate("test:tval").is_ok());
        let v = resolve(&CredentialRef::Test {
            value: "tval".into(),
        })
        .unwrap();
        assert_eq!(v.as_str(), "tval");
    }

    #[test]
    fn legacy_migration_accepts_only_conservative_bare_names() {
        assert_eq!(
            migrate_legacy_phantom_ref("GH_TOKEN_ACME").as_deref(),
            Some("phm:GH_TOKEN_ACME")
        );
        for unsafe_raw in [
            "ghp_secret_value",
            "ghp_secret/value",
            " name",
            "name:other",
            "name\nnext",
        ] {
            assert!(migrate_legacy_phantom_ref(unsafe_raw).is_none());
        }
    }

    #[test]
    fn inject_keys() {
        assert!(inject_keys_for_provider("supabase").contains(&"SUPABASE_ACCESS_TOKEN"));
        assert!(inject_keys_for_provider("github").contains(&"GH_TOKEN"));
    }
}

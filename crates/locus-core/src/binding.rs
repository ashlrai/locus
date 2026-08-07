//! Binding — the atomic unit of authority.
//!
//! principal × tenant × providers × credential_refs × policy

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Least-privilege scope for a single provider account.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Scope {
    /// Supabase project ref, Vercel project id, etc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_ref: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_id: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub orgs: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repos: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,

    /// Catch-all for provider-specific keys without schema churn.
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

/// One provider account inside a Binding.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderBinding {
    pub provider: String,
    pub account: String,
    /// Opaque pointer: `phm:NAME`, `keychain:…`, `env:VAR` — never the secret.
    pub credential_ref: String,
    #[serde(default)]
    pub scope: Scope,
}

/// Session / tool-call policy for a Binding.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Policy {
    #[serde(default = "default_allow")]
    pub default: String, // "allow" | "deny"

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub require_approval: Vec<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_ttl: Option<String>,

    #[serde(default = "default_parallel")]
    pub parallel_sessions: u32,
}

fn default_allow() -> String {
    "allow".into()
}
fn default_parallel() -> u32 {
    4
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            default: default_allow(),
            require_approval: Vec::new(),
            max_ttl: Some("8h".into()),
            parallel_sessions: default_parallel(),
        }
    }
}

/// Human-editable binding body (inside `[binding]` or file root).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BindingBody {
    pub id: String,
    pub alias: String,
    pub tenant: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub policy: Policy,
    #[serde(default)]
    pub providers: Vec<ProviderBinding>,
}

/// Resolved binding used at runtime.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Binding {
    pub id: String,
    pub alias: String,
    pub tenant: String,
    pub principal: Option<String>,
    pub description: Option<String>,
    pub policy: Policy,
    pub providers: Vec<ProviderBinding>,
}

impl Binding {
    pub fn from_body(b: BindingBody) -> Self {
        Self {
            id: b.id,
            alias: b.alias,
            tenant: b.tenant,
            principal: b.principal,
            description: b.description,
            policy: b.policy,
            providers: b.providers,
        }
    }

    pub fn parse_toml(s: &str) -> crate::Result<Self> {
        // Prefer structured [binding] form
        #[derive(Deserialize)]
        struct Wrapped {
            binding: BindingBody,
        }
        if let Ok(w) = toml::from_str::<Wrapped>(s) {
            return Ok(Self::from_body(w.binding));
        }

        // Flat form: top-level fields
        let body: BindingBody = toml::from_str(s)?;
        Ok(Self::from_body(body))
    }

    pub fn to_toml(&self) -> crate::Result<String> {
        #[derive(Serialize)]
        struct Wrapped<'a> {
            binding: &'a BindingBody,
        }
        let body = BindingBody {
            id: self.id.clone(),
            alias: self.alias.clone(),
            tenant: self.tenant.clone(),
            principal: self.principal.clone(),
            description: self.description.clone(),
            policy: self.policy.clone(),
            providers: self.providers.clone(),
        };
        Ok(toml::to_string_pretty(&Wrapped { binding: &body })?)
    }

    pub fn provider(&self, name: &str) -> Option<&ProviderBinding> {
        self.providers
            .iter()
            .find(|p| p.provider.eq_ignore_ascii_case(name))
    }

    pub fn provider_names(&self) -> Vec<&str> {
        self.providers.iter().map(|p| p.provider.as_str()).collect()
    }

    /// Credential refs visible under this binding only (isolation surface).
    pub fn credential_refs(&self) -> Vec<&str> {
        self.providers
            .iter()
            .map(|p| p.credential_ref.as_str())
            .collect()
    }

    pub fn validate(&self) -> crate::Result<()> {
        if self.id.is_empty() || self.alias.is_empty() || self.tenant.is_empty() {
            return Err(crate::LocusError::msg(
                "binding id, alias, and tenant are required",
            ));
        }
        if !self
            .alias
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(crate::LocusError::msg(format!(
                "invalid alias '{}': use letters, digits, '-', '_'",
                self.alias
            )));
        }
        if self.providers.is_empty() {
            return Err(crate::LocusError::msg(
                "binding must declare at least one provider",
            ));
        }
        for p in &self.providers {
            if p.provider.is_empty() || p.account.is_empty() || p.credential_ref.is_empty() {
                return Err(crate::LocusError::msg(
                    "each provider needs provider, account, and credential_ref",
                ));
            }
        }
        Ok(())
    }
}

/// Summary for list/status (never includes secrets).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindingSummary {
    pub id: String,
    pub alias: String,
    pub tenant: String,
    pub providers: Vec<String>,
    pub description: Option<String>,
}

impl From<&Binding> for BindingSummary {
    fn from(b: &Binding) -> Self {
        Self {
            id: b.id.clone(),
            alias: b.alias.clone(),
            tenant: b.tenant.clone(),
            providers: b.providers.iter().map(|p| p.provider.clone()).collect(),
            description: b.description.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[binding]
id = "bnd_acme"
alias = "acme"
tenant = "acme-corp"
description = "Acme client"

[binding.policy]
default = "allow"
max_ttl = "8h"

[[binding.providers]]
provider = "supabase"
account = "acme-prod"
credential_ref = "phm:SUPABASE_ACME"
scope = { project_ref = "abcdefghij", read_only = true }

[[binding.providers]]
provider = "github"
account = "acme-corp"
credential_ref = "phm:GH_TOKEN_ACME"
scope = { orgs = ["acme-corp"] }
"#;

    #[test]
    fn parse_sample_binding() {
        let b = Binding::parse_toml(SAMPLE).unwrap();
        assert_eq!(b.alias, "acme");
        assert_eq!(b.tenant, "acme-corp");
        assert_eq!(b.providers.len(), 2);
        assert_eq!(
            b.provider("supabase").unwrap().scope.project_ref.as_deref(),
            Some("abcdefghij")
        );
        b.validate().unwrap();
    }

    #[test]
    fn roundtrip_toml() {
        let b = Binding::parse_toml(SAMPLE).unwrap();
        let s = b.to_toml().unwrap();
        let b2 = Binding::parse_toml(&s).unwrap();
        assert_eq!(b, b2);
    }
}

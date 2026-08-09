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

/// Upstream MCP stdio spawn for a provider (auto-spawn when pinned).
///
/// TOML (inline, preferred):
/// ```toml
/// [[binding.providers]]
/// provider = "github"
/// account = "acme"
/// credential_ref = "phm:GH_TOKEN_ACME"
/// upstream = { command = "npx", args = ["-y", "@pkg"] }
/// ```
///
/// Nested table (applies to the most recent `[[binding.providers]]` entry):
/// ```toml
/// [binding.providers.upstream]
/// command = "python3"
/// args = ["-u", "-c", "..."]
/// resolve_secrets = true
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct UpstreamSpec {
    /// Executable (e.g. `npx`, `python3`, path to MCP binary).
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Resolve `phm:` / `env:` credential_refs into the child env when spawning.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub resolve_secrets: bool,
    /// Explicit acknowledgement that this child runs as the current OS user.
    ///
    /// Host execution is disabled by default because a same-user process can
    /// read Locus control-plane files, including `LOCUS_HOME/daemon.key`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unsafe_host_execution: bool,
    /// Closed tool/argument manifest enforced before forwarding to upstream.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub capabilities: BTreeMap<String, UpstreamToolCapability>,
}

/// One explicitly admitted upstream tool and its top-level argument semantics.
///
/// Argument values are `passthrough`, `account`, or a frozen scope source:
/// `scope.project_ref`, `scope.team_id`, `scope.account_id`, `scope.orgs`,
/// `scope.repos`, `scope.projects`, `scope.env`. Any undeclared tool, argument,
/// or semantic is denied before the worker receives a JSON-RPC call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct UpstreamToolCapability {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub arguments: BTreeMap<String, String>,
}

impl UpstreamToolCapability {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_argument(mut self, name: impl Into<String>, semantics: impl Into<String>) -> Self {
        self.arguments.insert(name.into(), semantics.into());
        self
    }
}

impl UpstreamSpec {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            args: Vec::new(),
            resolve_secrets: false,
            unsafe_host_execution: false,
            capabilities: BTreeMap::new(),
        }
    }

    pub fn with_args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    pub fn resolve_secrets(mut self, yes: bool) -> Self {
        self.resolve_secrets = yes;
        self
    }

    pub fn unsafe_host_execution(mut self, yes: bool) -> Self {
        self.unsafe_host_execution = yes;
        self
    }

    pub fn with_capability(
        mut self,
        tool: impl Into<String>,
        capability: UpstreamToolCapability,
    ) -> Self {
        self.capabilities.insert(tool.into(), capability);
        self
    }

    pub fn validate(&self) -> crate::Result<()> {
        if self.command.trim().is_empty() {
            return Err(crate::LocusError::msg(
                "provider.upstream.command must be non-empty when upstream is set",
            ));
        }
        if self.capabilities.is_empty() {
            return Err(crate::LocusError::msg(
                "provider.upstream.capabilities must explicitly declare every admitted tool",
            ));
        }
        for (tool, capability) in &self.capabilities {
            if tool.trim().is_empty() {
                return Err(crate::LocusError::msg(
                    "provider.upstream.capabilities tool names must be non-empty",
                ));
            }
            for (argument, semantics) in &capability.arguments {
                if argument.trim().is_empty() {
                    return Err(crate::LocusError::msg(format!(
                        "provider.upstream.capabilities.{tool} contains an empty argument name"
                    )));
                }
                if !matches!(
                    semantics.as_str(),
                    "passthrough"
                        | "account"
                        | "scope.project_ref"
                        | "scope.team_id"
                        | "scope.account_id"
                        | "scope.orgs"
                        | "scope.repos"
                        | "scope.projects"
                        | "scope.env"
                ) {
                    return Err(crate::LocusError::msg(format!(
                        "provider.upstream.capabilities.{tool}.{argument} has unknown semantics `{semantics}`"
                    )));
                }
            }
        }
        Ok(())
    }
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
    /// Optional upstream MCP server to auto-spawn for this provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<UpstreamSpec>,
}

impl ProviderBinding {
    /// Construct a provider binding without upstream (synthetic tools only).
    pub fn new(
        provider: impl Into<String>,
        account: impl Into<String>,
        credential_ref: impl Into<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            account: account.into(),
            credential_ref: credential_ref.into(),
            scope: Scope::default(),
            upstream: None,
        }
    }

    pub fn with_scope(mut self, scope: Scope) -> Self {
        self.scope = scope;
        self
    }

    pub fn with_upstream(mut self, upstream: UpstreamSpec) -> Self {
        self.upstream = Some(upstream);
        self
    }

    /// True when this provider should spawn an MCP stdio worker.
    pub fn has_upstream(&self) -> bool {
        self.upstream
            .as_ref()
            .is_some_and(|u| !u.command.is_empty())
    }
}

/// One ordered structured policy rule (`[[binding.policy.rules]]`).
///
/// First matching rule wins during evaluation (see [`crate::policy::evaluate`]).
///
/// ```toml
/// [[binding.policy.rules]]
/// match = "supabase.*"
/// action = "allow"
/// [[binding.policy.rules]]
/// match = "*.delete*"
/// action = "require_approval"
/// [[binding.policy.rules]]
/// match = "vercel.deploy.prod"
/// action = "dual_control"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PolicyRule {
    /// Tool-name glob (`*` greedy). Field name in TOML/JSON is `match`.
    #[serde(rename = "match")]
    pub match_glob: String,
    /// `allow` | `deny` | `require_approval` | `dual_control`
    pub action: String,
}

impl PolicyRule {
    pub fn new(match_glob: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            match_glob: match_glob.into(),
            action: action.into(),
        }
    }
}

/// Session / tool-call policy for a Binding.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Policy {
    #[serde(default = "default_allow")]
    pub default: String, // "allow" | "deny"

    /// Ordered structured rules. First match wins before legacy globs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<PolicyRule>,

    /// Legacy: tool globs that require a human grant before execution.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub require_approval: Vec<String>,

    /// Legacy: tool globs that need two distinct principal grants before approval.
    ///
    /// Matching tools also go through the require_approval gate even if they
    /// are not listed in `require_approval`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dual_control: Vec<String>,

    /// When true, every tool matched by `require_approval` (legacy globs or
    /// structured `require_approval` rules) needs dual-control (two distinct
    /// principals). Combined with explicit `dual_control` globs/rules.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dual_control_all_approvals: bool,

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
            rules: Vec::new(),
            require_approval: Vec::new(),
            dual_control: Vec::new(),
            dual_control_all_approvals: false,
            max_ttl: Some("8h".into()),
            parallel_sessions: default_parallel(),
        }
    }
}

impl Policy {
    /// Whether this tool needs two distinct principal grants.
    ///
    /// True when:
    /// - a structured rule with `action = "dual_control"` matches, or
    /// - a legacy `dual_control` glob matches, or
    /// - `dual_control_all_approvals` and the tool is gated by
    ///   `require_approval` (legacy glob or structured rule).
    pub fn requires_dual_control(&self, tool: &str) -> bool {
        use crate::policy::glob_match;

        // Structured dual_control rules (any match — dual is additive even if
        // a higher-priority rule already decided require_approval).
        for rule in &self.rules {
            if rule.action.eq_ignore_ascii_case("dual_control")
                && glob_match(&rule.match_glob, tool)
            {
                return true;
            }
        }

        if self.dual_control_all_approvals {
            for pat in &self.require_approval {
                if glob_match(pat, tool) {
                    return true;
                }
            }
            for rule in &self.rules {
                if rule.action.eq_ignore_ascii_case("require_approval")
                    && glob_match(&rule.match_glob, tool)
                {
                    return true;
                }
            }
        }
        for pat in &self.dual_control {
            if glob_match(pat, tool) {
                return true;
            }
        }
        false
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
        validate_name_component("alias", &self.alias)?;
        // id is also used in paths / seals — keep to the same charset
        validate_name_component("id", &self.id)?;
        if let Some(ref p) = self.principal {
            validate_name_component("principal", p)?;
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
            if let Some(up) = &p.upstream {
                up.validate()?;
            }
        }
        Ok(())
    }
}

/// Safe name for path components and identity labels: ASCII alnum + `-` `_`.
/// Rejects empty, path separators, `..`, and other punctuation (prompt / path injection).
pub fn validate_name_component(field: &str, value: &str) -> crate::Result<()> {
    if value.is_empty() {
        return Err(crate::LocusError::msg(format!(
            "invalid {field}: must not be empty"
        )));
    }
    if value.contains('/') || value.contains('\\') || value.contains("..") || value.contains('\0') {
        return Err(crate::LocusError::msg(format!(
            "invalid {field} '{value}': path separators and '..' are not allowed"
        )));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(crate::LocusError::msg(format!(
            "invalid {field} '{value}': use letters, digits, '-', '_'"
        )));
    }
    if value.len() > 128 {
        return Err(crate::LocusError::msg(format!(
            "invalid {field}: exceeds maximum length (128)"
        )));
    }
    Ok(())
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

    const SAMPLE_WITH_UPSTREAM: &str = r#"
[binding]
id = "bnd_acme"
alias = "acme"
tenant = "acme-corp"

[[binding.providers]]
provider = "github"
account = "acme"
credential_ref = "phm:GH_TOKEN_ACME"
scope = { orgs = ["acme-corp"] }
upstream = { command = "npx", args = ["-y", "@github/mcp"], resolve_secrets = true, capabilities = { ping = { arguments = {} } } }

[[binding.providers]]
provider = "supabase"
account = "acme-prod"
credential_ref = "phm:SUPABASE_ACME"
scope = { project_ref = "abcdefghij", read_only = true }
"#;

    #[test]
    fn parse_upstream_inline() {
        let b = Binding::parse_toml(SAMPLE_WITH_UPSTREAM).unwrap();
        b.validate().unwrap();
        let gh = b.provider("github").unwrap();
        assert!(gh.has_upstream());
        let up = gh.upstream.as_ref().unwrap();
        assert_eq!(up.command, "npx");
        assert_eq!(up.args, vec!["-y", "@github/mcp"]);
        assert!(up.resolve_secrets);
        assert!(!up.unsafe_host_execution);
        assert!(up.capabilities.contains_key("ping"));
        assert!(!b.provider("supabase").unwrap().has_upstream());
    }

    #[test]
    fn parse_upstream_nested_table() {
        let toml = r#"
[binding]
id = "bnd_x"
alias = "x"
tenant = "t"

[[binding.providers]]
provider = "github"
account = "a"
credential_ref = "env:X"
scope = { orgs = ["o"] }

[binding.providers.upstream]
command = "python3"
args = ["-u", "-c", "print(1)"]
resolve_secrets = false

[binding.providers.upstream.capabilities.ping]
arguments = {}
"#;
        let b = Binding::parse_toml(toml).unwrap();
        b.validate().unwrap();
        let up = b.provider("github").unwrap().upstream.as_ref().unwrap();
        assert_eq!(up.command, "python3");
        assert_eq!(up.args.len(), 3);
        assert!(!up.resolve_secrets);
    }

    #[test]
    fn upstream_empty_command_fails_validate() {
        let toml = r#"
[binding]
id = "bnd_x"
alias = "x"
tenant = "t"

[[binding.providers]]
provider = "github"
account = "a"
credential_ref = "env:X"
upstream = { command = "" }
"#;
        let b = Binding::parse_toml(toml).unwrap();
        assert!(b.validate().is_err());
    }

    #[test]
    fn upstream_requires_closed_capability_manifest() {
        let mut b = Binding::parse_toml(SAMPLE_WITH_UPSTREAM).unwrap();
        b.providers[0]
            .upstream
            .as_mut()
            .unwrap()
            .capabilities
            .clear();
        let error = b.validate().unwrap_err().to_string();
        assert!(error.contains("capabilities"), "unexpected: {error}");

        b.providers[0]
            .upstream
            .as_mut()
            .unwrap()
            .capabilities
            .insert(
                "ping".into(),
                UpstreamToolCapability::new().with_argument("target", "guessed_selector"),
            );
        let error = b.validate().unwrap_err().to_string();
        assert!(error.contains("unknown semantics"), "unexpected: {error}");
    }

    #[test]
    fn roundtrip_preserves_upstream() {
        let b = Binding::parse_toml(SAMPLE_WITH_UPSTREAM).unwrap();
        let s = b.to_toml().unwrap();
        let b2 = Binding::parse_toml(&s).unwrap();
        assert_eq!(b, b2);
        assert!(b2.provider("github").unwrap().has_upstream());
    }

    #[test]
    fn upstream_example_is_valid() {
        let binding = Binding::parse_toml(include_str!("../../../examples/upstream.binding.toml"))
            .expect("parse examples/upstream.binding.toml");
        binding
            .validate()
            .expect("validate examples/upstream.binding.toml");
    }

    #[test]
    fn validate_rejects_empty_providers() {
        let raw = r#"
[binding]
id = "bnd_empty"
alias = "empty"
tenant = "t"
providers = []
"#;
        let b = Binding::parse_toml(raw).unwrap();
        let err = b.validate().unwrap_err().to_string();
        assert!(err.contains("at least one provider"), "unexpected: {err}");
    }

    #[test]
    fn validate_rejects_bad_alias() {
        for alias in ["has space", "bad!", "slash/x", "dot.x", ""] {
            let body = BindingBody {
                id: "bnd_x".into(),
                alias: alias.into(),
                tenant: "t".into(),
                principal: None,
                description: None,
                policy: Policy::default(),
                providers: vec![ProviderBinding {
                    provider: "github".into(),
                    account: "a".into(),
                    credential_ref: "phm:X".into(),
                    scope: Scope::default(),
                    upstream: None,
                }],
            };
            let b = Binding::from_body(body);
            let err = b.validate().unwrap_err().to_string();
            if alias.is_empty() {
                assert!(
                    err.contains("required") || err.contains("alias"),
                    "empty alias: {err}"
                );
            } else {
                assert!(
                    err.contains("invalid alias") || err.contains(alias),
                    "alias={alias:?}: {err}"
                );
            }
        }
    }

    #[test]
    fn parse_toml_malformed_is_error() {
        let err = Binding::parse_toml("not = [valid").unwrap_err();
        let msg = err.to_string();
        assert!(!msg.is_empty());
    }

    #[test]
    fn validate_rejects_incomplete_provider() {
        let raw = r#"
[binding]
id = "bnd_bad"
alias = "badprov"
tenant = "t"

[[binding.providers]]
provider = "github"
account = ""
credential_ref = "phm:X"
"#;
        let b = Binding::parse_toml(raw).unwrap();
        let err = b.validate().unwrap_err().to_string();
        assert!(
            err.contains("provider") || err.contains("account") || err.contains("credential_ref"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn validate_rejects_bad_principal_and_alias() {
        let mut b = Binding::parse_toml(SAMPLE).unwrap();
        b.principal = Some("../evil".into());
        assert!(b.validate().is_err());
        b.principal = Some("mason".into());
        b.validate().unwrap();
        b.alias = "../escape".into();
        assert!(b.validate().is_err());
        b.alias = "acme".into();
        b.principal = Some("user name".into());
        assert!(b.validate().is_err());
    }

    #[test]
    fn validate_name_component_charset() {
        assert!(validate_name_component("alias", "acme").is_ok());
        assert!(validate_name_component("principal", "mason_1").is_ok());
        assert!(validate_name_component("alias", "../x").is_err());
        assert!(validate_name_component("alias", "a/b").is_err());
        assert!(validate_name_component("principal", "").is_err());
    }
}

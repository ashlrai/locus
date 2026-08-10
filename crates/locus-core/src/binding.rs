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
/// Built-in recipe (expands to command/args — see `locus upstream list`):
/// ```toml
/// upstream = { recipe = "github-mcp", resolve_secrets = true }
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
    /// Optional built-in recipe id (e.g. `github-mcp`, `filesystem-mcp`).
    /// When set and `command` is empty, Locus fills command/args from the
    /// recipe table (`adapters/recipes.toml`). Explicit `command` / `args`
    /// override the recipe defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe: Option<String>,
    /// Executable (e.g. `npx`, `python3`, path to MCP binary).
    /// Optional when `recipe` is set — filled by [`Self::expand`].
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Resolve `phm:` / `env:` credential_refs into the child env when spawning.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub resolve_secrets: bool,
    /// Best-effort worker sandbox: restricted PATH + `LOCUS_WORKER_SANDBOXED=1`.
    /// Also enabled globally via `LOCUS_WORKER_SANDBOX=1`. See docs/workers.md.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sandbox: bool,
}

impl UpstreamSpec {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            recipe: None,
            command: command.into(),
            args: Vec::new(),
            resolve_secrets: false,
            sandbox: false,
        }
    }

    /// Spec that expands a built-in recipe at validate / spawn time.
    pub fn from_recipe(recipe: impl Into<String>) -> Self {
        Self {
            recipe: Some(recipe.into()),
            command: String::new(),
            args: Vec::new(),
            resolve_secrets: false,
            sandbox: false,
        }
    }

    pub fn with_args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_recipe(mut self, recipe: impl Into<String>) -> Self {
        self.recipe = Some(recipe.into());
        self
    }

    pub fn resolve_secrets(mut self, yes: bool) -> Self {
        self.resolve_secrets = yes;
        self
    }

    /// Enable best-effort sandbox (restricted PATH + marker env).
    pub fn sandbox(mut self, yes: bool) -> Self {
        self.sandbox = yes;
        self
    }

    /// True when this table declares an upstream (recipe and/or command).
    pub fn is_declared(&self) -> bool {
        !self.command.trim().is_empty()
            || self.recipe.as_ref().is_some_and(|r| !r.trim().is_empty())
    }

    /// Expand a recipe into concrete `command` / `args` when needed.
    ///
    /// - No recipe → requires non-empty `command`.
    /// - Recipe set → look up builtins; empty `command`/`args` take recipe
    ///   defaults; non-empty fields win (full override, not merge).
    /// - Pure recipe path (empty command *and* empty args): adopt
    ///   `recipe.default_resolve_secrets` / `recipe.default_sandbox` via OR
    ///   (`binding_flag || recipe.default_*`). Explicit command/args keep
    ///   the binding's flags as written.
    ///
    /// Note: TOML bool default is false, so omitted flags mean false until
    /// pure-recipe expand. CLI snippets include recommended flags.
    pub fn expand(&self) -> crate::Result<Self> {
        let recipe_name = self
            .recipe
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty());

        let Some(name) = recipe_name else {
            if self.command.trim().is_empty() {
                return Err(crate::LocusError::msg(
                    "provider.upstream.command must be non-empty when upstream is set (or set recipe)",
                ));
            }
            return Ok(self.clone());
        };

        let recipe = crate::recipes::get_recipe(name)?;
        let command = if self.command.trim().is_empty() {
            recipe.command.clone()
        } else {
            self.command.clone()
        };
        let args = if self.args.is_empty() {
            recipe.args.clone()
        } else {
            self.args.clone()
        };
        if command.trim().is_empty() {
            return Err(crate::LocusError::msg(format!(
                "upstream recipe `{name}` resolved to an empty command"
            )));
        }
        // Pure recipe path: adopt recommended resolve_secrets / sandbox when
        // the binding did not set command (recipe-only). If the user wrote an
        // explicit command, keep flags as given.
        let pure_recipe = self.command.trim().is_empty() && self.args.is_empty();
        let resolve_secrets = if pure_recipe {
            self.resolve_secrets || recipe.default_resolve_secrets
        } else {
            self.resolve_secrets
        };
        let sandbox = if pure_recipe {
            self.sandbox || recipe.default_sandbox
        } else {
            self.sandbox
        };

        Ok(Self {
            recipe: self.recipe.clone(),
            command,
            args,
            resolve_secrets,
            sandbox,
        })
    }

    pub fn validate(&self) -> crate::Result<()> {
        let expanded = self.expand()?;
        if expanded.command.trim().is_empty() {
            return Err(crate::LocusError::msg(
                "provider.upstream.command must be non-empty when upstream is set",
            ));
        }
        Ok(())
    }
}

/// One provider account inside a Binding.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderBinding {
    pub provider: String,
    pub account: String,
    /// Explicit pointer: `phm:NAME` or `env:VAR` — never the secret.
    /// `test:` is accepted only when this crate is compiled for unit tests.
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
        self.upstream.as_ref().is_some_and(|u| u.is_declared())
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
            crate::credential::CredentialRef::validate(&p.credential_ref)?;
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
upstream = { command = "npx", args = ["-y", "@github/mcp"], resolve_secrets = true }

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
        assert!(!b.provider("supabase").unwrap().has_upstream());
    }

    #[test]
    fn parse_upstream_recipe_only() {
        let toml = r#"
[binding]
id = "bnd_x"
alias = "x"
tenant = "t"

[[binding.providers]]
provider = "github"
account = "a"
credential_ref = "phm:GH"
upstream = { recipe = "github-mcp" }
"#;
        let b = Binding::parse_toml(toml).unwrap();
        b.validate().unwrap();
        let up = b.provider("github").unwrap().upstream.as_ref().unwrap();
        assert_eq!(up.recipe.as_deref(), Some("github-mcp"));
        assert!(up.command.is_empty(), "raw parse keeps command empty");
        let expanded = up.expand().unwrap();
        assert_eq!(expanded.command, "npx");
        assert!(expanded.args.iter().any(|a| a.contains("server-github")));
        assert!(expanded.resolve_secrets, "recipe default_resolve_secrets");
        assert!(expanded.sandbox, "recipe default_sandbox on real providers");
        assert!(b.provider("github").unwrap().has_upstream());
    }

    #[test]
    fn expand_pure_recipe_adopts_default_sandbox() {
        // Real provider recipe: default_sandbox = true → pure expand enables it.
        let up = UpstreamSpec::from_recipe("github-mcp");
        assert!(!up.sandbox, "raw from_recipe starts sandbox-off");
        let expanded = up.expand().unwrap();
        assert!(expanded.sandbox, "pure recipe adopts default_sandbox");
        assert!(expanded.resolve_secrets);

        // Explicit sandbox = true still true after expand.
        let forced = UpstreamSpec::from_recipe("github-mcp").sandbox(true);
        assert!(forced.expand().unwrap().sandbox);

        // Demo recipe stays sandbox-off unless the binding forces it.
        let demo = UpstreamSpec::from_recipe("everything-mcp");
        assert!(!demo.expand().unwrap().sandbox);
        let demo_on = UpstreamSpec::from_recipe("everything-mcp").sandbox(true);
        assert!(demo_on.expand().unwrap().sandbox);

        // Explicit command path does not auto-adopt recipe defaults.
        let override_cmd = UpstreamSpec {
            recipe: Some("github-mcp".into()),
            command: "custom-mcp".into(),
            args: vec!["--flag".into()],
            resolve_secrets: false,
            sandbox: false,
        };
        let expanded = override_cmd.expand().unwrap();
        assert_eq!(expanded.command, "custom-mcp");
        assert!(
            !expanded.sandbox,
            "explicit command keeps sandbox as written"
        );
        assert!(
            !expanded.resolve_secrets,
            "explicit command keeps resolve_secrets as written"
        );
    }

    #[test]
    fn recipe_args_override() {
        let up = UpstreamSpec::from_recipe("filesystem-mcp").with_args([
            "-y",
            "@modelcontextprotocol/server-filesystem",
            "/tmp/locus-demo",
        ]);
        let expanded = up.expand().unwrap();
        assert_eq!(expanded.command, "npx");
        assert_eq!(
            expanded.args,
            vec![
                "-y",
                "@modelcontextprotocol/server-filesystem",
                "/tmp/locus-demo"
            ]
        );
    }

    #[test]
    fn unknown_recipe_fails_validate() {
        let toml = r#"
[binding]
id = "bnd_x"
alias = "x"
tenant = "t"

[[binding.providers]]
provider = "github"
account = "a"
credential_ref = "env:X"
upstream = { recipe = "not-a-real-recipe" }
"#;
        let b = Binding::parse_toml(toml).unwrap();
        let err = b.validate().unwrap_err().to_string();
        assert!(err.contains("unknown") || err.contains("recipe"), "{err}");
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
    fn roundtrip_preserves_upstream() {
        let b = Binding::parse_toml(SAMPLE_WITH_UPSTREAM).unwrap();
        let s = b.to_toml().unwrap();
        let b2 = Binding::parse_toml(&s).unwrap();
        assert_eq!(b, b2);
        assert!(b2.provider("github").unwrap().has_upstream());
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
    fn validate_rejects_raw_and_unsupported_credential_refs() {
        let mut b = Binding::parse_toml(SAMPLE).unwrap();
        for raw in [
            "ghp_raw_token_canary",
            "sk-live-raw-token-canary",
            "oauth:provider:token",
        ] {
            b.providers[0].credential_ref = raw.into();
            let err = b.validate().unwrap_err().to_string();
            assert!(!err.contains(raw), "validation error leaked candidate ref");
        }
        for raw in ["phm:", "env:BAD-NAME"] {
            b.providers[0].credential_ref = raw.into();
            assert!(b.validate().is_err());
        }
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

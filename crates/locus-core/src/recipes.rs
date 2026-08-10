//! Built-in upstream MCP recipes.
//!
//! Bindings can set `upstream = { recipe = "github-mcp" }` instead of
//! hand-writing `command` / `args`. See [`adapters/recipes.toml`] at the
//! repo root (embedded at compile time).

use crate::error::{LocusError, Result};
use serde::{Deserialize, Serialize};

/// Embedded recipes table (repo `adapters/recipes.toml`).
const RECIPES_TOML: &str = include_str!("../../../adapters/recipes.toml");

/// Whether the recipe can operate inside the current OS sandbox contract.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SandboxCompatibility {
    /// The command needs only the documented read/write/outbound grants.
    #[default]
    Compatible,
    /// The command needs authority the sandbox intentionally refuses.
    Incompatible,
}

impl SandboxCompatibility {
    /// Stable machine/display value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Compatible => "compatible",
            Self::Incompatible => "incompatible",
        }
    }
}

/// Machine-readable activation state for a built-in recipe.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RecipeReadiness {
    /// Recipe defaults are directly usable.
    #[default]
    Ready,
    /// Binding must explicitly set `sandbox = false` to acknowledge risk.
    ExplicitUnsandboxedRequired,
}

impl RecipeReadiness {
    /// Stable machine/display value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::ExplicitUnsandboxedRequired => "explicit_unsandboxed_required",
        }
    }
}

/// One known upstream MCP spawn recipe.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpstreamRecipe {
    /// Stable id used in binding TOML (`recipe = "…"`).
    pub id: String,
    /// Human title for CLI list.
    #[serde(default)]
    pub title: String,
    /// Provider ids this recipe is suggested for (`github`, `supabase`, …).
    #[serde(default)]
    pub providers: Vec<String>,
    /// Executable after expansion.
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Recommended `resolve_secrets` when the binding omits it.
    #[serde(default)]
    pub default_resolve_secrets: bool,
    /// Recommended required OS sandbox when the binding omits it.
    /// Recipe expansion adopts this even when command or args are overridden.
    /// Demo / local recipes keep this false so offline wiring stays unblocked.
    #[serde(default)]
    pub default_sandbox: bool,
    /// Whether the command can function under the current worker sandbox.
    #[serde(default)]
    pub sandbox_compatibility: SandboxCompatibility,
    /// Whether the recipe is directly ready or requires explicit authority opt-in.
    #[serde(default)]
    pub readiness: RecipeReadiness,
    /// Stable risk codes for machine consumers and readiness UIs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub risks: Vec<String>,
    /// Human diagnostic corresponding to `readiness` and `risks`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub readiness_detail: String,
    /// Env var names the upstream server typically reads (hints only).
    #[serde(default)]
    pub env_hints: Vec<String>,
    /// Operator notes (URL-style remote MCP, security caveats, …).
    #[serde(default)]
    pub notes: String,
}

impl UpstreamRecipe {
    fn validate_policy(&self) -> Result<()> {
        if self.sandbox_compatibility == SandboxCompatibility::Incompatible
            && self.readiness != RecipeReadiness::ExplicitUnsandboxedRequired
        {
            return Err(LocusError::msg(format!(
                "upstream recipe `{}` has incompatible sandbox metadata without an explicit unsandboxed readiness gate",
                self.id
            )));
        }
        if self.readiness == RecipeReadiness::ExplicitUnsandboxedRequired
            && (self.sandbox_compatibility != SandboxCompatibility::Incompatible
                || self.default_sandbox
                || self.risks.is_empty()
                || self.readiness_detail.trim().is_empty())
        {
            return Err(LocusError::msg(format!(
                "upstream recipe `{}` has an incomplete explicit-unsandboxed policy",
                self.id
            )));
        }
        Ok(())
    }

    /// Resolve the binding's tri-state sandbox choice against recipe policy.
    pub fn resolve_sandbox_choice(&self, configured: Option<bool>) -> Result<bool> {
        if self.readiness == RecipeReadiness::ExplicitUnsandboxedRequired {
            return match configured {
                Some(false) => Ok(false),
                Some(true) => Err(LocusError::msg(format!(
                    "upstream recipe `{}` is not compatible with the worker sandbox: {}",
                    self.id, self.readiness_detail
                ))),
                None => Err(LocusError::msg(format!(
                    "upstream recipe `{}` is unavailable by default; set sandbox = false to acknowledge risks [{}]: {}",
                    self.id,
                    self.risks.join(","),
                    self.readiness_detail
                ))),
            };
        }
        Ok(configured.unwrap_or(self.default_sandbox))
    }

    /// Diagnostic retained by worker config so later global sandbox forcing
    /// cannot turn an incompatible recipe into a false sandbox claim.
    pub fn sandbox_incompatibility(&self) -> Option<String> {
        (self.sandbox_compatibility == SandboxCompatibility::Incompatible).then(|| {
            format!(
                "upstream recipe `{}` cannot run in the worker sandbox: {}",
                self.id, self.readiness_detail
            )
        })
    }
}

#[derive(Debug, Deserialize)]
struct RecipesFile {
    #[serde(default)]
    recipes: Vec<UpstreamRecipe>,
}

/// Parse and return all built-in recipes (sorted by id).
pub fn all_recipes() -> Result<Vec<UpstreamRecipe>> {
    let file: RecipesFile = toml::from_str(RECIPES_TOML)
        .map_err(|e| LocusError::msg(format!("builtin recipes.toml parse error: {e}")))?;
    let mut recipes = file.recipes;
    for recipe in &recipes {
        recipe.validate_policy()?;
    }
    recipes.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(recipes)
}

/// Look up a recipe by id (case-insensitive).
pub fn get_recipe(id: &str) -> Result<UpstreamRecipe> {
    let needle = id.trim();
    if needle.is_empty() {
        return Err(LocusError::msg("upstream recipe id must be non-empty"));
    }
    all_recipes()?
        .into_iter()
        .find(|r| r.id.eq_ignore_ascii_case(needle))
        .ok_or_else(|| {
            LocusError::msg(format!(
                "unknown upstream recipe `{needle}` — run `locus upstream list`"
            ))
        })
}

/// Recipes whose `providers` list includes this provider (case-insensitive).
pub fn suggest_for_provider(provider: &str) -> Result<Vec<UpstreamRecipe>> {
    let p = provider.trim();
    if p.is_empty() {
        return Err(LocusError::msg("provider must be non-empty"));
    }
    Ok(all_recipes()?
        .into_iter()
        .filter(|r| {
            r.providers.iter().any(|x| x.eq_ignore_ascii_case(p))
                || r.id.eq_ignore_ascii_case(p)
                || r.id
                    .to_ascii_lowercase()
                    .starts_with(&format!("{}-", p.to_ascii_lowercase()))
        })
        .collect())
}

/// Copy-paste TOML fragment for a recipe (for CLI suggest).
///
/// Compatible provider recipes include `sandbox = true`. Incompatible recipes
/// emit their required explicit `sandbox = false` authority acknowledgement.
pub fn recipe_toml_snippet(recipe: &UpstreamRecipe) -> String {
    let mut line = format!("upstream = {{ recipe = \"{}\"", recipe.id);
    if recipe.default_resolve_secrets {
        line.push_str(", resolve_secrets = true");
    }
    if recipe.readiness == RecipeReadiness::ExplicitUnsandboxedRequired {
        line.push_str(", sandbox = false");
    } else if recipe.default_sandbox || (recipe.default_resolve_secrets && !is_demo_recipe(recipe))
    {
        line.push_str(", sandbox = true");
    }
    line.push_str(" }");
    line
}

/// Demo / offline wiring recipes — never auto-suggest sandbox.
fn is_demo_recipe(recipe: &UpstreamRecipe) -> bool {
    let id = recipe.id.to_ascii_lowercase();
    id.contains("demo")
        || id == "filesystem-mcp"
        || id == "everything-mcp"
        || recipe
            .providers
            .iter()
            .any(|p| matches!(p.to_ascii_lowercase().as_str(), "demo" | "mock" | "local"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_recipes_parse_and_have_core_ids() {
        let all = all_recipes().unwrap();
        assert!(
            all.len() >= 5,
            "expected several recipes, got {}",
            all.len()
        );
        for id in [
            "github-mcp",
            "github-official",
            "supabase-mcp",
            "vercel-mcp",
            "filesystem-mcp",
            "everything-mcp",
        ] {
            assert!(all.iter().any(|r| r.id == id), "missing recipe {id}");
            let r = get_recipe(id).unwrap();
            assert!(!r.command.is_empty(), "{id} command empty");
            assert!(!r.args.is_empty(), "{id} should ship default args");
        }
    }

    #[test]
    fn get_recipe_case_insensitive() {
        let a = get_recipe("GitHub-MCP").unwrap();
        let b = get_recipe("github-mcp").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn get_recipe_unknown_errors() {
        let err = get_recipe("no-such-recipe").unwrap_err().to_string();
        assert!(err.contains("unknown"), "{err}");
    }

    #[test]
    fn suggest_github_returns_mcp_recipes() {
        let s = suggest_for_provider("github").unwrap();
        assert!(s.iter().any(|r| r.id == "github-mcp"));
        assert!(s.iter().any(|r| r.id == "github-official"));
    }

    #[test]
    fn suggest_supabase() {
        let s = suggest_for_provider("supabase").unwrap();
        assert!(s.iter().any(|r| r.id == "supabase-mcp"));
        assert!(s[0].notes.contains("mcp.supabase.com") || s[0].notes.contains("stdio"));
    }

    #[test]
    fn suggest_vercel() {
        let s = suggest_for_provider("vercel").unwrap();
        assert!(
            s.iter().any(|r| r.id == "vercel-mcp"),
            "expected vercel-mcp suggestion, got {:?}",
            s.iter().map(|r| &r.id).collect::<Vec<_>>()
        );
        let r = get_recipe("vercel-mcp").unwrap();
        assert!(
            r.notes.contains("mcp.vercel.com"),
            "vercel recipe must document official remote endpoint"
        );
        assert!(
            r.args.iter().any(|a| a == "mcp-remote"),
            "vercel bridge must use documented mcp-remote package"
        );
        assert!(
            r.args.iter().any(|a| a.contains("mcp.vercel.com")),
            "vercel bridge must target official remote URL"
        );
    }

    #[test]
    fn snippet_includes_resolve_when_default() {
        let r = get_recipe("github-mcp").unwrap();
        let snip = recipe_toml_snippet(&r);
        assert!(snip.contains("recipe = \"github-mcp\""));
        assert!(snip.contains("resolve_secrets = true"));
        assert!(
            snip.contains("sandbox = true"),
            "real provider recipes should suggest sandbox: {snip}"
        );
    }

    #[test]
    fn snippet_vercel_requires_explicit_unsandboxed_acknowledgement() {
        let r = get_recipe("vercel-mcp").unwrap();
        let snip = recipe_toml_snippet(&r);
        assert!(snip.contains("recipe = \"vercel-mcp\""));
        assert!(snip.contains("sandbox = false"), "{snip}");
        // OAuth remote — resolve_secrets not recommended by default
        assert!(
            !snip.contains("resolve_secrets"),
            "vercel snippet must not force resolve_secrets: {snip}"
        );
        assert!(!r.default_resolve_secrets);
        assert!(!r.default_sandbox);
        assert_eq!(r.sandbox_compatibility, SandboxCompatibility::Incompatible);
        assert_eq!(r.readiness, RecipeReadiness::ExplicitUnsandboxedRequired);
        assert!(r.risks.iter().any(|risk| risk == "oauth_loopback_listener"));
    }

    #[test]
    fn snippet_demo_skips_sandbox() {
        let r = get_recipe("everything-mcp").unwrap();
        let snip = recipe_toml_snippet(&r);
        assert!(
            !snip.contains("sandbox"),
            "demo recipe must not force sandbox: {snip}"
        );
        assert!(!r.default_sandbox);
    }

    #[test]
    fn compatible_real_recipes_default_sandbox() {
        for id in ["github-mcp", "supabase-mcp"] {
            let r = get_recipe(id).unwrap();
            assert!(
                r.default_sandbox,
                "{id} should default_sandbox for recipe expansion"
            );
        }
        for id in [
            "github-official",
            "vercel-mcp",
            "filesystem-mcp",
            "everything-mcp",
        ] {
            let r = get_recipe(id).unwrap();
            assert!(!r.default_sandbox, "{id} must not claim sandbox by default");
        }
    }

    #[test]
    fn incompatible_recipes_publish_truthful_machine_risk() {
        let docker = get_recipe("github-official").unwrap();
        assert_eq!(
            docker.sandbox_compatibility,
            SandboxCompatibility::Incompatible
        );
        assert_eq!(
            docker.readiness,
            RecipeReadiness::ExplicitUnsandboxedRequired
        );
        assert!(docker.risks.iter().any(|risk| risk == "host_docker_daemon"));
        assert!(recipe_toml_snippet(&docker).contains("sandbox = false"));

        let vercel = get_recipe("vercel-mcp").unwrap();
        assert_eq!(
            vercel.sandbox_compatibility,
            SandboxCompatibility::Incompatible
        );
        assert!(vercel
            .risks
            .iter()
            .any(|risk| risk == "oauth_loopback_listener"));

        for recipe in [docker, vercel] {
            let value = serde_json::to_value(recipe).unwrap();
            assert_eq!(value["sandbox_compatibility"], "incompatible");
            assert_eq!(value["readiness"], "explicit_unsandboxed_required");
            assert!(value["risks"]
                .as_array()
                .is_some_and(|risks| !risks.is_empty()));
            assert!(value["readiness_detail"]
                .as_str()
                .is_some_and(|detail| !detail.is_empty()));
        }
    }

    #[test]
    fn top_adapters_resolve_secrets_defaults() {
        // Token-injectable stdio recipes must default resolve_secrets on.
        for id in ["github-mcp", "github-official", "supabase-mcp"] {
            let r = get_recipe(id).unwrap();
            assert!(
                r.default_resolve_secrets,
                "{id} should default_resolve_secrets"
            );
            assert!(
                !r.env_hints.is_empty(),
                "{id} should document credential env hints"
            );
        }
        // Vercel official path is remote OAuth — secrets default off.
        let v = get_recipe("vercel-mcp").unwrap();
        assert!(
            !v.default_resolve_secrets,
            "vercel-mcp is OAuth bridge; resolve_secrets off by default"
        );
    }

    #[test]
    fn top_adapter_commands_are_well_known() {
        let gh = get_recipe("github-mcp").unwrap();
        assert_eq!(gh.command, "npx");
        assert!(gh
            .args
            .iter()
            .any(|a| a == "@modelcontextprotocol/server-github"));

        let official = get_recipe("github-official").unwrap();
        assert_eq!(official.command, "docker");
        assert!(official
            .args
            .iter()
            .any(|a| a == "ghcr.io/github/github-mcp-server"));
        assert!(official
            .args
            .iter()
            .any(|a| a == "GITHUB_PERSONAL_ACCESS_TOKEN"));

        let sb = get_recipe("supabase-mcp").unwrap();
        assert_eq!(sb.command, "npx");
        assert!(sb
            .args
            .iter()
            .any(|a| a.starts_with("@supabase/mcp-server-supabase")));
        assert!(sb.args.iter().any(|a| a == "--read-only"));

        let vercel = get_recipe("vercel-mcp").unwrap();
        assert_eq!(vercel.command, "npx");
        assert_eq!(
            vercel.args,
            vec![
                "-y".to_string(),
                "mcp-remote".to_string(),
                "https://mcp.vercel.com".to_string()
            ]
        );
    }

    #[test]
    fn recipe_ids_unique() {
        let all = all_recipes().unwrap();
        let mut seen = std::collections::BTreeSet::new();
        for r in &all {
            assert!(seen.insert(r.id.as_str()), "duplicate recipe id `{}`", r.id);
            assert!(!r.id.trim().is_empty());
            assert!(!r.title.trim().is_empty(), "{} missing title", r.id);
        }
    }
}

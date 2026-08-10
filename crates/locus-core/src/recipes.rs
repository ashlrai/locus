//! Built-in upstream MCP recipes.
//!
//! Bindings can set `upstream = { recipe = "github-mcp" }` instead of
//! hand-writing `command` / `args`. See [`adapters/recipes.toml`] at the
//! repo root (embedded at compile time).

use crate::error::{LocusError, Result};
use serde::{Deserialize, Serialize};

/// Embedded recipes table (repo `adapters/recipes.toml`).
const RECIPES_TOML: &str = include_str!("../../../adapters/recipes.toml");

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
    /// Recommended best-effort worker sandbox when the binding omits it.
    /// Pure-recipe expand adopts this (same pattern as `default_resolve_secrets`).
    /// Demo / local recipes keep this false so offline wiring stays unblocked.
    #[serde(default)]
    pub default_sandbox: bool,
    /// Env var names the upstream server typically reads (hints only).
    #[serde(default)]
    pub env_hints: Vec<String>,
    /// Operator notes (URL-style remote MCP, security caveats, …).
    #[serde(default)]
    pub notes: String,
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
/// Non-demo recipes include `sandbox = true` when `default_sandbox` is set
/// (or when secrets are recommended — real provider recipes), so operators
/// copy a hardened binding by default. Demo recipes stay sandbox-off.
pub fn recipe_toml_snippet(recipe: &UpstreamRecipe) -> String {
    let mut line = format!("upstream = {{ recipe = \"{}\"", recipe.id);
    if recipe.default_resolve_secrets {
        line.push_str(", resolve_secrets = true");
    }
    // Prefer explicit sandbox when the recipe defaults to it, or for real
    // provider recipes (secrets-on) so snippets stay hardened even if
    // default_sandbox is still false in recipes.toml.
    if recipe.default_sandbox || (recipe.default_resolve_secrets && !is_demo_recipe(recipe)) {
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
            all.len() >= 4,
            "expected several recipes, got {}",
            all.len()
        );
        for id in [
            "github-mcp",
            "github-official",
            "supabase-mcp",
            "filesystem-mcp",
            "everything-mcp",
        ] {
            assert!(all.iter().any(|r| r.id == id), "missing recipe {id}");
            let r = get_recipe(id).unwrap();
            assert!(!r.command.is_empty(), "{id} command empty");
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
    fn real_recipes_default_sandbox() {
        for id in ["github-mcp", "github-official", "supabase-mcp"] {
            let r = get_recipe(id).unwrap();
            assert!(
                r.default_sandbox,
                "{id} should default_sandbox for pure-recipe expand"
            );
        }
        for id in ["filesystem-mcp", "everything-mcp"] {
            let r = get_recipe(id).unwrap();
            assert!(!r.default_sandbox, "{id} demo must stay sandbox-off");
        }
    }
}

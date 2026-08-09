use super::{AdapterTool, ProviderAdapter, ToolCallResult};
use crate::binding::{Binding, ProviderBinding};
use crate::error::{LocusError, Result};
use serde_json::{json, Value};

pub struct GithubAdapter;

/// Parse `owner/repo` from args (`full_name`, or `org`/`owner` + `repo`).
fn parse_repo_ref(args: &Value) -> Result<(String, String, String)> {
    if let Some(full) = args.get("full_name").and_then(|v| v.as_str()) {
        let full = full.trim().trim_start_matches("https://github.com/");
        let full = full.trim_start_matches("http://github.com/");
        let full = full.trim_end_matches('/').trim_end_matches(".git");
        let mut parts = full.splitn(2, '/');
        let owner = parts.next().unwrap_or("").trim();
        let repo = parts.next().unwrap_or("").trim();
        if owner.is_empty() || repo.is_empty() {
            return Err(LocusError::msg(
                "github.check_repo: full_name must be `owner/repo`",
            ));
        }
        return Ok((owner.into(), repo.into(), format!("{owner}/{repo}")));
    }

    let owner = args
        .get("org")
        .or_else(|| args.get("owner"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let repo = args
        .get("repo")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());

    match (owner, repo) {
        (Some(o), Some(r)) => {
            // Allow repo as "owner/name" if org omitted-style passed in repo alone.
            if r.contains('/') {
                let mut parts = r.splitn(2, '/');
                let o2 = parts.next().unwrap_or(o);
                let r2 = parts.next().unwrap_or("");
                if r2.is_empty() {
                    return Err(LocusError::msg(
                        "github.check_repo: repo must be a name or owner/name",
                    ));
                }
                Ok((o2.into(), r2.into(), format!("{o2}/{r2}")))
            } else {
                Ok((o.into(), r.into(), format!("{o}/{r}")))
            }
        }
        (None, Some(r)) if r.contains('/') => {
            let mut parts = r.splitn(2, '/');
            let o = parts.next().unwrap_or("");
            let name = parts.next().unwrap_or("");
            if o.is_empty() || name.is_empty() {
                return Err(LocusError::msg(
                    "github.check_repo: repo must be `owner/repo` when org is omitted",
                ));
            }
            Ok((o.into(), name.into(), format!("{o}/{name}")))
        }
        _ => Err(LocusError::msg(
            "github.check_repo: provide full_name, or org+repo (or owner+repo)",
        )),
    }
}

/// Enforce frozen org/repo allowlists. Empty allowlist ⇒ no restriction for that axis.
fn check_repo_allowed(
    owner: &str,
    repo: &str,
    full: &str,
    provider: &ProviderBinding,
) -> Result<()> {
    let orgs = &provider.scope.orgs;
    let repos = &provider.scope.repos;

    if !orgs.is_empty()
        && !orgs
            .iter()
            .any(|o| o.eq_ignore_ascii_case(owner) || o.eq_ignore_ascii_case("*"))
    {
        return Err(LocusError::msg(format!(
            "scope freeze: refusing org/owner={owner:?}; binding allows orgs={orgs:?}"
        )));
    }

    if !repos.is_empty() {
        let allowed = repos.iter().any(|r| {
            let r = r.as_str();
            r.eq_ignore_ascii_case(full)
                || r.eq_ignore_ascii_case(repo)
                || r.eq_ignore_ascii_case(&format!("{owner}/{repo}"))
                || r == "*"
                || (r.ends_with("/*") && r[..r.len() - 2].eq_ignore_ascii_case(owner))
        });
        if !allowed {
            return Err(LocusError::msg(format!(
                "scope freeze: refusing repo={full:?}; binding allows repos={repos:?}"
            )));
        }
    }

    Ok(())
}

impl ProviderAdapter for GithubAdapter {
    fn name(&self) -> &'static str {
        "github"
    }

    fn tools(&self, provider: &ProviderBinding, binding: &Binding) -> Vec<AdapterTool> {
        let orgs = if provider.scope.orgs.is_empty() {
            "<any>".into()
        } else {
            provider.scope.orgs.join(",")
        };
        let repos = if provider.scope.repos.is_empty() {
            "<any>".into()
        } else {
            provider.scope.repos.join(",")
        };
        vec![
            AdapterTool {
                name: "github.scope".into(),
                description: format!(
                    "Frozen GitHub scope for tenant `{}`: account={}, orgs=[{orgs}], repos=[{repos}].",
                    binding.tenant, provider.account
                ),
                input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
                provider: "github".into(),
                destructive: false,
            },
            AdapterTool {
                name: "github.whoami".into(),
                description: "Show which GitHub identity this pin uses (account + org/repo allowlist). Does not call the GitHub API in phase 1.".into(),
                input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
                provider: "github".into(),
                destructive: false,
            },
            AdapterTool {
                name: "github.check_repo".into(),
                description: format!(
                    "Check whether an org/repo is allowed by this pin's frozen allowlist (orgs=[{orgs}], repos=[{repos}]). Deny if outside scope."
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "full_name": {
                            "type": "string",
                            "description": "owner/repo (e.g. acme-corp/web)"
                        },
                        "org": {
                            "type": "string",
                            "description": "GitHub org or owner (alias of owner)"
                        },
                        "owner": {
                            "type": "string",
                            "description": "GitHub owner (alias of org)"
                        },
                        "repo": {
                            "type": "string",
                            "description": "Repository name, or owner/name"
                        }
                    },
                    "additionalProperties": false
                }),
                provider: "github".into(),
                destructive: false,
            },
        ]
    }

    fn call(
        &self,
        tool: &str,
        args: &Value,
        provider: &ProviderBinding,
        binding: &Binding,
    ) -> Result<ToolCallResult> {
        match tool {
            "github.scope" | "github.whoami" => Ok(ToolCallResult {
                ok: true,
                content: json!({
                    "provider": "github",
                    "account": provider.account,
                    "orgs": provider.scope.orgs,
                    "repos": provider.scope.repos,
                    "credential": crate::credential::credential_metadata(&provider.credential_ref),
                    "tenant": binding.tenant,
                    "binding": binding.alias,
                    "frozen_selectors": ["orgs", "repos"],
                    "worker_hint": "locus exec sets GH_CONFIG_DIR to a private session dir — never mutates global gh auth",
                    "note": "Phase 1 identity tool — real gh/API fan-out uses private config + GH_TOKEN from resolved credential_ref."
                }),
                policy: None,
            }),
            "github.check_repo" => {
                let (owner, repo, full) = parse_repo_ref(args)?;
                check_repo_allowed(&owner, &repo, &full, provider)?;
                Ok(ToolCallResult {
                    ok: true,
                    content: json!({
                        "provider": "github",
                        "allowed": true,
                        "owner": owner,
                        "repo": repo,
                        "full_name": full,
                        "orgs": provider.scope.orgs,
                        "repos": provider.scope.repos,
                        "tenant": binding.tenant,
                        "binding": binding.alias,
                    }),
                    policy: None,
                })
            }
            other => Ok(ToolCallResult {
                ok: false,
                content: json!({"error": format!("unknown github tool: {other}")}),
                policy: None,
            }),
        }
    }
}

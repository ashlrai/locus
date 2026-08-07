use super::{AdapterTool, ProviderAdapter, ToolCallResult};
use crate::binding::{Binding, ProviderBinding};
use crate::error::Result;
use serde_json::{json, Value};

pub struct GithubAdapter;

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
        vec![
            AdapterTool {
                name: "github.scope".into(),
                description: format!(
                    "Frozen GitHub scope for tenant `{}`: account={}, orgs=[{orgs}], repos={:?}.",
                    binding.tenant, provider.account, provider.scope.repos
                ),
                input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
                provider: "github".into(),
                destructive: false,
            },
            AdapterTool {
                name: "github.whoami".into(),
                description: "Show which GitHub identity this pin uses (account + org allowlist). Does not call the GitHub API in phase 1.".into(),
                input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
                provider: "github".into(),
                destructive: false,
            },
        ]
    }

    fn call(
        &self,
        tool: &str,
        _args: &Value,
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
                    "credential_ref": provider.credential_ref,
                    "tenant": binding.tenant,
                    "binding": binding.alias,
                    "worker_hint": "locus exec sets GH_CONFIG_DIR to a private session dir — never mutates global gh auth",
                    "note": "Phase 1 identity tool — real gh/API fan-out uses private config + GH_TOKEN from resolved credential_ref."
                }),
                policy: None,
            }),
            other => Ok(ToolCallResult {
                ok: false,
                content: json!({"error": format!("unknown github tool: {other}")}),
                policy: None,
            }),
        }
    }
}

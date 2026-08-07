use super::{freeze_string_arg, AdapterTool, ProviderAdapter, ToolCallResult};
use crate::binding::{Binding, ProviderBinding};
use crate::error::Result;
use serde_json::{json, Value};

pub struct VercelAdapter;

impl ProviderAdapter for VercelAdapter {
    fn name(&self) -> &'static str {
        "vercel"
    }

    fn tools(&self, provider: &ProviderBinding, binding: &Binding) -> Vec<AdapterTool> {
        let team = provider.scope.team_id.as_deref().unwrap_or("<unset>");
        vec![
            AdapterTool {
                name: "vercel.scope".into(),
                description: format!(
                    "Frozen Vercel scope for tenant `{}`: team_id={team}, projects={:?}, env={:?}.",
                    binding.tenant, provider.scope.projects, provider.scope.env
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "team_id": { "type": "string" }
                    },
                    "additionalProperties": false
                }),
                provider: "vercel".into(),
                destructive: false,
            },
            AdapterTool {
                name: "vercel.deploy.prod".into(),
                description:
                    "SYNTHETIC prod deploy stub — policy-gated. Does not deploy in phase 1.".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "confirm": { "type": "boolean" }
                    },
                    "additionalProperties": false
                }),
                provider: "vercel".into(),
                destructive: true,
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
        let frozen_team = provider.scope.team_id.as_deref();
        let team_id = freeze_string_arg(args, "team_id", frozen_team)?;

        match tool {
            "vercel.scope" => Ok(ToolCallResult {
                ok: true,
                content: json!({
                    "provider": "vercel",
                    "account": provider.account,
                    "team_id": team_id.or_else(|| provider.scope.team_id.clone()),
                    "projects": provider.scope.projects,
                    "env": provider.scope.env,
                    "credential_ref": provider.credential_ref,
                    "tenant": binding.tenant,
                    "binding": binding.alias,
                    "note": "Phase 1 identity tool — remote Vercel MCP/API fan-out lands next."
                }),
                policy: None,
            }),
            "vercel.deploy.prod" => Ok(ToolCallResult {
                ok: true,
                content: json!({
                    "stub": true,
                    "action": "deploy.prod",
                    "team_id": provider.scope.team_id,
                    "projects": provider.scope.projects,
                    "message": "Synthetic tool — no deployment created."
                }),
                policy: None,
            }),
            other => Ok(ToolCallResult {
                ok: false,
                content: json!({"error": format!("unknown vercel tool: {other}")}),
                policy: None,
            }),
        }
    }
}

use super::{freeze_string_arg, AdapterTool, ProviderAdapter, ToolCallResult};
use crate::binding::{Binding, ProviderBinding};
use crate::error::Result;
use serde_json::{json, Value};

pub struct AwsAdapter;

fn extra_str(provider: &ProviderBinding, key: &str) -> Option<String> {
    provider
        .scope
        .extra
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

impl ProviderAdapter for AwsAdapter {
    fn name(&self) -> &'static str {
        "aws"
    }

    fn tools(&self, provider: &ProviderBinding, binding: &Binding) -> Vec<AdapterTool> {
        let account_id = provider.scope.account_id.as_deref().unwrap_or("<unset>");
        let profile = extra_str(provider, "profile").unwrap_or_else(|| "<unset>".into());
        let role = extra_str(provider, "role_arn").unwrap_or_else(|| "<unset>".into());
        let region = extra_str(provider, "region").unwrap_or_else(|| "<unset>".into());
        vec![
            AdapterTool {
                name: "aws.scope".into(),
                description: format!(
                    "Frozen AWS scope for tenant `{}`: account_id={account_id}, profile={profile}, role={role}, region={region}.",
                    binding.tenant
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "account_id": {
                            "type": "string",
                            "description": "Ignored if binding freezes account_id; mismatch is denied."
                        },
                        "profile": {
                            "type": "string",
                            "description": "Ignored if binding freezes profile; mismatch is denied."
                        },
                        "region": {
                            "type": "string",
                            "description": "Ignored if binding freezes region; mismatch is denied."
                        }
                    },
                    "additionalProperties": false
                }),
                provider: "aws".into(),
                destructive: false,
            },
            AdapterTool {
                name: "aws.whoami".into(),
                description: format!(
                    "Show which AWS identity this pin uses: account_id={account_id}, profile={profile}, region={region}. Private AWS config dir — no STS in phase 2 scaffolding."
                ),
                input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
                provider: "aws".into(),
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
        let frozen_account = provider.scope.account_id.as_deref();
        let account_id = freeze_string_arg(args, "account_id", frozen_account)?;

        let frozen_profile = provider.scope.extra.get("profile").and_then(|v| v.as_str());
        let profile = freeze_string_arg(args, "profile", frozen_profile)?;

        let frozen_region = provider.scope.extra.get("region").and_then(|v| v.as_str());
        let region = freeze_string_arg(args, "region", frozen_region)?;

        let role_arn = extra_str(provider, "role_arn");
        let resolved_account = account_id
            .clone()
            .or_else(|| provider.scope.account_id.clone());
        let resolved_profile = profile
            .clone()
            .or_else(|| frozen_profile.map(str::to_string));
        let resolved_region = region.clone().or_else(|| frozen_region.map(str::to_string));

        match tool {
            "aws.scope" | "aws.whoami" => Ok(ToolCallResult {
                ok: true,
                content: json!({
                    "provider": "aws",
                    "account": provider.account,
                    "account_id": resolved_account,
                    "profile": resolved_profile,
                    "role_arn": role_arn,
                    "region": resolved_region,
                    "credential": crate::credential::credential_metadata(&provider.credential_ref),
                    "tenant": binding.tenant,
                    "binding": binding.alias,
                    "frozen_selectors": ["account_id", "profile", "region", "role_arn"],
                    "identity": format!(
                        "aws:{}:{}",
                        resolved_account.as_deref().unwrap_or(&provider.account),
                        resolved_profile.as_deref().unwrap_or("default")
                    ),
                    "worker_hint": "locus exec sets AWS_CONFIG_FILE / AWS_SHARED_CREDENTIALS_FILE to private session dirs — never mutates global ~/.aws",
                    "note": "Phase 2 identity tool — real AWS CLI/MCP fan-out uses private config + keys from resolved credential_ref."
                }),
                policy: None,
            }),
            other => Ok(ToolCallResult {
                ok: false,
                content: json!({"error": format!("unknown aws tool: {other}")}),
                policy: None,
            }),
        }
    }
}

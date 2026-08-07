use super::{freeze_string_arg, AdapterTool, ProviderAdapter, ToolCallResult};
use crate::binding::{Binding, ProviderBinding};
use crate::error::Result;
use serde_json::{json, Value};

pub struct AwsAdapter;

impl ProviderAdapter for AwsAdapter {
    fn name(&self) -> &'static str {
        "aws"
    }

    fn tools(&self, provider: &ProviderBinding, binding: &Binding) -> Vec<AdapterTool> {
        let account_id = provider.scope.account_id.as_deref().unwrap_or("<unset>");
        // Profile / role may live in extra
        let profile = provider
            .scope
            .extra
            .get("profile")
            .and_then(|v| v.as_str())
            .unwrap_or("<unset>");
        let role = provider
            .scope
            .extra
            .get("role_arn")
            .and_then(|v| v.as_str())
            .unwrap_or("<unset>");
        vec![
            AdapterTool {
                name: "aws.scope".into(),
                description: format!(
                    "Frozen AWS scope for tenant `{}`: account_id={account_id}, profile={profile}, role={role}.",
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
                        }
                    },
                    "additionalProperties": false
                }),
                provider: "aws".into(),
                destructive: false,
            },
            AdapterTool {
                name: "aws.whoami".into(),
                description: "Show which AWS identity this pin uses (account_id + private AWS config dir). Does not call STS in phase 2 scaffolding.".into(),
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

        let role_arn = provider
            .scope
            .extra
            .get("role_arn")
            .and_then(|v| v.as_str())
            .map(str::to_string);

        match tool {
            "aws.scope" | "aws.whoami" => Ok(ToolCallResult {
                ok: true,
                content: json!({
                    "provider": "aws",
                    "account": provider.account,
                    "account_id": account_id.or_else(|| provider.scope.account_id.clone()),
                    "profile": profile.or_else(|| frozen_profile.map(str::to_string)),
                    "role_arn": role_arn,
                    "credential_ref": provider.credential_ref,
                    "tenant": binding.tenant,
                    "binding": binding.alias,
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

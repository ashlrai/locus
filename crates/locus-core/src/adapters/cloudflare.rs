use super::{freeze_string_arg, AdapterTool, ProviderAdapter, ToolCallResult};
use crate::binding::{Binding, ProviderBinding};
use crate::error::Result;
use serde_json::{json, Value};

pub struct CloudflareAdapter;

fn zones_from_scope(provider: &ProviderBinding) -> Vec<String> {
    if !provider.scope.projects.is_empty() {
        return provider.scope.projects.clone();
    }
    provider
        .scope
        .extra
        .get("zones")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

impl ProviderAdapter for CloudflareAdapter {
    fn name(&self) -> &'static str {
        "cloudflare"
    }

    fn tools(&self, provider: &ProviderBinding, binding: &Binding) -> Vec<AdapterTool> {
        let account = provider.scope.account_id.as_deref().unwrap_or("<unset>");
        let zones = zones_from_scope(provider);
        let zone_hint = if zones.is_empty() {
            "<any>".into()
        } else {
            zones.join(",")
        };
        vec![
            AdapterTool {
                name: "cloudflare.scope".into(),
                description: format!(
                    "Frozen Cloudflare scope for tenant `{}`: account_id={account}, account={}, zones=[{zone_hint}].",
                    binding.tenant, provider.account
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "account_id": {
                            "type": "string",
                            "description": "Ignored if binding freezes account_id; mismatch is denied."
                        }
                    },
                    "additionalProperties": false
                }),
                provider: "cloudflare".into(),
                destructive: false,
            },
            AdapterTool {
                name: "cloudflare.whoami".into(),
                description: format!(
                    "Show which Cloudflare identity this pin uses: account_id={account}, zones=[{zone_hint}]. Does not call the Cloudflare API."
                ),
                input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
                provider: "cloudflare".into(),
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
        let frozen = provider.scope.account_id.as_deref();
        let account_id = freeze_string_arg(args, "account_id", frozen)?;
        let zones = zones_from_scope(provider);
        let resolved = account_id
            .clone()
            .or_else(|| provider.scope.account_id.clone());

        match tool {
            "cloudflare.scope" | "cloudflare.whoami" => Ok(ToolCallResult {
                ok: true,
                content: json!({
                    "provider": "cloudflare",
                    "account": provider.account,
                    "account_id": resolved,
                    "zones": zones,
                    "credential_ref": provider.credential_ref,
                    "tenant": binding.tenant,
                    "binding": binding.alias,
                    "frozen_selectors": ["account_id", "zones"],
                    "identity": format!(
                        "cloudflare:{}",
                        resolved.as_deref().unwrap_or(&provider.account)
                    ),
                    "worker_hint": "locus exec sets CLOUDFLARE_ACCOUNT_ID from frozen scope — never trust ambient wrangler.toml alone",
                    "note": "Phase 2 identity tool — real Cloudflare MCP/API fan-out uses private worker env + CLOUDFLARE_API_TOKEN from resolved credential_ref."
                }),
                policy: None,
            }),
            other => Ok(ToolCallResult {
                ok: false,
                content: json!({"error": format!("unknown cloudflare tool: {other}")}),
                policy: None,
            }),
        }
    }
}

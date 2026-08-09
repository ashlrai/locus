//! Template ProviderAdapter — copy to crates/locus-core/src/adapters/<provider>.rs
//! and register in mod.rs. See docs/adapter-sdk.md and this folder's README.

use super::{freeze_string_arg, AdapterTool, ProviderAdapter, ToolCallResult};
use crate::binding::{Binding, ProviderBinding};
use crate::error::Result;
use serde_json::{json, Value};

/// Replace with your provider name (e.g. `CloudflareAdapter`).
pub struct MyProviderAdapter;

impl ProviderAdapter for MyProviderAdapter {
    fn name(&self) -> &'static str {
        "myprovider"
    }

    fn tools(&self, provider: &ProviderBinding, binding: &Binding) -> Vec<AdapterTool> {
        let account = provider
            .scope
            .account_id
            .as_deref()
            .unwrap_or("<unset>");
        vec![
            AdapterTool {
                name: "myprovider.scope".into(),
                description: format!(
                    "Frozen myprovider scope for tenant `{}` binding `{}`: account_id={account}. Identity only — no remote calls.",
                    binding.tenant, binding.alias
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
                provider: "myprovider".into(),
                destructive: false,
            },
            // Optional: myprovider.whoami, destructive stubs, etc.
        ]
    }

    fn call(
        &self,
        tool: &str,
        args: &Value,
        provider: &ProviderBinding,
        binding: &Binding,
    ) -> Result<ToolCallResult> {
        // INV: freeze every account selector the model might smuggle.
        let frozen = provider.scope.account_id.as_deref();
        let account_id = freeze_string_arg(args, "account_id", frozen)?;
        let resolved = account_id.or_else(|| provider.scope.account_id.clone());

        match tool {
            "myprovider.scope" => Ok(ToolCallResult {
                ok: true,
                content: json!({
                    "provider": "myprovider",
                    "account": provider.account,
                    "account_id": resolved,
                    "credential_ref": provider.credential_ref,
                    "tenant": binding.tenant,
                    "binding": binding.alias,
                    // Never include resolved secret values.
                }),
                policy: None,
            }),
            other => Err(crate::error::LocusError::msg(format!(
                "unknown tool {other} for myprovider"
            ))),
        }
    }
}

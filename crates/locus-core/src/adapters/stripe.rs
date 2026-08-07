use super::{freeze_string_arg, AdapterTool, ProviderAdapter, ToolCallResult};
use crate::binding::{Binding, ProviderBinding};
use crate::error::Result;
use serde_json::{json, Value};

pub struct StripeAdapter;

impl ProviderAdapter for StripeAdapter {
    fn name(&self) -> &'static str {
        "stripe"
    }

    fn tools(&self, provider: &ProviderBinding, binding: &Binding) -> Vec<AdapterTool> {
        let livemode = provider
            .scope
            .extra
            .get("livemode")
            .and_then(|v| v.as_bool())
            .or(provider.scope.read_only.map(|ro| !ro));
        let mode_hint = match livemode {
            Some(true) => "live",
            Some(false) => "test",
            None => "<unset>",
        };
        vec![
            AdapterTool {
                name: "stripe.scope".into(),
                description: format!(
                    "Frozen Stripe scope for tenant `{}`: account={}, mode={mode_hint}.",
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
                provider: "stripe".into(),
                destructive: false,
            },
            AdapterTool {
                name: "stripe.whoami".into(),
                description: "Show which Stripe identity this pin uses (account + livemode). Does not call the Stripe API.".into(),
                input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
                provider: "stripe".into(),
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

        let livemode = provider
            .scope
            .extra
            .get("livemode")
            .and_then(|v| v.as_bool())
            .or(provider.scope.read_only.map(|ro| !ro));

        match tool {
            "stripe.scope" | "stripe.whoami" => Ok(ToolCallResult {
                ok: true,
                content: json!({
                    "provider": "stripe",
                    "account": provider.account,
                    "account_id": account_id.or_else(|| provider.scope.account_id.clone()),
                    "livemode": livemode,
                    "credential_ref": provider.credential_ref,
                    "tenant": binding.tenant,
                    "binding": binding.alias,
                    "note": "Phase 2 identity tool — real Stripe API fan-out injects STRIPE_API_KEY / STRIPE_SECRET_KEY into isolated workers only."
                }),
                policy: None,
            }),
            other => Ok(ToolCallResult {
                ok: false,
                content: json!({"error": format!("unknown stripe tool: {other}")}),
                policy: None,
            }),
        }
    }
}

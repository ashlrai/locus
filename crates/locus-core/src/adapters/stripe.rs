use super::{freeze_bool_arg, freeze_string_arg, AdapterTool, ProviderAdapter, ToolCallResult};
use crate::binding::{Binding, ProviderBinding};
use crate::error::Result;
use serde_json::{json, Value};

pub struct StripeAdapter;

/// Resolved livemode from binding scope (`extra.livemode`, else inverse of `read_only`).
fn frozen_livemode(provider: &ProviderBinding) -> Option<bool> {
    provider
        .scope
        .extra
        .get("livemode")
        .and_then(|v| v.as_bool())
        .or(provider.scope.read_only.map(|ro| !ro))
}

impl ProviderAdapter for StripeAdapter {
    fn name(&self) -> &'static str {
        "stripe"
    }

    fn tools(&self, provider: &ProviderBinding, binding: &Binding) -> Vec<AdapterTool> {
        let livemode = frozen_livemode(provider);
        let mode_hint = match livemode {
            Some(true) => "live",
            Some(false) => "test",
            None => "<unset>",
        };
        let acct = provider
            .scope
            .account_id
            .as_deref()
            .unwrap_or(provider.account.as_str());
        vec![
            AdapterTool {
                name: "stripe.scope".into(),
                description: format!(
                    "Frozen Stripe scope for tenant `{}`: account={acct}, mode={mode_hint}. account_id and livemode are frozen.",
                    binding.tenant
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "account_id": {
                            "type": "string",
                            "description": "Ignored if binding freezes account_id; mismatch is denied."
                        },
                        "livemode": {
                            "type": "boolean",
                            "description": "Ignored if binding freezes livemode; mismatch is denied."
                        }
                    },
                    "additionalProperties": false
                }),
                provider: "stripe".into(),
                destructive: false,
            },
            AdapterTool {
                name: "stripe.whoami".into(),
                description: format!(
                    "Show which Stripe identity this pin uses: account={acct}, mode={mode_hint}. Does not call the Stripe API."
                ),
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

        let frozen_live = frozen_livemode(provider);
        let livemode = freeze_bool_arg(args, "livemode", frozen_live)?;
        let mode = match livemode {
            Some(true) => "live",
            Some(false) => "test",
            None => "unset",
        };

        match tool {
            "stripe.scope" | "stripe.whoami" => Ok(ToolCallResult {
                ok: true,
                content: json!({
                    "provider": "stripe",
                    "account": provider.account,
                    "account_id": account_id.or_else(|| provider.scope.account_id.clone()),
                    "livemode": livemode,
                    "mode": mode,
                    "read_only": provider.scope.read_only,
                    "credential": crate::credential::credential_metadata(&provider.credential_ref),
                    "tenant": binding.tenant,
                    "binding": binding.alias,
                    "frozen_selectors": ["account_id", "livemode"],
                    "identity": format!(
                        "stripe:{}:{}",
                        provider.scope.account_id.as_deref().unwrap_or(&provider.account),
                        mode
                    ),
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

use super::{freeze_string_arg, AdapterTool, ProviderAdapter, ToolCallResult};
use crate::binding::{Binding, ProviderBinding};
use crate::error::Result;
use serde_json::{json, Value};

pub struct ResendAdapter;

fn domain_allowlist(provider: &ProviderBinding) -> Vec<String> {
    if !provider.scope.projects.is_empty() {
        return provider.scope.projects.clone();
    }
    provider
        .scope
        .extra
        .get("domains")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

impl ProviderAdapter for ResendAdapter {
    fn name(&self) -> &'static str {
        "resend"
    }

    fn tools(&self, provider: &ProviderBinding, binding: &Binding) -> Vec<AdapterTool> {
        let domains = domain_allowlist(provider);
        let domain_hint = if domains.is_empty() {
            "<any>".into()
        } else {
            domains.join(",")
        };
        vec![
            AdapterTool {
                name: "resend.scope".into(),
                description: format!(
                    "Frozen Resend scope for tenant `{}`: account={}, domains=[{domain_hint}]. Domain selector is allowlist-frozen.",
                    binding.tenant, provider.account
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "domain": {
                            "type": "string",
                            "description": "Optional domain selector; denied if not in binding allowlist."
                        }
                    },
                    "additionalProperties": false
                }),
                provider: "resend".into(),
                destructive: false,
            },
            AdapterTool {
                name: "resend.whoami".into(),
                description: format!(
                    "Show which Resend identity this pin uses: account={}, domains=[{domain_hint}]. Does not call the Resend API.",
                    provider.account
                ),
                input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
                provider: "resend".into(),
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
        let allowlist = domain_allowlist(provider);

        // Freeze: if allowlist is non-empty and model passes domain, it must match.
        let domain = freeze_string_arg(args, "domain", None)?;
        if let Some(ref d) = domain {
            if !allowlist.is_empty() && !allowlist.iter().any(|a| a == d || a == "*") {
                return Err(crate::error::LocusError::msg(format!(
                    "scope freeze: refusing domain={d:?}; binding allows {:?}",
                    allowlist
                )));
            }
        }

        match tool {
            "resend.scope" | "resend.whoami" => Ok(ToolCallResult {
                ok: true,
                content: json!({
                    "provider": "resend",
                    "account": provider.account,
                    "domains": allowlist,
                    "domain": domain,
                    "credential": crate::credential::credential_metadata(&provider.credential_ref),
                    "tenant": binding.tenant,
                    "binding": binding.alias,
                    "frozen_selectors": ["domains"],
                    "identity": format!(
                        "resend:{}:domains={}",
                        provider.account,
                        if allowlist.is_empty() {
                            "*".into()
                        } else {
                            allowlist.join(",")
                        }
                    ),
                    "note": "Phase 2 identity tool — real Resend API fan-out injects RESEND_API_KEY into isolated workers only."
                }),
                policy: None,
            }),
            other => Ok(ToolCallResult {
                ok: false,
                content: json!({"error": format!("unknown resend tool: {other}")}),
                policy: None,
            }),
        }
    }
}

use super::{freeze_string_arg, AdapterTool, ProviderAdapter, ToolCallResult};
use crate::binding::{Binding, ProviderBinding};
use crate::error::Result;
use serde_json::{json, Value};

pub struct ResendAdapter;

impl ProviderAdapter for ResendAdapter {
    fn name(&self) -> &'static str {
        "resend"
    }

    fn tools(&self, provider: &ProviderBinding, binding: &Binding) -> Vec<AdapterTool> {
        // Domain allowlist lives in scope.extra or projects as a convenience list.
        let domains = if provider.scope.projects.is_empty() {
            provider
                .scope
                .extra
                .get("domains")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        } else {
            provider.scope.projects.clone()
        };
        let domain_hint = if domains.is_empty() {
            "<any>".into()
        } else {
            domains.join(",")
        };
        vec![
            AdapterTool {
                name: "resend.scope".into(),
                description: format!(
                    "Frozen Resend scope for tenant `{}`: account={}, domains=[{domain_hint}].",
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
                description: "Show which Resend identity this pin uses (account + domain allowlist). Does not call the Resend API.".into(),
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
        let allowlist: Vec<String> = if !provider.scope.projects.is_empty() {
            provider.scope.projects.clone()
        } else {
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
        };

        // Freeze: if allowlist is non-empty and model passes domain, it must match.
        let domain = freeze_string_arg(args, "domain", None)?;
        if let Some(ref d) = domain {
            if !allowlist.is_empty() && !allowlist.iter().any(|a| a == d) {
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
                    "credential_ref": provider.credential_ref,
                    "tenant": binding.tenant,
                    "binding": binding.alias,
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

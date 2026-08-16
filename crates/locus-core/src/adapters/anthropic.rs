use super::{freeze_string_arg, AdapterTool, ProviderAdapter, ToolCallResult};
use crate::binding::{Binding, ProviderBinding};
use crate::error::Result;
use serde_json::{json, Value};

pub struct AnthropicAdapter;

/// Scope mapping (per-tenant model-API spend isolation):
/// `scope.account_id` freezes the Anthropic organization id and
/// `scope.project_ref` freezes the workspace id. Both are enforced here and by
/// the alias freeze net in `mod.rs` (org_id / organization / workspace /
/// workspace_id spellings at any depth).
impl ProviderAdapter for AnthropicAdapter {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    fn tools(&self, provider: &ProviderBinding, binding: &Binding) -> Vec<AdapterTool> {
        let org = provider
            .scope
            .account_id
            .as_deref()
            .unwrap_or(provider.account.as_str());
        let workspace = provider.scope.project_ref.as_deref().unwrap_or("<unset>");
        let selector_props = json!({
            "org_id": {
                "type": "string",
                "description": "Anthropic organization id. Ignored if binding freezes it; mismatch is denied."
            },
            "workspace_id": {
                "type": "string",
                "description": "Anthropic workspace id. Ignored if binding freezes it; mismatch is denied."
            }
        });
        vec![
            AdapterTool {
                name: "anthropic.scope".into(),
                description: format!(
                    "Frozen Anthropic scope for tenant `{}`: org={org}, workspace={workspace}. org_id and workspace_id are frozen.",
                    binding.tenant
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": selector_props,
                    "additionalProperties": false
                }),
                provider: "anthropic".into(),
                destructive: false,
            },
            AdapterTool {
                name: "anthropic.whoami".into(),
                description: format!(
                    "Show which Anthropic identity this pin uses: org={org}, workspace={workspace}. Does not call the Anthropic API."
                ),
                input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
                provider: "anthropic".into(),
                destructive: false,
            },
            AdapterTool {
                name: "anthropic.usage".into(),
                description: format!(
                    "SYNTHETIC read-only usage/spend stub for org={org}, workspace={workspace}. Reports the frozen spend boundary; does not call the Admin API in phase 1."
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": selector_props,
                    "additionalProperties": false
                }),
                provider: "anthropic".into(),
                destructive: false,
            },
            AdapterTool {
                name: "anthropic.keys.list".into(),
                description: format!(
                    "SYNTHETIC read-only API-key listing stub for org={org}, workspace={workspace}. Returns no key material ever; does not call the Admin API in phase 1."
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": selector_props,
                    "additionalProperties": false
                }),
                provider: "anthropic".into(),
                destructive: false,
            },
            AdapterTool {
                name: "anthropic.keys.create".into(),
                description: format!(
                    "SYNTHETIC destructive API-key create stub — policy-gated. Frozen org={org}, workspace={workspace}. Does not create keys in phase 1."
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "confirm": { "type": "boolean" },
                        "name": { "type": "string", "description": "Key label (synthetic)." },
                        "org_id": {
                            "type": "string",
                            "description": "Ignored if binding freezes org_id; mismatch is denied."
                        },
                        "workspace_id": {
                            "type": "string",
                            "description": "Ignored if binding freezes workspace_id; mismatch is denied."
                        }
                    },
                    "additionalProperties": false
                }),
                provider: "anthropic".into(),
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
        // INV: org + workspace freeze on ALL tools (including destructive stubs).
        let frozen_org = provider.scope.account_id.as_deref();
        let org_id = freeze_string_arg(args, "org_id", frozen_org)?;
        let _ = freeze_string_arg(args, "account_id", frozen_org)?;
        let _ = freeze_string_arg(args, "org", frozen_org)?;
        let _ = freeze_string_arg(args, "organization", frozen_org)?;

        let frozen_workspace = provider.scope.project_ref.as_deref();
        let workspace_id = freeze_string_arg(args, "workspace_id", frozen_workspace)?;
        let _ = freeze_string_arg(args, "workspace", frozen_workspace)?;
        let _ = freeze_string_arg(args, "project_ref", frozen_workspace)?;

        let resolved_org = org_id.or_else(|| provider.scope.account_id.clone());
        let resolved_workspace = workspace_id.or_else(|| provider.scope.project_ref.clone());

        match tool {
            "anthropic.scope" | "anthropic.whoami" => Ok(ToolCallResult {
                ok: true,
                content: json!({
                    "provider": "anthropic",
                    "account": provider.account,
                    "org_id": resolved_org,
                    "workspace_id": resolved_workspace,
                    "read_only": provider.scope.read_only,
                    "credential": crate::credential::credential_metadata(&provider.credential_ref),
                    "tenant": binding.tenant,
                    "binding": binding.alias,
                    "frozen_selectors": ["org_id", "workspace_id"],
                    "identity": format!(
                        "anthropic:{}:{}",
                        provider.scope.account_id.as_deref().unwrap_or(&provider.account),
                        provider.scope.project_ref.as_deref().unwrap_or("*")
                    ),
                    "note": "Phase 2 identity tool — real Anthropic Admin API fan-out injects ANTHROPIC_ADMIN_KEY into isolated workers only."
                }),
                policy: None,
            }),
            "anthropic.usage" => Ok(ToolCallResult {
                ok: true,
                content: json!({
                    "stub": true,
                    "action": "usage",
                    "org_id": resolved_org,
                    "workspace_id": resolved_workspace,
                    "spend_boundary": format!(
                        "anthropic:{}:{}",
                        provider.scope.account_id.as_deref().unwrap_or(&provider.account),
                        provider.scope.project_ref.as_deref().unwrap_or("*")
                    ),
                    "message": "Synthetic tool — no usage fetched. Per-tenant spend reporting requires phase-2 Admin API workers."
                }),
                policy: None,
            }),
            "anthropic.keys.list" => Ok(ToolCallResult {
                ok: true,
                content: json!({
                    "stub": true,
                    "action": "keys.list",
                    "org_id": resolved_org,
                    "workspace_id": resolved_workspace,
                    "keys": [],
                    "message": "Synthetic tool — no keys listed. Key material is never returned through MCP."
                }),
                policy: None,
            }),
            "anthropic.keys.create" => Ok(ToolCallResult {
                ok: true,
                content: json!({
                    "stub": true,
                    "action": "keys.create",
                    "name": args.get("name"),
                    "org_id": resolved_org,
                    "workspace_id": resolved_workspace,
                    "message": "Synthetic tool — no key created. Real mutations require phase-2 workers + approval UX."
                }),
                policy: None,
            }),
            other => Ok(ToolCallResult {
                ok: false,
                content: json!({"error": format!("unknown anthropic tool: {other}")}),
                policy: None,
            }),
        }
    }
}

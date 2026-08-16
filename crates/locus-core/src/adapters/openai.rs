use super::{freeze_string_arg, AdapterTool, ProviderAdapter, ToolCallResult};
use crate::binding::{Binding, ProviderBinding};
use crate::error::Result;
use serde_json::{json, Value};

pub struct OpenaiAdapter;

/// Scope mapping (per-tenant model-API spend isolation):
/// `scope.account_id` freezes the OpenAI organization id and
/// `scope.project_ref` freezes the project id. Both are enforced here and by
/// the alias freeze net in `mod.rs` (org_id / organization / project /
/// project_id spellings at any depth).
impl ProviderAdapter for OpenaiAdapter {
    fn name(&self) -> &'static str {
        "openai"
    }

    fn tools(&self, provider: &ProviderBinding, binding: &Binding) -> Vec<AdapterTool> {
        let org = provider
            .scope
            .account_id
            .as_deref()
            .unwrap_or(provider.account.as_str());
        let project = provider.scope.project_ref.as_deref().unwrap_or("<unset>");
        let selector_props = json!({
            "org_id": {
                "type": "string",
                "description": "OpenAI organization id. Ignored if binding freezes it; mismatch is denied."
            },
            "project_id": {
                "type": "string",
                "description": "OpenAI project id. Ignored if binding freezes it; mismatch is denied."
            }
        });
        vec![
            AdapterTool {
                name: "openai.scope".into(),
                description: format!(
                    "Frozen OpenAI scope for tenant `{}`: org={org}, project={project}. org_id and project_id are frozen.",
                    binding.tenant
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": selector_props,
                    "additionalProperties": false
                }),
                provider: "openai".into(),
                destructive: false,
            },
            AdapterTool {
                name: "openai.whoami".into(),
                description: format!(
                    "Show which OpenAI identity this pin uses: org={org}, project={project}. Does not call the OpenAI API."
                ),
                input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
                provider: "openai".into(),
                destructive: false,
            },
            AdapterTool {
                name: "openai.usage".into(),
                description: format!(
                    "SYNTHETIC read-only usage/spend stub for org={org}, project={project}. Reports the frozen spend boundary; does not call the Admin API in phase 1."
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": selector_props,
                    "additionalProperties": false
                }),
                provider: "openai".into(),
                destructive: false,
            },
            AdapterTool {
                name: "openai.keys.list".into(),
                description: format!(
                    "SYNTHETIC read-only API-key listing stub for org={org}, project={project}. Returns no key material ever; does not call the Admin API in phase 1."
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": selector_props,
                    "additionalProperties": false
                }),
                provider: "openai".into(),
                destructive: false,
            },
            AdapterTool {
                name: "openai.keys.create".into(),
                description: format!(
                    "SYNTHETIC destructive API-key create stub — policy-gated. Frozen org={org}, project={project}. Does not create keys in phase 1."
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
                        "project_id": {
                            "type": "string",
                            "description": "Ignored if binding freezes project_id; mismatch is denied."
                        }
                    },
                    "additionalProperties": false
                }),
                provider: "openai".into(),
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
        // INV: org + project freeze on ALL tools (including destructive stubs).
        let frozen_org = provider.scope.account_id.as_deref();
        let org_id = freeze_string_arg(args, "org_id", frozen_org)?;
        let _ = freeze_string_arg(args, "account_id", frozen_org)?;
        let _ = freeze_string_arg(args, "org", frozen_org)?;
        let _ = freeze_string_arg(args, "organization", frozen_org)?;

        let frozen_project = provider.scope.project_ref.as_deref();
        let project_id = freeze_string_arg(args, "project_id", frozen_project)?;
        let _ = freeze_string_arg(args, "project", frozen_project)?;
        let _ = freeze_string_arg(args, "project_ref", frozen_project)?;

        let resolved_org = org_id.or_else(|| provider.scope.account_id.clone());
        let resolved_project = project_id.or_else(|| provider.scope.project_ref.clone());

        match tool {
            "openai.scope" | "openai.whoami" => Ok(ToolCallResult {
                ok: true,
                content: json!({
                    "provider": "openai",
                    "account": provider.account,
                    "org_id": resolved_org,
                    "project_id": resolved_project,
                    "read_only": provider.scope.read_only,
                    "credential": crate::credential::credential_metadata(&provider.credential_ref),
                    "tenant": binding.tenant,
                    "binding": binding.alias,
                    "frozen_selectors": ["org_id", "project_id"],
                    "identity": format!(
                        "openai:{}:{}",
                        provider.scope.account_id.as_deref().unwrap_or(&provider.account),
                        provider.scope.project_ref.as_deref().unwrap_or("*")
                    ),
                    "note": "Phase 2 identity tool — real OpenAI Admin API fan-out injects OPENAI_ADMIN_KEY into isolated workers only."
                }),
                policy: None,
            }),
            "openai.usage" => Ok(ToolCallResult {
                ok: true,
                content: json!({
                    "stub": true,
                    "action": "usage",
                    "org_id": resolved_org,
                    "project_id": resolved_project,
                    "spend_boundary": format!(
                        "openai:{}:{}",
                        provider.scope.account_id.as_deref().unwrap_or(&provider.account),
                        provider.scope.project_ref.as_deref().unwrap_or("*")
                    ),
                    "message": "Synthetic tool — no usage fetched. Per-tenant spend reporting requires phase-2 Admin API workers."
                }),
                policy: None,
            }),
            "openai.keys.list" => Ok(ToolCallResult {
                ok: true,
                content: json!({
                    "stub": true,
                    "action": "keys.list",
                    "org_id": resolved_org,
                    "project_id": resolved_project,
                    "keys": [],
                    "message": "Synthetic tool — no keys listed. Key material is never returned through MCP."
                }),
                policy: None,
            }),
            "openai.keys.create" => Ok(ToolCallResult {
                ok: true,
                content: json!({
                    "stub": true,
                    "action": "keys.create",
                    "name": args.get("name"),
                    "org_id": resolved_org,
                    "project_id": resolved_project,
                    "message": "Synthetic tool — no key created. Real mutations require phase-2 workers + approval UX."
                }),
                policy: None,
            }),
            other => Ok(ToolCallResult {
                ok: false,
                content: json!({"error": format!("unknown openai tool: {other}")}),
                policy: None,
            }),
        }
    }
}

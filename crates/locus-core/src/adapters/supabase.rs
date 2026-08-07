use super::{freeze_string_arg, AdapterTool, ProviderAdapter, ToolCallResult};
use crate::binding::{Binding, ProviderBinding};
use crate::error::Result;
use serde_json::{json, Value};

pub struct SupabaseAdapter;

impl ProviderAdapter for SupabaseAdapter {
    fn name(&self) -> &'static str {
        "supabase"
    }

    fn tools(&self, provider: &ProviderBinding, binding: &Binding) -> Vec<AdapterTool> {
        let proj = provider.scope.project_ref.as_deref().unwrap_or("<unset>");
        let ro = provider.scope.read_only.unwrap_or(false);
        vec![
            AdapterTool {
                name: "supabase.scope".into(),
                description: format!(
                    "Frozen Supabase scope for tenant `{}` binding `{}`: project_ref={proj}, read_only={ro}. Identity only — no SQL."
                    , binding.tenant, binding.alias
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "project_ref": {
                            "type": "string",
                            "description": "Ignored if binding freezes project_ref; mismatch is denied."
                        }
                    },
                    "additionalProperties": false
                }),
                provider: "supabase".into(),
                destructive: false,
            },
            AdapterTool {
                name: "supabase.project_ref".into(),
                description: format!(
                    "Return the frozen Supabase project_ref for this pin ({proj}). Agents must not invent another ref."
                ),
                input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
                provider: "supabase".into(),
                destructive: false,
            },
            AdapterTool {
                name: "supabase.table.delete".into(),
                description: "SYNTHETIC destructive stub — always gated by policy. Does not delete anything in phase 1.".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "table": { "type": "string" },
                        "confirm": { "type": "boolean" }
                    },
                    "required": ["table"],
                    "additionalProperties": false
                }),
                provider: "supabase".into(),
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
        let frozen = provider.scope.project_ref.as_deref();
        let project_ref = freeze_string_arg(args, "project_ref", frozen)?;

        match tool {
            "supabase.scope" | "supabase.project_ref" => Ok(ToolCallResult {
                ok: true,
                content: json!({
                    "provider": "supabase",
                    "account": provider.account,
                    "project_ref": project_ref.or_else(|| provider.scope.project_ref.clone()),
                    "read_only": provider.scope.read_only,
                    "credential_ref": provider.credential_ref,
                    "tenant": binding.tenant,
                    "binding": binding.alias,
                    "note": "Phase 1 identity tool — remote Supabase MCP fan-out lands next."
                }),
                policy: None,
            }),
            "supabase.table.delete" => Ok(ToolCallResult {
                ok: true,
                content: json!({
                    "stub": true,
                    "action": "delete",
                    "table": args.get("table"),
                    "project_ref": provider.scope.project_ref,
                    "message": "Synthetic tool — no rows deleted. Real mutations require phase-2 workers + approval UX."
                }),
                policy: None,
            }),
            other => Ok(ToolCallResult {
                ok: false,
                content: json!({"error": format!("unknown supabase tool: {other}")}),
                policy: None,
            }),
        }
    }
}

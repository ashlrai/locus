//! Provider adapters — frozen scope + synthetic tools.
//!
//! Full upstream MCP fan-out lands when workers spawn; today each adapter
//! exposes **safe read-only identity tools** that report the pinned scope
//! and never call remote APIs with ambient credentials.

mod aws;
mod cloudflare;
mod github;
mod resend;
mod stripe;
mod supabase;
mod vercel;

use crate::binding::{Binding, ProviderBinding};
use crate::error::{LocusError, Result};
use crate::policy::{evaluate, Decision, PolicyVerdict};
use crate::store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub use aws::AwsAdapter;
pub use cloudflare::CloudflareAdapter;
pub use github::GithubAdapter;
pub use resend::ResendAdapter;
pub use stripe::StripeAdapter;
pub use supabase::SupabaseAdapter;
pub use vercel::VercelAdapter;

/// Context for require_approval gating during tools/call.
#[derive(Debug, Clone, Copy)]
pub struct ApprovalGate<'a> {
    pub store: &'a Store,
    pub session_id: &'a str,
}

/// A tool exposed through locus-mcp for the active pin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub provider: String,
    /// If true, mutating / high-risk (still synthetic in phase 1).
    pub destructive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    pub ok: bool,
    pub content: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<PolicyVerdict>,
}

/// Trait every provider adapter implements.
pub trait ProviderAdapter: Send + Sync {
    fn name(&self) -> &'static str;
    fn tools(&self, provider: &ProviderBinding, binding: &Binding) -> Vec<AdapterTool>;
    fn call(
        &self,
        tool: &str,
        args: &Value,
        provider: &ProviderBinding,
        binding: &Binding,
    ) -> Result<ToolCallResult>;
}

/// Freeze: ignore model-supplied selectors that conflict with scope.
pub fn freeze_string_arg(args: &Value, key: &str, frozen: Option<&str>) -> Result<Option<String>> {
    let model_val = args.get(key).and_then(|v| v.as_str());
    match (frozen, model_val) {
        (Some(f), Some(m)) if m != f => Err(LocusError::msg(format!(
            "scope freeze: refusing {key}={m:?}; binding freezes {key}={f:?}"
        ))),
        (Some(f), _) => Ok(Some(f.to_string())),
        (None, Some(m)) => Ok(Some(m.to_string())),
        (None, None) => Ok(None),
    }
}

pub fn adapter_for(provider: &str) -> Option<Box<dyn ProviderAdapter>> {
    match provider.to_ascii_lowercase().as_str() {
        "supabase" => Some(Box::new(SupabaseAdapter)),
        "github" => Some(Box::new(GithubAdapter)),
        "vercel" => Some(Box::new(VercelAdapter)),
        "cloudflare" => Some(Box::new(CloudflareAdapter)),
        "aws" => Some(Box::new(AwsAdapter)),
        "resend" => Some(Box::new(ResendAdapter)),
        "stripe" => Some(Box::new(StripeAdapter)),
        _ => None,
    }
}

/// All tools for a binding (exclusive pin — unprefixed `provider.tool`).
pub fn tools_for_binding(binding: &Binding) -> Vec<AdapterTool> {
    let mut out = Vec::new();
    for p in &binding.providers {
        if let Some(adapter) = adapter_for(&p.provider) {
            out.extend(adapter.tools(p, binding));
        } else {
            // Generic identity tool for unknown providers
            out.push(AdapterTool {
                name: format!("{}.scope", p.provider),
                description: format!(
                    "Show frozen scope for pinned {} account `{}` (no remote calls).",
                    p.provider, p.account
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }),
                provider: p.provider.clone(),
                destructive: false,
            });
        }
    }
    out
}

/// Dispatch a tool call against the pinned binding (no approval store).
///
/// For `require_approval` tools this always blocks unless a matching grant
/// would be checked via [`call_tool_gated`]. Prefer gated calls from MCP.
pub fn call_tool(binding: &Binding, tool_name: &str, args: &Value) -> Result<ToolCallResult> {
    call_tool_gated(binding, tool_name, args, None)
}

/// Dispatch a tool call with optional approval grant store.
///
/// When policy says `require_approval`:
/// 1. If a still-valid grant exists for tool+binding+args_digest → allow
/// 2. Else if `confirm=true` and `approval_id` names a valid matching grant → allow
/// 3. Else create/reuse a pending approval record and block with `approval_id`
pub fn call_tool_gated(
    binding: &Binding,
    tool_name: &str,
    args: &Value,
    gate: Option<ApprovalGate<'_>>,
) -> Result<ToolCallResult> {
    // Policy gate
    let verdict = evaluate(&binding.policy, tool_name);
    match verdict.decision {
        Decision::Deny => {
            return Ok(ToolCallResult {
                ok: false,
                content: json!({ "error": "denied_by_policy", "detail": verdict.reason }),
                policy: Some(verdict),
            });
        }
        Decision::RequireApproval => {
            if let Some(gate) = gate {
                if check_require_approval(gate, &binding.alias, tool_name, args, &verdict)?
                    .is_some()
                {
                    // Fall through to execute
                } else {
                    // Blocked — pending record created inside check
                    let pending = gate.store.create_pending_approval(
                        tool_name,
                        &binding.alias,
                        args,
                        gate.session_id,
                    )?;
                    return Ok(ToolCallResult {
                        ok: false,
                        content: json!({
                            "error": "requires_approval",
                            "detail": verdict.reason,
                            "approval_id": pending.id,
                            "args_digest": pending.args_digest,
                            "hint": format!(
                                "Human: run `locus approve grant {}` then re-call (same args), or re-call with confirm=true and approval_id={}",
                                pending.id, pending.id
                            ),
                        }),
                        policy: Some(verdict),
                    });
                }
            } else {
                // No store: block with instructions (unit tests / synthetic without gate)
                return Ok(ToolCallResult {
                    ok: false,
                    content: json!({
                        "error": "requires_approval",
                        "detail": verdict.reason,
                        "hint": "Re-call via locus-mcp after `locus approve grant <id>`, or adjust binding.policy.require_approval"
                    }),
                    policy: Some(verdict),
                });
            }
        }
        Decision::Allow => {}
    }

    dispatch_tool(binding, tool_name, args, verdict)
}

/// Returns `Some(())` when the call is allowed through the approval gate.
fn check_require_approval(
    gate: ApprovalGate<'_>,
    binding_alias: &str,
    tool_name: &str,
    args: &Value,
    _verdict: &PolicyVerdict,
) -> Result<Option<()>> {
    // Path 1: matching approved grant within TTL (same tool+binding+args_digest)
    if let Some(rec) = gate.store.find_valid_grant(tool_name, binding_alias, args)? {
        let _ = gate.store.audit(
            "approval.use",
            binding_alias,
            Some(json!({
                "id": rec.id,
                "tool": tool_name,
                "via": "args_digest_match",
            })),
        );
        return Ok(Some(()));
    }

    // Path 2: confirm=true AND valid approval_id
    let confirm = args
        .get("confirm")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if confirm {
        if let Some(id) = args.get("approval_id").and_then(|v| v.as_str()) {
            match gate
                .store
                .check_approval_id(id, tool_name, binding_alias, args)
            {
                Ok(rec) => {
                    let _ = gate.store.audit(
                        "approval.use",
                        binding_alias,
                        Some(json!({
                            "id": rec.id,
                            "tool": tool_name,
                            "via": "approval_id",
                        })),
                    );
                    return Ok(Some(()));
                }
                Err(_) => {
                    // Invalid id — fall through to pending block
                }
            }
        }
    }

    Ok(None)
}

fn dispatch_tool(
    binding: &Binding,
    tool_name: &str,
    args: &Value,
    verdict: PolicyVerdict,
) -> Result<ToolCallResult> {
    // Find owning provider by tool prefix `provider.`
    let provider_name = tool_name.split('.').next().unwrap_or("");
    let p = binding
        .provider(provider_name)
        .ok_or_else(|| LocusError::msg(format!("no provider for tool '{tool_name}' in binding")))?;

    // Known adapters run enforce_call (scope freeze). Unknown providers get a
    // generic identity response for `*.scope` only.
    if let Some(adapter) = adapter_for(&p.provider) {
        let mut result = adapter.call(tool_name, args, p, binding)?;
        result.policy = Some(verdict);
        return Ok(result);
    }

    if tool_name.ends_with(".scope") {
        return Ok(ToolCallResult {
            ok: true,
            content: json!({
                "provider": p.provider,
                "account": p.account,
                "credential_ref": p.credential_ref,
                "scope": p.scope,
                "tenant": binding.tenant,
                "binding": binding.alias,
            }),
            policy: Some(verdict),
        });
    }

    Err(LocusError::msg(format!(
        "no adapter for provider {} (tool {tool_name})",
        p.provider
    )))
}

/// Control-plane tools always available (even unbound, subset).
pub fn control_tools(pinned: bool) -> Vec<AdapterTool> {
    let mut tools = vec![
        AdapterTool {
            name: "locus_whoami".into(),
            description: "Show the active Locus pin: tenant, binding, providers, frozen scopes. Never returns secrets.".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            provider: "locus".into(),
            destructive: false,
        },
        AdapterTool {
            name: "locus_status".into(),
            description: "Short pin status: pinned|unpinned, binding alias, tenant, seal ok.".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            provider: "locus".into(),
            destructive: false,
        },
        AdapterTool {
            name: "locus_list_bindings".into(),
            description: "List configured binding aliases and tenants (no secrets).".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            provider: "locus".into(),
            destructive: false,
        },
        AdapterTool {
            name: "locus_request_pin".into(),
            description: "Request the human to pin a binding. Agents cannot pin themselves — returns instructions. Pass alias.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "alias": { "type": "string", "description": "Binding alias to request (e.g. acme)" }
                },
                "required": ["alias"],
                "additionalProperties": false
            }),
            provider: "locus".into(),
            destructive: false,
        },
    ];
    if pinned {
        tools.push(AdapterTool {
            name: "locus_providers".into(),
            description: "List providers and frozen scopes for the active pin.".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            provider: "locus".into(),
            destructive: false,
        });
    }
    tools
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::{BindingBody, Policy, Scope};

    fn acme() -> Binding {
        Binding::from_body(BindingBody {
            id: "bnd_acme".into(),
            alias: "acme".into(),
            tenant: "acme-corp".into(),
            principal: None,
            description: None,
            policy: Policy {
                require_approval: vec!["*.delete*".into()],
                ..Policy::default()
            },
            providers: vec![ProviderBinding {
                provider: "supabase".into(),
                account: "acme".into(),
                credential_ref: "phm:SUPABASE_ACME".into(),
                scope: Scope {
                    project_ref: Some("proj_acme".into()),
                    read_only: Some(true),
                    ..Scope::default()
                },
            }],
        })
    }

    #[test]
    fn tools_include_supabase_and_control() {
        let b = acme();
        let tools = tools_for_binding(&b);
        assert!(tools.iter().any(|t| t.name == "supabase.scope"));
        assert!(tools.iter().any(|t| t.name.starts_with("supabase.")));
    }

    #[test]
    fn freeze_rejects_wrong_project() {
        let b = acme();
        let err = call_tool(&b, "supabase.scope", &json!({"project_ref": "proj_evil"}));
        assert!(err.is_err(), "expected scope freeze deny, got {err:?}");
        let msg = err.unwrap_err().to_string();
        assert!(
            msg.contains("scope freeze") || msg.contains("proj_evil"),
            "unexpected message: {msg}"
        );
        // helper unit check
        assert!(freeze_string_arg(
            &json!({"project_ref": "proj_evil"}),
            "project_ref",
            Some("proj_acme"),
        )
        .is_err());
    }

    #[test]
    fn require_approval_blocks_delete() {
        let b = acme();
        let r = call_tool(&b, "supabase.table.delete", &json!({})).unwrap();
        assert!(!r.ok);
        assert_eq!(
            r.content.get("error").and_then(|v| v.as_str()),
            Some("requires_approval")
        );
    }

    #[test]
    fn confirm_alone_does_not_bypass_without_grant() {
        let b = acme();
        let r = call_tool(
            &b,
            "supabase.table.delete",
            &json!({ "confirm": true }),
        )
        .unwrap();
        assert!(!r.ok);
        assert_eq!(
            r.content.get("error").and_then(|v| v.as_str()),
            Some("requires_approval")
        );
    }
}

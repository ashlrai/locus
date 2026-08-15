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
    /// Session principal / requester label recorded on pending approvals.
    pub principal: Option<&'a str>,
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

/// Freeze a boolean selector (e.g. Stripe `livemode`).
pub fn freeze_bool_arg(args: &Value, key: &str, frozen: Option<bool>) -> Result<Option<bool>> {
    let model_val = args.get(key).and_then(|v| v.as_bool());
    match (frozen, model_val) {
        (Some(f), Some(m)) if m != f => Err(LocusError::msg(format!(
            "scope freeze: refusing {key}={m}; binding freezes {key}={f}"
        ))),
        (Some(f), _) => Ok(Some(f)),
        (None, Some(m)) => Ok(Some(m)),
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

/// Providers with a built-in synthetic adapter (dispatchable via [`adapter_for`]).
///
/// Keep in sync with the `adapter_for` match above — enforced by the
/// `known_providers_all_dispatch` test.
pub fn known_providers() -> &'static [&'static str] {
    &[
        "supabase",
        "github",
        "vercel",
        "cloudflare",
        "aws",
        "resend",
        "stripe",
    ]
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
/// For `require_approval` tools this always blocks. Prefer gated calls from MCP
/// so callers receive a stable advisory record and explicit authority status.
pub fn call_tool(binding: &Binding, tool_name: &str, args: &Value) -> Result<ToolCallResult> {
    call_tool_gated(binding, tool_name, args, None)
}

/// Run policy + approval only.
///
/// Returns `Ok(None)` when the call may proceed, or `Ok(Some(block))` when
/// denied / pending approval. Used by synthetic dispatch and upstream workers.
pub fn enforce_policy(
    binding: &Binding,
    tool_name: &str,
    args: &Value,
    gate: Option<ApprovalGate<'_>>,
) -> Result<Option<ToolCallResult>> {
    // Scope is part of authorization and must fail before approval creation or
    // any caller has an opportunity to start an upstream worker.
    preflight_scope_freeze(binding, tool_name, args)?;
    let verdict = evaluate(&binding.policy, tool_name);
    match verdict.decision {
        Decision::Deny => Ok(Some(ToolCallResult {
            ok: false,
            content: json!({ "error": "denied_by_policy", "detail": verdict.reason }),
            policy: Some(verdict),
        })),
        Decision::RequireApproval => {
            if let Some(gate) = gate {
                if check_require_approval(gate, &binding.alias, tool_name, args, &verdict)?
                    .is_some()
                {
                    return Ok(None);
                }
                let requester = gate
                    .principal
                    .filter(|p| !p.is_empty())
                    .unwrap_or("unknown");
                let pending = gate.store.create_pending_approval(
                    tool_name,
                    &binding.alias,
                    args,
                    gate.session_id,
                    requester,
                )?;
                let dual = binding.policy.requires_dual_control(tool_name);
                let required = crate::approval::required_grant_count(dual);
                let advisory_assertions = pending.grants.len();
                let hint = if dual {
                    format!(
                        "Dual-control requires {required} externally authenticated approvers. \
                         `locus approve grant {} --as <label>` records local advisory evidence only; \
                         external approval authority is not configured, so provider execution remains blocked.",
                        pending.id
                    )
                } else {
                    format!(
                        "`locus approve grant {} --as <label>` records local advisory evidence only. \
                         External approval authority is not configured, so provider execution remains blocked.",
                        pending.id
                    )
                };
                Ok(Some(ToolCallResult {
                    ok: false,
                    content: json!({
                        "error": "requires_approval",
                        "detail": verdict.reason,
                        "approval_id": pending.id,
                        "args_digest": pending.args_digest,
                        "dual_control": dual,
                        "grants": advisory_assertions,
                        "advisory_assertions": advisory_assertions,
                        "authoritative_grants": 0,
                        "required_grants": required,
                        "required_authoritative_grants": required,
                        "approval_authority": "local_advisory",
                        "authoritative_path_enabled": false,
                        "requester": pending.requester,
                        "hint": hint,
                    }),
                    policy: Some(verdict),
                }))
            } else {
                let dual = binding.policy.requires_dual_control(tool_name);
                Ok(Some(ToolCallResult {
                    ok: false,
                    content: json!({
                        "error": "requires_approval",
                        "detail": verdict.reason,
                        "dual_control": dual,
                        "approval_authority": "local_advisory",
                        "authoritative_path_enabled": false,
                        "hint": "Local approval assertions are advisory and cannot unlock provider execution. External approval authority is not configured."
                    }),
                    policy: Some(verdict),
                }))
            }
        }
        Decision::Allow => Ok(None),
    }
}

/// Dispatch a tool call with an optional advisory approval-record store.
///
/// Order (fail closed):
/// 1. **Scope freeze** — wrong-tenant selectors error before any approval is minted
/// 2. Policy `require_approval` / dual_control gate
/// 3. Adapter dispatch
///
/// When policy says `require_approval` (or dual_control):
/// 1. If a still-valid externally authenticated grant exists for the exact call → allow
/// 2. Else if `confirm=true` and `approval_id` names such a grant → allow
/// 3. Else create/reuse a pending approval record and block with `approval_id`
///
/// No external verifier ships today, so locally asserted approvals remain
/// advisory and every gated provider call remains blocked.
pub fn call_tool_gated(
    binding: &Binding,
    tool_name: &str,
    args: &Value,
    gate: Option<ApprovalGate<'_>>,
) -> Result<ToolCallResult> {
    // INV: freeze before policy — model cannot smuggle a wrong project_ref/team_id
    // past require_approval into a grantable call.
    if let Some(blocked) = enforce_policy(binding, tool_name, args, gate)? {
        return Ok(blocked);
    }
    let verdict = evaluate(&binding.policy, tool_name);
    dispatch_tool(binding, tool_name, args, verdict)
}

/// Shared account-selector freezes applied before policy evaluation.
///
/// Adapters may re-check the same keys inside `call()`; this preflight ensures
/// destructive tools under `require_approval` still deny wrong selectors as
/// hard errors (not soft `requires_approval` results).
///
/// Real MCP servers spell the same selector many ways (camelCase, provider
/// jargon), so every frozen selector is enforced for its canonical key AND its
/// provider-native alias spellings at any depth inside object and array args
/// (bounded scan — args nesting past the limit deny fail closed, and
/// non-string values under a frozen selector key deny as type mismatches). A tool
/// whose provider prefix is not declared in the pinned binding is denied
/// outright (fail closed): this binding can never authorize it.
fn preflight_scope_freeze(binding: &Binding, tool_name: &str, args: &Value) -> Result<()> {
    let provider_name = tool_name.split('.').next().unwrap_or("");
    let Some(p) = binding.provider(provider_name) else {
        return Err(LocusError::msg(format!(
            "scope freeze: provider `{provider_name}` is not part of the pinned binding (tool `{tool_name}`); denied fail closed"
        )));
    };
    freeze_string_arg(args, "project_ref", p.scope.project_ref.as_deref())?;
    freeze_string_arg(args, "team_id", p.scope.team_id.as_deref())?;
    freeze_string_arg(args, "account_id", p.scope.account_id.as_deref())?;

    // Provider-native / camelCase spellings of the same frozen selectors.
    let spellings = selector_alias_spellings(&p.provider);
    freeze_selector_aliases(
        args,
        spellings.project_ref,
        "project_ref",
        p.scope.project_ref.as_deref(),
    )?;
    freeze_selector_aliases(
        args,
        spellings.team_id,
        "team_id",
        p.scope.team_id.as_deref(),
    )?;
    freeze_selector_aliases(
        args,
        spellings.account_id,
        "account_id",
        p.scope.account_id.as_deref(),
    )?;
    freeze_org_selector_aliases(args, &p.scope.orgs)?;
    // Stripe (and similar) may freeze livemode via scope.extra
    let frozen_live = p
        .scope
        .extra
        .get("livemode")
        .and_then(|v| v.as_bool())
        .or_else(|| {
            // also accept boolean-ish string
            p.scope.extra.get("livemode").and_then(|v| match v {
                toml::Value::Boolean(b) => Some(*b),
                toml::Value::String(s) if s == "true" => Some(true),
                toml::Value::String(s) if s == "false" => Some(false),
                _ => None,
            })
        });
    freeze_bool_arg(args, "livemode", frozen_live)?;
    // Nested / typed coverage for livemode: a frozen mode cannot be flipped via
    // a nested object, an array member, or a non-boolean spelling.
    if let Some(fl) = frozen_live {
        let check = |key: &str, v: &Value| -> Result<()> {
            if key != "livemode" {
                return Ok(());
            }
            match v {
                Value::Bool(m) if *m == fl => Ok(()),
                Value::Null => Ok(()),
                other => Err(LocusError::msg(format!(
                    "scope freeze: refusing livemode={other}; binding freezes livemode={fl}"
                ))),
            }
        };
        walk_frozen_keys(args, FREEZE_SCAN_MAX_DEPTH, &check)?;
    }
    Ok(())
}

/// Alias spellings per canonical frozen selector. The freeze scan enforces the
/// canonical snake_case key AND these alias spellings real MCP servers use
/// (camelCase, provider jargon) at every scanned depth. Unknown-but-declared
/// providers (custom upstream workers) get the generic set.
struct SelectorAliasSpellings {
    project_ref: &'static [&'static str],
    team_id: &'static [&'static str],
    account_id: &'static [&'static str],
}

fn selector_alias_spellings(provider: &str) -> SelectorAliasSpellings {
    const PROJECT_GENERIC: &[&str] = &["projectRef", "project_id", "projectId"];
    const TEAM_GENERIC: &[&str] = &["teamId"];
    const ACCOUNT_GENERIC: &[&str] = &["accountId"];
    match provider.to_ascii_lowercase().as_str() {
        "aws" => SelectorAliasSpellings {
            project_ref: PROJECT_GENERIC,
            team_id: TEAM_GENERIC,
            account_id: &["accountId", "aws_account_id", "awsAccountId"],
        },
        "stripe" => SelectorAliasSpellings {
            project_ref: PROJECT_GENERIC,
            team_id: TEAM_GENERIC,
            account_id: &["accountId", "stripe_account", "stripeAccount"],
        },
        _ => SelectorAliasSpellings {
            project_ref: PROJECT_GENERIC,
            team_id: TEAM_GENERIC,
            account_id: ACCOUNT_GENERIC,
        },
    }
}

/// Max arg-nesting depth scanned by the freeze net. Args that nest deeper than
/// this while a selector is frozen are denied outright (fail closed) — a
/// selector can never hide below the scan horizon. Real MCP tool args stay far
/// shallower than this.
const FREEZE_SCAN_MAX_DEPTH: usize = 32;

/// Bounded-depth walk over object entries and array members, applying `check`
/// to every object key/value pair. Depth exhaustion with structure still
/// unscanned is a hard deny (fail closed), never a silent skip.
fn walk_frozen_keys<F>(value: &Value, depth: usize, check: &F) -> Result<()>
where
    F: Fn(&str, &Value) -> Result<()>,
{
    match value {
        Value::Object(map) => {
            if depth == 0 {
                return Err(LocusError::msg(
                    "scope freeze: args nest deeper than the freeze scan limit; denied fail closed",
                ));
            }
            for (k, v) in map {
                check(k, v)?;
                walk_frozen_keys(v, depth - 1, check)?;
            }
        }
        Value::Array(items) => {
            if depth == 0 {
                return Err(LocusError::msg(
                    "scope freeze: args nest deeper than the freeze scan limit; denied fail closed",
                ));
            }
            for v in items {
                walk_frozen_keys(v, depth - 1, check)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn json_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Deny selector spellings (canonical key + aliases) that conflict with the
/// frozen canonical value — at any scanned depth, inside arrays, and for any
/// JSON type. A frozen selector key holding a non-string value (other than
/// `null`, which carries no selector) is a hard deny: type games cannot dodge
/// the freeze.
fn freeze_selector_aliases(
    args: &Value,
    aliases: &[&str],
    canonical: &str,
    frozen: Option<&str>,
) -> Result<()> {
    let Some(f) = frozen else {
        return Ok(());
    };
    let check = |key: &str, v: &Value| -> Result<()> {
        if key != canonical && !aliases.contains(&key) {
            return Ok(());
        }
        match v {
            Value::String(m) if m.as_str() == f => Ok(()),
            Value::Null => Ok(()),
            Value::String(m) => Err(LocusError::msg(format!(
                "scope freeze: refusing {key}={m:?}; binding freezes {canonical}={f:?}"
            ))),
            other => Err(LocusError::msg(format!(
                "scope freeze: refusing non-string {key} ({}); binding freezes {canonical}={f:?}",
                json_type_name(other)
            ))),
        }
    };
    walk_frozen_keys(args, FREEZE_SCAN_MAX_DEPTH, &check)
}

/// When orgs are frozen, model-supplied org/owner selectors must be members —
/// at any scanned depth, inside arrays, and only as plain strings (non-string
/// org selectors are a hard deny).
fn freeze_org_selector_aliases(args: &Value, orgs: &[String]) -> Result<()> {
    if orgs.is_empty() {
        return Ok(());
    }
    const ORG_KEYS: &[&str] = &["org", "owner", "organization"];
    let check = |key: &str, v: &Value| -> Result<()> {
        if !ORG_KEYS.contains(&key) {
            return Ok(());
        }
        match v {
            Value::String(m) if orgs.iter().any(|o| o == m) => Ok(()),
            Value::Null => Ok(()),
            Value::String(m) => Err(LocusError::msg(format!(
                "scope freeze: refusing {key}={m:?}; binding freezes orgs={orgs:?}"
            ))),
            other => Err(LocusError::msg(format!(
                "scope freeze: refusing non-string {key} ({}); binding freezes orgs={orgs:?}",
                json_type_name(other)
            ))),
        }
    };
    walk_frozen_keys(args, FREEZE_SCAN_MAX_DEPTH, &check)
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
    if let Some(rec) = gate
        .store
        .find_valid_grant(tool_name, binding_alias, args)?
    {
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
                "credential": crate::credential::credential_metadata(&p.credential_ref),
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
            description: "REQUIRED before infrastructure work when pin is unclear. Returns active pin: tenant, binding, providers, frozen scopes. Never secrets. If unpinned, ask human to pin — you cannot.".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            provider: "locus".into(),
            destructive: false,
        },
        AdapterTool {
            name: "locus_safe_next".into(),
            description: "Single best next human/agent action for identity safety (enter, re-pin, approve, doctor fix, or ready). Call when stuck, unpinned, blocked, or unsure what to do. Never secrets.".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            provider: "locus".into(),
            destructive: false,
        },
        AdapterTool {
            name: "locus_status".into(),
            description: "Short pin status: pinned|unpinned, binding alias, tenant, seal ok, frozen. Prefer locus_whoami or locus_safe_next for decisions.".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            provider: "locus".into(),
            destructive: false,
        },
        AdapterTool {
            name: "locus_heartbeat".into(),
            description: "Identity heartbeat: runtime drift / doctor-lite (seal, freeze, binding match). Safe for agents — never secrets. Call when pin health is unclear or tools fail with freeze/seal errors.".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            provider: "locus".into(),
            destructive: false,
        },
        AdapterTool {
            name: "locus_enter_hint".into(),
            description: "Shell command for the HUMAN to enter/pin a binding. AGENTS CANNOT PIN — surface this command to the operator; do not claim you switched accounts.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "alias": {
                        "type": "string",
                        "description": "Optional binding alias (e.g. acme). When omitted, returns generic `locus enter`."
                    }
                },
                "additionalProperties": false
            }),
            provider: "locus".into(),
            destructive: false,
        },
        AdapterTool {
            name: "locus_list_bindings".into(),
            description: "List configured binding aliases and tenants (no secrets). Use to discover which alias to request via locus_request_pin.".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            provider: "locus".into(),
            destructive: false,
        },
        AdapterTool {
            name: "locus_request_pin".into(),
            description: "Request the HUMAN to pin a binding. AGENTS CANNOT PIN — records a request and returns the exact shell command. Pass alias from locus_list_bindings.".into(),
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
        AdapterTool {
            name: "locus_verify_claim".into(),
            description: "Verification plane (M5): score a free-text claim before acting. Returns {claim, confidence: unknown|low|medium|high, needs_tool, suggestion, signals, grounding?}. Heuristic — numbers/URLs/versions/currency ($)/percentages/absolute language (always|never) ⇒ needs_tool + low confidence; identity claims ground against whoami when pinned. Suggestion names concrete next steps (provider reads, locus exec, whoami). Never secrets. For hub session pack use locus_verify_session (or CLI: locus verify session --json).".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "Claim to score (factual assertion before acting)"
                    },
                    "claim": {
                        "type": "string",
                        "description": "Alias for text"
                    }
                },
                "additionalProperties": false
            }),
            provider: "locus".into(),
            destructive: false,
        },
        AdapterTool {
            name: "locus_verify_session".into(),
            description: "Verification plane (M5): hub session pack — same JSON as `locus verify session --json`. Returns {kind:\"session\", version, whoami?, doctor, safe_next, session_ok}. Available unpinned. Gate on session_ok (isError only on hard store failures). Never secrets — aliases, verdicts, scopes only.".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            provider: "locus".into(),
            destructive: false,
        },
    ];
    if pinned {
        tools.push(AdapterTool {
            name: "locus_providers".into(),
            description: "Providers and frozen scopes for the active pin only. Frozen project_ref/team_id/orgs cannot be overridden by tool args.".into(),
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
    use std::collections::BTreeMap;

    /// `known_providers()` and the `adapter_for` dispatch match must stay in
    /// sync — every published provider dispatches, and the two well-known
    /// non-adapter names never appear.
    #[test]
    fn known_providers_all_dispatch() {
        for p in known_providers() {
            assert!(
                adapter_for(p).is_some(),
                "known provider '{p}' must dispatch"
            );
        }
        assert!(adapter_for("unknown-provider").is_none());
    }

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
                upstream: None,
            }],
        })
    }

    fn multi_provider() -> Binding {
        let mut stripe_extra = BTreeMap::new();
        stripe_extra.insert("livemode".into(), toml::Value::Boolean(false));
        Binding::from_body(BindingBody {
            id: "bnd_multi".into(),
            alias: "multi".into(),
            tenant: "acme-corp".into(),
            principal: None,
            description: None,
            policy: Policy::default(),
            providers: vec![
                ProviderBinding {
                    provider: "cloudflare".into(),
                    account: "acme-cf".into(),
                    credential_ref: "phm:CF_ACME".into(),
                    scope: Scope {
                        account_id: Some("cf_acct_acme".into()),
                        ..Scope::default()
                    },
                    upstream: None,
                },
                ProviderBinding {
                    provider: "stripe".into(),
                    account: "acme-stripe".into(),
                    credential_ref: "phm:STRIPE_ACME".into(),
                    scope: Scope {
                        account_id: Some("acct_acme".into()),
                        extra: stripe_extra,
                        ..Scope::default()
                    },
                    upstream: None,
                },
                ProviderBinding {
                    provider: "aws".into(),
                    account: "acme-aws".into(),
                    credential_ref: "phm:AWS_ACME".into(),
                    scope: Scope {
                        account_id: Some("123456789012".into()),
                        ..Scope::default()
                    },
                    upstream: None,
                },
            ],
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
    fn freeze_cloudflare_account_id() {
        let b = multi_provider();
        let ok = call_tool(
            &b,
            "cloudflare.scope",
            &json!({ "account_id": "cf_acct_acme" }),
        )
        .unwrap();
        assert!(ok.ok);
        assert_eq!(
            ok.content.get("account_id").and_then(|v| v.as_str()),
            Some("cf_acct_acme")
        );

        let err = call_tool(
            &b,
            "cloudflare.scope",
            &json!({ "account_id": "cf_acct_evil" }),
        );
        assert!(
            err.is_err(),
            "expected cloudflare account_id freeze: {err:?}"
        );
        let msg = err.unwrap_err().to_string();
        assert!(
            msg.contains("scope freeze") && msg.contains("account_id"),
            "unexpected: {msg}"
        );
    }

    #[test]
    fn freeze_stripe_livemode() {
        let b = multi_provider();
        // Matching frozen livemode=false is allowed
        let ok = call_tool(&b, "stripe.scope", &json!({ "livemode": false })).unwrap();
        assert!(ok.ok);
        assert_eq!(
            ok.content.get("livemode").and_then(|v| v.as_bool()),
            Some(false)
        );

        // Model cannot flip to live
        let err = call_tool(&b, "stripe.scope", &json!({ "livemode": true }));
        assert!(err.is_err(), "expected stripe livemode freeze: {err:?}");
        let msg = err.unwrap_err().to_string();
        assert!(
            msg.contains("scope freeze") && msg.contains("livemode"),
            "unexpected: {msg}"
        );

        // account_id freeze still enforced
        let err2 = call_tool(
            &b,
            "stripe.scope",
            &json!({ "account_id": "acct_evil", "livemode": false }),
        );
        assert!(err2.is_err());
        assert!(err2.unwrap_err().to_string().contains("account_id"));
    }

    #[test]
    fn freeze_aws_account_id() {
        let b = multi_provider();
        let ok = call_tool(&b, "aws.scope", &json!({ "account_id": "123456789012" })).unwrap();
        assert!(ok.ok);
        assert_eq!(
            ok.content.get("account_id").and_then(|v| v.as_str()),
            Some("123456789012")
        );

        let err = call_tool(&b, "aws.scope", &json!({ "account_id": "999999999999" }));
        assert!(err.is_err(), "expected aws account_id freeze: {err:?}");
        let msg = err.unwrap_err().to_string();
        assert!(
            msg.contains("scope freeze") && msg.contains("account_id"),
            "unexpected: {msg}"
        );
    }

    #[test]
    fn freeze_rejects_camelcase_and_provider_native_alias_spellings() {
        // supabase: project_ref frozen → projectId / project_id / projectRef all deny
        let b = acme();
        for key in ["projectId", "project_id", "projectRef"] {
            let err = call_tool(&b, "supabase.scope", &json!({ key: "proj_evil" }));
            assert!(err.is_err(), "{key} must be frozen");
            let msg = err.unwrap_err().to_string();
            assert!(
                msg.contains("scope freeze") && msg.contains("project_ref"),
                "unexpected: {msg}"
            );
        }
        // Matching alias value passes through the freeze
        let ok = call_tool(&b, "supabase.scope", &json!({ "projectId": "proj_acme" })).unwrap();
        assert!(ok.ok);

        // vercel: teamId alias frozen
        let vb = vercel_binding();
        let err = call_tool(&vb, "vercel.scope", &json!({ "teamId": "team_evil" }));
        assert!(err.is_err(), "teamId must be frozen");
        assert!(err.unwrap_err().to_string().contains("team_id"));

        // cloudflare / aws / stripe: accountId + provider-native spellings
        let mb = multi_provider();
        let err = call_tool(
            &mb,
            "cloudflare.scope",
            &json!({ "accountId": "cf_acct_evil" }),
        );
        assert!(err.is_err(), "cloudflare accountId must be frozen");
        let err = call_tool(&mb, "aws.scope", &json!({ "awsAccountId": "999999999999" }));
        assert!(err.is_err(), "aws awsAccountId must be frozen");
        let err = call_tool(
            &mb,
            "stripe.scope",
            &json!({ "stripe_account": "acct_evil" }),
        );
        assert!(err.is_err(), "stripe_account must be frozen");
    }

    #[test]
    fn freeze_scans_shallow_nested_selector_objects() {
        let b = acme();
        let err = call_tool(
            &b,
            "supabase.scope",
            &json!({ "options": { "projectId": "proj_evil" } }),
        );
        assert!(err.is_err(), "nested alias must freeze: {err:?}");
        assert!(err.unwrap_err().to_string().contains("scope freeze"));
        let ok = call_tool(
            &b,
            "supabase.scope",
            &json!({ "options": { "projectId": "proj_acme" } }),
        )
        .unwrap();
        assert!(ok.ok);
    }

    #[test]
    fn freeze_scans_nested_canonical_arrays_and_deep_objects() {
        let b = acme();
        // canonical snake_case key nested inside an option object
        let err = call_tool(
            &b,
            "supabase.scope",
            &json!({ "options": { "project_ref": "proj_evil" } }),
        );
        assert!(err.is_err(), "nested canonical key must freeze: {err:?}");
        assert!(err.unwrap_err().to_string().contains("scope freeze"));
        // object inside an array
        let err = call_tool(
            &b,
            "supabase.scope",
            &json!({ "filters": [{ "projectId": "proj_evil" }] }),
        );
        assert!(err.is_err(), "array-nested alias must freeze: {err:?}");
        // deeper than one level
        let err = call_tool(
            &b,
            "supabase.scope",
            &json!({ "a": { "b": { "c": { "projectId": "proj_evil" } } } }),
        );
        assert!(err.is_err(), "deep-nested alias must freeze: {err:?}");
        // matching values pass at any depth
        let ok = call_tool(
            &b,
            "supabase.scope",
            &json!({ "filters": [{ "project_ref": "proj_acme" }] }),
        )
        .unwrap();
        assert!(ok.ok);
    }

    #[test]
    fn freeze_denies_non_string_selector_values() {
        let b = acme();
        for args in [
            json!({ "projectId": 123 }),
            json!({ "project_ref": 123 }),
            json!({ "options": { "projectId": true } }),
            json!({ "projectId": ["proj_evil"] }),
        ] {
            let err = call_tool(&b, "supabase.scope", &args);
            assert!(err.is_err(), "non-string selector must deny: {args}");
            assert!(err.unwrap_err().to_string().contains("scope freeze"));
        }
        // explicit null carries no selector — the frozen value applies
        let ok = call_tool(&b, "supabase.scope", &json!({ "projectId": null })).unwrap();
        assert!(ok.ok);
    }

    #[test]
    fn freeze_stripe_livemode_nested_and_org_arrays() {
        let mb = multi_provider();
        // nested livemode flip denies
        let err = call_tool(
            &mb,
            "stripe.scope",
            &json!({ "options": { "livemode": true } }),
        );
        assert!(err.is_err(), "nested livemode flip must deny: {err:?}");
        assert!(err.unwrap_err().to_string().contains("livemode"));
        // non-bool livemode spelling denies (type mismatch)
        let err = call_tool(&mb, "stripe.scope", &json!({ "livemode": "true" }));
        assert!(err.is_err(), "non-bool livemode must deny: {err:?}");
        // matching nested livemode passes
        let ok = call_tool(
            &mb,
            "stripe.scope",
            &json!({ "options": { "livemode": false } }),
        )
        .unwrap();
        assert!(ok.ok);

        let gb = github_binding();
        let err = call_tool(
            &gb,
            "github.scope",
            &json!({ "items": [{ "owner": "evil-corp" }] }),
        );
        assert!(err.is_err(), "array-nested owner must deny: {err:?}");
        let err = call_tool(&gb, "github.scope", &json!({ "owner": 42 }));
        assert!(err.is_err(), "non-string owner must deny: {err:?}");
    }

    #[test]
    fn freeze_denies_args_nested_beyond_scan_depth() {
        let b = acme();
        let mut v = json!({ "projectId": "proj_evil" });
        for _ in 0..40 {
            v = json!({ "wrap": v });
        }
        let err = call_tool(&b, "supabase.scope", &v);
        assert!(
            err.is_err(),
            "over-deep args must deny fail closed: {err:?}"
        );
        assert!(err.unwrap_err().to_string().contains("scope freeze"));
    }

    #[test]
    fn freeze_owner_org_aliases_against_frozen_orgs() {
        let b = github_binding();
        let err = call_tool(&b, "github.scope", &json!({ "owner": "evil-corp" }));
        assert!(err.is_err(), "owner outside frozen orgs must deny");
        assert!(err.unwrap_err().to_string().contains("scope freeze"));
        let ok = call_tool(&b, "github.scope", &json!({ "owner": "acme-corp" })).unwrap();
        assert!(ok.ok);
        // Shallow nested object spelling
        let err = call_tool(
            &b,
            "github.scope",
            &json!({ "repository": { "owner": "evil-corp" } }),
        );
        assert!(err.is_err(), "nested owner outside frozen orgs must deny");
    }

    #[test]
    fn unknown_provider_tool_denied_fail_closed() {
        let b = acme(); // only supabase declared
        for tool in ["vercel.scope", "doesnotexist.anything"] {
            let err = call_tool(&b, tool, &json!({}));
            assert!(err.is_err(), "{tool} must be denied");
            let msg = err.unwrap_err().to_string();
            assert!(
                msg.contains("not part of the pinned binding"),
                "unexpected: {msg}"
            );
        }
        // enforce_policy alone (upstream worker path) also denies before workers.
        let err = enforce_policy(&b, "github.create_issue", &json!({}), None);
        assert!(err.is_err(), "upstream path must deny undeclared provider");
    }

    #[test]
    fn require_approval_blocks_delete() {
        let b = acme();
        let r = call_tool(&b, "supabase.table.delete", &json!({"table": "users"})).unwrap();
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
            &json!({ "table": "users", "confirm": true }),
        )
        .unwrap();
        assert!(!r.ok);
        assert_eq!(
            r.content.get("error").and_then(|v| v.as_str()),
            Some("requires_approval")
        );
    }

    #[test]
    fn supabase_project_ref_freeze_on_destructive_tool() {
        // Policy must allow so adapter freeze path is exercised (policy runs first).
        let b = Binding::from_body(BindingBody {
            id: "bnd_sb".into(),
            alias: "sb".into(),
            tenant: "acme-corp".into(),
            principal: None,
            description: None,
            policy: Policy::default(), // allow destructive stubs
            providers: vec![ProviderBinding {
                provider: "supabase".into(),
                account: "acme".into(),
                credential_ref: "phm:SUPABASE_ACME".into(),
                scope: Scope {
                    project_ref: Some("proj_acme".into()),
                    read_only: Some(true),
                    ..Scope::default()
                },
                upstream: None,
            }],
        });
        // project_ref freeze must apply to table.delete, not only scope tools
        let err = call_tool(
            &b,
            "supabase.table.delete",
            &json!({ "table": "users", "project_ref": "proj_evil" }),
        );
        assert!(
            err.is_err(),
            "expected project_ref freeze on table.delete: {err:?}"
        );
        let msg = err.unwrap_err().to_string();
        assert!(
            msg.contains("scope freeze") && msg.contains("project_ref"),
            "unexpected: {msg}"
        );

        // Matching ref is allowed through freeze (synthetic stub succeeds)
        let ok = call_tool(
            &b,
            "supabase.table.delete",
            &json!({ "table": "users", "project_ref": "proj_acme" }),
        )
        .unwrap();
        assert!(ok.ok);
        assert_eq!(
            ok.content.get("project_ref").and_then(|v| v.as_str()),
            Some("proj_acme")
        );
    }

    fn github_binding() -> Binding {
        Binding::from_body(BindingBody {
            id: "bnd_gh".into(),
            alias: "gh".into(),
            tenant: "acme-corp".into(),
            principal: None,
            description: None,
            policy: Policy::default(),
            providers: vec![ProviderBinding {
                provider: "github".into(),
                account: "acme-gh".into(),
                credential_ref: "phm:GH_ACME".into(),
                scope: Scope {
                    orgs: vec!["acme-corp".into()],
                    repos: vec!["acme-corp/web".into(), "acme-corp/api".into()],
                    ..Scope::default()
                },
                upstream: None,
            }],
        })
    }

    #[test]
    fn github_check_repo_allow_and_deny() {
        let b = github_binding();
        let ok = call_tool(
            &b,
            "github.check_repo",
            &json!({ "full_name": "acme-corp/web" }),
        )
        .unwrap();
        assert!(ok.ok);
        assert_eq!(
            ok.content.get("allowed").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            ok.content.get("full_name").and_then(|v| v.as_str()),
            Some("acme-corp/web")
        );

        let ok2 = call_tool(
            &b,
            "github.check_repo",
            &json!({ "org": "acme-corp", "repo": "api" }),
        )
        .unwrap();
        assert!(ok2.ok);

        // wrong org
        let err = call_tool(
            &b,
            "github.check_repo",
            &json!({ "full_name": "evil-corp/web" }),
        );
        assert!(err.is_err(), "expected org freeze: {err:?}");
        assert!(err.unwrap_err().to_string().contains("scope freeze"));

        // right org, wrong repo
        let err2 = call_tool(
            &b,
            "github.check_repo",
            &json!({ "org": "acme-corp", "repo": "secrets" }),
        );
        assert!(err2.is_err(), "expected repo freeze: {err2:?}");
        let msg = err2.unwrap_err().to_string();
        assert!(
            msg.contains("scope freeze") && msg.contains("repo"),
            "unexpected: {msg}"
        );
    }

    fn vercel_binding() -> Binding {
        Binding::from_body(BindingBody {
            id: "bnd_vc".into(),
            alias: "vc".into(),
            tenant: "acme-corp".into(),
            principal: None,
            description: None,
            policy: Policy {
                require_approval: vec!["vercel.deploy.prod".into()],
                ..Policy::default()
            },
            providers: vec![ProviderBinding {
                provider: "vercel".into(),
                account: "acme-vc".into(),
                credential_ref: "phm:VERCEL_ACME".into(),
                scope: Scope {
                    team_id: Some("team_acme".into()),
                    projects: vec!["acme-web".into()],
                    env: vec!["preview".into(), "development".into()],
                    ..Scope::default()
                },
                upstream: None,
            }],
        })
    }

    #[test]
    fn vercel_env_target_freeze() {
        let b = vercel_binding();
        let ok = call_tool(
            &b,
            "vercel.scope",
            &json!({ "team_id": "team_acme", "env": "preview" }),
        )
        .unwrap();
        assert!(ok.ok);
        assert_eq!(
            ok.content.get("env_target").and_then(|v| v.as_str()),
            Some("preview")
        );

        // production not in allowlist
        let err = call_tool(&b, "vercel.scope", &json!({ "env": "production" }));
        assert!(err.is_err(), "expected env freeze: {err:?}");
        let msg = err.unwrap_err().to_string();
        assert!(
            msg.contains("scope freeze") && (msg.contains("env") || msg.contains("production")),
            "unexpected: {msg}"
        );

        // target alias also frozen
        let err2 = call_tool(&b, "vercel.scope", &json!({ "target": "production" }));
        assert!(err2.is_err());

        // team_id still frozen
        let err3 = call_tool(
            &b,
            "vercel.scope",
            &json!({ "team_id": "team_evil", "env": "preview" }),
        );
        assert!(err3.is_err());
        assert!(err3.unwrap_err().to_string().contains("team_id"));
    }

    #[test]
    fn whoami_fields_complete_for_p2_providers() {
        let b = multi_provider();
        for tool in ["stripe.whoami", "aws.whoami", "cloudflare.whoami"] {
            let r = call_tool(&b, tool, &json!({})).unwrap();
            assert!(r.ok, "{tool} failed: {:?}", r.content);
            assert!(
                r.content.get("identity").is_some(),
                "{tool} missing identity"
            );
            assert!(
                r.content.get("frozen_selectors").is_some(),
                "{tool} missing frozen_selectors"
            );
            assert_eq!(
                r.content.get("tenant").and_then(|v| v.as_str()),
                Some("acme-corp")
            );
            assert!(r.content.get("credential_ref").is_none());
            assert_eq!(r.content["credential"]["present"], true);
            assert_eq!(r.content["credential"]["source"], "phantom");
        }

        // stripe mode label
        let s = call_tool(&b, "stripe.whoami", &json!({})).unwrap();
        assert_eq!(s.content.get("mode").and_then(|v| v.as_str()), Some("test"));
        assert_eq!(
            s.content.get("livemode").and_then(|v| v.as_bool()),
            Some(false)
        );

        // resend whoami with domain allowlist
        let resend = Binding::from_body(BindingBody {
            id: "bnd_rs".into(),
            alias: "rs".into(),
            tenant: "acme-corp".into(),
            principal: None,
            description: None,
            policy: Policy::default(),
            providers: vec![ProviderBinding {
                provider: "resend".into(),
                account: "acme-mail".into(),
                credential_ref: "phm:RESEND_ACME".into(),
                scope: Scope {
                    projects: vec!["acme.com".into()],
                    ..Scope::default()
                },
                upstream: None,
            }],
        });
        let r = call_tool(&resend, "resend.whoami", &json!({})).unwrap();
        assert!(r.ok);
        assert!(r.content.get("identity").is_some());
        assert!(r.content.get("domains").is_some());
        assert!(r.content.get("frozen_selectors").is_some());
    }

    #[test]
    fn control_tools_include_heartbeat_and_enter_hint() {
        let unbound = control_tools(false);
        let names: Vec<_> = unbound.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"locus_heartbeat"));
        assert!(names.contains(&"locus_enter_hint"));
        assert!(names.contains(&"locus_whoami"));
        assert!(names.contains(&"locus_safe_next"));
        assert!(names.contains(&"locus_verify_claim"));
        assert!(names.contains(&"locus_verify_session"));
        assert!(!names.contains(&"locus_providers"));
        let safe = unbound
            .iter()
            .find(|t| t.name == "locus_safe_next")
            .unwrap();
        assert!(safe.description.to_lowercase().contains("enter"));
        assert!(safe.description.to_lowercase().contains("approve"));
        let vs = unbound
            .iter()
            .find(|t| t.name == "locus_verify_session")
            .unwrap();
        assert!(vs.description.contains("session_ok"));
        assert!(vs.description.to_lowercase().contains("unpinned"));

        let pinned = control_tools(true);
        let names: Vec<_> = pinned.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"locus_heartbeat"));
        assert!(names.contains(&"locus_enter_hint"));
        assert!(names.contains(&"locus_safe_next"));
        assert!(names.contains(&"locus_verify_claim"));
        assert!(names.contains(&"locus_verify_session"));
        assert!(names.contains(&"locus_providers"));
    }

    #[test]
    fn provider_scope_responses_never_disclose_credential_refs() {
        let canary = "phm:DO_NOT_DISCLOSE_CREDENTIAL_REF_CANARY";
        let providers = [
            "supabase",
            "github",
            "vercel",
            "cloudflare",
            "stripe",
            "aws",
            "resend",
            "custom",
        ]
        .into_iter()
        .map(|provider| ProviderBinding {
            provider: provider.into(),
            account: format!("{provider}-account"),
            credential_ref: canary.into(),
            scope: Scope::default(),
            upstream: None,
        })
        .collect();
        let binding = Binding::from_body(BindingBody {
            id: "bnd_redaction".into(),
            alias: "redaction".into(),
            tenant: "redaction-tenant".into(),
            principal: None,
            description: None,
            policy: Policy::default(),
            providers,
        });

        for tool in [
            "supabase.scope",
            "github.scope",
            "vercel.scope",
            "cloudflare.scope",
            "stripe.scope",
            "aws.scope",
            "resend.scope",
            "custom.scope",
        ] {
            let result = call_tool(&binding, tool, &json!({})).unwrap();
            let serialized = serde_json::to_string(&result.content).unwrap();
            assert!(!serialized.contains(canary), "{tool} leaked credential ref");
            assert!(
                result.content.get("credential_ref").is_none(),
                "{tool} retained credential_ref field"
            );
            assert_eq!(result.content["credential"]["present"], true);
            assert_eq!(result.content["credential"]["source"], "phantom");
        }
    }
}

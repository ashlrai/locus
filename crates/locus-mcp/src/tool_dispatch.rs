//! Tool call validation, authorization, execution, and audit records.

use super::tool_catalog::swarm_scope_healthy;
use super::tool_control::call_control;
use super::{
    anchor, attach_anchor_context, audit_anchor_event, peers, pin_changed_refusal, primary_binding,
    resolve_scope_session, route, routes, rpc_error, runtime_unhealthy_hint, scope_drift, store,
    worker_manager, AnchorDecision, AnchorScope, RequestScope, SWARM_BROKER,
};
use locus_core::{
    call_tool_gated, control_tools, enforce_policy, split_namespaced_tool, tools_for_binding,
    ApprovalGate, Binding, Store,
};
use serde_json::{json, Value};

/// Control tools publish closed schemas (`additionalProperties: false`); hold
/// calls to them before dispatch. An unknown `locus_*` name, a non-object
/// `arguments`, or a key the schema does not declare is a JSON-RPC -32602,
/// the same answer an unknown provider tool gets. Value checks (blank or
/// non-string `alias`, ...) stay in the tools so they can name the next step.
fn check_control_args(name: &str, args: &Value) -> std::result::Result<(), Value> {
    let tools = control_tools(true);
    let Some(tool) = tools.iter().find(|t| t.name == name) else {
        // A mistyped control name (`locus_whoam`) names the tool it was
        // probably meant to be, so the agent retries instead of listing.
        let names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
        let hint = locus_core::nearest_alias(name, &names)
            .map(|n| format!(" Did you mean `{n}`?"))
            .unwrap_or_default();
        return Err(rpc_error(
            -32602,
            format!("unknown tool `{name}`: control tools are listed by tools/list.{hint}"),
        ));
    };
    let Some(obj) = args.as_object() else {
        return Err(rpc_error(
            -32602,
            format!("invalid arguments for `{name}`: expected a JSON object"),
        ));
    };
    let declared = tool
        .input_schema
        .get("properties")
        .and_then(Value::as_object);
    let mut undeclared: Vec<&str> = obj
        .keys()
        .map(String::as_str)
        .filter(|k| !declared.is_some_and(|d| d.contains_key(*k)))
        .collect();
    if undeclared.is_empty() {
        return Ok(());
    }
    undeclared.sort_unstable();
    let mut allowed: Vec<&str> = declared
        .map(|d| d.keys().map(String::as_str).collect())
        .unwrap_or_default();
    allowed.sort_unstable();
    let allowed_owned: Vec<String> = allowed.iter().map(|k| k.to_string()).collect();
    let suggestions: Vec<String> = undeclared
        .iter()
        .filter_map(|k| {
            locus_core::nearest_alias(k, &allowed_owned).map(|near| format!("`{k}` -> `{near}`"))
        })
        .collect();
    let hint = if suggestions.is_empty() {
        String::new()
    } else {
        format!(" Did you mean {}?", suggestions.join(", "))
    };
    Err(rpc_error(
        -32602,
        format!(
            "invalid arguments for `{name}`: undeclared argument(s) [{}]; the schema allows [{}] (additionalProperties: false).{hint}",
            undeclared.join(", "),
            allowed.join(", ")
        ),
    ))
}

pub(super) fn handle_tools_call(
    params: &Value,
    scope: &RequestScope,
) -> std::result::Result<Value, Value> {
    let name = params
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| rpc_error(-32602, "missing tool name".into()))?;
    if let Some(broker) = SWARM_BROKER.get() {
        let s = store().map_err(|_| rpc_error(-32000, "swarm store unavailable".into()))?;
        let healthy = swarm_scope_healthy(&s, scope);
        let allowed = broker
            .authorize_call(&s, scope.tenant.as_ref(), name, healthy)
            .map_err(|_| rpc_error(-32000, "swarm audit unavailable".into()))?;
        if !allowed {
            return Err(rpc_error(
                -32003,
                "tool unavailable for this swarm agent".into(),
            ));
        }
    }
    let args = match params.get("arguments") {
        None | Some(Value::Null) => json!({}),
        Some(v) => v.clone(),
    };

    // Control tools (allowed even when frozen - whoami/status/heartbeat report freeze)
    if name.starts_with("locus_") {
        check_control_args(name, &args)?;
        return call_control(name, &args, scope);
    }

    // Every provider tool is `provider.tool` (namespaced: `alias__provider.tool`),
    // so a dotless name is unknown to every binding. Say so before the runtime
    // health checks: the answer needs no pin and reveals nothing about one.
    if !name.contains('.') {
        return Err(rpc_error(
            -32602,
            format!(
                "unknown tool `{name}`: provider tools are named `provider.tool` and control tools `locus_*`; see tools/list"
            ),
        ));
    }

    // Provider tools require a healthy pin
    let s = store().map_err(|e| rpc_error(-32000, e.to_string()))?;

    // Continuous drift check - freezes session if binding file mutated.
    let drift = scope_drift(&s, scope).map_err(|e| rpc_error(-32000, e.to_string()))?;

    // Tenant grant death (revoked / expired / swept) outranks every other
    // refusal - never a silent identity fallback.
    if let Some(t) = &scope.tenant {
        if let Some(issue) = drift
            .issues
            .iter()
            .find(|i| i.starts_with("grant_"))
            .cloned()
        {
            return Ok(tool_text(
                json!({
                    "error": issue,
                    "grant_id": t.grant_id,
                    "safe_next": format!("locus mcp mint --binding {}", t.binding_alias),
                    "hint": "this tenant grant is no longer valid; the operator must mint a fresh token",
                }),
                true,
            ));
        }
    }

    // Anchored-identity check BEFORE the drift early-returns: a cross-process
    // re-pin stales the executor grant, so drift is already unhealthy - the
    // wrong-account refusal (`pin_changed`) must outrank `runtime_unhealthy`.
    // RuntimeDrift carries identity fields even when unhealthy; drift.issues
    // ride along as underlying_issues so the authority-plane facts stay
    // visible. Establishment never happens here (unhealthy observations).
    let session_anchor = scope.anchor.get();
    if let (Some(anchored), true) = (&session_anchor, drift.pinned) {
        if let Some(current) = anchor::drift_observation(&drift) {
            if !anchored.same_primary_identity(&current) {
                return Ok(pin_changed_refusal(
                    &scope.anchor,
                    anchored,
                    &current,
                    &drift.issues,
                ));
            }
        }
    }

    // Fail closed on any unhealthy runtime (invalid seal, freeze, expiry, drift).
    if !drift.ok {
        if !drift.pinned {
            let mut body = json!({
                "error": "not_pinned",
                "issues": drift.issues,
                "hint": "Human must run: locus enter <alias> (or `locus pin <alias>`). Agents: locus_enter_hint / locus_request_pin."
            });
            attach_anchor_context(&mut body, session_anchor.as_ref());
            return Ok(tool_text(body, true));
        }
        if !drift.seal_ok {
            return Ok(tool_text(
                json!({
                    "error": "invalid_seal",
                    "issues": drift.issues,
                    "hint": "Session seal is invalid. Human must re-pin: `locus leave` then `locus pin <alias>`.",
                }),
                true,
            ));
        }
        if drift.frozen {
            return Ok(tool_text(
                json!({
                    "error": "session_frozen: re-pin",
                    "reason": drift.issues,
                    "hint": "Binding changed under the active pin. Human must run `locus leave` then `locus pin <alias>`.",
                }),
                true,
            ));
        }
        if drift.expired {
            if let Some(t) = &scope.tenant {
                return Ok(tool_text(
                    json!({
                        "error": "grant_expired",
                        "grant_id": t.grant_id,
                        "issues": drift.issues,
                        "safe_next": format!("locus mcp mint --binding {}", t.binding_alias),
                        "hint": "grant TTL elapsed; the operator must mint a fresh tenant token",
                    }),
                    true,
                ));
            }
            return Ok(tool_text(
                json!({
                    "error": "session_expired",
                    "issues": drift.issues,
                    "hint": "Pin TTL expired. Human must re-pin: `locus pin <alias>`.",
                }),
                true,
            ));
        }
        return Ok(tool_text(
            json!({
                "error": "runtime_unhealthy",
                "hint": runtime_unhealthy_hint(&drift.issues),
                "issues": drift.issues,
            }),
            true,
        ));
    }

    let pinned = resolve_scope_session(scope).map_err(|e| rpc_error(-32000, e.to_string()))?;
    let Some((session, bindings)) = pinned else {
        let mut body = json!({
            "error": "not_pinned",
            "hint": "Human must run: locus pin <alias>. Agents: call locus_request_pin or locus_enter_hint."
        });
        attach_anchor_context(&mut body, session_anchor.as_ref());
        return Ok(tool_text(body, true));
    };

    if session.is_frozen() {
        return Ok(tool_text(
            json!({
                "error": "session_frozen: re-pin",
                "reason": session.frozen_reason,
                "hint": "Human must re-pin after binding drift."
            }),
            true,
        ));
    }

    // Healthy-path anchor observe against the SAME loaded (session, bindings)
    // handed to the provider gate below (window identical to today's drift
    // window; no store mutation). Establish on first healthy observation;
    // same-identity re-pin (`locus enter <same>`) re-anchors silently;
    // different identity fails closed with `pin_changed`.
    let obs = anchor::observation(&session, &bindings);
    match scope.anchor.observe(&obs, true) {
        Some(AnchorDecision::Established) => {
            audit_anchor_event(
                "mcp.anchor_established",
                &obs.binding_alias,
                json!({ "anchor": anchor::identity_json(&obs) }),
            );
        }
        Some(AnchorDecision::Repinned) => {
            audit_anchor_event(
                "mcp.anchor_repin",
                &obs.binding_alias,
                json!({ "anchor": anchor::identity_json(&obs) }),
            );
        }
        Some(AnchorDecision::Mismatch { anchored }) => {
            return Ok(pin_changed_refusal(&scope.anchor, &anchored, &obs, &[]));
        }
        Some(AnchorDecision::Match) | None => {}
    }

    if name.starts_with("locus.peers.") {
        return peers::call(scope, &session, &bindings, name, &args);
    }

    // Resolve target binding + un-prefixed tool name for namespaced sessions.
    let (binding, tool_name): (&Binding, &str) = if session.is_namespaced() {
        match split_namespaced_tool(name) {
            Some((alias, rest)) => {
                let b = bindings
                    .iter()
                    .find(|(a, _)| a == alias)
                    .map(|(_, b)| b)
                    .ok_or_else(|| {
                        rpc_error(
                            -32602,
                            format!("unknown namespace alias `{alias}` for tool `{name}`"),
                        )
                    })?;
                // Alias must be in this session
                if !session.all_aliases().iter().any(|a| a == alias) {
                    return Ok(tool_text(
                        json!({
                            "error": "namespace_not_in_session",
                            "alias": alias,
                            "tool": name,
                        }),
                        true,
                    ));
                }
                (b, rest)
            }
            None => {
                return Ok(tool_text(
                    json!({
                        "error": "namespaced_tool_required",
                        "detail": "This session is namespaced; call tools as `alias__tool` (e.g. acme__github.scope).",
                        "tool": name,
                        "namespaces": session.all_aliases(),
                    }),
                    true,
                ));
            }
        }
    } else {
        let b = primary_binding(&bindings)
            .ok_or_else(|| rpc_error(-32000, "pinned session has no bindings".into()))?;
        (b, name)
    };

    if let Err(error) = routes().validate(&session, &bindings) {
        return Ok(tool_text(
            json!({"error": "invalid_route", "detail": error.to_string()}),
            true,
        ));
    }
    if !routes().allows_tool(tool_name) {
        return Ok(tool_text(
            json!({"error": "route_not_enabled", "tool": name, "hint": "This provider is not selected by the server's launch configuration."}),
            true,
        ));
    }

    let principal_owned = session.principal.clone();
    let gate = ApprovalGate {
        store: &s,
        session_id: &session.session_id,
        principal: principal_owned.as_deref(),
    };

    // Signed community admission precedes credentials, worker startup and routing.
    let args = if let Some(provider) = binding
        .providers
        .iter()
        .find(|provider| tool_name.starts_with(&format!("{}.", provider.provider)))
    {
        match provider.community_tool_args(&binding.policy, tool_name, &args) {
            Ok(scoped) => scoped,
            Err(error) => {
                return Ok(tool_text(
                    json!({"error":"community_admission_denied", "detail":error.to_string(), "tool":name}),
                    true,
                ))
            }
        }
    } else {
        args
    };

    let synthetic = tools_for_binding(binding);
    let is_synthetic = synthetic.iter().any(|t| t.name == tool_name);

    if is_synthetic {
        match call_tool_gated(binding, tool_name, &args, Some(gate)) {
            Ok(r) => {
                let ticket = if r.ok {
                    s.mint_capability_ticket(&session.session_id, &binding.id, tool_name)
                        .ok()
                } else {
                    None
                };
                let ticket_id = ticket.as_ref().map(|t| t.ticket_id.as_str());
                audit_tool_block(&s, &binding.alias, tool_name, &r.content);
                audit_tool_call(
                    &s,
                    scope,
                    &binding.alias,
                    tool_name,
                    &session.session_id,
                    ticket_id,
                    r.ok,
                );
                Ok(tool_text(r.content, !r.ok))
            }
            Err(e) => Ok(tool_text(
                scope_or_err(&s, &binding.alias, tool_name, &args, e),
                true,
            )),
        }
    } else {
        // Upstream (or unknown) tool: policy + worker fan-out.
        match enforce_policy(binding, tool_name, &args, Some(gate)) {
            Ok(Some(blocked)) => {
                audit_tool_block(&s, &binding.alias, tool_name, &blocked.content);
                Ok(tool_text(blocked.content, true))
            }
            Ok(None) => {
                let Some(provider) = tool_name.split('.').next().filter(|p| !p.is_empty()) else {
                    return Ok(tool_text(
                        json!({ "error": "invalid_tool_name", "tool": name }),
                        true,
                    ));
                };
                if let Err(error) = route::check_upstream(binding, provider) {
                    return Ok(tool_text(
                        json!({"error": "worker_ensure_failed", "detail": error.to_string(), "tool": name}),
                        true,
                    ));
                }
                let mut mgr = worker_manager()
                    .lock()
                    .map_err(|_| rpc_error(-32000, "worker manager lock poisoned".into()))?;
                if let Err(e) = mgr.ensure_provider(&session, binding, provider) {
                    return Ok(tool_text(
                        json!({
                            "error": "worker_ensure_failed",
                            "detail": e.to_string(),
                            "tool": name,
                        }),
                        true,
                    ));
                }
                let ticket = s
                    .mint_capability_ticket(&session.session_id, &binding.id, tool_name)
                    .ok();
                let ticket_id = ticket.as_ref().map(|t| t.ticket_id.as_str());
                match mgr.call_tool(&session, binding, tool_name, &args) {
                    Ok(r) => {
                        audit_tool_call(
                            &s,
                            scope,
                            &binding.alias,
                            tool_name,
                            &session.session_id,
                            ticket_id,
                            r.ok,
                        );
                        Ok(tool_text(r.content, !r.ok))
                    }
                    Err(e) => Ok(tool_text(
                        json!({ "error": e.to_string(), "tool": name }),
                        true,
                    )),
                }
            }
            Err(e) => Ok(tool_text(json!({ "error": e.to_string() }), true)),
        }
    }
}

/// Audit successful tools/call path with optional capability `ticket_id` (not a secret).
/// Multi-tenant requests additionally stamp grant_id + http_session_id.
fn audit_tool_call(
    s: &Store,
    scope: &RequestScope,
    alias: &str,
    tool: &str,
    session_id: &str,
    ticket_id: Option<&str>,
    ok: bool,
) {
    let mut detail = json!({
        "tool": tool,
        "session_id": session_id,
        "ticket_id": ticket_id,
        "ok": ok,
    });
    if let Some(t) = &scope.tenant {
        detail["grant_id"] = json!(t.grant_id);
        if let AnchorScope::Http(id) = &scope.anchor {
            detail["http_session_id"] = json!(id);
        }
    }
    let _ = s.audit("mcp.tools_call", alias, Some(detail));
}

fn audit_tool_block(s: &Store, alias: &str, tool: &str, content: &Value) {
    // Never log raw tool args - digests + meta only (secrets stay out of audit).
    if let Some(err) = content.get("error").and_then(|v| v.as_str()) {
        if err == "requires_approval" {
            let _ = s.audit(
                "mcp.require_approval",
                alias,
                Some(json!({
                    "tool": tool,
                    "status": "pending",
                    "approval_id": content.get("approval_id"),
                    "args_digest": content.get("args_digest"),
                    "dual_control": content.get("dual_control"),
                    "grants": content.get("grants"),
                    "required_grants": content.get("required_grants"),
                    "approval_authority": content.get("approval_authority"),
                    "authoritative_path_enabled": content.get("authoritative_path_enabled"),
                    "detail": content.get("detail"),
                })),
            );
        }
    }
}

fn scope_or_err(
    s: &Store,
    alias: &str,
    tool: &str,
    args: &Value,
    e: locus_core::LocusError,
) -> Value {
    let msg = e.to_string();
    if msg.contains("scope freeze") {
        // Digest only - raw args may contain secrets or PII
        let _ = s.audit(
            "mcp.scope_freeze",
            alias,
            Some(json!({
                "tool": tool,
                "error": msg,
                "args_digest": locus_core::args_digest(args),
            })),
        );
    }
    json!({ "error": msg })
}

pub(super) fn tool_text(value: Value, is_error: bool) -> Value {
    // Compact JSON only - pretty-print embeds raw newlines that break NDJSON stdio.
    let text = match &value {
        Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_else(|_| other.to_string()),
    };
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn control_argument_validation_rejects_non_objects_and_unknown_keys() {
        for args in [Value::Null, json!([]), json!("alias")] {
            let error = check_control_args("locus_request_pin", &args).unwrap_err();
            assert_eq!(error["code"], -32602);
            assert!(error["message"]
                .as_str()
                .unwrap()
                .contains("expected a JSON object"));
        }
        let error = check_control_args("locus_request_pin", &json!({"z": 1, "a": 2})).unwrap_err();
        assert_eq!(error["code"], -32602);
        assert!(error["message"]
            .as_str()
            .unwrap()
            .contains("undeclared argument(s) [a, z]"));
        assert!(check_control_args("locus_request_pin", &json!({"alias": "fixture"})).is_ok());
    }

    #[test]
    fn mistyped_control_tool_and_argument_name_the_nearest_match() {
        let err = check_control_args("locus_whoam", &json!({})).unwrap_err();
        let msg = err["message"].as_str().unwrap();
        assert!(msg.starts_with("unknown tool `locus_whoam`"), "{msg}");
        assert!(msg.ends_with("Did you mean `locus_whoami`?"), "{msg}");

        let err = check_control_args("locus_nonsense_tool", &json!({})).unwrap_err();
        assert!(
            !err["message"].as_str().unwrap().contains("Did you mean"),
            "{err}"
        );

        let err = check_control_args("locus_request_pin", &json!({"aliass": "acme"})).unwrap_err();
        let msg = err["message"].as_str().unwrap();
        assert_eq!(err["code"], json!(-32602), "{err}");
        assert!(msg.contains("undeclared argument(s) [aliass]"), "{msg}");
        assert!(msg.ends_with("Did you mean `aliass` -> `alias`?"), "{msg}");
    }

    #[test]
    fn dotless_tool_name_is_unknown_before_runtime_health() {
        for name in ["nonexistent_tool", "acme__bogus", "githubscope"] {
            let err = handle_tools_call(
                &json!({ "name": name, "arguments": {} }),
                &RequestScope::process(),
            )
            .unwrap_err();
            assert_eq!(err["code"], json!(-32602), "{name}: {err}");
            let msg = err["message"].as_str().unwrap();
            assert!(msg.starts_with(&format!("unknown tool `{name}`")), "{msg}");
        }
    }
}

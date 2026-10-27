//! Control tool implementations. Identity and policy checks remain at the dispatch gate.

use super::tool_dispatch::tool_text;
use super::{
    cwd, has_executor_authority_issue, mcp_anchor_report, resource_bindings_json, rpc_error,
    scope_drift, scope_identity_observation, scope_whoami, store, unknown_binding_message,
    RequestScope, EXECUTOR_AUTHORITY_HINT, EXECUTOR_AUTHORITY_ISSUE,
};
use locus_core::{
    compute_safe_next, gather_doctor_external, validate_control_args, verify_claim, verify_session,
};
use serde_json::{json, Value};

/// Error body for `locus_whoami`. A server launched without an executor
/// capability (the plain client entry) cannot observe the pin at all; it must
/// not claim `pinned: false` or tell the human to re-pin, which never helps.
fn whoami_error_body(e: &locus_core::LocusError) -> Value {
    if matches!(e, locus_core::LocusError::ExecutorAuthorityUnavailable(_)) {
        return json!({
            "pinned": Value::Null,
            "status": EXECUTOR_AUTHORITY_ISSUE,
            "error": e.to_string(),
            "hint": EXECUTOR_AUTHORITY_HINT,
        });
    }
    json!({
        "pinned": false,
        "error": e.to_string(),
        "hint": "Run `locus pin <alias>` in this workspace. Agents: locus_enter_hint."
    })
}

pub(super) fn call_control(
    name: &str,
    args: &Value,
    scope: &RequestScope,
) -> std::result::Result<Value, Value> {
    // The control schemas are closed (additionalProperties: false); a
    // misspelled or mistyped argument is a tool error the model can correct,
    // never silently ignored.
    if let Err(e) = validate_control_args(name, args) {
        return Ok(tool_text(
            json!({
                "error": e.to_string(),
                "hint": format!("Retry `{name}` with only the arguments in its tools/list inputSchema."),
            }),
            true,
        ));
    }
    let s = store().map_err(|e| rpc_error(-32000, e.to_string()))?;
    // Heartbeat: detect drift and freeze when identity control tools are polled.
    if matches!(
        name,
        "locus_whoami"
            | "locus_status"
            | "locus_providers"
            | "locus_heartbeat"
            | "locus_safe_next"
            | "locus_verify_session"
    ) {
        let _ = scope_drift(&s, scope);
    }
    // Additive `mcp_anchor` block for identity-reporting control tools -
    // omitted entirely when this MCP session has no anchor (keeps unpinned
    // response shapes untouched). (report, current_matches_anchor).
    let anchor_report: Option<(Value, bool)> = if matches!(
        name,
        "locus_whoami"
            | "locus_status"
            | "locus_heartbeat"
            | "locus_safe_next"
            | "locus_verify_session"
    ) {
        scope.anchor.get().map(|a| {
            let who = scope_whoami(&s, scope).ok();
            let current = scope_identity_observation(&s, scope);
            mcp_anchor_report(&a, who.as_ref(), current.as_ref())
        })
    } else {
        None
    };
    let attach_report = |mut body: Value| -> Value {
        if let Some((report, _)) = &anchor_report {
            body["mcp_anchor"] = report.clone();
        }
        body
    };
    // Anchored-identity mismatch while a *different* pin is globally active
    // (not merely unpinned) - drives safe_next / verify_session overrides.
    let anchor_mismatch_active = anchor_report
        .as_ref()
        .map(|(report, matches)| {
            !matches
                && report
                    .get("current_alias")
                    .map(|v| !v.is_null())
                    .unwrap_or(false)
        })
        .unwrap_or(false);
    match name {
        "locus_safe_next" => {
            if let Some(t) = &scope.tenant {
                // Tenant-scoped: derived from this grant's drift only - never
                // the operator store's global safe_next.
                let drift = scope_drift(&s, scope).map_err(|e| rpc_error(-32000, e.to_string()))?;
                let dead = drift
                    .issues
                    .iter()
                    .find(|i| i.starts_with("grant_"))
                    .cloned();
                let (action, command, ready) = match (&dead, drift.ok) {
                    (Some(issue), _) => (
                        issue.clone(),
                        Some(format!("locus mcp mint --binding {}", t.binding_alias)),
                        false,
                    ),
                    (None, true) => ("proceed".to_string(), None, true),
                    (None, false) => (
                        "remint_grant".to_string(),
                        Some(format!("locus mcp mint --binding {}", t.binding_alias)),
                        false,
                    ),
                };
                let body = attach_report(json!({
                    "scope": "tenant",
                    "grant_id": t.grant_id,
                    "ready": ready,
                    "action": action,
                    "command": command,
                    "issues": drift.issues,
                }));
                return Ok(tool_text(body, !ready));
            }
            let next =
                compute_safe_next(&s, &cwd()).map_err(|e| rpc_error(-32000, e.to_string()))?;
            let mut body = attach_report(serde_json::to_value(&next).unwrap_or(json!({})));
            let mut is_err = !next.ready;
            if anchor_mismatch_active {
                // Session-local override: the only safe next action is to
                // re-initialize this client (or restore the anchored pin).
                let anchored_alias = anchor_report
                    .as_ref()
                    .and_then(|(r, _)| r["anchored_alias"].as_str())
                    .unwrap_or("")
                    .to_string();
                body["action"] = json!("reinitialize_client");
                body["ready"] = json!(false);
                body["command"] = json!(format!("locus enter {anchored_alias}"));
                is_err = true;
            } else if let Some(issues) = scope_drift(&s, scope)
                .ok()
                .map(|d| d.issues)
                .filter(|issues| !next.ready && has_executor_authority_issue(issues))
            {
                // Plain client entry: the pin is unobservable from this
                // process. `locus doctor` (operator capability) reports
                // healthy, so pointing there loops; name the real fix.
                body["action"] = json!("launch_supervised");
                body["command"] = json!("locus mcp --supervised");
                body["message"] = json!(EXECUTOR_AUTHORITY_HINT);
                body["issues"] = json!(issues);
                is_err = true;
            }
            // Informational: isError only when not ready so agents notice the gate.
            Ok(tool_text(body, is_err))
        }
        "locus_whoami" => match scope_whoami(&s, scope) {
            Ok(w) => {
                let mut body = serde_json::to_value(w).unwrap_or(json!({}));
                if let Some(t) = &scope.tenant {
                    body["grant_id"] = json!(t.grant_id);
                }
                Ok(tool_text(attach_report(body), false))
            }
            Err(e) => Ok(tool_text(attach_report(whoami_error_body(&e)), true)),
        },
        "locus_status" => {
            if let Some(t) = &scope.tenant {
                let drift = scope_drift(&s, scope).map_err(|e| rpc_error(-32000, e.to_string()))?;
                let body = attach_report(json!({
                    "pinned": drift.pinned,
                    "scope": "tenant",
                    "grant_id": t.grant_id,
                    "binding": drift.binding_alias,
                    "tenant": drift.tenant_session,
                    "session_id": drift.session_id,
                    "seal_ok": drift.seal_ok,
                    "expired": drift.expired,
                    "frozen": drift.frozen,
                    "issues": drift.issues,
                }));
                return Ok(tool_text(body, false));
            }
            let active = s
                .active_session()
                .map_err(|e| rpc_error(-32000, e.to_string()))?;
            match active {
                None => Ok(tool_text(
                    attach_report(json!({ "pinned": false, "status": "unpinned" })),
                    false,
                )),
                Some(session) => {
                    let key = s.seal_key().map_err(|e| rpc_error(-32000, e.to_string()))?;
                    let seal_ok = session.verify_seal(&key).is_ok();
                    // The HMAC seal alone does not make a pin usable: its
                    // authority anchor (broker) may be retired or unobservable
                    // from this process. Report the same runtime verdict the
                    // provider gate uses so `pinned: true, seal_ok: true`
                    // never reads as "ready" when every provider call refuses.
                    let drift = scope_drift(&s, scope).ok();
                    let ok = drift.as_ref().is_some_and(|d| d.ok);
                    let issues = drift.as_ref().map(|d| d.issues.clone()).unwrap_or_default();
                    let hint = if ok {
                        None
                    } else if has_executor_authority_issue(&issues) {
                        Some(EXECUTOR_AUTHORITY_HINT)
                    } else {
                        Some("pin present but not usable \u{2014} call locus_heartbeat / locus_safe_next; a human may need to re-pin (`locus enter <alias>`; if enter is refused, `locus leave --force` first)")
                    };
                    Ok(tool_text(
                        attach_report(json!({
                            "pinned": true,
                            "ok": ok,
                            "binding": session.binding_alias,
                            "tenant": session.tenant,
                            "session_id": session.session_id,
                            "seal_ok": seal_ok,
                            "authority_anchor_ok": drift.as_ref().is_some_and(|d| d.authority_anchor_ok),
                            "expired": session.is_expired(),
                            "frozen": session.frozen,
                            "frozen_reason": session.frozen_reason,
                            "mode": if session.is_namespaced() { "namespaced" } else { "exclusive" },
                            "namespaces": session.all_aliases(),
                            "issues": issues,
                            "hint": hint,
                        })),
                        !ok,
                    ))
                }
            }
        }
        "locus_heartbeat" => {
            // Doctor-lite: full RuntimeDrift + operator hint. Never secrets.
            let drift = scope_drift(&s, scope).map_err(|e| rpc_error(-32000, e.to_string()))?;
            let hint = if drift.ok {
                None
            } else if !drift.pinned {
                Some("not pinned \u{2014} human: `locus enter <alias>` (or `locus pin <alias>`)")
            } else if drift.frozen {
                Some("session frozen after drift \u{2014} human: `locus leave` then `locus pin <alias>`")
            } else if !drift.seal_ok {
                Some("invalid seal \u{2014} human must re-pin")
            } else if drift.expired {
                Some("pin TTL expired \u{2014} human must re-pin")
            } else if has_executor_authority_issue(&drift.issues) {
                Some(EXECUTOR_AUTHORITY_HINT)
            } else {
                Some("runtime unhealthy \u{2014} run `locus doctor`")
            };
            let body = attach_report(json!({
                "ok": drift.ok,
                "pinned": drift.pinned,
                "seal_ok": drift.seal_ok,
                "frozen": drift.frozen,
                "expired": drift.expired,
                "binding": drift.binding_alias,
                "tenant": drift.tenant_session,
                "binding_id_match": drift.binding_id_match,
                "tenant_match": drift.tenant_match,
                "providers_match": drift.providers_match,
                "issues": drift.issues,
                "providers": drift.providers,
                "hint": hint,
                "runtime": drift,
            }));
            // Informational probe - isError only when unhealthy so agents notice.
            Ok(tool_text(body, !drift.ok))
        }
        "locus_enter_hint" if scope.tenant.is_some() => Ok(tool_text(
            json!({
                "error": "tenant_fixed_by_grant",
                "hint": "this session's tenant is fixed by its operator-minted grant; a different binding needs a new grant token (`locus mcp mint`)",
            }),
            true,
        )),
        "locus_enter_hint" => {
            let alias = args
                .get("alias")
                .and_then(|a| a.as_str())
                .map(str::trim)
                .filter(|a| !a.is_empty());
            let (enter_cmd, pin_cmd) = match alias {
                Some(a) => (format!("locus enter {a}"), format!("locus pin {a}")),
                None => ("locus enter".into(), "locus pin".into()),
            };
            let exists = alias.map(|a| s.load_binding(a).is_ok());
            if let Some(a) = alias {
                let _ = s.audit(
                    "mcp.enter_hint",
                    a,
                    Some(json!({ "binding_exists": exists })),
                );
            }
            if let (Some(a), Some(false)) = (alias, exists) {
                let (message, suggestion) = unknown_binding_message(&s, a);
                return Ok(tool_text(
                    json!({
                        "agents_cannot_pin": true,
                        "alias": a,
                        "binding_exists": false,
                        "did_you_mean": suggestion,
                        "message": message,
                    }),
                    true,
                ));
            }
            let message = match alias {
                Some(_) => format!(
                    "Agents cannot pin. Ask the human to run `{enter_cmd}` (or `{pin_cmd}`) in a terminal, then continue."
                ),
                None => "Agents cannot pin. Ask the human to run `locus enter <alias>` (or `locus pin <alias>`) in a terminal.".into(),
            };
            Ok(tool_text(
                json!({
                    "agents_cannot_pin": true,
                    "command": enter_cmd,
                    "pin_command": pin_cmd,
                    "alias": alias,
                    "binding_exists": exists,
                    "message": message,
                }),
                false,
            ))
        }
        "locus_list_bindings" => {
            // Tenant scopes see only their own grant's aliases (never the
            // operator's full binding roster).
            let list = resource_bindings_json(scope)?;
            Ok(tool_text(list, false))
        }
        "locus_request_pin" if scope.tenant.is_some() => {
            let t = scope.tenant.as_ref().expect("guarded");
            let _ = s.audit(
                "mcp.request_pin",
                &t.binding_alias,
                Some(json!({ "refused": "tenant_fixed_by_grant", "grant_id": t.grant_id })),
            );
            Ok(tool_text(
                json!({
                    "error": "tenant_fixed_by_grant",
                    "grant_id": t.grant_id,
                    "hint": "this session's tenant is fixed by its operator-minted grant; the operator must mint a new grant token for a different binding (`locus mcp mint --binding <alias>`)",
                }),
                true,
            ))
        }
        "locus_request_pin" => {
            let alias = args
                .get("alias")
                .and_then(|a| a.as_str())
                .map(str::trim)
                .unwrap_or("");
            if alias.is_empty() {
                return Ok(tool_text(
                    json!({
                        "error": "alias required",
                        "hint": "pass {\"alias\": \"<binding alias>\"}; locus_list_bindings lists the configured aliases",
                    }),
                    true,
                ));
            }
            // Verify binding exists
            let exists = s.load_binding(alias).is_ok();
            let _ = s.audit("mcp.request_pin", alias, Some(json!({ "exists": exists })));
            if !exists {
                // Audited above for forensics, but there is nothing to pin:
                // no request is pending and no `locus enter` would succeed.
                let (message, suggestion) = unknown_binding_message(&s, alias);
                return Ok(tool_text(
                    json!({
                        "requested": alias,
                        "binding_exists": false,
                        "did_you_mean": suggestion,
                        "message": message,
                    }),
                    true,
                ));
            }
            Ok(tool_text(
                json!({
                    "requested": alias,
                    "binding_exists": exists,
                    "message": format!(
                        "Pin request recorded for `{alias}`. Agents cannot pin themselves. \
                         Human: run `locus enter {alias}` (or `locus pin {alias}`) in the terminal, then continue."
                    ),
                    "command": format!("locus enter {alias}"),
                    "pin_command": format!("locus pin {alias}"),
                }),
                false,
            ))
        }
        "locus_providers" => match scope_whoami(&s, scope) {
            Ok(w) => Ok(tool_text(json!({ "providers": w.providers }), false)),
            // Same body as locus_whoami so an unobservable pin names the
            // supervised launch instead of a bare error string.
            Err(e) => Ok(tool_text(whoami_error_body(&e), true)),
        },
        "locus_verify_claim" => {
            let text = args
                .get("text")
                .or_else(|| args.get("claim"))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|t| !t.is_empty());
            let Some(text) = text else {
                return Ok(tool_text(
                    json!({ "error": "text (or claim) required \u{2014} free-text assertion to score" }),
                    true,
                ));
            };
            let who = scope_whoami(&s, scope).ok();
            let result = verify_claim(text, who.as_ref());
            let binding = who
                .as_ref()
                .map(|w| w.binding_alias.as_str())
                .unwrap_or("-");
            let _ = s.audit(
                "mcp.verify_claim",
                binding,
                Some(json!({
                    "confidence": result.confidence.as_str(),
                    "needs_tool": result.needs_tool,
                    "signals": result.signals,
                    // Truncate claim in audit - never store huge blobs.
                    "claim_len": result.claim.len(),
                    "claim_preview": result.claim.chars().take(120).collect::<String>(),
                })),
            );
            Ok(tool_text(
                serde_json::to_value(&result).unwrap_or(json!({})),
                false,
            ))
        }
        "locus_verify_session" => {
            if let Some(t) = &scope.tenant {
                // Tenant-scoped pack: this grant's drift + whoami only. The
                // full operator pack (global doctor / global pin) must never
                // reach a tenant session.
                let drift = scope_drift(&s, scope).map_err(|e| rpc_error(-32000, e.to_string()))?;
                let who = scope_whoami(&s, scope).ok();
                let session_ok = drift.ok && who.is_some();
                let body = attach_report(json!({
                    "scope": "tenant",
                    "grant_id": t.grant_id,
                    "session_ok": session_ok,
                    "whoami": who,
                    "runtime": drift,
                    "safe_next": {
                        "action": if session_ok { "proceed" } else { "remint_grant" },
                        "ready": session_ok,
                        "command": if session_ok { Value::Null } else {
                            json!(format!("locus mcp mint --binding {}", t.binding_alias))
                        },
                    },
                }));
                let _ = s.audit(
                    "mcp.verify_session",
                    &t.binding_alias,
                    Some(json!({
                        "session_ok": session_ok,
                        "grant_id": t.grant_id,
                        "scope": "tenant",
                    })),
                );
                return Ok(tool_text(body, false));
            }
            // Same pack as `locus verify session --json`. Available unpinned.
            // isError only on hard store failures - agents/hub gate on session_ok.
            let external =
                gather_doctor_external(&s, cwd()).map_err(|e| rpc_error(-32000, e.to_string()))?;
            let pack = verify_session(&s, &cwd(), external)
                .map_err(|e| rpc_error(-32000, e.to_string()))?;
            let binding = pack
                .whoami
                .as_ref()
                .map(|w| w.binding_alias.as_str())
                .unwrap_or("-");
            let _ = s.audit(
                "mcp.verify_session",
                binding,
                Some(json!({
                    // Keys aligned with the CLI's `verify.session` audit event.
                    "session_ok": pack.session_ok,
                    "safe_next": pack.safe_next.action,
                    "doctor_verdict": pack.doctor.verdict,
                    "doctor_ok": pack.doctor.ok,
                    "has_whoami": pack.whoami.is_some(),
                })),
            );
            let mut body = attach_report(serde_json::to_value(&pack).unwrap_or(json!({})));
            if anchor_mismatch_active {
                // Hub gating: an anchored-identity mismatch is a per-session
                // failure even when the global pin itself is healthy. The hub
                // gates on session_ok - force it false; isError stays false
                // (the pack itself returned fine).
                let anchored_alias = anchor_report
                    .as_ref()
                    .and_then(|(r, _)| r["anchored_alias"].as_str())
                    .unwrap_or("")
                    .to_string();
                body["session_ok"] = json!(false);
                body["mcp_anchor_mismatch"] = json!(true);
                body["safe_next"]["action"] = json!("reinitialize_client");
                body["safe_next"]["ready"] = json!(false);
                body["safe_next"]["command"] = json!(format!("locus enter {anchored_alias}"));
            }
            Ok(tool_text(body, false))
        }
        other => Ok(tool_text(
            json!({ "error": format!("unknown control tool: {other}") }),
            true,
        )),
    }
}

#[cfg(test)]
mod whoami_error_tests {
    use super::*;

    #[test]
    fn whoami_error_without_executor_authority_does_not_claim_unpinned() {
        let body = whoami_error_body(&locus_core::LocusError::ExecutorAuthorityUnavailable(
            "this process requires a supervised executor capability".into(),
        ));
        assert!(body["pinned"].is_null());
        assert_eq!(body["status"], "executor_authority_unavailable");
        let hint = body["hint"].as_str().unwrap();
        assert!(hint.contains("locus mcp --supervised"));
        assert!(!hint.contains("Run `locus pin"));

        let other = whoami_error_body(&locus_core::LocusError::NotPinned);
        assert_eq!(other["pinned"], false);
        assert!(other["hint"].as_str().unwrap().contains("locus pin"));
    }
}

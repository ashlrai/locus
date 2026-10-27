//! Tool discovery and presentation shaping for the active request scope.

use super::{
    anchor, anchor_matches_current, audit_anchor_event, maybe_mcp_auto_pin, resolve_scope_session,
    rpc_error, scope_drift, scope_identity_observation, store, tenant_grant_recheck,
    worker_manager, AnchorDecision, RequestScope, SWARM_BROKER,
};
use locus_core::{control_tools, split_namespaced_tool, AdapterTool, Binding, Session, Store};
use serde_json::{json, Value};

/// Map tools to MCP list payload; ensure `locus_whoami` first + locus tags.
fn tools_list_payload(mut tools: Vec<AdapterTool>, pin_alias: Option<&str>) -> Value {
    tag_tool_descriptions(&mut tools, pin_alias);
    ensure_whoami_first(&mut tools);
    let list: Vec<Value> = tools
        .into_iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "inputSchema": t.input_schema,
            })
        })
        .collect();
    json!({ "tools": list })
}

fn ensure_whoami_first(tools: &mut Vec<AdapterTool>) {
    if let Some(i) = tools.iter().position(|t| t.name == "locus_whoami") {
        if i != 0 {
            let t = tools.remove(i);
            tools.insert(0, t);
        }
    }
}

/// Prefix every tool description with `[locus:<alias|unpinned>]`.
fn tag_tool_descriptions(tools: &mut [AdapterTool], pin_alias: Option<&str>) {
    for t in tools.iter_mut() {
        if t.description.starts_with("[locus:") {
            continue;
        }
        let tag = if t.name.starts_with("locus_") {
            match pin_alias {
                Some(a) => format!("[locus:{a}]"),
                None => "[locus:unpinned]".into(),
            }
        } else if let Some((alias, _)) = split_namespaced_tool(&t.name) {
            format!("[locus:{alias}]")
        } else {
            match pin_alias {
                Some(a) => format!("[locus:{a}]"),
                None => "[locus:unpinned]".into(),
            }
        };
        // Drop legacy `[alias] ` prefix from namespaced soft-fail path if present.
        let rest = t.description.as_str();
        let rest = if rest.starts_with('[') && !rest.starts_with("[locus:") {
            if let Some(idx) = rest.find("] ") {
                &rest[idx + 2..]
            } else {
                rest
            }
        } else {
            rest
        };
        t.description = format!("{tag} {rest}");
    }
}

/// Policy-aware presentation shaping of the provider tool catalog.
///
/// Per tool: resolve the owning binding and the bare `provider.tool` name
/// (stripping the `alias__` namespace prefix so policy globs see the exact
/// string the call gate evaluates), then delegate to
/// [`locus_core::adapters::shape_catalog_tool`]:
/// - destructive tools of a `read_only = true` provider scope are omitted;
/// - `require_approval`-gated tools get a description marker.
///
/// Presentation only - call-time enforcement is untouched, and a hidden tool
/// invoked anyway still hits the same call-time denial. Control (`locus_*`)
/// tools never pass through here.
fn shape_provider_catalog(
    tools: Vec<AdapterTool>,
    session: &Session,
    bindings: &[(String, Binding)],
) -> Vec<AdapterTool> {
    let mut out = Vec::with_capacity(tools.len());
    for mut t in tools {
        // Resolve (binding, bare name) without holding a borrow on `t`.
        let resolved: Option<(&Binding, String)> = if session.is_namespaced() {
            split_namespaced_tool(&t.name).and_then(|(alias, bare)| {
                bindings
                    .iter()
                    .find(|(a, _)| a == alias)
                    .map(|(_, b)| (b, bare.to_string()))
            })
        } else {
            bindings.first().map(|(_, b)| (b, t.name.clone()))
        };
        let keep = match resolved {
            Some((binding, bare)) => {
                locus_core::adapters::shape_catalog_tool(binding, &bare, &mut t)
            }
            // Unresolvable owner (should not happen): leave listing untouched -
            // shaping is presentation-only and never a gate.
            None => true,
        };
        if keep {
            out.push(t);
        }
    }
    out
}

/// Unpinned / frozen / invalid seal ⇒ only locus_* control tools.
/// Healthy pin ⇒ control + provider tools (synthetic + upstream MCP when declared).
/// Namespaced multi-bind prefixes tools as `alias__tool`.
pub(super) fn handle_tools_list(scope: &RequestScope) -> std::result::Result<Value, Value> {
    if let Some(broker) = SWARM_BROKER.get() {
        let s = store().map_err(|_| rpc_error(-32000, "swarm store unavailable".into()))?;
        // Even control tools disappear when the grant/session is unhealthy.
        // The existing catalog builder still enforces every binding policy and
        // anchor check; the graph is an additional intersection, never a union.
        let catalog = if swarm_scope_healthy(&s, scope) {
            handle_binding_tools_list(scope)?
        } else {
            json!({"tools": []})
        };
        return broker
            .filter_catalog(&s, scope.tenant.as_ref(), catalog)
            .map_err(|_| rpc_error(-32000, "swarm audit unavailable".into()));
    }
    handle_binding_tools_list(scope)
}

pub(super) fn swarm_scope_healthy(s: &Store, scope: &RequestScope) -> bool {
    scope
        .tenant
        .as_ref()
        .is_some_and(|t| tenant_grant_recheck(s, t).is_ok())
        && scope_drift(s, scope).is_ok_and(|drift| drift.ok)
        && scope.anchor.get().is_none_or(|a| {
            anchor_matches_current(&a, scope_identity_observation(s, scope).as_ref())
        })
}

fn handle_binding_tools_list(scope: &RequestScope) -> std::result::Result<Value, Value> {
    // Advisory auto-pin probe when still unpinned (once per process; never
    // grants authority). Tenant scopes never touch the global pin.
    if scope.tenant.is_none() {
        let _ = maybe_mcp_auto_pin();
    }

    let s = store().map_err(|e| rpc_error(-32000, e.to_string()))?;
    // Heartbeat on every tools/list: freeze session if binding material drifted.
    let drift = scope_drift(&s, scope).map_err(|e| rpc_error(-32000, e.to_string()))?;

    // Control tools always. `locus_providers` when a pin exists (even frozen).
    let mut tools: Vec<AdapterTool> = control_tools(drift.pinned);
    let pin_alias = drift.binding_alias.clone();

    // Anchor check BEFORE the drift early-return so the catalog reflects the
    // anchored identity even when a cross-process re-pin staled the executor
    // grant (drift unhealthy but identity fields populated).
    let session_anchor = scope.anchor.get();
    if let (Some(anchored), true) = (&session_anchor, drift.pinned) {
        if let Some(current) = anchor::drift_observation(&drift) {
            if !anchored.same_primary_identity(&current) {
                return Ok(tools_list_payload(
                    control_tools(true),
                    Some(anchored.binding_alias.as_str()),
                ));
            }
        }
    }

    // Privileged provider catalog only when runtime is healthy (pinned, seal ok,
    // unfrozen, unexpired, binding matches). Fail closed otherwise.
    if !drift.ok {
        debug_assert!(tools.iter().all(|t| t.name.starts_with("locus_")));
        return Ok(tools_list_payload(tools, pin_alias.as_deref()));
    }

    let pinned = resolve_scope_session(scope).map_err(|e| rpc_error(-32000, e.to_string()))?;
    if let Some((ref session, ref bindings)) = pinned {
        // Belt + suspenders: frozen session never lists provider tools.
        if session.is_frozen() {
            return Ok(tools_list_payload(
                tools,
                Some(session.binding_alias.as_str()),
            ));
        }
        // Anchor observe on the healthy catalog path: establish on the first
        // healthy pinned observation; on mismatch collapse to control tools
        // tagged with the ANCHORED alias (fail closed, session-local).
        let obs = anchor::observation(session, bindings);
        match scope.anchor.observe(&obs, true) {
            Some(AnchorDecision::Mismatch { anchored }) => {
                if scope.anchor.note_mismatch(&anchored, &obs) {
                    audit_anchor_event(
                        "mcp.anchor_mismatch",
                        &anchored.binding_alias,
                        json!({
                            "anchored": anchor::identity_json(&anchored),
                            "current": anchor::identity_json(&obs),
                            "underlying_issues": [],
                        }),
                    );
                }
                return Ok(tools_list_payload(
                    tools,
                    Some(anchored.binding_alias.as_str()),
                ));
            }
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
            Some(AnchorDecision::Match) | None => {}
        }
        let mgr = worker_manager()
            .lock()
            .map_err(|_| rpc_error(-32000, "worker manager lock poisoned".into()))?;
        // Discovery is side-effect free: merge synthetic tools and schemas from
        // workers that were already started by an authorized call. Never spawn
        // a credential-bearing child from tools/list.
        tools.extend(shape_provider_catalog(
            mgr.tools_for_session(session, bindings),
            session,
            bindings,
        ));
        return Ok(tools_list_payload(
            tools,
            Some(session.binding_alias.as_str()),
        ));
    }
    Ok(tools_list_payload(tools, pin_alias.as_deref()))
}

#[cfg(test)]
mod catalog_shaping_tests {
    use super::*;
    use chrono::Duration as ChronoDuration;
    use locus_core::adapters::REQUIRES_APPROVAL_MARKER;
    use locus_core::binding::{BindingBody, Policy, ProviderBinding, Scope};
    use locus_core::seal::SealKey;
    use locus_core::session::{namespace_tool, PinSource, SessionMode};
    use locus_core::tools_for_binding;

    /// Production-shaped fixture: supabase pinned read_only with approval
    /// gates on deletes and prod deploys (mirrors the cmp pin).
    fn cmp_binding() -> Binding {
        Binding::from_body(BindingBody {
            id: "bnd_cmp".into(),
            alias: "cmp".into(),
            tenant: "cmp-co".into(),
            principal: None,
            description: None,
            policy: Policy {
                require_approval: vec!["*.delete*".into(), "vercel.deploy.*".into()],
                ..Policy::default()
            },
            providers: vec![
                ProviderBinding {
                    provider: "supabase".into(),
                    account: "cmp".into(),
                    credential_ref: "phm:SUPABASE_CMP".into(),
                    scope: Scope {
                        project_ref: Some("proj_cmp".into()),
                        read_only: Some(true),
                        ..Scope::default()
                    },
                    upstream: None,
                },
                ProviderBinding {
                    provider: "vercel".into(),
                    account: "cmp-vc".into(),
                    credential_ref: "phm:VERCEL_CMP".into(),
                    scope: Scope {
                        team_id: Some("team_cmp".into()),
                        ..Scope::default()
                    },
                    upstream: None,
                },
            ],
        })
    }

    fn session(mode: SessionMode) -> Session {
        let namespaced = mode == SessionMode::Namespaced;
        let s = Session::new(
            "bnd_cmp",
            "cmp",
            "cmp-co",
            None,
            PinSource::Explicit,
            Some("test".into()),
            ChronoDuration::hours(1),
            "/tmp/unused-worker-home".into(),
            &SealKey::generate(),
        )
        .with_mode(mode);
        if namespaced {
            s.with_namespaces(vec!["other".into()], vec!["fp".into()])
        } else {
            s
        }
    }

    #[test]
    fn catalog_payloads_match_pre_refactor_snapshots() {
        let mut catalogs = vec![("unpinned", tools_list_payload(control_tools(false), None))];
        for (label, mode) in [
            ("exclusive", SessionMode::Exclusive),
            ("namespaced", SessionMode::Namespaced),
        ] {
            let b = cmp_binding();
            let session = session(mode);
            let mut providers = tools_for_binding(&b);
            if session.is_namespaced() {
                for tool in &mut providers {
                    tool.name = namespace_tool("cmp", &tool.name);
                }
            }
            let mut tools = control_tools(true);
            tools.extend(shape_provider_catalog(
                providers,
                &session,
                &[("cmp".into(), b)],
            ));
            catalogs.push((label, tools_list_payload(tools, Some("cmp"))));
        }
        for (label, catalog) in catalogs {
            let fixture = match label {
                "unpinned" => include_str!("../tests/fixtures/catalog_unpinned.json"),
                "exclusive" => include_str!("../tests/fixtures/catalog_exclusive.json"),
                "namespaced" => include_str!("../tests/fixtures/catalog_namespaced.json"),
                _ => unreachable!(),
            };
            let expected: Value = serde_json::from_str(fixture).unwrap();
            assert_eq!(catalog, expected, "{label} tools/list changed");
        }
    }

    #[test]
    fn exclusive_catalog_hides_read_only_destructive_and_annotates_gated() {
        let b = cmp_binding();
        let s = session(SessionMode::Exclusive);
        let shaped =
            shape_provider_catalog(tools_for_binding(&b), &s, &[("cmp".to_string(), b.clone())]);
        // read_only supabase: destructive tool omitted, reads survive.
        assert!(!shaped.iter().any(|t| t.name == "supabase.table.delete"));
        assert!(shaped.iter().any(|t| t.name == "supabase.scope"));
        // vercel not read_only: destructive tool listed, approval-annotated
        // (matches the `vercel.deploy.*` glob exactly as call-time does).
        let deploy = shaped
            .iter()
            .find(|t| t.name == "vercel.deploy.prod")
            .expect("vercel deploy stays listed");
        assert!(deploy.description.contains(REQUIRES_APPROVAL_MARKER));
        // Ungated read tool never annotated.
        let scope_tool = shaped.iter().find(|t| t.name == "vercel.scope").unwrap();
        assert!(!scope_tool.description.contains(REQUIRES_APPROVAL_MARKER));
    }

    #[test]
    fn namespaced_catalog_strips_prefix_before_policy_match() {
        let b = cmp_binding();
        let s = session(SessionMode::Namespaced);
        assert!(s.is_namespaced());
        let mut tools = tools_for_binding(&b);
        for t in &mut tools {
            t.name = namespace_tool("cmp", &t.name);
        }
        let shaped = shape_provider_catalog(tools, &s, &[("cmp".to_string(), b.clone())]);
        // Hidden under the alias prefix too.
        assert!(!shaped
            .iter()
            .any(|t| t.name == "cmp__supabase.table.delete"));
        assert!(shaped.iter().any(|t| t.name == "cmp__supabase.scope"));
        // `vercel.deploy.*` must match the BARE name (it would never match
        // the `cmp__`-prefixed spelling) - proves prefix stripping.
        let deploy = shaped
            .iter()
            .find(|t| t.name == "cmp__vercel.deploy.prod")
            .expect("namespaced vercel deploy stays listed");
        assert!(deploy.description.contains(REQUIRES_APPROVAL_MARKER));
    }

    #[test]
    fn shaping_without_binding_leaves_tools_untouched() {
        // Presentation shaping is never a gate: unresolvable owners pass
        // through unchanged (control catalogs never enter this path at all).
        let b = cmp_binding();
        let s = session(SessionMode::Exclusive);
        let tools = tools_for_binding(&b);
        let n = tools.len();
        let shaped = shape_provider_catalog(tools.clone(), &s, &[]);
        assert_eq!(shaped.len(), n);
        for (orig, out) in tools.iter().zip(shaped.iter()) {
            assert_eq!(orig.name, out.name);
            assert_eq!(orig.description, out.description);
        }
    }
}

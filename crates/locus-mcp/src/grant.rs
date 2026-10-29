//! Multi-tenant grant auth for `locus-mcp --http --multi-tenant`.
//!
//! Tenant identity rides the `X-Locus-Tenant-Token` header (`lmt_<grant_id>.<secret>`)
//! on EVERY request — possession of an `Mcp-Session-Id` alone is never
//! authority. Verification is delegated to
//! [`locus_core::Store::verify_mcp_grant_token`] (constant-time MAC compare,
//! revocation, expiry). All failures collapse to a uniform 401 `invalid_grant`
//! body; only a token whose HMAC verified may learn it expired.

use locus_core::{args_digest, McpGrant, McpGrantAuthError, Store, SwarmGraph};
use serde_json::{json, Value};

/// Header carrying the tenant bearer token (never a tool argument).
pub const TENANT_TOKEN_HEADER: &str = "x-locus-tenant-token";

/// Additional narrowing over an authenticated tenant grant. A graph node ID is
/// derived from the operator's immutable graph, never from headers or tool args.
/// This filter cannot grant authority absent from the live binding catalog.
pub struct SwarmBroker {
    graph: SwarmGraph,
}

impl SwarmBroker {
    pub fn new(graph: SwarmGraph) -> Self {
        Self { graph }
    }

    pub fn filter_catalog(
        &self,
        store: &Store,
        tenant: Option<&TenantCtx>,
        mut catalog: Value,
    ) -> locus_core::Result<Value> {
        let agent = tenant.and_then(|t| self.graph.agent_for_grant(&t.grant_id));
        let tools = catalog
            .get_mut("tools")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| locus_core::LocusError::msg("invalid swarm catalog"))?;
        tools.retain(|tool| {
            tool.get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| agent.is_some_and(|a| a.allows(name)))
        });
        store.audit(
            "mcp.swarm.tools_list",
            tenant.map_or("-", |t| t.binding_alias.as_str()),
            Some(json!({
                "agent_id": agent.map(|a| &a.id),
                "grant_id": tenant.map(|t| &t.grant_id),
                "tool_count": tools.len(),
            })),
        )?;
        Ok(catalog)
    }

    /// Record authorization BEFORE execution. Denied names are hashed, since
    /// hostile callers can put arbitrary secret-shaped strings in that field.
    pub fn authorize_call(
        &self,
        store: &Store,
        tenant: Option<&TenantCtx>,
        name: &str,
        healthy: bool,
    ) -> locus_core::Result<bool> {
        let agent = tenant.and_then(|t| self.graph.agent_for_grant(&t.grant_id));
        let allowed = healthy && agent.is_some_and(|a| a.allows(name));
        store.audit(
            "mcp.swarm.tool_call",
            tenant.map_or("-", |t| t.binding_alias.as_str()),
            Some(json!({
                "agent_id": agent.map(|a| &a.id),
                "grant_id": tenant.map(|t| &t.grant_id),
                "allowed": allowed,
                "tool": if allowed { Some(name) } else { None },
                "tool_digest": args_digest(&json!(name)),
            })),
        )?;
        Ok(allowed)
    }
}

/// Per-request tenant context resolved from a verified grant token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantCtx {
    pub grant_id: String,
    pub session_id: String,
    pub binding_alias: String,
    pub tenant: String,
}

impl TenantCtx {
    pub fn from_grant(grant: &McpGrant) -> Self {
        Self {
            grant_id: grant.grant_id.clone(),
            session_id: grant.session_id.clone(),
            binding_alias: grant.binding_alias.clone(),
            tenant: grant.tenant.clone(),
        }
    }
}

/// Transport-level grant auth failure → HTTP status + body.
#[derive(Debug, Clone)]
pub enum GrantAuthError {
    /// Missing header, parse failure, MAC mismatch — deliberately
    /// indistinguishable.
    Invalid,
    /// Token parsed and the grant record is definitively dead (revoked /
    /// deleted). Externally IDENTICAL to `Invalid` (uniform 401 body); the
    /// server additionally sweeps the dead grant's HTTP sessions + workers.
    /// Never produced on a MAC mismatch, so a forged token naming a live
    /// grant cannot trigger sweeps.
    DeadGrant { grant_id: String },
    /// HMAC verified but the grant TTL elapsed (safe to hint a re-mint).
    Expired {
        grant_id: String,
        binding_alias: String,
    },
}

impl GrantAuthError {
    pub fn status(&self) -> u16 {
        401
    }

    pub fn body(&self) -> Value {
        match self {
            // DeadGrant is deliberately byte-identical to Invalid: revocation
            // is never advertised to the token holder.
            GrantAuthError::Invalid | GrantAuthError::DeadGrant { .. } => json!({
                "error": "invalid_grant",
                "hint": "present a valid X-Locus-Tenant-Token (operator: `locus mcp mint --binding <alias>`)",
            }),
            GrantAuthError::Expired {
                binding_alias,
                grant_id: _,
            } => json!({
                "error": "invalid_grant",
                "reason": "grant_expired",
                "safe_next": format!("locus mcp mint --binding {binding_alias}"),
                "hint": "grant TTL elapsed; operator must mint a fresh tenant token",
            }),
        }
    }
}

/// Extract the raw tenant token header value, if present and non-empty.
pub fn extract_tenant_token(headers: &[(String, String)]) -> Option<&str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(TENANT_TOKEN_HEADER))
        .map(|(_, v)| v.trim())
        .filter(|v| !v.is_empty())
}

/// Resolve and verify the tenant grant for an MT request. Fail closed on any
/// missing/invalid token. Audits `mcp.grant_auth_fail` (grant_id only, and
/// only when the token parsed — the raw token never reaches the audit log).
pub fn resolve_tenant_grant(
    store: &Store,
    headers: &[(String, String)],
) -> Result<TenantCtx, GrantAuthError> {
    let Some(raw) = extract_tenant_token(headers) else {
        return Err(GrantAuthError::Invalid);
    };
    match store.verify_mcp_grant_token(raw) {
        Ok(grant) => Ok(TenantCtx::from_grant(&grant)),
        Err(McpGrantAuthError::Invalid {
            grant_id,
            grant_dead,
        }) => {
            if let Some(id) = grant_id {
                let _ = store.audit(
                    "mcp.grant_auth_fail",
                    "-",
                    Some(json!({ "grant_id": id.as_str() })),
                );
                if grant_dead {
                    // Revoked/deleted grant re-presented: let the server sweep
                    // its sessions and tear down its credential-bearing
                    // workers. External body stays the uniform 401.
                    return Err(GrantAuthError::DeadGrant { grant_id: id });
                }
            }
            Err(GrantAuthError::Invalid)
        }
        Err(McpGrantAuthError::Expired { grant }) => {
            let _ = store.audit(
                "mcp.grant_auth_fail",
                &grant.binding_alias,
                Some(json!({ "grant_id": grant.grant_id, "reason": "grant_expired" })),
            );
            Err(GrantAuthError::Expired {
                grant_id: grant.grant_id.clone(),
                binding_alias: grant.binding_alias.clone(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use locus_core::parse_mcp_grant_token;

    fn swarm_fixture() -> (tempfile::TempDir, Store, SwarmBroker, TenantCtx) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let graph = SwarmGraph::from_slice(
            br#"{"version":1,"nodes":[{"id":"reader","grant_id":"0123456789abcdef","tools":["github.read"]}]}"#,
        )
        .unwrap();
        let tenant = TenantCtx {
            grant_id: "0123456789abcdef".into(),
            session_id: "ses_swarm".into(),
            binding_alias: "example".into(),
            tenant: "example".into(),
        };
        (dir, store, SwarmBroker::new(graph), tenant)
    }

    #[test]
    fn swarm_catalog_intersects_live_tools_and_requires_a_known_grant() {
        let (_dir, store, broker, tenant) = swarm_fixture();
        let live = json!({"tools":[{"name":"github.read"},{"name":"github.write"},
            {"name":"locus_whoami"},{"name":7},{}],"cursor":"retained"});
        let filtered = broker
            .filter_catalog(&store, Some(&tenant), live.clone())
            .unwrap();
        assert_eq!(
            filtered,
            json!({"tools":[{"name":"github.read"}],"cursor":"retained"})
        );
        let mut unknown = tenant.clone();
        unknown.grant_id = "fedcba9876543210".into();
        for ctx in [None, Some(&unknown)] {
            let filtered = broker.filter_catalog(&store, ctx, live.clone()).unwrap();
            assert_eq!(filtered["tools"], json!([]));
        }
        // A graph grant cannot add a tool absent from the live catalog.
        assert_eq!(
            broker
                .filter_catalog(&store, Some(&tenant), json!({"tools":[]}))
                .unwrap()["tools"],
            json!([])
        );
        for malformed in [json!({}), json!({"tools":null}), json!({"tools":{}})] {
            assert!(broker
                .filter_catalog(&store, Some(&tenant), malformed)
                .is_err());
        }
    }

    #[test]
    fn swarm_calls_require_health_and_exact_grants_and_redact_denials() {
        let (_dir, store, broker, tenant) = swarm_fixture();
        let hostile = "synthetic-private-tool-input";
        for (ctx, name, healthy, expected) in [
            (Some(&tenant), "github.read", true, true),
            (Some(&tenant), "github.read", false, false),
            (None, "github.read", true, false),
            (Some(&tenant), "github.read.extra", true, false),
            (Some(&tenant), hostile, true, false),
        ] {
            assert_eq!(
                broker.authorize_call(&store, ctx, name, healthy).unwrap(),
                expected
            );
        }
        let audit = std::fs::read_to_string(store.audit_path()).unwrap();
        assert!(!audit.contains(hostile));
        let events: Vec<Value> = audit
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(events.len(), 5);
        for (index, event) in events.iter().enumerate() {
            assert_eq!(event["op"], "mcp.swarm.tool_call");
            assert_eq!(event["detail"]["allowed"], index == 0);
            assert_eq!(
                event["detail"]["tool"].as_str(),
                (index == 0).then_some("github.read")
            );
        }
        assert_eq!(
            events[4]["detail"]["tool_digest"],
            args_digest(&json!(hostile))
        );
    }

    #[test]
    fn swarm_audit_failure_blocks_both_discovery_and_execution() {
        let (_dir, store, broker, tenant) = swarm_fixture();
        std::fs::create_dir(store.audit_path()).unwrap();
        assert!(broker
            .filter_catalog(
                &store,
                Some(&tenant),
                json!({"tools":[{"name":"github.read"}]})
            )
            .is_err());
        assert!(broker
            .authorize_call(&store, Some(&tenant), "github.read", true)
            .is_err());
    }

    #[test]
    fn absent_and_unknown_tenant_grants_have_uniform_failures() {
        let (_dir, store, _, _) = swarm_fixture();
        let unknown = format!("lmt_{}.{}", "f".repeat(16), "a".repeat(64));
        let invalid = GrantAuthError::Invalid.body();
        assert_eq!(
            resolve_tenant_grant(&store, &[]).unwrap_err().body(),
            invalid
        );
        for raw in ["malformed", unknown.as_str()] {
            let headers = vec![(TENANT_TOKEN_HEADER.into(), raw.into())];
            let error = resolve_tenant_grant(&store, &headers).unwrap_err();
            assert_eq!(error.status(), 401);
            assert_eq!(error.body(), invalid);
        }
    }

    #[test]
    fn token_parse_shape_is_strict() {
        let good_id = "0123456789abcdef";
        let good_secret = "a".repeat(64);
        let good = format!("lmt_{good_id}.{good_secret}");
        assert!(parse_mcp_grant_token(&good).is_some());
        // Wrong prefix, uppercase, traversal chars, short/long parts all fail.
        for bad in [
            format!("LMT_{good_id}.{good_secret}"),
            format!("lmt_{}.{}", "0123456789ABCDEF", good_secret),
            format!("lmt_{}.{}", "../../../../etc/p", good_secret),
            format!("lmt_{good_id}.{}", "b".repeat(63)),
            format!("lmt_{good_id}.{}", "B".repeat(64)),
            format!("lmt_{}.{}", "0123456789abcde", good_secret),
            format!("lmt_{good_id}{good_secret}"),
            String::new(),
        ] {
            assert!(parse_mcp_grant_token(&bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn extract_header_case_insensitive_and_trimmed() {
        let headers = vec![(
            "X-Locus-Tenant-Token".to_string(),
            "  lmt_x.y  ".to_string(),
        )];
        assert_eq!(extract_tenant_token(&headers), Some("lmt_x.y"));
        let empty = vec![("x-locus-tenant-token".to_string(), "   ".to_string())];
        assert_eq!(extract_tenant_token(&empty), None);
        assert_eq!(extract_tenant_token(&[]), None);
    }

    #[test]
    fn dead_grant_body_is_byte_identical_to_invalid() {
        // Revocation must never be advertised to the token holder.
        assert_eq!(
            GrantAuthError::Invalid.body(),
            GrantAuthError::DeadGrant {
                grant_id: "0123456789abcdef".into()
            }
            .body()
        );
        assert_eq!(
            GrantAuthError::DeadGrant {
                grant_id: "0123456789abcdef".into()
            }
            .status(),
            401
        );
    }

    #[test]
    fn uniform_invalid_body_is_values_free() {
        let body = GrantAuthError::Invalid.body().to_string();
        assert!(body.contains("invalid_grant"));
        assert!(!body.contains("lmt_"));
        let expired = GrantAuthError::Expired {
            grant_id: "0123456789abcdef".into(),
            binding_alias: "cmp".into(),
        }
        .body()
        .to_string();
        assert!(expired.contains("grant_expired"));
        assert!(expired.contains("locus mcp mint"));
    }
}

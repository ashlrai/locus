//! Multi-tenant grant auth for `locus-mcp --http --multi-tenant`.
//!
//! Tenant identity rides the `X-Locus-Tenant-Token` header (`lmt_<grant_id>.<secret>`)
//! on EVERY request — possession of an `Mcp-Session-Id` alone is never
//! authority. Verification is delegated to
//! [`locus_core::Store::verify_mcp_grant_token`] (constant-time MAC compare,
//! revocation, expiry). All failures collapse to a uniform 401 `invalid_grant`
//! body; only a token whose HMAC verified may learn it expired.

use locus_core::{McpGrant, McpGrantAuthError, Store};
use serde_json::{json, Value};

/// Header carrying the tenant bearer token (never a tool argument).
pub const TENANT_TOKEN_HEADER: &str = "x-locus-tenant-token";

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

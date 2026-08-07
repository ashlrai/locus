//! Human approval grants for `policy.require_approval` tools.
//!
//! Records live under `$LOCUS_HOME/approvals/{id}.json`. Agents blocked by
//! policy receive a stable `approval_id`; a human runs `locus approve grant`
//! and the next matching tools/call is allowed for the grant TTL.

use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Default grant lifetime after `locus approve grant` (15 minutes).
pub fn default_grant_ttl() -> Duration {
    Duration::minutes(15)
}

/// Status of an approval record on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Denied,
}

impl ApprovalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Denied => "denied",
        }
    }
}

/// One approval file under `$LOCUS_HOME/approvals/{id}.json`.
///
/// Never stores raw tool args (only `args_digest`). Safe to list/print.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalRecord {
    pub id: String,
    pub tool: String,
    pub binding: String,
    /// SHA-256 hex digest of canonicalized args (meta/secret keys stripped).
    pub args_digest: String,
    pub created_at: DateTime<Utc>,
    pub status: ApprovalStatus,
    pub session_id: String,
    /// Set when status becomes `approved`. Grant is valid until this instant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granted_at: Option<DateTime<Utc>>,
}

impl ApprovalRecord {
    pub fn is_pending(&self) -> bool {
        self.status == ApprovalStatus::Pending
    }

    /// True when granted and not past `expires_at`.
    pub fn is_valid_grant(&self) -> bool {
        if self.status != ApprovalStatus::Approved {
            return false;
        }
        match self.expires_at {
            Some(exp) => Utc::now() <= exp,
            None => false,
        }
    }

    pub fn matches_call(&self, tool: &str, binding: &str, args_digest: &str) -> bool {
        self.tool == tool && self.binding == binding && self.args_digest == args_digest
    }
}

/// Mint a stable approval id: `appr_<24 hex chars>`.
pub fn mint_approval_id() -> String {
    let mut buf = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut buf);
    format!("appr_{}", hex::encode(buf))
}

/// Keys never folded into the digest (approval meta + common secret names).
const STRIP_KEYS: &[&str] = &[
    "confirm",
    "approval_id",
    "password",
    "secret",
    "token",
    "api_key",
    "apikey",
    "access_token",
    "refresh_token",
    "authorization",
    "auth",
    "credentials",
    "private_key",
    "client_secret",
];

/// SHA-256 digest of args for matching re-calls. Does not include secrets or
/// approval control fields. Format: `sha256:<hex>`.
pub fn args_digest(args: &Value) -> String {
    let sanitized = sanitize_for_digest(args);
    let canonical = canonical_json(&sanitized);
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

fn sanitize_for_digest(v: &Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, val) in map {
                let lower = k.to_ascii_lowercase();
                if STRIP_KEYS.contains(&lower.as_str()) {
                    continue;
                }
                // Nested objects: strip secret-like keys recursively
                out.insert(k.clone(), sanitize_for_digest(val));
            }
            Value::Object(out)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(sanitize_for_digest).collect()),
        other => other.clone(),
    }
}

/// Deterministic JSON string: object keys sorted at every level.
fn canonical_json(v: &Value) -> String {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<_> = map.keys().collect();
            keys.sort();
            let mut parts = Vec::with_capacity(keys.len());
            for k in keys {
                let val = map.get(k).expect("key present");
                parts.push(format!(
                    "{}:{}",
                    serde_json::to_string(k).unwrap_or_else(|_| format!("\"{k}\"")),
                    canonical_json(val)
                ));
            }
            format!("{{{}}}", parts.join(","))
        }
        Value::Array(arr) => {
            let parts: Vec<_> = arr.iter().map(canonical_json).collect();
            format!("[{}]", parts.join(","))
        }
        // serde_json already produces stable primitives
        other => serde_json::to_string(other).unwrap_or_else(|_| "null".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn digest_stable_and_key_order_independent() {
        let a = json!({"table": "users", "limit": 1});
        let b = json!({"limit": 1, "table": "users"});
        assert_eq!(args_digest(&a), args_digest(&b));
    }

    #[test]
    fn digest_ignores_confirm_and_secrets() {
        let base = json!({"table": "users"});
        let with_meta = json!({
            "table": "users",
            "confirm": true,
            "approval_id": "appr_abc",
            "token": "super-secret",
        });
        assert_eq!(args_digest(&base), args_digest(&with_meta));
    }

    #[test]
    fn digest_changes_with_args() {
        let a = json!({"table": "users"});
        let b = json!({"table": "orders"});
        assert_ne!(args_digest(&a), args_digest(&b));
    }

    #[test]
    fn valid_grant_respects_ttl() {
        let mut rec = ApprovalRecord {
            id: "appr_test".into(),
            tool: "supabase.table.delete".into(),
            binding: "acme".into(),
            args_digest: "sha256:dead".into(),
            created_at: Utc::now(),
            status: ApprovalStatus::Approved,
            session_id: "ses_1".into(),
            expires_at: Some(Utc::now() + Duration::minutes(5)),
            granted_at: Some(Utc::now()),
        };
        assert!(rec.is_valid_grant());
        rec.expires_at = Some(Utc::now() - Duration::minutes(1));
        assert!(!rec.is_valid_grant());
        rec.status = ApprovalStatus::Pending;
        rec.expires_at = Some(Utc::now() + Duration::minutes(5));
        assert!(!rec.is_valid_grant());
    }
}

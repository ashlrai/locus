//! Short-lived capability tickets for tools/call audit and optional verify.
//!
//! A ticket is an HMAC over `session_id|binding_id|tool|exp` using the daemon
//! seal key. The **ticket_id** (hex HMAC digest) is safe to put in audit logs —
//! it is not a secret that grants ambient access by itself; workers still sit
//! behind the sealed pin + policy gate. TTL defaults to 30 seconds (DESIGN §4.2).

use crate::error::{LocusError, Result};
use crate::seal::SealKey;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// Default capability ticket lifetime (matches DESIGN tools/call path).
pub const DEFAULT_TICKET_TTL_SECS: i64 = 30;

/// Prefix for ticket_id strings (`cap_<64-hex>`).
pub const TICKET_ID_PREFIX: &str = "cap_";

/// Minted capability ticket — metadata + HMAC ticket_id for audit.
///
/// The raw HMAC is encoded into `ticket_id`. Never put resolved secrets here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityTicket {
    /// Stable audit id: `cap_` + hex(HMAC-SHA256(material)).
    pub ticket_id: String,
    pub session_id: String,
    pub binding_id: String,
    pub tool: String,
    /// Unix epoch seconds when the ticket expires.
    pub exp: i64,
    /// RFC3339 expiry (convenience for logs).
    pub expires_at: String,
}

impl CapabilityTicket {
    /// Canonical HMAC material: `session_id|binding_id|tool|exp`.
    pub fn material(session_id: &str, binding_id: &str, tool: &str, exp: i64) -> String {
        format!("{session_id}|{binding_id}|{tool}|{exp}")
    }

    /// Whether wall-clock now is past `exp`.
    pub fn is_expired(&self) -> bool {
        Utc::now().timestamp() > self.exp
    }

    /// Whether wall-clock now is past `exp` at a given instant (tests).
    pub fn is_expired_at(&self, now: DateTime<Utc>) -> bool {
        now.timestamp() > self.exp
    }
}

/// Mint a short-lived capability ticket.
///
/// `ttl` defaults to [`DEFAULT_TICKET_TTL_SECS`] when `None` or non-positive.
pub fn mint_ticket(
    key: &SealKey,
    session_id: &str,
    binding_id: &str,
    tool: &str,
    ttl: Option<Duration>,
) -> Result<CapabilityTicket> {
    if session_id.is_empty() || binding_id.is_empty() || tool.is_empty() {
        return Err(LocusError::msg(
            "capability ticket requires non-empty session_id, binding_id, and tool",
        ));
    }
    let ttl = match ttl {
        Some(d) if d.num_seconds() > 0 => d,
        _ => Duration::seconds(DEFAULT_TICKET_TTL_SECS),
    };
    let expires = Utc::now() + ttl;
    let exp = expires.timestamp();
    let material = CapabilityTicket::material(session_id, binding_id, tool, exp);
    let ticket_id = ticket_id_from_material(key, &material);
    Ok(CapabilityTicket {
        ticket_id,
        session_id: session_id.to_string(),
        binding_id: binding_id.to_string(),
        tool: tool.to_string(),
        exp,
        expires_at: expires.to_rfc3339(),
    })
}

/// Verify a ticket against the seal key: recompute HMAC and check TTL.
pub fn verify_ticket(key: &SealKey, ticket: &CapabilityTicket) -> Result<()> {
    if ticket.session_id.is_empty() || ticket.binding_id.is_empty() || ticket.tool.is_empty() {
        return Err(LocusError::msg("capability ticket fields incomplete"));
    }
    if !ticket.ticket_id.starts_with(TICKET_ID_PREFIX) {
        return Err(LocusError::msg("capability ticket_id has invalid prefix"));
    }
    if ticket.is_expired() {
        return Err(LocusError::msg("capability ticket expired"));
    }
    let material = CapabilityTicket::material(
        &ticket.session_id,
        &ticket.binding_id,
        &ticket.tool,
        ticket.exp,
    );
    let expected = ticket_id_from_material(key, &material);
    if !ct_eq(&expected, &ticket.ticket_id) {
        return Err(LocusError::msg("capability ticket HMAC mismatch"));
    }
    Ok(())
}

/// Verify from discrete fields (store helper / external reconstruct).
pub fn verify_ticket_parts(
    key: &SealKey,
    ticket_id: &str,
    session_id: &str,
    binding_id: &str,
    tool: &str,
    exp: i64,
) -> Result<()> {
    let ticket = CapabilityTicket {
        ticket_id: ticket_id.to_string(),
        session_id: session_id.to_string(),
        binding_id: binding_id.to_string(),
        tool: tool.to_string(),
        exp,
        expires_at: DateTime::from_timestamp(exp, 0)
            .map(|t| t.to_rfc3339())
            .unwrap_or_default(),
    };
    verify_ticket(key, &ticket)
}

fn ticket_id_from_material(key: &SealKey, material: &str) -> String {
    // SealKey exposes seal() as `hmac-sha256:<hex>`; we want a compact audit id.
    // Recompute with the same key bytes via a thin re-seal and strip prefix,
    // or hash the seal output for a stable `cap_` id.
    let seal = key.seal(material);
    let hex = seal.strip_prefix("hmac-sha256:").unwrap_or(seal.as_str());
    format!("{TICKET_ID_PREFIX}{hex}")
}

/// Constant-time-ish equality for equal-length strings.
fn ct_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seal::SealKey;

    #[test]
    fn mint_and_verify_roundtrip() {
        let key = SealKey::generate();
        let t = mint_ticket(
            &key,
            "ses_abc",
            "bnd_acme",
            "github.scope",
            Some(Duration::seconds(60)),
        )
        .unwrap();
        assert!(t.ticket_id.starts_with("cap_"));
        assert_eq!(t.ticket_id.len(), 4 + 64); // cap_ + sha256 hex
        assert!(!t.is_expired());
        verify_ticket(&key, &t).unwrap();
        verify_ticket_parts(
            &key,
            &t.ticket_id,
            &t.session_id,
            &t.binding_id,
            &t.tool,
            t.exp,
        )
        .unwrap();
    }

    #[test]
    fn reject_wrong_key() {
        let key1 = SealKey::generate();
        let key2 = SealKey::generate();
        let t = mint_ticket(&key1, "ses_1", "bnd_1", "tool.a", None).unwrap();
        let err = verify_ticket(&key2, &t).unwrap_err();
        assert!(err.to_string().contains("HMAC mismatch"));
    }

    #[test]
    fn reject_tampered_fields() {
        let key = SealKey::generate();
        let mut t = mint_ticket(&key, "ses_1", "bnd_1", "tool.a", None).unwrap();
        t.tool = "tool.b".into();
        assert!(verify_ticket(&key, &t).is_err());
    }

    #[test]
    fn reject_expired() {
        let key = SealKey::generate();
        // mint with negative TTL falls back to default — force exp in the past
        let mut t = mint_ticket(
            &key,
            "ses_1",
            "bnd_1",
            "tool.a",
            Some(Duration::seconds(30)),
        )
        .unwrap();
        t.exp = Utc::now().timestamp() - 10;
        // Recompute ticket_id for the past exp so HMAC would match if not expired
        let material = CapabilityTicket::material(&t.session_id, &t.binding_id, &t.tool, t.exp);
        t.ticket_id = ticket_id_from_material(&key, &material);
        let err = verify_ticket(&key, &t).unwrap_err();
        assert!(err.to_string().contains("expired"));
    }

    #[test]
    fn reject_empty_fields() {
        let key = SealKey::generate();
        assert!(mint_ticket(&key, "", "b", "t", None).is_err());
        assert!(mint_ticket(&key, "s", "", "t", None).is_err());
        assert!(mint_ticket(&key, "s", "b", "", None).is_err());
    }

    #[test]
    fn material_format() {
        assert_eq!(
            CapabilityTicket::material("ses", "bnd", "gh.scope", 1700000000),
            "ses|bnd|gh.scope|1700000000"
        );
    }

    #[test]
    fn default_ttl_positive() {
        let key = SealKey::generate();
        let t = mint_ticket(&key, "s", "b", "t", None).unwrap();
        let now = Utc::now().timestamp();
        assert!(t.exp > now);
        assert!(t.exp <= now + DEFAULT_TICKET_TTL_SECS + 2);
    }

    #[test]
    fn ticket_id_stable_for_same_inputs() {
        let key =
            SealKey::from_hex("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
                .unwrap();
        let material = CapabilityTicket::material("ses", "bnd", "tool", 99);
        let a = ticket_id_from_material(&key, &material);
        let b = ticket_id_from_material(&key, &material);
        assert_eq!(a, b);
        assert_eq!(
            a,
            format!(
                "cap_{}",
                key.seal(&material).strip_prefix("hmac-sha256:").unwrap()
            )
        );
    }
}

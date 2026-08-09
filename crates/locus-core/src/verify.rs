//! Verification plane — proposal → verify → act (M5 stubs).
//!
//! Pure heuristics + stable JSON shape for hub/agent extension. No ML models.
//! Locus answers *as whom*; this module answers *should this claim be grounded
//! before acting?* Sibling planes: Phantom (secrets), Locus identity (pin).

use crate::store::{AuditEvent, Whoami};
use serde::{Deserialize, Serialize};

/// Confidence band for a free-text claim (heuristic, not calibrated probability).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimConfidence {
    Unknown,
    Low,
    Medium,
    High,
}

impl ClaimConfidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

impl std::fmt::Display for ClaimConfidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Optional identity grounding attached when the claim is pin-related and a
/// session is active. Never includes secrets — aliases / tenant only.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaimGrounding {
    /// Grounding source, e.g. `whoami`.
    pub kind: String,
    pub binding_alias: String,
    pub tenant: String,
    pub binding_id: String,
    pub seal_ok: bool,
    pub frozen: bool,
}

impl ClaimGrounding {
    pub fn from_whoami(w: &Whoami) -> Self {
        Self {
            kind: "whoami".into(),
            binding_alias: w.binding_alias.clone(),
            tenant: w.tenant.clone(),
            binding_id: w.binding_id.clone(),
            seal_ok: w.seal_ok,
            frozen: w.frozen,
        }
    }
}

/// Result of [`verify_claim`] — stable contract for CLI, MCP, and hub.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClaimVerification {
    pub claim: String,
    pub confidence: ClaimConfidence,
    pub needs_tool: bool,
    pub suggestion: String,
    /// Heuristic signal tags that fired (hub may extend / re-score).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub signals: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grounding: Option<ClaimGrounding>,
}

/// Threshold of low-confidence-looking audit detail hits before doctor warns.
pub const DOCTOR_LOW_CONFIDENCE_AUDIT_THRESHOLD: usize = 5;

/// Scan window (most recent events) for doctor low-confidence signals.
pub const DOCTOR_LOW_CONFIDENCE_AUDIT_SCAN: usize = 50;

/// Score a free-text claim with pure heuristics.
///
/// Rules (M5 stub):
/// - Numbers, URLs, or version-like tokens → `needs_tool=true`, `confidence=low`
/// - Identity-related language + active pin → attach whoami grounding; confidence
///   at least `medium` when seal is ok and not frozen
/// - Identity-related without pin → `needs_tool=true` (whoami/pin), `confidence=low`
/// - Otherwise → `confidence=unknown`, soft suggestion
pub fn verify_claim(text: &str, whoami: Option<&Whoami>) -> ClaimVerification {
    let claim = text.trim().to_string();
    let mut signals: Vec<String> = Vec::new();

    let has_url = claim_has_url(&claim);
    let has_version = claim_has_version(&claim);
    let has_number = claim_has_significant_number(&claim);
    let identity = claim_is_identity_related(&claim);

    if has_url {
        signals.push("url".into());
    }
    if has_version {
        signals.push("version".into());
    }
    if has_number {
        signals.push("number".into());
    }
    if identity {
        signals.push("identity".into());
    }

    let factual = has_url || has_version || has_number;

    let grounding = if identity {
        whoami.map(ClaimGrounding::from_whoami)
    } else {
        None
    };

    if grounding.is_some() {
        signals.push("grounding_whoami".into());
    }

    let (confidence, needs_tool, suggestion) = match (factual, identity, &grounding) {
        // Factual claim — always ask for tool grounding in this stub.
        (true, _, _) => {
            let mut sug = String::from(
                "Claim includes numbers, URLs, or versions — ground with a tool before acting \
                 (e.g. list/get/status against the pinned provider).",
            );
            if identity {
                if let Some(g) = &grounding {
                    sug.push_str(&format!(
                        " Identity context: pin `{}` tenant `{}` (whoami attached).",
                        g.binding_alias, g.tenant
                    ));
                } else {
                    sug.push_str(" Identity language detected but no active pin — run `locus pin` / `locus whoami` first.");
                }
            }
            (ClaimConfidence::Low, true, sug)
        }
        // Identity-only with healthy pin.
        (false, true, Some(g)) if g.seal_ok && !g.frozen => (
            ClaimConfidence::Medium,
            false,
            format!(
                "Identity claim grounded against active pin `{}` (tenant `{}`). \
                 Call `locus_whoami` / `locus heartbeat` if drift is possible.",
                g.binding_alias, g.tenant
            ),
        ),
        // Identity-only with unhealthy pin.
        (false, true, Some(g)) => (
            ClaimConfidence::Low,
            true,
            format!(
                "Identity claim with pin `{}` but seal_ok={} frozen={} — re-pin or run `locus doctor` before acting.",
                g.binding_alias, g.seal_ok, g.frozen
            ),
        ),
        // Identity language, unbound.
        (false, true, None) => (
            ClaimConfidence::Low,
            true,
            "Identity-related claim with no active pin. Human: `locus enter <alias>` then `locus whoami` before acting."
                .into(),
        ),
        // Soft / qualitative claim.
        (false, false, _) => (
            ClaimConfidence::Unknown,
            false,
            "No strong verification signal (no numbers/URLs/versions/identity). \
             Proceed carefully, or ground with tools if the claim will drive mutations."
                .into(),
        ),
    };

    ClaimVerification {
        claim,
        confidence,
        needs_tool,
        suggestion,
        signals,
        grounding,
    }
}

/// Count recent audit events whose detail text looks like a low-confidence
/// factual claim (numbers / URLs / versions). Used by doctor as a light WARN.
pub fn count_low_confidence_audit_signals(events: &[AuditEvent], scan_n: usize) -> usize {
    let start = events.len().saturating_sub(scan_n);
    let mut n = 0usize;
    for ev in &events[start..] {
        if audit_event_looks_low_confidence(ev) {
            n += 1;
        }
    }
    n
}

fn audit_event_looks_low_confidence(ev: &AuditEvent) -> bool {
    // Skip pure identity control ops — those are the grounding surface.
    if ev.op.contains("whoami")
        || ev.op.contains("heartbeat")
        || ev.op == "session.pin"
        || ev.op == "session.leave"
        || ev.op.starts_with("verify.")
    {
        return false;
    }
    let blob = match &ev.detail {
        Some(d) => d.to_string(),
        None => return false,
    };
    claim_has_url(&blob) || claim_has_version(&blob) || claim_has_significant_number(&blob)
}

/// Doctor finding helper: warn when many low-confidence-looking audit details appear.
pub fn doctor_low_confidence_message(count: usize) -> Option<String> {
    if count >= DOCTOR_LOW_CONFIDENCE_AUDIT_THRESHOLD {
        Some(format!(
            "{count} recent audit event(s) look low-confidence (numbers/URLs/versions in detail) — \
             ground claims with tools; try `locus verify claim --text \"…\"`"
        ))
    } else {
        None
    }
}

// ── Heuristics ─────────────────────────────────────────────────────────────

fn claim_has_url(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.contains("http://")
        || lower.contains("https://")
        || lower.contains("www.")
        || lower.contains("://")
}

/// Semver-ish or multi-part version tokens: `1.2`, `1.2.3`, `v0.1.1`.
fn claim_has_version(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // optional leading 'v' / 'V'
        let start = if (bytes[i] == b'v' || bytes[i] == b'V')
            && i + 1 < bytes.len()
            && bytes[i + 1].is_ascii_digit()
        {
            i + 1
        } else if bytes[i].is_ascii_digit() {
            i
        } else {
            i += 1;
            continue;
        };
        let mut j = start;
        let mut dots = 0u32;
        let mut digits_in_part = 0u32;
        while j < bytes.len() {
            if bytes[j].is_ascii_digit() {
                digits_in_part += 1;
                j += 1;
            } else if bytes[j] == b'.' && digits_in_part > 0 {
                dots += 1;
                digits_in_part = 0;
                j += 1;
            } else {
                break;
            }
        }
        // Require at least one dot and a trailing digit part.
        if dots >= 1 && digits_in_part > 0 {
            return true;
        }
        i = start + 1;
    }
    false
}

/// Digits that look like quantities (not single digits in prose).
fn claim_has_significant_number(s: &str) -> bool {
    // Percentages
    if s.contains('%') {
        let bytes = s.as_bytes();
        for (idx, &b) in bytes.iter().enumerate() {
            if b == b'%' && idx > 0 && bytes[idx - 1].is_ascii_digit() {
                return true;
            }
        }
    }
    // Runs of 2+ digits, or digit runs with commas/underscores (1,000 / 1_000)
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            let mut digits = 0u32;
            while i < bytes.len()
                && (bytes[i].is_ascii_digit() || bytes[i] == b',' || bytes[i] == b'_')
            {
                if bytes[i].is_ascii_digit() {
                    digits += 1;
                }
                i += 1;
            }
            // Skip if this run is part of a version (handled separately) — still counts as number.
            if digits >= 2 {
                // Avoid matching lone years? years are still factual claims — count them.
                let _ = start;
                return true;
            }
        } else {
            i += 1;
        }
    }
    false
}

fn claim_is_identity_related(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    const KEYS: &[&str] = &[
        "whoami",
        "pinned",
        "pinning",
        " active pin",
        "binding",
        "tenant",
        "principal",
        "acting as",
        "act as",
        "wrong account",
        "right account",
        "identity",
        "session",
        "locus enter",
        "locus pin",
        "as whom",
        "which tenant",
        "which account",
        "logged in",
        "signed in",
        "my account",
        "our account",
        "client account",
        "workspace pin",
    ];
    // Also match start-of-string "pin " / "pinned "
    if lower.starts_with("pin ")
        || lower.starts_with("pinned ")
        || lower.starts_with("pin:")
        || lower.contains(" pin ")
        || lower.contains(" pinned ")
    {
        return true;
    }
    KEYS.iter().any(|k| lower.contains(k))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{ProviderView, Whoami};

    fn sample_whoami(seal_ok: bool, frozen: bool) -> Whoami {
        Whoami {
            session_id: "sess_1".into(),
            binding_alias: "acme".into(),
            binding_id: "bnd_acme".into(),
            tenant: "acme-corp".into(),
            principal: None,
            providers: vec![ProviderView {
                provider: "github".into(),
                account: "acme".into(),
                credential: crate::credential::credential_metadata("phm:GH"),
                project_ref: None,
                team_id: None,
                account_id: None,
                read_only: None,
                orgs: vec![],
                repos: vec![],
            }],
            expires_at: "2099-01-01T00:00:00Z".into(),
            worker_home: "/tmp".into(),
            seal_ok,
            frozen,
            frozen_reason: if frozen {
                Some("binding_id_drift".into())
            } else {
                None
            },
            mode: "exclusive".into(),
            namespaces: vec![],
        }
    }

    #[test]
    fn factual_url_needs_tool_low() {
        let r = verify_claim("Deploy to https://api.example.com/v2", None);
        assert!(r.needs_tool);
        assert_eq!(r.confidence, ClaimConfidence::Low);
        assert!(r.signals.iter().any(|s| s == "url"));
        assert!(r.grounding.is_none());
    }

    #[test]
    fn factual_version_needs_tool() {
        let r = verify_claim("We are on locus 0.1.1 already", None);
        assert!(r.needs_tool);
        assert_eq!(r.confidence, ClaimConfidence::Low);
        assert!(r.signals.iter().any(|s| s == "version" || s == "number"));
    }

    #[test]
    fn factual_number_and_percent() {
        let r = verify_claim("Error rate is 12% across 400 requests", None);
        assert!(r.needs_tool);
        assert_eq!(r.confidence, ClaimConfidence::Low);
        assert!(r.signals.iter().any(|s| s == "number"));
    }

    #[test]
    fn identity_with_pin_grounds_medium() {
        let w = sample_whoami(true, false);
        let r = verify_claim("I am acting as the acme tenant right now", Some(&w));
        assert!(!r.needs_tool);
        assert_eq!(r.confidence, ClaimConfidence::Medium);
        assert!(r.signals.iter().any(|s| s == "identity"));
        let g = r.grounding.expect("grounding");
        assert_eq!(g.binding_alias, "acme");
        assert_eq!(g.tenant, "acme-corp");
        assert_eq!(g.kind, "whoami");
    }

    #[test]
    fn identity_without_pin_low() {
        let r = verify_claim("We are pinned to personal", None);
        assert!(r.needs_tool);
        assert_eq!(r.confidence, ClaimConfidence::Low);
        assert!(r.grounding.is_none());
    }

    #[test]
    fn soft_claim_unknown() {
        let r = verify_claim("This change looks reasonable", None);
        assert!(!r.needs_tool);
        assert_eq!(r.confidence, ClaimConfidence::Unknown);
        assert!(r.signals.is_empty());
    }

    #[test]
    fn frozen_pin_lowers_identity_confidence() {
        let w = sample_whoami(true, true);
        let r = verify_claim("wrong account is impossible with this pin", Some(&w));
        assert!(r.needs_tool);
        assert_eq!(r.confidence, ClaimConfidence::Low);
        assert!(r.grounding.is_some());
    }

    #[test]
    fn audit_signal_count_and_doctor_threshold() {
        let mut events = Vec::new();
        for i in 0..6 {
            events.push(AuditEvent {
                ts: format!("2026-01-01T00:00:0{i}Z"),
                op: "mcp.tool_call".into(),
                binding: "acme".into(),
                detail: Some(serde_json::json!({ "note": format!("hit https://x.test/{i}") })),
            });
        }
        // whoami should not count
        events.push(AuditEvent {
            ts: "2026-01-01T00:00:10Z".into(),
            op: "mcp.whoami".into(),
            binding: "acme".into(),
            detail: Some(serde_json::json!({ "url": "https://ignore.me" })),
        });
        let n = count_low_confidence_audit_signals(&events, 50);
        assert_eq!(n, 6);
        assert!(doctor_low_confidence_message(n).is_some());
        assert!(doctor_low_confidence_message(2).is_none());
    }

    #[test]
    fn version_heuristic_basic() {
        assert!(claim_has_version("v1.2.3"));
        assert!(claim_has_version("release 2.0"));
        assert!(!claim_has_version("just text"));
        assert!(!claim_has_version("item 1 alone"));
    }
}

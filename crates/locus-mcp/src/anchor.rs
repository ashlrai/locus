//! MCP session identity anchor — fail-closed pin-swap protection.
//!
//! Every MCP session (stdio process / HTTP `Mcp-Session-Id`) anchors to the
//! **identity** of the binding observed at initialize (or the first healthy
//! pinned observation): primary `binding_id` + `tenant` + `mode` + sorted
//! `(alias, binding_id, tenant)` namespace triples. Never the `session_id` — a
//! same-alias re-pin (TTL refresh via `locus enter <same>`) keeps the same
//! identity and is allowed silently with a session_id re-anchor. A cross-alias
//! re-pin under a live MCP session trips the anchor and provider tools refuse
//! with `pin_changed` until the client re-initializes.
//!
//! The anchor layer is purely session-local: it never mutates `active.json`,
//! never freezes the store session, and never carries secrets — aliases,
//! tenants, binding ids and session ids only.

use locus_core::{Binding, RuntimeDrift, Session};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

/// One namespaced secondary binding identity: alias + the binding id and
/// tenant it had when anchored. Binding ids are generated deterministically
/// (`bnd_{alias}`), so a deleted-and-recreated secondary alias keeps its id —
/// the **tenant** is the discriminator that catches a recreated alias
/// pointing at a different tenant.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NamespaceAnchor {
    pub alias: String,
    pub binding_id: String,
    /// Tenant of the secondary binding at anchor time. `#[serde(default)]`
    /// keeps legacy persisted anchors loadable; an empty tenant is treated
    /// as unresolved and never matches (fail closed — re-initialize the
    /// client to re-anchor with a full identity).
    #[serde(default)]
    pub tenant: String,
}

impl NamespaceAnchor {
    /// A secondary identity is resolved only when both its binding id and
    /// tenant were captured. Unresolved entries (binding missing at
    /// observation time, or legacy anchors without a tenant) never match.
    pub fn is_resolved(&self) -> bool {
        !self.binding_id.is_empty() && !self.tenant.is_empty()
    }
}

/// Fail-closed namespace comparison: pairwise equality (both sides sorted by
/// alias) where every entry on both sides must also be resolved — two
/// different unresolvable aliases must never compare equal via empty
/// placeholder fields.
fn namespaces_match(a: &[NamespaceAnchor], b: &[NamespaceAnchor]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b.iter())
            .all(|(x, y)| x.is_resolved() && y.is_resolved() && x == y)
}

/// Identity anchored by an MCP session. `session_id` / `backing` /
/// `anchored_at_unix` are informational only and excluded from identity
/// comparison ([`SessionAnchor::same_identity`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionAnchor {
    pub binding_id: String,
    pub binding_alias: String,
    pub tenant: String,
    /// `"exclusive"` | `"namespaced"` (same mapping as `locus_status`).
    pub mode: String,
    /// Secondary namespace pairs, sorted by alias. Empty for exclusive pins.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub namespaces: Vec<NamespaceAnchor>,
    /// Informational; refreshed on a same-identity re-pin (`Repinned`).
    pub session_id: String,
    /// Observability only: `"active"` | `"run"` | `"ci"` | `"env_session"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backing: Option<String>,
    pub anchored_at_unix: u64,
}

impl SessionAnchor {
    /// Full identity comparison: binding_id + tenant + mode + namespace
    /// triples (alias, binding_id, tenant — every entry must be resolved on
    /// both sides). Ignores `session_id`, `backing`, and `anchored_at_unix`.
    pub fn same_identity(&self, other: &SessionAnchor) -> bool {
        self.binding_id == other.binding_id
            && self.tenant == other.tenant
            && self.mode == other.mode
            && namespaces_match(&self.namespaces, &other.namespaces)
    }

    /// Primary-only comparison for observations built from [`RuntimeDrift`]
    /// (which carries no namespace/mode identity when unhealthy): binding_id +
    /// tenant only.
    pub fn same_primary_identity(&self, other: &SessionAnchor) -> bool {
        self.binding_id == other.binding_id && self.tenant == other.tenant
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Build a full identity observation from a loaded healthy `(session, bindings)`
/// pair (the same pair handed to the provider gate — no extra store I/O).
pub fn observation(session: &Session, bindings: &[(String, Binding)]) -> SessionAnchor {
    let mode = if session.is_namespaced() {
        "namespaced"
    } else {
        "exclusive"
    };
    // Secondary namespace identities only (primary identity is the top-level
    // binding_id); binding id + tenant zipped from the loaded bindings, sorted
    // by alias. An alias missing from the loaded slice keeps empty fields —
    // unresolved entries hard-mismatch in `namespaces_match` (fail closed)
    // rather than anchoring an empty placeholder that could compare equal.
    let mut namespaces: Vec<NamespaceAnchor> = session
        .namespaces
        .iter()
        .map(|alias| {
            let found = bindings.iter().find(|(a, _)| a == alias).map(|(_, b)| b);
            NamespaceAnchor {
                alias: alias.clone(),
                binding_id: found.map(|b| b.id.clone()).unwrap_or_default(),
                tenant: found.map(|b| b.tenant.clone()).unwrap_or_default(),
            }
        })
        .collect();
    namespaces.sort_by(|a, b| a.alias.cmp(&b.alias));

    SessionAnchor {
        binding_id: session.binding_id.clone(),
        binding_alias: session.binding_alias.clone(),
        tenant: session.tenant.clone(),
        mode: mode.into(),
        namespaces,
        session_id: session.session_id.clone(),
        backing: session
            .backing
            .as_ref()
            .map(|b| b.backing_type.as_str().to_string()),
        anchored_at_unix: now_unix(),
    }
}

/// Primary-only identity observation from [`RuntimeDrift`] — populated even
/// when the runtime is unhealthy (stale executor grant after a cross-process
/// re-pin), so `pin_changed` can outrank `runtime_unhealthy`. Returns `None`
/// when unpinned. Used only for the pre-drift-fail mismatch **check**; never
/// for anchor establishment.
pub fn drift_observation(d: &RuntimeDrift) -> Option<SessionAnchor> {
    if !d.pinned {
        return None;
    }
    Some(SessionAnchor {
        binding_id: d.binding_id_session.clone()?,
        binding_alias: d.binding_alias.clone().unwrap_or_default(),
        tenant: d.tenant_session.clone()?,
        // Primary-only observation: mode/namespace identity unknown here.
        mode: String::new(),
        namespaces: Vec::new(),
        session_id: d.session_id.clone().unwrap_or_default(),
        backing: d.backing_type.map(|b| b.as_str().to_string()),
        anchored_at_unix: now_unix(),
    })
}

/// Outcome of comparing a healthy observation against the stored anchor.
#[derive(Debug, Clone, PartialEq)]
pub enum AnchorDecision {
    /// No anchor existed; this observation established it.
    Established,
    /// Same identity, same session — proceed.
    Match,
    /// Same identity, new session_id (same-alias re-pin) — stored session_id
    /// refreshed; proceed.
    Repinned,
    /// Different identity — fail closed (`pin_changed`).
    Mismatch { anchored: Box<SessionAnchor> },
}

/// Pure compare-and-set against an anchor slot.
///
/// Returns `None` when there is no anchor and `allow_establish` is false
/// (nothing anchored, nothing to enforce). Never establishes from a mismatch
/// and never clears an existing anchor.
pub fn decide(
    slot: &mut Option<SessionAnchor>,
    obs: &SessionAnchor,
    allow_establish: bool,
) -> Option<AnchorDecision> {
    match slot {
        None => {
            if allow_establish {
                *slot = Some(obs.clone());
                Some(AnchorDecision::Established)
            } else {
                None
            }
        }
        Some(anchor) => {
            // Primary-only observations (empty mode) compare binding_id+tenant.
            let same = if obs.mode.is_empty() && obs.namespaces.is_empty() {
                anchor.same_primary_identity(obs)
            } else {
                anchor.same_identity(obs)
            };
            if !same {
                return Some(AnchorDecision::Mismatch {
                    anchored: Box::new(anchor.clone()),
                });
            }
            if anchor.session_id != obs.session_id {
                anchor.session_id = obs.session_id.clone();
                if obs.backing.is_some() {
                    anchor.backing = obs.backing.clone();
                }
                Some(AnchorDecision::Repinned)
            } else {
                Some(AnchorDecision::Match)
            }
        }
    }
}

/// Dedupe key for `mcp.anchor_mismatch` audits: one report per
/// (anchored_session_id, current_session_id) pair.
pub fn mismatch_key(anchored: &SessionAnchor, current: &SessionAnchor) -> (String, String) {
    (anchored.session_id.clone(), current.session_id.clone())
}

/// Values-free identity summary for refusal bodies / control-tool reports.
pub fn identity_json(a: &SessionAnchor) -> Value {
    json!({
        "alias": a.binding_alias,
        "tenant": a.tenant,
        "binding_id": a.binding_id,
        "session_id": a.session_id,
        "mode": if a.mode.is_empty() { Value::Null } else { Value::String(a.mode.clone()) },
    })
}

/// Structured `pin_changed` tool error (fail closed, session-local).
///
/// `underlying_issues` carries the drift issues (e.g.
/// `executor_authority_unavailable`) so the authority-plane facts stay
/// visible without masking the wrong-account refusal.
pub fn pin_changed_error(
    anchored: &SessionAnchor,
    current: &SessionAnchor,
    underlying_issues: &[String],
) -> Value {
    let mut body = json!({
        "error": "pin_changed",
        "detail": format!(
            "The global Locus pin changed after this MCP session anchored to `{}` (tenant `{}`). \
             Provider tools are disabled for this session to prevent wrong-account actions.",
            anchored.binding_alias, anchored.tenant
        ),
        "anchored": identity_json(anchored),
        "current": identity_json(current),
        "safe_next": {
            "action": "reinitialize_client",
            "ready": false,
            "command": format!("locus enter {}", anchored.binding_alias),
        },
        "hint": format!(
            "Restart/reconnect this MCP client (HTTP: POST initialize with this Mcp-Session-Id) \
             to adopt `{}`, or run `locus enter {}` to restore the anchored identity. \
             locus_whoami/locus_status/locus_safe_next/locus_verify_session still work and report this mismatch.",
            current.binding_alias, anchored.binding_alias
        ),
    });
    if !underlying_issues.is_empty() {
        body["underlying_issues"] = json!(underlying_issues);
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor(alias: &str, id: &str, tenant: &str, sid: &str) -> SessionAnchor {
        SessionAnchor {
            binding_id: id.into(),
            binding_alias: alias.into(),
            tenant: tenant.into(),
            mode: "exclusive".into(),
            namespaces: Vec::new(),
            session_id: sid.into(),
            backing: Some("active".into()),
            anchored_at_unix: 1,
        }
    }

    #[test]
    fn same_identity_ignores_session_backing_and_time() {
        let a = anchor("acme", "bnd_acme", "acme-corp", "sess_1");
        let mut b = anchor("acme", "bnd_acme", "acme-corp", "sess_2");
        b.backing = Some("ci".into());
        b.anchored_at_unix = 999;
        assert!(a.same_identity(&b));
        assert!(a.same_primary_identity(&b));
    }

    #[test]
    fn identity_trips_on_binding_id_tenant_or_mode() {
        let a = anchor("acme", "bnd_acme", "acme-corp", "s");
        let mut other = a.clone();
        other.binding_id = "bnd_acme_v2".into();
        assert!(!a.same_identity(&other), "recreated alias must trip");
        let mut other = a.clone();
        other.tenant = "evil-corp".into();
        assert!(!a.same_identity(&other));
        let mut other = a.clone();
        other.mode = "namespaced".into();
        assert!(!a.same_identity(&other));
    }

    #[test]
    fn namespace_pairs_order_insensitive_via_sorted_observation() {
        // observation() sorts; anchors built with the same sorted pairs match.
        let mut a = anchor("acme", "bnd_acme", "acme-corp", "s1");
        a.mode = "namespaced".into();
        a.namespaces = vec![
            NamespaceAnchor {
                alias: "alpha".into(),
                binding_id: "bnd_alpha".into(),
                tenant: "alpha-corp".into(),
            },
            NamespaceAnchor {
                alias: "beta".into(),
                binding_id: "bnd_beta".into(),
                tenant: "beta-corp".into(),
            },
        ];
        let mut b = a.clone();
        b.session_id = "s2".into();
        assert!(a.same_identity(&b));

        // Recreated secondary alias (same alias, new binding_id) must trip.
        let mut c = a.clone();
        c.namespaces[1].binding_id = "bnd_beta_recreated".into();
        assert!(!a.same_identity(&c));
    }

    #[test]
    fn recreated_secondary_alias_same_deterministic_id_trips_on_tenant() {
        // Binding ids are deterministic (`bnd_{alias}`), so a deleted-and-
        // recreated secondary alias keeps its id. The tenant must trip.
        let mut a = anchor("work", "bnd_work", "work-corp", "s1");
        a.mode = "namespaced".into();
        a.namespaces = vec![NamespaceAnchor {
            alias: "clientA".into(),
            binding_id: "bnd_clientA".into(),
            tenant: "clienta-corp".into(),
        }];

        let mut recreated = a.clone();
        recreated.session_id = "s2".into();
        recreated.namespaces[0].tenant = "other-corp".into();
        assert!(
            !a.same_identity(&recreated),
            "recreated secondary alias with same deterministic id but a \
             different tenant must mismatch"
        );

        let mut slot = Some(a.clone());
        assert!(matches!(
            decide(&mut slot, &recreated, true),
            Some(AnchorDecision::Mismatch { .. })
        ));
    }

    #[test]
    fn unresolved_secondary_identity_never_matches() {
        // Two anchors with the same unresolvable alias (empty binding_id /
        // tenant placeholders) must not compare equal — fail closed.
        let mut a = anchor("work", "bnd_work", "work-corp", "s1");
        a.mode = "namespaced".into();
        a.namespaces = vec![NamespaceAnchor {
            alias: "ghost".into(),
            binding_id: String::new(),
            tenant: String::new(),
        }];
        let b = a.clone();
        assert!(!a.same_identity(&b), "unresolved entries must never match");

        // Legacy persisted anchor without a tenant (serde default) is
        // unresolved too — never matches a fully-resolved observation.
        let mut legacy = a.clone();
        legacy.namespaces[0].binding_id = "bnd_ghost".into();
        let mut full = a.clone();
        full.namespaces[0] = NamespaceAnchor {
            alias: "ghost".into(),
            binding_id: "bnd_ghost".into(),
            tenant: "ghost-corp".into(),
        };
        assert!(!legacy.same_identity(&full));
        assert!(full.same_identity(&full.clone()));
    }

    #[test]
    fn observation_captures_secondary_tenant_and_marks_missing_unresolved() {
        use locus_core::Binding;
        let mk = |alias: &str, tenant: &str| Binding {
            id: format!("bnd_{alias}"),
            alias: alias.into(),
            tenant: tenant.into(),
            principal: None,
            description: None,
            policy: Default::default(),
            providers: Vec::new(),
        };
        let session: Session = serde_json::from_value(json!({
            "session_id": "sess_obs",
            "binding_id": "bnd_work",
            "binding_alias": "work",
            "tenant": "work-corp",
            "source": "explicit",
            "pinned_at": "2026-01-01T00:00:00Z",
            "expires_at": "2027-01-01T00:00:00Z",
            "mode": "namespaced",
            "seal": "",
            "worker_home": "/tmp/locus-anchor-test",
            "namespaces": ["clientA", "missing"],
        }))
        .expect("test session deserializes");
        let bindings = vec![("clientA".to_string(), mk("clientA", "clienta-corp"))];

        let obs = observation(&session, &bindings);
        assert_eq!(obs.namespaces.len(), 2);
        // Sorted by alias: clientA then missing.
        assert_eq!(obs.namespaces[0].alias, "clientA");
        assert_eq!(obs.namespaces[0].tenant, "clienta-corp");
        assert!(obs.namespaces[0].is_resolved());
        assert_eq!(obs.namespaces[1].alias, "missing");
        assert!(!obs.namespaces[1].is_resolved());
        // An observation carrying an unresolved secondary never matches even
        // an identical one — provider tools stay refused (fail closed).
        assert!(!obs.same_identity(&obs.clone()));
    }

    #[test]
    fn decide_matrix() {
        let obs1 = anchor("acme", "bnd_acme", "acme-corp", "sess_1");

        // allow_establish=false never establishes.
        let mut slot: Option<SessionAnchor> = None;
        assert_eq!(decide(&mut slot, &obs1, false), None);
        assert!(slot.is_none());

        // Establish.
        assert_eq!(
            decide(&mut slot, &obs1, true),
            Some(AnchorDecision::Established)
        );
        assert_eq!(slot.as_ref().unwrap().session_id, "sess_1");

        // Match (same session).
        assert_eq!(decide(&mut slot, &obs1, true), Some(AnchorDecision::Match));

        // Repinned (same identity, new session) — refreshes stored session_id.
        let obs2 = anchor("acme", "bnd_acme", "acme-corp", "sess_2");
        assert_eq!(
            decide(&mut slot, &obs2, true),
            Some(AnchorDecision::Repinned)
        );
        assert_eq!(slot.as_ref().unwrap().session_id, "sess_2");

        // Mismatch — never rotates the anchor, reports the anchored identity.
        let evil = anchor("beta", "bnd_beta", "beta-corp", "sess_3");
        match decide(&mut slot, &evil, true) {
            Some(AnchorDecision::Mismatch { anchored }) => {
                assert_eq!(anchored.binding_alias, "acme");
                assert_eq!(anchored.session_id, "sess_2");
            }
            other => panic!("expected mismatch, got {other:?}"),
        }
        assert_eq!(slot.as_ref().unwrap().binding_alias, "acme");
    }

    #[test]
    fn drift_observation_is_primary_only_and_none_when_unpinned() {
        let mut slot = Some(anchor("acme", "bnd_acme", "acme-corp", "sess_1"));

        // Primary-only observation: mode empty ⇒ binding_id+tenant comparison
        // even though the anchor has mode "exclusive".
        let mut obs = anchor("acme", "bnd_acme", "acme-corp", "sess_9");
        obs.mode = String::new();
        assert_eq!(
            decide(&mut slot, &obs, true),
            Some(AnchorDecision::Repinned)
        );

        let mut evil = anchor("beta", "bnd_beta", "beta-corp", "sess_9");
        evil.mode = String::new();
        assert!(matches!(
            decide(&mut slot, &evil, true),
            Some(AnchorDecision::Mismatch { .. })
        ));
    }

    #[test]
    fn pin_changed_error_shape_and_no_secret_fields() {
        let a = anchor("acme", "bnd_acme", "acme-corp", "s1");
        let c = anchor("beta", "bnd_beta", "beta-corp", "s2");
        let body = pin_changed_error(&a, &c, &["executor_authority_unavailable".into()]);
        assert_eq!(body["error"], "pin_changed");
        assert_eq!(body["anchored"]["alias"], "acme");
        assert_eq!(body["current"]["alias"], "beta");
        assert_eq!(body["safe_next"]["action"], "reinitialize_client");
        assert_eq!(body["safe_next"]["ready"], false);
        assert_eq!(body["safe_next"]["command"], "locus enter acme");
        assert_eq!(
            body["underlying_issues"][0],
            "executor_authority_unavailable"
        );
        let raw = body.to_string().to_ascii_lowercase();
        for banned in ["phm:", "credential", "token", "secret"] {
            assert!(!raw.contains(banned), "refusal leaked `{banned}`: {raw}");
        }
    }

    #[test]
    fn serde_roundtrip_and_legacy_fields_optional() {
        let mut a = anchor("acme", "bnd_acme", "acme-corp", "s1");
        a.namespaces = vec![NamespaceAnchor {
            alias: "beta".into(),
            binding_id: "bnd_beta".into(),
            tenant: "beta-corp".into(),
        }];
        let raw = serde_json::to_string(&a).unwrap();
        let back: SessionAnchor = serde_json::from_str(&raw).unwrap();
        assert!(a.same_identity(&back));
        assert_eq!(back.session_id, "s1");

        // Missing optional fields (backing / namespaces) deserialize cleanly.
        let legacy = r#"{"binding_id":"b","binding_alias":"a","tenant":"t","mode":"exclusive","session_id":"s","anchored_at_unix":0}"#;
        let parsed: SessionAnchor = serde_json::from_str(legacy).unwrap();
        assert!(parsed.backing.is_none());
        assert!(parsed.namespaces.is_empty());
    }

    #[test]
    fn mismatch_key_is_session_pair() {
        let a = anchor("acme", "bnd_acme", "acme-corp", "s1");
        let c = anchor("beta", "bnd_beta", "beta-corp", "s2");
        assert_eq!(mismatch_key(&a, &c), ("s1".into(), "s2".into()));
    }
}

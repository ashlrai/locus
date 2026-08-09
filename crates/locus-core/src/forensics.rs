//! Forensics pack export — shareable incident bundle with **no secrets**.
//!
//! ```text
//! locus forensics export [--binding acme] [--out pack.json]
//! ```
//!
//! Includes pin/session metadata, binding summaries, last N audit events,
//! doctor snapshot, pending approvals, and an optional HMAC chain tip over
//! the audit tip. Never resolves CredentialRefs or embeds token values.

use crate::approval::ApprovalRecord;
use crate::binding::BindingSummary;
use crate::doctor::{
    build_doctor_report, count_near_misses, DoctorExternal, DoctorReport, NearMissSummary,
};
use crate::store::{AuditEvent, Store};
use crate::VERSION;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

// Re-export near-miss helpers for forensics consumers.
pub use crate::doctor::{
    is_near_miss_op, is_require_approval_near_miss, is_scope_freeze_near_miss,
};

/// Default number of audit events in a pack.
pub const DEFAULT_AUDIT_LAST: usize = 200;

/// Options for [`export_forensics_pack`].
#[derive(Debug, Clone, Default)]
pub struct ForensicsExportOptions {
    /// When set, filter audit events and pending approvals to this binding alias.
    pub binding: Option<String>,
    /// Max audit events (newest). Default: [`DEFAULT_AUDIT_LAST`].
    pub audit_last: Option<usize>,
    /// Doctor external facts (Phantom PATH, cwd). Defaults are safe for tests.
    pub doctor_external: Option<DoctorExternal>,
}

/// HMAC / content tip over the audit log (no secret material).
///
/// Full append-only HMAC chaining is a roadmap item (DESIGN §4.5). Until then
/// we export a tip digest of the last event + count, optionally sealed with
/// the daemon seal key when available.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditChainTip {
    /// True when a tip could be computed (audit non-empty or empty-but-known).
    pub available: bool,
    pub event_count: usize,
    /// SHA-256 hex of the last audit event JSON line (canonical serde).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_event_digest: Option<String>,
    /// `hmac-sha256:…` of `last_event_digest` under the seal key, if seal ok.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seal_hmac: Option<String>,
    /// Timestamp of the last event when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_event_ts: Option<String>,
    /// Algorithm label for consumers.
    pub algorithm: String,
}

/// Session / pin slice safe for forensics (no secrets).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ForensicsSessionMeta {
    pub session_id: String,
    pub binding_alias: String,
    pub binding_id: String,
    pub tenant: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    pub pinned_at: String,
    pub expires_at: String,
    pub seal_ok: bool,
    pub expired: bool,
    #[serde(default)]
    pub frozen: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frozen_reason: Option<String>,
    pub mode: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub namespaces: Vec<String>,
}

/// Pending approval row (already digests-only).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ForensicsApprovalSummary {
    pub id: String,
    pub tool: String,
    pub binding: String,
    pub args_digest: String,
    pub status: String,
    pub session_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub requester: String,
    pub grants: usize,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

impl From<&ApprovalRecord> for ForensicsApprovalSummary {
    fn from(r: &ApprovalRecord) -> Self {
        Self {
            id: r.id.clone(),
            tool: r.tool.clone(),
            binding: r.binding.clone(),
            args_digest: r.args_digest.clone(),
            status: r.status.as_str().to_string(),
            session_id: r.session_id.clone(),
            requester: r.requester.clone(),
            grants: r.grants.len(),
            created_at: r.created_at.to_rfc3339(),
            expires_at: r.expires_at.map(|t| t.to_rfc3339()),
        }
    }
}

/// Full forensics pack — stable JSON shape for support / SIEM handoff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForensicsPack {
    /// Pack schema version (integer string for forward compat).
    pub pack_version: u32,
    pub locus_version: String,
    pub exported_at: String,
    pub home: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter_binding: Option<String>,
    /// Active pin/session when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pin: Option<ForensicsSessionMeta>,
    /// Binding summaries (alias/tenant/providers only).
    pub bindings: Vec<BindingSummary>,
    /// Last N audit events (newest last in file order; filtered if binding set).
    pub audit_events: Vec<AuditEvent>,
    pub audit_event_count: usize,
    /// Doctor mission-control snapshot (no secrets).
    pub doctor: DoctorReport,
    /// Pending approvals (digests only).
    pub pending_approvals: Vec<ForensicsApprovalSummary>,
    /// Near-miss summary over last 24h from audit.
    pub near_miss: NearMissSummary,
    /// Audit chain tip if computable.
    pub chain_tip: AuditChainTip,
}

/// Stable top-level keys expected in forensics pack JSON.
pub const FORENSICS_PACK_JSON_KEYS: &[&str] = &[
    "pack_version",
    "locus_version",
    "exported_at",
    "home",
    "bindings",
    "audit_events",
    "audit_event_count",
    "doctor",
    "pending_approvals",
    "near_miss",
    "chain_tip",
];

/// Validate serialized forensics pack has stable keys (and no obvious secret fields).
pub fn forensics_pack_json_has_stable_keys(value: &serde_json::Value) -> Result<(), Vec<String>> {
    let obj = match value.as_object() {
        Some(o) => o,
        None => return Err(vec!["root is not an object".into()]),
    };
    let mut missing = Vec::new();
    for k in FORENSICS_PACK_JSON_KEYS {
        if !obj.contains_key(*k) {
            missing.push((*k).to_string());
        }
    }
    if let Some(nm) = obj.get("near_miss") {
        for k in ["window_hours", "count", "scope_freeze", "require_approval"] {
            if nm.get(k).is_none() {
                missing.push(format!("near_miss.{k}"));
            }
        }
    }
    if let Some(ct) = obj.get("chain_tip") {
        for k in ["available", "event_count", "algorithm"] {
            if ct.get(k).is_none() {
                missing.push(format!("chain_tip.{k}"));
            }
        }
    }
    if let Some(doc) = obj.get("doctor") {
        if doc.get("verdict").is_none() {
            missing.push("doctor.verdict".into());
        }
        if doc.get("ok").is_none() {
            missing.push("doctor.ok".into());
        }
    }
    // Soft check: pack must not contain common secret field names at top level.
    for banned in ["token", "secret", "password", "api_key", "authorization"] {
        if obj.contains_key(banned) {
            missing.push(format!("forbidden top-level key: {banned}"));
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(missing)
    }
}

/// Build a forensics pack from the store. Never resolves credentials.
pub fn export_forensics_pack(
    store: &Store,
    opts: ForensicsExportOptions,
) -> crate::Result<ForensicsPack> {
    let last_n = opts.audit_last.unwrap_or(DEFAULT_AUDIT_LAST).max(1);
    let filter = opts.binding.as_deref();

    let external = opts.doctor_external.unwrap_or(DoctorExternal {
        phantom_on_path: false,
        unresolved_phm: Vec::new(),
        cwd: None,
    });
    let doctor = build_doctor_report(store, external)?;

    let mut bindings = store.list_bindings()?;
    if let Some(alias) = filter {
        bindings.retain(|b| b.alias == alias);
    }

    let pin = session_meta(store)?;

    let all_events = store.read_audit_events()?;
    let audit_events = filter_tail_events(&all_events, last_n, filter);
    let chain_tip = compute_chain_tip(store, &all_events);

    let pending = store.pending_approvals()?;
    let pending_approvals: Vec<ForensicsApprovalSummary> = pending
        .iter()
        .filter(|r| filter.map(|a| r.binding == a).unwrap_or(true))
        .map(ForensicsApprovalSummary::from)
        .collect();

    let near_miss = count_near_misses(&all_events, 24, filter);

    Ok(ForensicsPack {
        pack_version: 1,
        locus_version: VERSION.to_string(),
        exported_at: Utc::now().to_rfc3339(),
        home: store.home().display().to_string(),
        filter_binding: opts.binding,
        pin,
        bindings,
        audit_event_count: audit_events.len(),
        audit_events,
        doctor,
        pending_approvals,
        near_miss,
        chain_tip,
    })
}

fn session_meta(store: &Store) -> crate::Result<Option<ForensicsSessionMeta>> {
    let Some(sess) = store.active_session()? else {
        return Ok(None);
    };
    let seal_ok = match store.seal_key() {
        Ok(key) => key.verify(&sess.material(), &sess.seal),
        Err(_) => false,
    };
    Ok(Some(ForensicsSessionMeta {
        session_id: sess.session_id.clone(),
        binding_alias: sess.binding_alias.clone(),
        binding_id: sess.binding_id.clone(),
        tenant: sess.tenant.clone(),
        principal: sess.principal.clone(),
        client: sess.client.clone(),
        pinned_at: sess.pinned_at.to_rfc3339(),
        expires_at: sess.expires_at.to_rfc3339(),
        seal_ok,
        expired: sess.is_expired(),
        frozen: sess.frozen,
        frozen_reason: sess.frozen_reason.clone(),
        mode: match sess.mode {
            crate::session::SessionMode::Exclusive => "exclusive".into(),
            crate::session::SessionMode::Namespaced => "namespaced".into(),
        },
        namespaces: sess.namespaces.clone(),
    }))
}

fn filter_tail_events(
    events: &[AuditEvent],
    last_n: usize,
    binding: Option<&str>,
) -> Vec<AuditEvent> {
    let filtered: Vec<AuditEvent> = events
        .iter()
        .filter(|e| binding.map(|b| e.binding == b).unwrap_or(true))
        .cloned()
        .collect();
    if filtered.len() > last_n {
        filtered[filtered.len() - last_n..].to_vec()
    } else {
        filtered
    }
}

fn compute_chain_tip(store: &Store, events: &[AuditEvent]) -> AuditChainTip {
    if events.is_empty() {
        return AuditChainTip {
            available: true,
            event_count: 0,
            last_event_digest: None,
            seal_hmac: None,
            last_event_ts: None,
            algorithm: "sha256+optional-seal-hmac".into(),
        };
    }
    let last = &events[events.len() - 1];
    let canonical = serde_json::to_string(last).unwrap_or_default();
    let digest = {
        let mut h = Sha256::new();
        h.update(canonical.as_bytes());
        hex::encode(h.finalize())
    };
    let seal_hmac = store.seal_key().ok().map(|key| key.seal(&digest));
    AuditChainTip {
        available: true,
        event_count: events.len(),
        last_event_digest: Some(digest),
        seal_hmac,
        last_event_ts: Some(last.ts.clone()),
        algorithm: "sha256+optional-seal-hmac".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::{Binding, BindingBody, Policy, ProviderBinding, Scope};
    use crate::store::Store;
    use tempfile::tempdir;

    fn sample_binding(alias: &str) -> Binding {
        Binding::from_body(BindingBody {
            id: format!("bnd_{alias}"),
            alias: alias.into(),
            tenant: format!("{alias}-corp"),
            principal: None,
            description: None,
            policy: Policy::default(),
            providers: vec![ProviderBinding {
                provider: "github".into(),
                account: alias.into(),
                credential_ref: "env:GH_TOKEN".into(),
                scope: Scope::default(),
                upstream: None,
            }],
        })
    }

    #[test]
    fn forensics_pack_shape_unpinned() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store.save_binding(&sample_binding("acme")).unwrap();
        store.save_binding(&sample_binding("personal")).unwrap();

        let pack = export_forensics_pack(
            &store,
            ForensicsExportOptions {
                binding: None,
                audit_last: Some(50),
                doctor_external: Some(DoctorExternal {
                    phantom_on_path: true,
                    unresolved_phm: Vec::new(),
                    cwd: Some(dir.path().to_path_buf()),
                }),
            },
        )
        .unwrap();

        let v = serde_json::to_value(&pack).unwrap();
        forensics_pack_json_has_stable_keys(&v).expect("stable keys");

        assert_eq!(pack.pack_version, 1);
        assert_eq!(pack.locus_version, VERSION);
        assert!(pack.pin.is_none());
        assert_eq!(pack.bindings.len(), 2);
        assert!(pack.pending_approvals.is_empty());
        assert!(pack.chain_tip.available);
        assert_eq!(pack.near_miss.window_hours, 24);
        // Doctor nested
        assert!(!pack.doctor.home.is_empty());
        // No secrets in serialized form
        let s = serde_json::to_string(&pack).unwrap();
        assert!(!s.contains("\"token\""));
        assert!(!s.contains("sk_live"));
    }

    #[test]
    fn forensics_pack_filter_binding_and_pin() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store.save_binding(&sample_binding("acme")).unwrap();
        store.save_binding(&sample_binding("personal")).unwrap();
        store.pin("acme", dir.path(), None, false).unwrap();

        let _ = store.audit(
            "mcp.scope_freeze",
            "acme",
            Some(serde_json::json!({"tool": "github.check_repo"})),
        );
        let _ = store.audit(
            "mcp.require_approval",
            "acme",
            Some(serde_json::json!({"tool": "x.delete"})),
        );
        let _ = store.audit("pin", "personal", None);

        let pack = export_forensics_pack(
            &store,
            ForensicsExportOptions {
                binding: Some("acme".into()),
                audit_last: Some(100),
                doctor_external: Some(DoctorExternal {
                    phantom_on_path: true,
                    unresolved_phm: Vec::new(),
                    cwd: Some(dir.path().to_path_buf()),
                }),
            },
        )
        .unwrap();

        let v = serde_json::to_value(&pack).unwrap();
        forensics_pack_json_has_stable_keys(&v).expect("stable keys");

        assert_eq!(pack.filter_binding.as_deref(), Some("acme"));
        assert_eq!(pack.bindings.len(), 1);
        assert_eq!(pack.bindings[0].alias, "acme");
        assert!(pack.pin.is_some());
        assert_eq!(
            pack.pin.as_ref().map(|p| p.binding_alias.as_str()),
            Some("acme")
        );
        assert!(pack.pin.as_ref().unwrap().seal_ok);
        // Audit filtered to acme only
        assert!(pack.audit_events.iter().all(|e| e.binding == "acme"));
        assert!(pack.near_miss.count >= 2);
        assert!(pack.near_miss.scope_freeze >= 1);
        assert!(pack.near_miss.require_approval >= 1);
        assert!(pack.chain_tip.last_event_digest.is_some());
        // Seal key present in fresh store
        assert!(pack.chain_tip.seal_hmac.is_some());
        assert!(pack
            .chain_tip
            .seal_hmac
            .as_ref()
            .unwrap()
            .starts_with("hmac-sha256:"));
    }

    #[test]
    fn near_miss_window_excludes_old() {
        use chrono::Duration;
        let events = vec![
            AuditEvent {
                ts: (Utc::now() - Duration::hours(48)).to_rfc3339(),
                op: "mcp.scope_freeze".into(),
                binding: "a".into(),
                detail: None,
            },
            AuditEvent {
                ts: Utc::now().to_rfc3339(),
                op: "mcp.scope_freeze".into(),
                binding: "a".into(),
                detail: None,
            },
            AuditEvent {
                ts: Utc::now().to_rfc3339(),
                op: "mcp.require_approval".into(),
                binding: "b".into(),
                detail: None,
            },
        ];
        let nm = count_near_misses(&events, 24, None);
        assert_eq!(nm.scope_freeze, 1);
        assert_eq!(nm.require_approval, 1);
        assert_eq!(nm.count, 2);

        let nm_a = count_near_misses(&events, 24, Some("a"));
        assert_eq!(nm_a.count, 1);
        assert_eq!(nm_a.scope_freeze, 1);
    }

    #[test]
    fn pack_keys_reject_non_object() {
        let err = forensics_pack_json_has_stable_keys(&serde_json::json!([])).unwrap_err();
        assert!(err.iter().any(|e| e.contains("not an object")));
    }
}

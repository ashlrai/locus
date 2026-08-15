//! `locus doctor` — single-pane "am I safe to act?" report.
//!
//! Pure assembly over [`Store`] + external facts (Phantom PATH, cwd). Never
//! includes secret values — only aliases, digests, counts, and issue codes.

use crate::config::{self, AutopinStatus, LocusConfig};
use crate::credential::CredentialResolutionIssue;
use crate::store::{ApprovalsHealth, AuditEvent, RuntimeDrift, Store};
use crate::verify::{
    count_low_confidence_audit_signals, doctor_low_confidence_message,
    DOCTOR_LOW_CONFIDENCE_AUDIT_SCAN,
};
use crate::VERSION;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;

// Re-export so consumers can depend on doctor::AutopinStatus shape via report.
pub use crate::config::AutopinStatus as DoctorAutopinStatus;

/// Overall mission-control verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum DoctorVerdict {
    Safe,
    Warn,
    Unsafe,
}

impl DoctorVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Safe => "SAFE",
            Self::Warn => "WARN",
            Self::Unsafe => "UNSAFE",
        }
    }

    /// Process exit code: SAFE=0, WARN=1, UNSAFE=2.
    pub fn exit_code(self) -> i32 {
        match self {
            Self::Safe => 0,
            Self::Warn => 1,
            Self::Unsafe => 2,
        }
    }

    fn escalate(self, other: Self) -> Self {
        use DoctorVerdict::*;
        match (self, other) {
            (Unsafe, _) | (_, Unsafe) => Unsafe,
            (Warn, _) | (_, Warn) => Warn,
            _ => Safe,
        }
    }
}

/// Severity of one doctor finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueSeverity {
    /// Posture information; never escalates the verdict (stays SAFE).
    Info,
    Warn,
    Unsafe,
}

/// One structured issue line (machine + human).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DoctorIssue {
    pub severity: IssueSeverity,
    /// Stable machine code (e.g. `invalid_seal`, `unresolved_phm`).
    pub code: String,
    pub message: String,
}

/// Active pin slice for doctor JSON.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DoctorPin {
    pub alias: String,
    pub tenant: String,
    pub binding_id: String,
    pub expires_at: String,
    /// Seconds until expiry (0 when already expired). Additive field.
    #[serde(default)]
    pub expires_in_secs: i64,
    /// Runtime-verified pin health (seal + backing + expiry, with the
    /// authority anchor counted as healthy when its check could not run for
    /// lack of a control capability — see `authority_anchor_verified` for the
    /// honest verification status).
    pub seal_ok: bool,
    /// Whether the authority anchor was actually verified (additive field).
    /// `Some(false)` with `seal_ok=true` means verification was **skipped**
    /// (no control capability in this environment — surfaced as the Warn
    /// finding `executor_authority_unavailable`), not that it passed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority_anchor_verified: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    pub expired: bool,
}

/// Workspace discovery for doctor.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceStatus {
    pub found: bool,
    /// False when a workspace policy file was encountered but could not be read or parsed.
    pub valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_binding: Option<String>,
    pub allowed_bindings: Vec<String>,
    pub require_pin: bool,
    /// When found: whether the active pin (if any) is on the allowlist.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pin_allowed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Recent audit summary (no secret material).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuditSummary {
    pub path: String,
    pub total: usize,
    /// Last N ops (newest first).
    pub last: Vec<AuditEvent>,
    /// Count of scope_freeze events among the scanned tail.
    pub scope_freeze: usize,
    /// Count of deny / approval.deny events among the scanned tail.
    pub deny: usize,
}

/// Near-miss counters (scope freeze + require_approval blocks) over a time window.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NearMissSummary {
    pub window_hours: u32,
    pub count: usize,
    pub scope_freeze: usize,
    pub require_approval: usize,
}

/// Whether an audit op is a "near miss" (scope freeze or require_approval block).
pub fn is_near_miss_op(op: &str) -> bool {
    is_scope_freeze_near_miss(op) || is_require_approval_near_miss(op)
}

pub fn is_scope_freeze_near_miss(op: &str) -> bool {
    op.contains("scope_freeze")
}

pub fn is_require_approval_near_miss(op: &str) -> bool {
    op.contains("require_approval")
        || op == "mcp.require_approval"
        || op.ends_with(".require_approval")
}

/// Count near-miss events in the last `window_hours` hours.
pub fn count_near_misses(
    events: &[AuditEvent],
    window_hours: u32,
    binding: Option<&str>,
) -> NearMissSummary {
    let cutoff = Utc::now() - Duration::hours(i64::from(window_hours));
    let mut scope_freeze = 0usize;
    let mut require_approval = 0usize;
    for ev in events {
        if let Some(b) = binding {
            if ev.binding != b {
                continue;
            }
        }
        if !event_in_near_miss_window(&ev.ts, cutoff) {
            continue;
        }
        if is_scope_freeze_near_miss(&ev.op) {
            scope_freeze += 1;
        }
        if is_require_approval_near_miss(&ev.op) {
            require_approval += 1;
        }
    }
    NearMissSummary {
        window_hours,
        count: scope_freeze + require_approval,
        scope_freeze,
        require_approval,
    }
}

fn event_in_near_miss_window(ts: &str, cutoff: DateTime<Utc>) -> bool {
    match DateTime::parse_from_rfc3339(ts) {
        Ok(dt) => dt.with_timezone(&Utc) >= cutoff,
        // Unparseable timestamps: exclude from windowed counts.
        Err(_) => false,
    }
}

/// External facts the CLI gathers (Phantom is out-of-process).
#[derive(Debug, Clone, Default)]
pub struct DoctorExternal {
    pub phantom_on_path: bool,
    pub unresolved_phm: Vec<CredentialResolutionIssue>,
    /// Working directory for workspace walk (defaults applied by caller).
    pub cwd: Option<std::path::PathBuf>,
}

/// Full doctor report — stable JSON schema for mission-control.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    pub version: String,
    pub home: String,
    pub seal_ok: bool,
    pub bindings: usize,
    /// Backward-compatible alias of active pin (or null).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned: Option<String>,
    /// Active pin detail when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pin: Option<DoctorPin>,
    /// Seal verification of active session when pinned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pin_seal_ok: Option<bool>,
    pub runtime: RuntimeDrift,
    pub approvals: ApprovalsHealth,
    pub pending_approvals: usize,
    pub dual_control_waiting: usize,
    pub phantom_on_path: bool,
    pub unresolved_phm: Vec<CredentialResolutionIssue>,
    pub autopin: AutopinStatus,
    pub workspace: WorkspaceStatus,
    pub audit: AuditSummary,
    /// Near-miss count (scope_freeze + require_approval blocks) in the last 24h.
    pub near_miss_count: usize,
    /// Structured near-miss breakdown (same window as `near_miss_count`).
    pub near_miss: NearMissSummary,
    /// Structured issues with severity.
    pub findings: Vec<DoctorIssue>,
    /// Flat message list (same order as findings) for older consumers.
    pub issues: Vec<String>,
    pub verdict: DoctorVerdict,
    /// True only when verdict is SAFE.
    pub ok: bool,
}

/// Gather real external facts: Phantom PATH probe + unresolved `phm:` refs.
///
/// Shared by CLI and MCP surfaces so doctor verdicts and `session_ok` agree
/// across transports (`locus verify session --json` == `locus_verify_session`).
pub fn gather_doctor_external(
    store: &Store,
    cwd: std::path::PathBuf,
) -> crate::Result<DoctorExternal> {
    let phantom = crate::credential::phantom_on_path();
    let unresolved_phm = crate::credential::collect_unresolved_phm_refs(store, phantom)?;
    Ok(DoctorExternal {
        phantom_on_path: phantom,
        unresolved_phm,
        cwd: Some(cwd),
    })
}

impl DoctorReport {
    /// Append a finding after construction, re-deriving `issues`, `verdict`,
    /// and `ok`. Used by operator-facing surfaces (CLI doctor/quickstart) to
    /// attach environment checks — e.g. `LOCUS_CONTROL_CAPABILITY` — that do
    /// not apply to executor-restricted transports like locus-mcp.
    pub fn push_finding(&mut self, severity: IssueSeverity, code: &str, message: String) {
        let escalate = match severity {
            IssueSeverity::Unsafe => DoctorVerdict::Unsafe,
            IssueSeverity::Warn => DoctorVerdict::Warn,
            IssueSeverity::Info => DoctorVerdict::Safe,
        };
        self.issues.push(message.clone());
        self.findings.push(DoctorIssue {
            severity,
            code: code.into(),
            message,
        });
        self.verdict = self.verdict.escalate(escalate);
        self.ok = self.verdict == DoctorVerdict::Safe;
    }
}

/// Operator-shell findings for `LOCUS_CONTROL_CAPABILITY` readiness.
///
/// Only meaningful where the operator is expected to hold control authority
/// (the CLI shell) — agent transports legitimately run without the capability
/// and must not attach these. Messages carry exact fix commands, never bearer
/// values.
pub fn control_capability_findings(
    status: &crate::authority_anchor::ControlCapabilityStatus,
    home: &Path,
) -> Vec<DoctorIssue> {
    let mut out = Vec::new();
    let mut push = |code: &str, message: String| {
        out.push(DoctorIssue {
            severity: IssueSeverity::Warn,
            code: code.into(),
            message,
        });
    };
    let file = crate::authority_anchor::control_capability_file(home);

    if status.env_present && !status.env_valid {
        push(
            "control_capability_invalid",
            "LOCUS_CONTROL_CAPABILITY is set but invalid (need 32 bytes as 64 lowercase hex) — \
             mint a valid one: export LOCUS_CONTROL_CAPABILITY=\"$(openssl rand -hex 32)\""
                .into(),
        );
    } else if !status.satisfied() {
        if status.persisted_valid {
            push(
                "control_capability_not_exported",
                format!(
                    "control capability persisted at {} but LOCUS_CONTROL_CAPABILITY is not \
                     exported — run: eval \"$(locus hook zsh)\"  (or export \
                     LOCUS_CONTROL_CAPABILITY=\"$(cat {})\")",
                    file.display(),
                    file.display()
                ),
            );
        } else {
            push(
                "control_capability_missing",
                "LOCUS_CONTROL_CAPABILITY is not set — control commands (init/enter/pin/leave) \
                 will fail. Fix: locus quickstart (mints + persists one), or: export \
                 LOCUS_CONTROL_CAPABILITY=\"$(openssl rand -hex 32)\"; then persist for new \
                 shells with eval \"$(locus hook zsh)\""
                    .into(),
            );
        }
    }

    if status.persisted && !status.persisted_valid {
        push(
            "control_capability_file_invalid",
            format!(
                "persisted control capability at {} is invalid (expected 64 lowercase hex) — \
                 delete it deliberately, then re-mint via locus quickstart",
                file.display()
            ),
        );
    }
    if status.persisted && !status.persisted_permissions_ok {
        push(
            "control_capability_file_permissions",
            format!(
                "persisted control capability at {} is readable by group/other — fix: chmod 600 {}",
                file.display(),
                file.display()
            ),
        );
    }
    if status.matches_persisted == Some(false) {
        push(
            "control_capability_mismatch",
            format!(
                "LOCUS_CONTROL_CAPABILITY does not match the persisted capability at {} — control \
                 operations may be refused. Locus never silently replaces a capability: export \
                 the persisted value (eval \"$(locus hook zsh)\") or deliberately remove the \
                 stale side",
                file.display()
            ),
        );
    }
    // Posture note (default trade-off, not a defect): a persisted capability
    // makes control authority ambient for same-user processes.
    if status.persisted && status.persisted_valid {
        out.push(DoctorIssue {
            severity: IssueSeverity::Info,
            code: "control_capability_persisted".into(),
            message: "control capability persisted — same-user processes can run control \
                      commands; use locus capability unpersist + shell export for strict posture"
                .into(),
        });
    }
    out
}

/// True when the runtime authority-anchor check failed *only* because this
/// process had no control/executor capability to authenticate the check with
/// (`executor_authority_unavailable`) — the check never ran, rather than ran
/// and mismatched.
///
/// Read-only contexts (doctor) must treat that as an environment gap — report
/// `control_capability_not_exported` and leave the pin intact — never as pin
/// tamper. Distinguishing rule, fail closed on ambiguity:
/// - capability **absent** from the environment → the anchor client errors
///   before contacting the broker (`executor_authority_unavailable`) and
///   `control_env_present` is false → not tamper evidence;
/// - capability **present but wrong/tampered** → it authenticates against the
///   live anchor and surfaces as `authority_anchor_mismatch` /
///   `authority_anchor_unavailable` (or is present-but-malformed, i.e.
///   `control_env_present` is true) → stays fail-closed (UNSAFE), and the
///   store freeze path for real binding drift is untouched.
fn anchor_unverified_due_to_absent_capability(
    issues: &[String],
    control_env_present: bool,
) -> bool {
    if control_env_present {
        return false;
    }
    let could_not_authenticate = issues.iter().any(|i| i == "executor_authority_unavailable");
    let anchor_or_seal_evidence = issues.iter().any(|i| {
        i == "authority_anchor_mismatch"
            || i == "authority_anchor_unavailable"
            || i == "invalid_seal"
    });
    could_not_authenticate && !anchor_or_seal_evidence
}

/// Build a complete doctor report from the store + external facts.
pub fn build_doctor_report(store: &Store, external: DoctorExternal) -> crate::Result<DoctorReport> {
    let home = store.home().display().to_string();
    let seal_ok = store.seal_key().is_ok();
    let binding_summaries = store.list_bindings()?;
    let bindings = binding_summaries.len();
    // Freeze on drift first so findings reflect the frozen session state.
    let runtime = store.check_drift_and_freeze()?;
    let active = store.active_session()?;
    let approvals = store.approvals_health()?;
    let pending = store.pending_approvals()?;
    let pending_approvals = pending.len();
    let dual_control_waiting = count_dual_control_waiting(store, &pending);

    // A missing operator capability in this (read-only) process means the
    // anchor check could not authenticate itself — an environment gap
    // (`control_capability_not_exported` already covers it), not evidence of
    // pin tamper. A wrong or tampered capability reaches the live anchor and
    // fails as `authority_anchor_mismatch` / `authority_anchor_unavailable`,
    // which stay fail-closed (UNSAFE) below.
    let capability_status = crate::authority_anchor::control_capability_status(store.home());
    let anchor_capability_absent = !runtime.authority_anchor_ok
        && anchor_unverified_due_to_absent_capability(
            &runtime.issues,
            capability_status.env_present,
        );

    let mut pin_seal_ok: Option<bool> = None;
    let mut pin: Option<DoctorPin> = None;
    if let Some(ref sess) = active {
        let runtime_verified = runtime.seal_ok
            && (runtime.authority_anchor_ok || anchor_capability_absent)
            && runtime.backing_ok
            && !sess.is_expired();
        pin_seal_ok = Some(runtime_verified);
        pin = Some(DoctorPin {
            alias: sess.binding_alias.clone(),
            tenant: sess.tenant.clone(),
            binding_id: sess.binding_id.clone(),
            expires_at: sess.expires_at.to_rfc3339(),
            expires_in_secs: (sess.expires_at - Utc::now()).num_seconds().max(0),
            seal_ok: runtime_verified,
            // Honest verification status: true only when the anchor check
            // actually ran and passed. A capability-absent skip keeps
            // `seal_ok=true` (no false UNSAFE) but reports `Some(false)` here
            // alongside the `executor_authority_unavailable` Warn finding.
            authority_anchor_verified: Some(runtime.authority_anchor_ok),
            principal: sess.principal.clone(),
            client: sess.client.clone(),
            expired: sess.is_expired(),
        });
    }

    let config_path = store.config_path();
    let cfg_opt = LocusConfig::load(&config_path).ok().flatten();
    let autopin = AutopinStatus::from_config(&config_path, cfg_opt.as_ref());
    // Keep load_config warm for side-effect-free default parse (corrupt → defaults).
    let _ = config::load_config(store.home());

    let cwd = external.cwd.clone().unwrap_or_else(|| {
        std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf())
    });
    let workspace = workspace_status(
        store,
        &cwd,
        active.as_ref().map(|s| s.binding_alias.as_str()),
    );

    let audit = audit_summary(store, 5, 50)?;
    let all_events = store.read_audit_events()?;
    let near_miss = count_near_misses(&all_events, 24, None);
    let near_miss_count = near_miss.count;

    let mut findings: Vec<DoctorIssue> = Vec::new();

    // ── Unsafe ────────────────────────────────────────────────────────────
    let migration_readiness = crate::credential_migration::migration_readiness(store);
    if !migration_readiness.ready() {
        findings.push(issue(
            IssueSeverity::Unsafe,
            "credential_migration_incomplete",
            format!(
                "credential migration reconciliation required (pending={}, invalid={}, scan_failed={})",
                migration_readiness.pending,
                migration_readiness.invalid,
                migration_readiness.scan_failed
            ),
        ));
    }
    if !seal_ok {
        findings.push(issue(
            IssueSeverity::Unsafe,
            "seal_key",
            "seal key missing or unreadable",
        ));
    }
    if let Some(false) = pin_seal_ok {
        findings.push(issue(
            IssueSeverity::Unsafe,
            "invalid_seal",
            "active pin seal invalid (session file may be tampered)",
        ));
    }
    for tag in &runtime.issues {
        match tag.as_str() {
            "not_pinned" => {}
            "invalid_seal" => {
                if !findings.iter().any(|f| f.code == "invalid_seal") {
                    findings.push(issue(
                        IssueSeverity::Unsafe,
                        "invalid_seal",
                        "runtime drift: invalid_seal",
                    ));
                }
            }
            "binding_id_drift" | "tenant_drift" | "binding_missing" | "providers_drift"
            | "namespace_drift" => {
                findings.push(issue(
                    IssueSeverity::Unsafe,
                    tag,
                    format!("runtime drift: {tag}"),
                ));
            }
            "session_frozen" => {
                findings.push(issue(
                    IssueSeverity::Unsafe,
                    "session_frozen",
                    "session_frozen: re-pin — binding drifted under active pin",
                ));
            }
            "session_expired" => {
                findings.push(issue(
                    IssueSeverity::Warn,
                    "session_expired",
                    "active pin is expired — re-pin before acting",
                ));
            }
            "executor_authority_unavailable" if anchor_capability_absent => {
                findings.push(issue(
                    IssueSeverity::Warn,
                    "executor_authority_unavailable",
                    "authority anchor unverified: no control capability in this environment \
                     (read-only check; pin left intact) — eval \"$(locus hook zsh)\" and re-run \
                     doctor to verify the anchor",
                ));
            }
            other => {
                findings.push(issue(
                    IssueSeverity::Warn,
                    other,
                    format!("runtime drift: {other}"),
                ));
            }
        }
    }
    // Expiring-soon pin (auto-leave TTL): warn in the final 5 minutes so the
    // operator can re-pin before fail-closed expiry cuts tool access mid-task.
    if let Some(ref p) = pin {
        if !p.expired && p.expires_in_secs > 0 && p.expires_in_secs < 300 {
            findings.push(issue(
                IssueSeverity::Warn,
                "pin_expiring",
                format!(
                    "active pin '{}' expires in {} — re-pin: locus enter {} (or set policy.default_ttl)",
                    p.alias,
                    human_remaining(p.expires_in_secs),
                    p.alias
                ),
            ));
        }
    }
    if approvals.exists && !approvals.writable {
        findings.push(issue(
            IssueSeverity::Unsafe,
            "approvals_not_writable",
            format!("approvals dir not writable: {}", approvals.dir),
        ));
    }
    if approvals.corrupt > 0 {
        findings.push(issue(
            IssueSeverity::Unsafe,
            "approvals_corrupt",
            format!(
                "approvals: {} corrupt record(s) in {}",
                approvals.corrupt, approvals.dir
            ),
        ));
    }

    // ── Warn ──────────────────────────────────────────────────────────────
    if bindings == 0 {
        findings.push(issue(
            IssueSeverity::Warn,
            "no_bindings",
            "no bindings configured (locus init --with-samples)",
        ));
    }
    if !approvals.exists {
        findings.push(issue(
            IssueSeverity::Warn,
            "approvals_missing",
            format!(
                "approvals dir missing: {} (created on first require_approval block)",
                approvals.dir
            ),
        ));
    }
    if !external.phantom_on_path {
        findings.push(issue(
            IssueSeverity::Warn,
            "phantom_missing",
            "phantom not on PATH (install Phantom Secrets for Phantom credential references)",
        ));
    }
    if !external.unresolved_phm.is_empty() {
        findings.push(issue(
            IssueSeverity::Warn,
            "unresolved_phm",
            format!(
                "unavailable credentials: {}",
                external
                    .unresolved_phm
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }
    if !autopin.ok {
        findings.push(issue(
            IssueSeverity::Warn,
            "autopin_invalid",
            autopin
                .note
                .clone()
                .unwrap_or_else(|| "autopin misconfigured".into()),
        ));
    }
    // Aliases starting with "locus" collide with the control-tool namespace:
    // the MCP gate never routes `locus*__tool` names, so such a binding is
    // silently unreachable from agents. New saves are rejected; flag legacy
    // files created by hand.
    for summary in &binding_summaries {
        if summary.alias.starts_with("locus") {
            findings.push(issue(
                IssueSeverity::Warn,
                "reserved_alias",
                format!(
                    "binding alias '{}' starts with reserved prefix 'locus' — its tools cannot be \
                     routed through the MCP gate; rename the binding file under bindings/",
                    summary.alias
                ),
            ));
        }
    }
    if !workspace.valid {
        findings.push(issue(
            IssueSeverity::Unsafe,
            "workspace_policy_invalid",
            workspace
                .error
                .clone()
                .unwrap_or_else(|| "workspace policy is invalid".into()),
        ));
    }
    if workspace.require_pin && active.is_none() {
        findings.push(issue(
            IssueSeverity::Warn,
            "require_pin",
            "workspace require_pin=true but no active pin",
        ));
    }
    if let Some(false) = workspace.pin_allowed {
        findings.push(issue(
            IssueSeverity::Warn,
            "workspace_allowlist",
            "active pin is outside workspace allowed_bindings (was force-pinned?)",
        ));
    }
    if pending_approvals > 0 {
        findings.push(issue(
            IssueSeverity::Warn,
            "pending_approvals",
            format!("{pending_approvals} pending approval(s)"),
        ));
    }
    if dual_control_waiting > 0 {
        findings.push(issue(
            IssueSeverity::Warn,
            "dual_control_waiting",
            format!(
                "{dual_control_waiting} dual-control approval(s) have exactly one externally authenticated principal and await a second"
            ),
        ));
    }
    if approvals.untrusted_approved > 0 {
        findings.push(issue(
            IssueSeverity::Unsafe,
            "untrusted_approved",
            format!(
                "{} approved-looking approval record(s) lack independently authenticated authority",
                approvals.untrusted_approved
            ),
        ));
    }
    if approvals.expired_grants > 0 {
        findings.push(issue(
            IssueSeverity::Warn,
            "expired_authenticated_grants",
            format!(
                "{} externally authenticated approval grant(s) expired",
                approvals.expired_grants
            ),
        ));
    }
    if audit.scope_freeze > 0 {
        findings.push(issue(
            IssueSeverity::Warn,
            "recent_scope_freeze",
            format!(
                "{} scope_freeze event(s) in recent audit tail",
                audit.scope_freeze
            ),
        ));
    }
    if audit.deny > 0 {
        findings.push(issue(
            IssueSeverity::Warn,
            "recent_deny",
            format!("{} deny event(s) in recent audit tail", audit.deny),
        ));
    }
    if near_miss_count > 0 {
        findings.push(issue(
            IssueSeverity::Warn,
            "near_miss",
            format!(
                "{near_miss_count} near-miss event(s) in last 24h (scope_freeze={}, require_approval={})",
                near_miss.scope_freeze, near_miss.require_approval
            ),
        ));
    }
    // Light M5 verification-plane signal: many recent audit details look ungrounded
    // (numbers / URLs / versions). Optional WARN — never escalates to UNSAFE alone.
    let low_conf_audit =
        count_low_confidence_audit_signals(&all_events, DOCTOR_LOW_CONFIDENCE_AUDIT_SCAN);
    if let Some(msg) = doctor_low_confidence_message(low_conf_audit) {
        findings.push(issue(IssueSeverity::Warn, "ungrounded_claims", msg));
    }

    let mut verdict = DoctorVerdict::Safe;
    for f in &findings {
        let v = match f.severity {
            IssueSeverity::Unsafe => DoctorVerdict::Unsafe,
            IssueSeverity::Warn => DoctorVerdict::Warn,
            IssueSeverity::Info => DoctorVerdict::Safe,
        };
        verdict = verdict.escalate(v);
    }

    let issues: Vec<String> = findings.iter().map(|f| f.message.clone()).collect();

    Ok(DoctorReport {
        version: VERSION.to_string(),
        home,
        seal_ok,
        bindings,
        pinned: active.as_ref().map(|a| a.binding_alias.clone()),
        pin,
        pin_seal_ok,
        runtime,
        approvals,
        pending_approvals,
        dual_control_waiting,
        phantom_on_path: external.phantom_on_path,
        unresolved_phm: external.unresolved_phm,
        autopin,
        workspace,
        audit,
        near_miss_count,
        near_miss,
        findings,
        issues,
        verdict,
        ok: verdict == DoctorVerdict::Safe,
    })
}

/// "4m59s" / "45s" — short remaining-time label for findings.
fn human_remaining(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs % 60 == 0 {
        format!("{}m", secs / 60)
    } else {
        format!("{}m{}s", secs / 60, secs % 60)
    }
}

fn issue(
    severity: IssueSeverity,
    code: impl Into<String>,
    message: impl Into<String>,
) -> DoctorIssue {
    DoctorIssue {
        severity,
        code: code.into(),
        message: message.into(),
    }
}

fn count_dual_control_waiting(store: &Store, pending: &[crate::approval::ApprovalRecord]) -> usize {
    pending
        .iter()
        .filter(|rec| {
            store.tool_requires_dual_control(&rec.binding, &rec.tool)
                && rec.authoritative_grant_count() == 1
        })
        .count()
}

fn workspace_status(store: &Store, cwd: &Path, active_alias: Option<&str>) -> WorkspaceStatus {
    match store.workspace_for(cwd) {
        Ok(Some((path, cfg))) => {
            let pin_allowed = active_alias.map(|a| cfg.allows(a));
            WorkspaceStatus {
                found: true,
                valid: true,
                path: Some(path.display().to_string()),
                default_binding: cfg.default_binding.clone(),
                allowed_bindings: cfg.allowed_bindings.clone(),
                require_pin: cfg.require_pin,
                pin_allowed,
                error: None,
            }
        }
        Ok(None) => WorkspaceStatus {
            found: false,
            valid: true,
            path: None,
            default_binding: None,
            allowed_bindings: Vec::new(),
            require_pin: false,
            pin_allowed: None,
            error: None,
        },
        Err(e) => WorkspaceStatus {
            found: true,
            valid: false,
            path: None,
            default_binding: None,
            allowed_bindings: Vec::new(),
            require_pin: false,
            pin_allowed: None,
            error: Some(e.to_string()),
        },
    }
}

fn audit_summary(store: &Store, last_n: usize, scan_n: usize) -> crate::Result<AuditSummary> {
    let events = store.read_audit_events()?;
    let total = events.len();
    let scan_start = total.saturating_sub(scan_n);
    let scanned = &events[scan_start..];
    let mut scope_freeze = 0usize;
    let mut deny = 0usize;
    for ev in scanned {
        if is_scope_freeze_op(&ev.op) {
            scope_freeze += 1;
        }
        if is_deny_op(&ev.op) {
            deny += 1;
        }
    }
    let last: Vec<AuditEvent> = events.iter().rev().take(last_n).cloned().collect();
    Ok(AuditSummary {
        path: store.audit_path().display().to_string(),
        total,
        last,
        scope_freeze,
        deny,
    })
}

fn is_scope_freeze_op(op: &str) -> bool {
    op.contains("scope_freeze")
}

fn is_deny_op(op: &str) -> bool {
    op == "approval.deny"
        || op.ends_with(".deny")
        || op.contains("denied")
        || op.contains("policy.deny")
        || op == "mcp.deny"
}

/// Filter helpers for `locus events`.
pub fn filter_audit_events(
    events: &[AuditEvent],
    last: usize,
    op_substr: Option<&str>,
    binding: Option<&str>,
) -> Vec<AuditEvent> {
    let mut filtered: Vec<AuditEvent> = events
        .iter()
        .filter(|e| {
            if let Some(op) = op_substr {
                // Exact match or substring for convenience (scope_freeze)
                if e.op != op && !e.op.contains(op) {
                    return false;
                }
            }
            if let Some(b) = binding {
                if e.binding != b {
                    return false;
                }
            }
            true
        })
        .cloned()
        .collect();
    if filtered.len() > last {
        filtered = filtered.split_off(filtered.len() - last);
    }
    filtered
}

/// Schema keys expected in doctor JSON (mission-control contract).
pub const DOCTOR_JSON_KEYS: &[&str] = &[
    "version",
    "home",
    "seal_ok",
    "bindings",
    "runtime",
    "approvals",
    "pending_approvals",
    "dual_control_waiting",
    "phantom_on_path",
    "unresolved_phm",
    "autopin",
    "workspace",
    "audit",
    "near_miss_count",
    "near_miss",
    "findings",
    "issues",
    "verdict",
    "ok",
];

/// Validate a serialized doctor report has the stable mission-control keys.
pub fn doctor_json_has_stable_keys(value: &serde_json::Value) -> Result<(), Vec<String>> {
    let obj = match value.as_object() {
        Some(o) => o,
        None => return Err(vec!["root is not an object".into()]),
    };
    let mut missing = Vec::new();
    for k in DOCTOR_JSON_KEYS {
        if !obj.contains_key(*k) {
            missing.push((*k).to_string());
        }
    }
    if let Some(ws) = obj.get("workspace") {
        for k in ["found", "valid", "allowed_bindings", "require_pin"] {
            if ws.get(k).is_none() {
                missing.push(format!("workspace.{k}"));
            }
        }
    }
    if let Some(au) = obj.get("audit") {
        for k in ["path", "total", "last", "scope_freeze", "deny"] {
            if au.get(k).is_none() {
                missing.push(format!("audit.{k}"));
            }
        }
    }
    if let Some(ap) = obj.get("approvals") {
        for k in [
            "dir",
            "exists",
            "writable",
            "total",
            "pending",
            "approved_valid",
            "expired_grants",
            "untrusted_approved",
            "denied",
            "corrupt",
            "approval_authority",
            "authoritative_path_enabled",
            "ok",
        ] {
            if ap.get(k).is_none() {
                missing.push(format!("approvals.{k}"));
            }
        }
    }
    if let Some(nm) = obj.get("near_miss") {
        for k in ["window_hours", "count", "scope_freeze", "require_approval"] {
            if nm.get(k).is_none() {
                missing.push(format!("near_miss.{k}"));
            }
        }
    }
    if let Some(ap) = obj.get("autopin") {
        for k in [
            "path",
            "present",
            "remote_autopin_enabled",
            "remote_rules",
            "ok",
        ] {
            if ap.get(k).is_none() {
                missing.push(format!("autopin.{k}"));
            }
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(missing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::{Binding, BindingBody, Policy, ProviderBinding, Scope};
    use crate::store::Store;
    use tempfile::tempdir;

    fn sample_binding(alias: &str, tenant: &str) -> Binding {
        Binding::from_body(BindingBody {
            id: format!("bnd_{alias}"),
            alias: alias.into(),
            tenant: tenant.into(),
            principal: None,
            description: None,
            policy: Policy {
                dual_control: vec!["*.delete*".into()],
                require_approval: vec!["*.delete*".into()],
                ..Policy::default()
            },
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
    fn reserved_locus_alias_is_flagged() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        // Simulate a hand-written legacy binding file (save_binding rejects these).
        let b = sample_binding("locusx", "acme-corp");
        std::fs::write(
            store.bindings_dir().join("locusx.toml"),
            b.to_toml().unwrap(),
        )
        .unwrap();

        let report = build_doctor_report(
            &store,
            DoctorExternal {
                phantom_on_path: true,
                unresolved_phm: Vec::new(),
                cwd: Some(dir.path().to_path_buf()),
            },
        )
        .unwrap();
        let flagged: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.code == "reserved_alias")
            .collect();
        assert_eq!(flagged.len(), 1);
        assert!(flagged[0].message.contains("locusx"));
        assert_eq!(report.verdict, DoctorVerdict::Warn);
    }

    #[test]
    fn push_finding_re_escalates_verdict() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp"))
            .unwrap();
        store.pin("acme", dir.path(), None, false).unwrap();
        let mut report = build_doctor_report(
            &store,
            DoctorExternal {
                phantom_on_path: true,
                unresolved_phm: Vec::new(),
                cwd: Some(dir.path().to_path_buf()),
            },
        )
        .unwrap();
        assert_eq!(report.verdict, DoctorVerdict::Safe);
        assert!(report.ok);

        // Info never escalates: SAFE stays SAFE.
        report.push_finding(
            IssueSeverity::Info,
            "control_capability_persisted",
            "posture".into(),
        );
        assert_eq!(report.verdict, DoctorVerdict::Safe);
        assert!(report.ok);

        report.push_finding(
            IssueSeverity::Warn,
            "control_capability_missing",
            "test".into(),
        );
        assert_eq!(report.verdict, DoctorVerdict::Warn);
        assert!(!report.ok);
        assert_eq!(report.findings.len(), report.issues.len());

        report.push_finding(IssueSeverity::Unsafe, "x", "y".into());
        assert_eq!(report.verdict, DoctorVerdict::Unsafe);
    }

    #[test]
    fn absent_capability_is_environment_gap_not_tamper_evidence() {
        let s = |tags: &[&str]| tags.iter().map(|t| (*t).to_string()).collect::<Vec<_>>();

        // Regression: doctor against a healthy pin with LOCUS_CONTROL_CAPABILITY
        // absent must read as "anchor unverified (env gap)", never as seal
        // tamper — the anchor check errored before it could authenticate.
        let absent_only = s(&["executor_authority_unavailable"]);
        assert!(anchor_unverified_due_to_absent_capability(
            &absent_only,
            false
        ));

        // Capability present (even malformed → same error tag) stays
        // fail-closed: present-but-wrong is not "absent".
        assert!(!anchor_unverified_due_to_absent_capability(
            &absent_only,
            true
        ));

        // Any authenticated anchor/seal evidence keeps the tamper verdict,
        // capability absent or not.
        for tamper in [
            "authority_anchor_mismatch",
            "authority_anchor_unavailable",
            "invalid_seal",
        ] {
            let mixed = s(&["executor_authority_unavailable", tamper]);
            assert!(
                !anchor_unverified_due_to_absent_capability(&mixed, false),
                "{tamper} must stay fail-closed"
            );
        }

        // Binding drift alongside absence: the seal verdict is still not
        // "tampered" (drift keeps its own UNSAFE finding + freeze path in the
        // store, which requires an authenticated anchor).
        let drift = s(&["executor_authority_unavailable", "providers_drift"]);
        assert!(anchor_unverified_due_to_absent_capability(&drift, false));

        // No anchor failure at all → nothing to excuse.
        assert!(!anchor_unverified_due_to_absent_capability(&s(&[]), false));
    }

    #[test]
    fn control_capability_findings_cover_all_states() {
        use crate::authority_anchor::ControlCapabilityStatus;
        let home = Path::new("/tmp/locus-test-home");
        let base = ControlCapabilityStatus {
            env_present: false,
            env_valid: false,
            persisted: false,
            persisted_valid: false,
            persisted_permissions_ok: true,
            matches_persisted: None,
            test_fallback: false,
        };

        // Missing everywhere → actionable missing finding.
        let f = control_capability_findings(&base, home);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].code, "control_capability_missing");
        assert!(f[0].message.contains("openssl rand -hex 32"));

        // Persisted but not exported → hook hint, includes the file path,
        // plus the ambient-authority posture info.
        let s = ControlCapabilityStatus {
            persisted: true,
            persisted_valid: true,
            ..base.clone()
        };
        let f = control_capability_findings(&s, home);
        assert_eq!(f[0].code, "control_capability_not_exported");
        assert!(f[0].message.contains("control_capability"));
        assert_eq!(f[1].code, "control_capability_persisted");
        assert_eq!(f[1].severity, IssueSeverity::Info);

        // Invalid env value.
        let s = ControlCapabilityStatus {
            env_present: true,
            ..base.clone()
        };
        let f = control_capability_findings(&s, home);
        assert_eq!(f[0].code, "control_capability_invalid");

        // Mismatch env vs persisted (both individually valid).
        let s = ControlCapabilityStatus {
            env_present: true,
            env_valid: true,
            persisted: true,
            persisted_valid: true,
            matches_persisted: Some(false),
            ..base.clone()
        };
        let f = control_capability_findings(&s, home);
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].code, "control_capability_mismatch");
        assert!(f[0].message.contains("never silently replaces"));
        assert_eq!(f[1].code, "control_capability_persisted");

        // Healthy default posture: env valid + matching persisted → only the
        // INFO posture note (never a warning; persistence is the default).
        let s = ControlCapabilityStatus {
            env_present: true,
            env_valid: true,
            persisted: true,
            persisted_valid: true,
            matches_persisted: Some(true),
            ..base.clone()
        };
        let f = control_capability_findings(&s, home);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].code, "control_capability_persisted");
        assert_eq!(f[0].severity, IssueSeverity::Info);
        assert!(f[0].message.contains("locus capability unpersist"));

        // Strict posture: env-only, nothing persisted → no findings at all.
        let s = ControlCapabilityStatus {
            env_present: true,
            env_valid: true,
            ..base.clone()
        };
        assert!(control_capability_findings(&s, home).is_empty());

        // Test-harness fallback alone is satisfied → no missing finding.
        let s = ControlCapabilityStatus {
            test_fallback: true,
            ..base
        };
        assert!(control_capability_findings(&s, home).is_empty());
    }

    #[test]
    fn doctor_json_schema_stable_unpinned() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let report = build_doctor_report(
            &store,
            DoctorExternal {
                phantom_on_path: true,
                unresolved_phm: Vec::new(),
                cwd: Some(dir.path().to_path_buf()),
            },
        )
        .unwrap();
        let v = serde_json::to_value(&report).unwrap();
        doctor_json_has_stable_keys(&v).expect("stable keys");
        assert_eq!(report.verdict, DoctorVerdict::Warn); // no bindings
        assert!(!report.ok);
        assert_eq!(report.verdict.exit_code(), 1);
        assert!(report.pin.is_none());
        assert_eq!(report.pending_approvals, 0);
        assert_eq!(report.dual_control_waiting, 0);
        assert_eq!(report.near_miss_count, 0);
        assert_eq!(report.near_miss.count, 0);
    }

    #[test]
    fn pin_expiring_warns_under_5m_only() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp"))
            .unwrap();
        let external = || DoctorExternal {
            phantom_on_path: true,
            unresolved_phm: Vec::new(),
            cwd: Some(dir.path().to_path_buf()),
        };

        // 10m remaining → no pin_expiring finding.
        store
            .pin_with_ttl("acme", dir.path(), None, false, Some(Duration::minutes(10)))
            .unwrap();
        let report = build_doctor_report(&store, external()).unwrap();
        assert!(
            !report.findings.iter().any(|f| f.code == "pin_expiring"),
            "10m left must not warn: {:?}",
            report.findings
        );

        // 2m remaining → Warn finding with remediation, expires_in_secs in (0, 300].
        store
            .pin_with_ttl("acme", dir.path(), None, false, Some(Duration::minutes(2)))
            .unwrap();
        let report = build_doctor_report(&store, external()).unwrap();
        let f = report
            .findings
            .iter()
            .find(|f| f.code == "pin_expiring")
            .expect("pin_expiring finding");
        assert_eq!(f.severity, IssueSeverity::Warn);
        assert!(f.message.contains("locus enter acme"), "{}", f.message);
        let pin = report.pin.expect("pin slice");
        assert!(pin.expires_in_secs > 0 && pin.expires_in_secs <= 300);
        assert!(!pin.expired);
        assert_ne!(report.verdict, DoctorVerdict::Unsafe);
    }

    #[test]
    fn doctor_safe_when_pinned_and_clean() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp"))
            .unwrap();
        store.pin("acme", dir.path(), None, false).unwrap();

        let report = build_doctor_report(
            &store,
            DoctorExternal {
                phantom_on_path: true,
                unresolved_phm: Vec::new(),
                cwd: Some(dir.path().to_path_buf()),
            },
        )
        .unwrap();
        let v = serde_json::to_value(&report).unwrap();
        doctor_json_has_stable_keys(&v).expect("stable keys");
        assert_eq!(report.pinned.as_deref(), Some("acme"));
        assert_eq!(
            report.pin.as_ref().map(|p| p.tenant.as_str()),
            Some("acme-corp")
        );
        assert_eq!(report.pin_seal_ok, Some(true));
        // The anchor check actually ran here (runtime.ok) — verified.
        assert_eq!(
            report.pin.as_ref().unwrap().authority_anchor_verified,
            Some(true)
        );
        assert!(report.seal_ok);
        assert!(report.runtime.ok);
        assert_ne!(report.verdict, DoctorVerdict::Unsafe);
        // Clean env with env: creds + phantom on path + approvals dir → SAFE
        // (audit may have pin events but no scope_freeze/deny)
        assert_eq!(
            report.verdict,
            DoctorVerdict::Safe,
            "issues={:?}",
            report.issues
        );
        assert!(report.ok);
        assert_eq!(report.verdict.exit_code(), 0);
    }

    #[test]
    fn doctor_unsafe_on_seal_tamper() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp"))
            .unwrap();
        store.pin("acme", dir.path(), None, false).unwrap();
        let path = store.active_session_path();
        let raw = std::fs::read_to_string(&path).unwrap();
        let mut sess: crate::session::Session = serde_json::from_str(&raw).unwrap();
        sess.binding_id = "bnd_evil".into();
        std::fs::write(&path, serde_json::to_string(&sess).unwrap()).unwrap();

        let report = build_doctor_report(
            &store,
            DoctorExternal {
                phantom_on_path: true,
                unresolved_phm: Vec::new(),
                cwd: Some(dir.path().to_path_buf()),
            },
        )
        .unwrap();
        assert_eq!(report.verdict, DoctorVerdict::Unsafe);
        assert_eq!(report.verdict.exit_code(), 2);
        assert!(!report.ok);
        assert!(report
            .findings
            .iter()
            .any(|f| f.code == "invalid_seal" || f.severity == IssueSeverity::Unsafe));
    }

    #[test]
    fn doctor_workspace_require_pin_warn() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp"))
            .unwrap();
        std::fs::write(
            dir.path().join(".locus.toml"),
            r#"
version = 1
default_binding = "acme"
allowed_bindings = ["acme"]
require_pin = true
"#,
        )
        .unwrap();

        let report = build_doctor_report(
            &store,
            DoctorExternal {
                phantom_on_path: true,
                unresolved_phm: Vec::new(),
                cwd: Some(dir.path().to_path_buf()),
            },
        )
        .unwrap();
        assert!(report.workspace.found);
        assert!(report.workspace.require_pin);
        assert!(report.findings.iter().any(|f| f.code == "require_pin"));
        assert_eq!(report.verdict, DoctorVerdict::Warn);
    }

    #[test]
    fn doctor_is_unsafe_for_malformed_workspace_policy() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp"))
            .unwrap();
        std::fs::write(dir.path().join(".locus.toml"), "allowed_bindings = [").unwrap();

        let report = build_doctor_report(
            &store,
            DoctorExternal {
                phantom_on_path: true,
                unresolved_phm: Vec::new(),
                cwd: Some(dir.path().to_path_buf()),
            },
        )
        .unwrap();
        assert_eq!(report.verdict, DoctorVerdict::Unsafe);
        assert!(!report.ok);
        assert!(report.workspace.found);
        assert!(!report.workspace.valid);
        assert!(report
            .findings
            .iter()
            .any(|finding| finding.code == "workspace_policy_invalid"));
    }

    #[cfg(unix)]
    #[test]
    fn doctor_is_unsafe_for_broken_workspace_link() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let store = Store::open(dir.path().join("locus-home")).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp"))
            .unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        symlink("missing-policy.toml", project.join(".locus.toml")).unwrap();

        let report = build_doctor_report(
            &store,
            DoctorExternal {
                phantom_on_path: true,
                unresolved_phm: Vec::new(),
                cwd: Some(project),
            },
        )
        .unwrap();
        assert_eq!(report.verdict, DoctorVerdict::Unsafe);
        assert!(!report.workspace.valid);
        assert!(report
            .findings
            .iter()
            .any(|finding| finding.code == "workspace_policy_invalid"));
    }

    #[test]
    fn doctor_autopin_status() {
        use crate::config::AutopinConfig;
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        // Write enabled autopin with no remotes → warn
        let cfg = LocusConfig {
            autopin: AutopinConfig {
                enabled: true,
                remotes: vec![],
            },
            ..Default::default()
        };
        store.save_config(&cfg).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp"))
            .unwrap();
        store.pin("acme", dir.path(), None, false).unwrap();

        let report = build_doctor_report(
            &store,
            DoctorExternal {
                phantom_on_path: true,
                unresolved_phm: Vec::new(),
                cwd: Some(dir.path().to_path_buf()),
            },
        )
        .unwrap();
        assert!(report.autopin.present);
        assert!(report.autopin.remote_autopin_enabled);
        assert_eq!(report.autopin.remote_rules, 0);
        assert!(!report.autopin.ok);
        assert!(report.findings.iter().any(|f| f.code == "autopin_invalid"));
    }

    #[test]
    fn filter_audit_events_last_and_op() {
        let events = vec![
            AuditEvent {
                ts: "1".into(),
                op: "pin".into(),
                binding: "a".into(),
                detail: None,
            },
            AuditEvent {
                ts: "2".into(),
                op: "mcp.scope_freeze".into(),
                binding: "a".into(),
                detail: None,
            },
            AuditEvent {
                ts: "3".into(),
                op: "approval.deny".into(),
                binding: "b".into(),
                detail: None,
            },
        ];
        let f = filter_audit_events(&events, 10, Some("scope_freeze"), None);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].op, "mcp.scope_freeze");
        let f2 = filter_audit_events(&events, 1, None, None);
        assert_eq!(f2.len(), 1);
        assert_eq!(f2[0].op, "approval.deny");
        let f3 = filter_audit_events(&events, 10, None, Some("b"));
        assert_eq!(f3.len(), 1);
        assert_eq!(f3[0].binding, "b");
    }

    #[test]
    fn near_miss_count_last_24h() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp"))
            .unwrap();
        store.pin("acme", dir.path(), None, false).unwrap();
        store
            .audit(
                "mcp.scope_freeze",
                "acme",
                Some(serde_json::json!({"tool": "x"})),
            )
            .unwrap();
        store
            .audit(
                "mcp.require_approval",
                "acme",
                Some(serde_json::json!({"tool": "y"})),
            )
            .unwrap();

        let report = build_doctor_report(
            &store,
            DoctorExternal {
                phantom_on_path: true,
                unresolved_phm: Vec::new(),
                cwd: Some(dir.path().to_path_buf()),
            },
        )
        .unwrap();
        assert!(report.near_miss_count >= 2);
        assert!(report.near_miss.scope_freeze >= 1);
        assert!(report.near_miss.require_approval >= 1);
        assert!(report.findings.iter().any(|f| f.code == "near_miss"));
        assert_eq!(report.verdict, DoctorVerdict::Warn);
    }

    #[test]
    fn live_doctor_report_validates_against_published_schema() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp"))
            .unwrap();
        store.pin("acme", dir.path(), None, false).unwrap();

        let report = build_doctor_report(
            &store,
            DoctorExternal {
                phantom_on_path: true,
                unresolved_phm: Vec::new(),
                cwd: Some(dir.path().to_path_buf()),
            },
        )
        .unwrap();
        let instance = serde_json::to_value(report).unwrap();
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../../../schema/doctor.schema.json")).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let errors: Vec<String> = validator
            .iter_errors(&instance)
            .map(|error| error.to_string())
            .collect();
        assert!(
            errors.is_empty(),
            "live doctor JSON violated schema: {}\n{}",
            errors.join("\n"),
            serde_json::to_string_pretty(&instance).unwrap()
        );
        assert_eq!(
            instance["approvals"]["approval_authority"],
            "local_advisory"
        );
        assert_eq!(instance["approvals"]["authoritative_path_enabled"], false);
    }

    #[test]
    fn advisory_labels_do_not_count_as_waiting_for_second_principal() {
        use crate::adapters::{call_tool_gated, ApprovalGate};
        use serde_json::json;

        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp"))
            .unwrap();
        let session = store.pin("acme", dir.path(), None, false).unwrap();
        let binding = store.load_binding("acme").unwrap();
        let gate = ApprovalGate {
            store: &store,
            session_id: &session.session_id,
            principal: Some("agent"),
        };
        let blocked = call_tool_gated(
            &binding,
            "github.delete_repo",
            &json!({ "name": "x" }),
            Some(gate),
        )
        .unwrap();
        let id = blocked.content["approval_id"].as_str().unwrap();
        store.grant_approval(id, None, "alice").unwrap();
        store.grant_approval(id, None, "bob").unwrap();

        let report = build_doctor_report(
            &store,
            DoctorExternal {
                phantom_on_path: true,
                unresolved_phm: Vec::new(),
                cwd: Some(dir.path().to_path_buf()),
            },
        )
        .unwrap();
        assert!(report.pending_approvals >= 1);
        assert_eq!(report.dual_control_waiting, 0);
        assert!(!report
            .findings
            .iter()
            .any(|f| f.code == "dual_control_waiting"));
        assert!(report
            .findings
            .iter()
            .any(|f| f.code == "pending_approvals"));
    }

    #[test]
    fn approved_looking_unverified_record_is_blocking_not_expired() {
        use crate::adapters::{call_tool_gated, ApprovalGate};
        use crate::approval::{ApprovalAuthority, ApprovalGrant, ApprovalStatus};
        use serde_json::json;

        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp"))
            .unwrap();
        let session = store.pin("acme", dir.path(), None, false).unwrap();
        let binding = store.load_binding("acme").unwrap();
        let blocked = call_tool_gated(
            &binding,
            "github.delete_repo",
            &json!({ "name": "x" }),
            Some(ApprovalGate {
                store: &store,
                session_id: &session.session_id,
                principal: Some("agent"),
            }),
        )
        .unwrap();
        let id = blocked.content["approval_id"].as_str().unwrap();
        let mut forged = store.load_approval(id).unwrap();
        forged.status = ApprovalStatus::Approved;
        forged.granted_at = Some(Utc::now());
        forged.expires_at = Some(Utc::now() + Duration::minutes(15));
        forged.grants.push(ApprovalGrant {
            principal: "forged".into(),
            granted_at: Utc::now(),
            authority: ApprovalAuthority::ExternalAuthenticated,
            envelope_id: Some("unsigned-envelope".into()),
        });
        std::fs::write(
            store.approvals_dir().join(format!("{id}.json")),
            serde_json::to_string_pretty(&forged).unwrap(),
        )
        .unwrap();

        let report = build_doctor_report(
            &store,
            DoctorExternal {
                phantom_on_path: true,
                unresolved_phm: Vec::new(),
                cwd: Some(dir.path().to_path_buf()),
            },
        )
        .unwrap();
        assert_eq!(report.approvals.approved_valid, 0);
        assert_eq!(report.approvals.untrusted_approved, 1);
        assert_eq!(report.approvals.expired_grants, 0);
        assert_eq!(report.verdict, DoctorVerdict::Unsafe);
        assert!(report
            .findings
            .iter()
            .any(|finding| finding.code == "untrusted_approved"
                && finding.severity == IssueSeverity::Unsafe));
    }
}

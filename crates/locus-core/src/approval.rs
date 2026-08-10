//! Approval records for `policy.require_approval` tools.
//!
//! Records live under `$LOCUS_HOME/approvals/{id}.json`. Agents blocked by
//! policy receive a stable `approval_id`. Local CLI assertions are advisory:
//! they are useful review evidence, but they never establish human identity or
//! authorize provider execution.
//!
//! Authoritative approval is reserved for independently authenticated external
//! envelopes. No external verifier is shipped yet, so that path fails closed.

use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const EXTERNAL_APPROVAL_AUTHORITY_BLOCKER: &str =
    "peer_authenticated_os_broker_and_non_agent_issue_capability_required";

/// Reserved external-grant lifetime (15 minutes).
///
/// Local advisory assertions never start this clock.
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

/// Trust level carried by an approval assertion.
///
/// `LocalAdvisory` is the only level this release can create. The
/// `ExternalAuthenticated` variant reserves the persisted schema for a future
/// verifier, but cannot authorize while [`external_approval_authority_enabled`]
/// is false.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalAuthority {
    #[default]
    LocalAdvisory,
    ExternalAuthenticated,
}

impl ApprovalAuthority {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LocalAdvisory => "local_advisory",
            Self::ExternalAuthenticated => "external_authenticated",
        }
    }
}

/// The external identity/signature verifier is intentionally unavailable.
///
/// This constant is an authority fence, not a feature flag. Enabling external
/// approvals requires a separately reviewed verifier with an independent trust
/// root, identity binding, nonce replay store, idempotency, expiry, scope, and
/// proposal-digest checks.
pub const fn external_approval_authority_enabled() -> bool {
    false
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

/// One principal's signature on an approval request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalGrant {
    pub principal: String,
    pub granted_at: DateTime<Utc>,
    /// Local assertions default to advisory for legacy record compatibility.
    #[serde(default)]
    pub authority: ApprovalAuthority,
    /// Reserved for a verified external envelope. Local grants never set it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope_id: Option<String>,
}

/// Closed schema required from a future external approval authority.
///
/// Persisting or editing this data is not verification. An authoritative
/// adapter must authenticate the signature with a trust root independent from
/// the Locus daemon key and atomically consume `(issuer, nonce)` before a grant
/// can be used.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExternalApprovalEnvelope {
    pub schema_version: u32,
    pub issuer: String,
    pub key_id: String,
    pub signature_algorithm: String,
    /// Peer-authenticated broker identity, separate from the agent process.
    pub broker_id: String,
    /// Digest of the broker's OS-isolation attestation.
    pub broker_attestation_sha256: String,
    /// Opaque capability issued through a channel inaccessible to the agent.
    pub issue_capability_id: String,
    /// Digest only; the issue capability itself must never enter Locus JSON.
    pub issue_capability_sha256: String,
    pub issue_capability_audience: String,
    pub envelope_id: String,
    pub idempotency_key: String,
    pub nonce: String,
    pub approval_id: String,
    pub approver_id: String,
    pub requester_id: String,
    pub tool: String,
    pub binding: String,
    pub session_id: String,
    pub args_digest: String,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub signature: String,
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
    /// Principal/session label that requested the tool call (best-effort).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub requester: String,
    /// Accumulated local advisory labels. They never satisfy dual control.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grants: Vec<ApprovalGrant>,
    /// Set when status becomes `approved`. Grant is valid until this instant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    /// Timestamp of the grant that completed approval (status → approved).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granted_at: Option<DateTime<Utc>>,
}

impl ApprovalRecord {
    pub fn is_pending(&self) -> bool {
        self.status == ApprovalStatus::Pending
    }

    /// True only for a cryptographically authenticated external grant.
    ///
    /// The external verifier is not implemented in this release, so no local
    /// or edited approval record can become valid execution authority.
    pub fn is_valid_grant(&self) -> bool {
        false
    }

    pub fn matches_call(&self, tool: &str, binding: &str, args_digest: &str) -> bool {
        self.tool == tool && self.binding == binding && self.args_digest == args_digest
    }

    /// Distinct local advisory labels that have been recorded.
    pub fn grant_principals(&self) -> Vec<&str> {
        self.grants.iter().map(|g| g.principal.as_str()).collect()
    }

    /// Whether `principal` has already granted this approval.
    pub fn has_grant_from(&self, principal: &str) -> bool {
        self.grants.iter().any(|g| g.principal == principal)
    }

    /// How many distinct advisory labels are still needed for display only.
    pub fn grants_remaining(&self, required: usize) -> usize {
        required.saturating_sub(self.grants.len())
    }
}

/// How many distinct principals must grant for this tool under `policy`.
pub fn required_grant_count(dual_control: bool) -> usize {
    if dual_control {
        2
    } else {
        1
    }
}

/// Compact progress string: `"1/2"`, `"0/1"`, …
pub fn format_grants_progress(grants: usize, required: usize) -> String {
    format!("{grants}/{required}")
}

/// Agent-facing hint when a tool is blocked pending approval (MCP / policy gate).
///
/// The legacy `grant` command only records local review evidence. It is named
/// here so operators can inspect that evidence without confusing it with the
/// independently authenticated authority required for provider execution.
pub fn agent_approval_hint(
    approval_id: &str,
    dual_control: bool,
    required: usize,
    grants: usize,
) -> String {
    let mode = if dual_control {
        "dual-control"
    } else {
        "single-approval"
    };
    format!(
        "locus approve grant {approval_id} --as <principal> records local advisory evidence only \
         ({grants} advisory labels observed; {mode} requires {required} externally authenticated \
         approver(s)); provider execution remains blocked: {EXTERNAL_APPROVAL_AUTHORITY_BLOCKER}"
    )
}

/// CLI / status line for dual-control progress after a grant.
///
/// Examples:
/// - externally verified dual: `"dual_control  grants 2/2  (alice, bob)  externally authenticated"`
/// - partial: `"dual_control  grants 1/2  (alice)  need 1 more distinct principal"`
/// - single: `"grants 1/1  (alice)  externally authenticated"`
pub fn format_dual_control_progress(
    grants: usize,
    required: usize,
    principals: &[String],
    dual_control: bool,
    externally_authenticated: bool,
) -> String {
    let progress = format_grants_progress(grants, required);
    let who = if principals.is_empty() {
        "-".to_string()
    } else {
        principals.join(", ")
    };
    let prefix = if dual_control {
        format!("dual_control  grants {progress}  ({who})")
    } else {
        format!("grants {progress}  ({who})")
    };
    if externally_authenticated {
        format!("{prefix}  externally authenticated")
    } else {
        let remaining = required.saturating_sub(grants);
        format!(
            "{prefix}  local advisory only; {remaining} more label(s) for review display; external authority required"
        )
    }
}

/// Next review action after a local advisory assertion.
pub fn next_grant_command(approval_id: &str, externally_authenticated: bool) -> String {
    if externally_authenticated {
        format!(
            "External authority for {approval_id} must be verified by the peer-authenticated broker before provider execution"
        )
    } else {
        format!(
            "locus approve grant {approval_id} --as <other-principal> records another local advisory only; external authority remains required"
        )
    }
}

/// Desktop notification body for a **new** pending approval (opt-in notify).
pub fn notification_body(rec: &ApprovalRecord) -> String {
    format!(
        "{} on {} — review advisory {} (external broker authority still required)",
        rec.tool, rec.binding, rec.id
    )
}

/// Desktop notification body after a **partial** dual-control grant (opt-in notify).
///
/// Reports local review activity without claiming it satisfies human authority.
pub fn partial_grant_notification_body(rec: &ApprovalRecord) -> String {
    let who = if rec.grants.is_empty() {
        "-".to_string()
    } else {
        rec.grant_principals().join(", ")
    };
    format!(
        "{} on {} — local advisory labels {}/2 ({}) — peer-authenticated broker authority still required for {}",
        rec.tool,
        rec.binding,
        rec.grants.len(),
        who,
        rec.id
    )
}

/// Whether desktop notifications are enabled.
///
/// **Default: OFF** — approval spam during agent/MCP use is worse than silence.
/// Enable with any of:
/// - env `LOCUS_NOTIFY=1` (or `true` / `yes`)
/// - `~/.locus/config.toml` → `[notify] enabled = true`
///
/// Always suppressed when `CI=true`, `LOCUS_QUIET=1`, or `LOCUS_NOTIFY=0`.
pub fn notifications_enabled() -> bool {
    // Explicit kill switch always wins
    if env_truthy("LOCUS_QUIET") || env_falsy("LOCUS_NOTIFY") {
        return false;
    }
    if std::env::var("CI")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false)
    {
        return false;
    }
    // Explicit enable
    if env_truthy("LOCUS_NOTIFY") {
        return true;
    }
    // Config opt-in (best-effort; never fail)
    if let Ok(home) = crate::store::locus_home() {
        let path = home.join("config.toml");
        if let Ok(Some(cfg)) = crate::config::LocusConfig::load(&path) {
            if cfg.notify.enabled {
                return true;
            }
        }
    }
    false
}

fn env_truthy(key: &str) -> bool {
    matches!(
        std::env::var(key).as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES") | Ok("on") | Ok("ON")
    )
}

fn env_falsy(key: &str) -> bool {
    matches!(
        std::env::var(key).as_deref(),
        Ok("0") | Ok("false") | Ok("FALSE") | Ok("no") | Ok("NO") | Ok("off") | Ok("OFF")
    )
}

/// Best-effort desktop notification when a **new** pending approval is created.
///
/// Opt-in only (see [`notifications_enabled`]). Rate-limited to one banner
/// per tool+binding every 60s. **No sound** (avoids notification fatigue).
pub fn try_notify_pending_approval(rec: &ApprovalRecord) {
    if !notifications_enabled() {
        return;
    }
    if !rate_limit_allow(&format!("pending::{}::{}", rec.binding, rec.tool)) {
        return;
    }
    #[cfg(target_os = "macos")]
    {
        notify_macos("Locus approval", &notification_body(rec));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = rec;
    }
}

/// Best-effort desktop notification when dual-control reaches a **partial** grant
/// (one principal granted; still needs a distinct second).
///
/// Opt-in only (see [`notifications_enabled`]). Default OFF — same kill switches
/// as pending notify (`CI`, `LOCUS_QUIET`, `LOCUS_NOTIFY=0`). Rate-limited per
/// approval id (separate from pending create so a partial can still fire after
/// a recent pending banner). **No sound**.
pub fn try_notify_partial_grant(rec: &ApprovalRecord) {
    if !notifications_enabled() {
        return;
    }
    // The store calls this only for dual-control. Notify exactly on the first
    // advisory label; later labels cannot manufacture authority or more alerts.
    if !is_first_pending_advisory(rec) {
        return;
    }
    if !rate_limit_allow(&format!("partial::{}", rec.id)) {
        return;
    }
    #[cfg(target_os = "macos")]
    {
        notify_macos("Locus dual-control", &partial_grant_notification_body(rec));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = rec;
    }
}

fn is_first_pending_advisory(rec: &ApprovalRecord) -> bool {
    rec.status == ApprovalStatus::Pending
        && rec.grants.len() == 1
        && rec.grants[0].authority == ApprovalAuthority::LocalAdvisory
}

/// Simple process-local rate limit keyed by an arbitrary string.
fn rate_limit_allow(key: &str) -> bool {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    static LAST: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);
    let mut guard = match LAST.lock() {
        Ok(g) => g,
        Err(_) => return false,
    };
    let map = guard.get_or_insert_with(HashMap::new);
    let now = Instant::now();
    if let Some(prev) = map.get(key) {
        if now.duration_since(*prev) < Duration::from_secs(60) {
            return false;
        }
    }
    map.insert(key.to_string(), now);
    true
}

#[cfg(target_os = "macos")]
fn notify_macos(title: &str, body: &str) {
    use std::path::Path;

    let osascript = Path::new("/usr/bin/osascript");
    if !osascript.is_file() {
        return;
    }

    // No sound name — silent banner only when user opted in
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        escape_applescript(body),
        escape_applescript(title)
    );
    spawn_and_reap_notification(osascript, &["-e".to_string(), script]);
}

#[cfg(target_os = "macos")]
fn spawn_and_reap_notification(program: &std::path::Path, args: &[String]) {
    use std::process::{Command, Stdio};

    let child = Command::new(program)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Ok(mut child) = child {
        // Never let desktop notification delivery stall a persisted mutation.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

#[cfg(target_os = "macos")]
fn escape_applescript(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .chars()
        .filter(|c| !c.is_control())
        .collect()
}

/// Mint a stable approval id: `appr_<24 hex chars>`.
pub fn mint_approval_id() -> String {
    let mut buf = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut buf);
    format!("appr_{}", hex::encode(buf))
}

/// Reject path traversal and malformed ids before joining under `approvals/`.
///
/// Accepts minted `appr_<hex>` and other single-component safe ids
/// (`[A-Za-z0-9_-]+`, no `.` / `/` / `\`). Path separators are always denied.
pub fn validate_approval_id(id: &str) -> crate::Result<()> {
    if id.is_empty() {
        return Err(crate::LocusError::msg("approval id is required"));
    }
    if id.contains('/') || id.contains('\\') || id.contains("..") || id.contains('\0') {
        return Err(crate::LocusError::msg(format!(
            "invalid approval id '{id}': path separators and '..' are not allowed"
        )));
    }
    if id.contains('.') {
        return Err(crate::LocusError::msg(format!(
            "invalid approval id '{id}': must not contain '.'"
        )));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(crate::LocusError::msg(format!(
            "invalid approval id '{id}': use letters, digits, '-', '_'"
        )));
    }
    if id.len() > 128 {
        return Err(crate::LocusError::msg(
            "invalid approval id: exceeds maximum length (128)",
        ));
    }
    Ok(())
}

/// Keys never folded into the digest (approval meta + common secret names).
///
/// Matching is case-insensitive. Exact names and common suffixes (`*_token`,
/// `*_secret`, `*_password`, `*_key`) are stripped so agents cannot inject
/// secrets or control fields into the digest (prompt-injection / confirm bypass).
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
    "bearer",
    "passwd",
    "passphrase",
];

fn is_stripped_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    if STRIP_KEYS.contains(&lower.as_str()) {
        return true;
    }
    lower.ends_with("_token")
        || lower.ends_with("_secret")
        || lower.ends_with("_password")
        || lower.ends_with("_passwd")
        || lower.ends_with("_key")
        || lower.ends_with("password")
        || lower.ends_with("secret")
}

/// SHA-256 digest of args for matching re-calls. Does not include secrets or
/// approval control fields (`confirm`, `approval_id`). Format: `sha256:<hex>`.
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
                if is_stripped_key(k) {
                    continue;
                }
                // Nested objects: strip secret-like / control keys recursively
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
    fn digest_strips_nested_secret_suffixes() {
        let base = json!({"table": "users", "nested": {"limit": 1}});
        let with_secrets = json!({
            "table": "users",
            "nested": {
                "limit": 1,
                "access_token": "leak-me",
                "db_password": "x",
                "client_secret": "y",
            },
            "Confirm": true, // case-insensitive control field
        });
        assert_eq!(args_digest(&base), args_digest(&with_secrets));
    }

    #[test]
    fn digest_changes_with_args() {
        let a = json!({"table": "users"});
        let b = json!({"table": "orders"});
        assert_ne!(args_digest(&a), args_digest(&b));
    }

    #[test]
    fn validate_approval_id_rejects_traversal() {
        assert!(validate_approval_id("appr_aabbccddeeff001122334455").is_ok());
        assert!(validate_approval_id("../etc/passwd").is_err());
        assert!(validate_approval_id("appr/../../../x").is_err());
        assert!(validate_approval_id("appr_foo.json").is_err());
        assert!(validate_approval_id("appr_foo\\bar").is_err());
        assert!(validate_approval_id("").is_err());
        assert!(validate_approval_id("appr with spaces").is_err());
    }

    /// Property-style: object key order must not affect the digest at any nesting level.
    #[test]
    fn digest_nested_key_order_independent() {
        let a = json!({
            "outer": { "z": 1, "a": { "y": true, "x": "v" }, "m": [2, 1] },
            "table": "users",
        });
        let b = json!({
            "table": "users",
            "outer": { "m": [2, 1], "a": { "x": "v", "y": true }, "z": 1 },
        });
        assert_eq!(args_digest(&a), args_digest(&b));

        // Array order is significant (not sorted)
        let c = json!({ "outer": { "m": [1, 2] } });
        let d = json!({ "outer": { "m": [2, 1] } });
        assert_ne!(args_digest(&c), args_digest(&d));
    }

    /// Property-style: secret-like keys are stripped recursively (any case).
    #[test]
    fn digest_strips_nested_secrets_case_insensitive() {
        let with_secrets = json!({
            "op": "delete",
            "filter": {
                "id": 42,
                "name": "row",
                "password": "p@ss",
                "API_KEY": "sk_live",
                "nested": {
                    "Authorization": "Bearer x",
                    "client_secret": "cs_x",
                    "keep": true,
                }
            },
            "token": "top-level-secret",
            "Private_Key": "-----BEGIN-----",
            "confirm": true,
        });
        // Secrets stripped; nested.keep remains
        let expected = json!({
            "op": "delete",
            "filter": {
                "id": 42,
                "name": "row",
                "nested": { "keep": true },
            },
        });
        assert_eq!(args_digest(&with_secrets), args_digest(&expected));

        let base_no_nested_keep = json!({
            "op": "delete",
            "filter": { "id": 42, "name": "row" },
        });
        assert_ne!(
            args_digest(&base_no_nested_keep),
            args_digest(&with_secrets)
        );
    }

    /// Property-style: many key-order permutations of the same leaf set digest equal.
    #[test]
    fn digest_key_order_permutations_property() {
        let keys = ["alpha", "beta", "gamma", "delta"];
        let values = [json!(1), json!("two"), json!(true), json!({"n": 0})];
        let mut digests = Vec::new();
        // Four rotations of key order
        for offset in 0..keys.len() {
            let mut map = serde_json::Map::new();
            for i in 0..keys.len() {
                let idx = (i + offset) % keys.len();
                map.insert(keys[idx].into(), values[idx].clone());
            }
            digests.push(args_digest(&Value::Object(map)));
        }
        assert!(
            digests.windows(2).all(|w| w[0] == w[1]),
            "key-order permutations produced different digests: {digests:?}"
        );
        assert!(digests[0].starts_with("sha256:"));
        assert_eq!(digests[0].len(), "sha256:".len() + 64);
    }

    #[test]
    fn digest_strips_all_known_secret_keys() {
        let mut with = serde_json::Map::new();
        with.insert("payload".into(), json!("ok"));
        for k in STRIP_KEYS {
            with.insert((*k).into(), json!("redacted"));
        }
        let base = json!({ "payload": "ok" });
        assert_eq!(args_digest(&Value::Object(with)), args_digest(&base));
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
            requester: "alice".into(),
            grants: vec![
                ApprovalGrant {
                    principal: "alice".into(),
                    granted_at: Utc::now(),
                    authority: ApprovalAuthority::ExternalAuthenticated,
                    envelope_id: Some("env_alice".into()),
                },
                ApprovalGrant {
                    principal: "bob".into(),
                    granted_at: Utc::now(),
                    authority: ApprovalAuthority::ExternalAuthenticated,
                    envelope_id: Some("env_bob".into()),
                },
            ],
            expires_at: Some(Utc::now() + Duration::minutes(5)),
            granted_at: Some(Utc::now()),
        };
        assert!(!rec.is_valid_grant(), "external verifier is disabled");
        rec.expires_at = Some(Utc::now() - Duration::minutes(1));
        assert!(!rec.is_valid_grant());
        rec.status = ApprovalStatus::Pending;
        rec.expires_at = Some(Utc::now() + Duration::minutes(5));
        assert!(!rec.is_valid_grant());
    }

    #[test]
    fn external_envelope_requires_closed_broker_and_capability_provenance() {
        let complete = json!({
            "schema_version": 1,
            "issuer": "locus-human-broker",
            "key_id": "broker-key-1",
            "signature_algorithm": "ed25519",
            "broker_id": "peer-broker-1",
            "broker_attestation_sha256": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "issue_capability_id": "cap-1",
            "issue_capability_sha256": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "issue_capability_audience": "locus-approval",
            "envelope_id": "env-1",
            "idempotency_key": "idem-1",
            "nonce": "nonce-1",
            "approval_id": "appr_test",
            "approver_id": "human-1",
            "requester_id": "agent-1",
            "tool": "supabase.table.delete",
            "binding": "acme",
            "session_id": "ses-1",
            "args_digest": "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            "issued_at": "2026-08-09T00:00:00Z",
            "expires_at": "2026-08-09T00:05:00Z",
            "signature": "base64-signature"
        });
        assert!(serde_json::from_value::<ExternalApprovalEnvelope>(complete.clone()).is_ok());

        let mut absent_attestation = complete.clone();
        absent_attestation
            .as_object_mut()
            .unwrap()
            .remove("broker_attestation_sha256");
        assert!(
            serde_json::from_value::<ExternalApprovalEnvelope>(absent_attestation).is_err(),
            "missing OS-broker attestation must fail closed"
        );

        let mut substituted_capability = complete;
        let fields = substituted_capability.as_object_mut().unwrap();
        fields.remove("issue_capability_sha256");
        fields.insert(
            "issue_capability".into(),
            json!("caller-controlled-raw-capability"),
        );
        assert!(
            serde_json::from_value::<ExternalApprovalEnvelope>(substituted_capability).is_err(),
            "raw or substituted issue capability must not satisfy the closed envelope"
        );

        let duplicate_broker = r#"{
            "schema_version":1,"issuer":"i","key_id":"k","signature_algorithm":"ed25519",
            "broker_id":"a","broker_id":"b","broker_attestation_sha256":"sha256:a",
            "issue_capability_id":"c","issue_capability_sha256":"sha256:b",
            "issue_capability_audience":"locus-approval","envelope_id":"e",
            "idempotency_key":"i","nonce":"n","approval_id":"a","approver_id":"h",
            "requester_id":"r","tool":"t","binding":"b","session_id":"s",
            "args_digest":"sha256:c","issued_at":"2026-08-09T00:00:00Z",
            "expires_at":"2026-08-09T00:05:00Z","signature":"s"
        }"#;
        assert!(
            serde_json::from_str::<ExternalApprovalEnvelope>(duplicate_broker).is_err(),
            "duplicate broker identity must fail closed"
        );
    }

    #[test]
    fn required_grant_count_dual() {
        assert_eq!(required_grant_count(false), 1);
        assert_eq!(required_grant_count(true), 2);
    }

    #[test]
    fn has_grant_from_and_remaining() {
        let rec = ApprovalRecord {
            id: "appr_test".into(),
            tool: "t".into(),
            binding: "b".into(),
            args_digest: "sha256:x".into(),
            created_at: Utc::now(),
            status: ApprovalStatus::Pending,
            session_id: "ses".into(),
            requester: "agent".into(),
            grants: vec![ApprovalGrant {
                principal: "alice".into(),
                granted_at: Utc::now(),
                authority: ApprovalAuthority::LocalAdvisory,
                envelope_id: None,
            }],
            expires_at: None,
            granted_at: None,
        };
        assert!(rec.has_grant_from("alice"));
        assert!(!rec.has_grant_from("bob"));
        assert_eq!(rec.grants_remaining(2), 1);
        assert_eq!(rec.grants_remaining(1), 0);
    }

    #[test]
    fn agent_approval_hint_dual_control() {
        let id = "appr_aabbccddeeff001122334455";
        let zero = agent_approval_hint(id, true, 2, 0);
        assert!(zero.contains("locus approve grant"));
        assert!(zero.contains(id));
        assert!(zero.contains("--as"));
        assert!(zero.contains("records local advisory evidence only"));
        assert!(zero.contains("requires 2 externally authenticated"));
        assert!(zero.contains(EXTERNAL_APPROVAL_AUTHORITY_BLOCKER));

        let partial = agent_approval_hint(id, true, 2, 1);
        assert!(partial.contains("1 advisory labels observed"));
        assert!(partial.contains("dual-control"));
        assert!(partial.contains("provider execution remains blocked"));
    }

    #[test]
    fn agent_approval_hint_single() {
        let id = "appr_aabbccddeeff001122334455";
        let h = agent_approval_hint(id, false, 1, 0);
        assert!(h.contains("locus approve grant"));
        assert!(h.contains(id));
        assert!(h.contains("single-approval requires 1 externally authenticated"));
        assert!(h.contains("records local advisory evidence only"));
    }

    #[test]
    fn format_dual_control_progress_partial_and_full() {
        let partial = format_dual_control_progress(1, 2, &["alice".into()], true, false);
        assert!(partial.contains("dual_control"));
        assert!(partial.contains("1/2"));
        assert!(partial.contains("alice"));
        assert!(partial.contains("1 more label"));
        assert!(partial.contains("external authority required"));

        let full = format_dual_control_progress(2, 2, &["alice".into(), "bob".into()], true, true);
        assert!(full.contains("2/2"));
        assert!(full.contains("externally authenticated"));
        assert!(full.contains("bob"));

        let single = format_dual_control_progress(1, 1, &["mason".into()], false, true);
        assert!(single.contains("grants 1/1"));
        assert!(!single.contains("dual_control"));
        assert!(single.contains("externally authenticated"));
    }

    #[test]
    fn next_grant_command_and_notification_body() {
        let id = "appr_aabbccddeeff001122334455";
        let next = next_grant_command(id, false);
        assert!(next.contains(&format!("locus approve grant {id}")));
        assert!(next.contains("another local advisory only"));
        assert!(next.contains("external authority remains required"));
        let done = next_grant_command(id, true);
        assert!(done.contains("peer-authenticated broker"));
        assert!(done.contains(id));

        let rec = ApprovalRecord {
            id: id.into(),
            tool: "vercel.deploy.prod".into(),
            binding: "acme".into(),
            args_digest: "sha256:x".into(),
            created_at: Utc::now(),
            status: ApprovalStatus::Pending,
            session_id: "ses".into(),
            requester: "agent".into(),
            grants: vec![],
            expires_at: None,
            granted_at: None,
        };
        let body = notification_body(&rec);
        assert!(body.contains("vercel.deploy.prod"));
        assert!(body.contains("acme"));
        assert!(
            body.contains(id) && body.contains("external broker authority still required"),
            "notify body must be explicit that local review is non-authoritative: {body}"
        );
        assert!(!body.contains("locus approve list"));
    }

    #[test]
    fn partial_grant_notification_body_stays_advisory() {
        let id = "appr_aabbccddeeff001122334455";
        let rec = ApprovalRecord {
            id: id.into(),
            tool: "supabase.table.delete".into(),
            binding: "acme".into(),
            args_digest: "sha256:x".into(),
            created_at: Utc::now(),
            status: ApprovalStatus::Pending,
            session_id: "ses".into(),
            requester: "agent".into(),
            grants: vec![ApprovalGrant {
                principal: "alice".into(),
                granted_at: Utc::now(),
                authority: ApprovalAuthority::LocalAdvisory,
                envelope_id: None,
            }],
            expires_at: None,
            granted_at: None,
        };
        let body = partial_grant_notification_body(&rec);
        assert!(
            body.contains("peer-authenticated broker authority still required"),
            "partial body must not claim local authority: {body}"
        );
        assert!(
            body.contains("1/2"),
            "partial body must show grants 1/2: {body}"
        );
        assert!(
            body.contains("alice"),
            "partial body must name granter: {body}"
        );
        assert!(
            body.contains("supabase.table.delete") && body.contains("acme"),
            "partial body must name tool/binding: {body}"
        );
        assert!(body.contains(id), "partial body must name approval: {body}");
    }

    #[test]
    fn partial_notification_is_only_eligible_for_first_advisory_label() {
        let mut rec = ApprovalRecord {
            id: "appr_aabbccddeeff001122334455".into(),
            tool: "supabase.table.delete".into(),
            binding: "acme".into(),
            args_digest: "sha256:x".into(),
            created_at: Utc::now(),
            status: ApprovalStatus::Pending,
            session_id: "ses".into(),
            requester: "agent".into(),
            grants: vec![],
            expires_at: None,
            granted_at: None,
        };
        assert!(!is_first_pending_advisory(&rec));

        rec.grants.push(ApprovalGrant {
            principal: "alice".into(),
            granted_at: Utc::now(),
            authority: ApprovalAuthority::LocalAdvisory,
            envelope_id: None,
        });
        assert!(is_first_pending_advisory(&rec));

        rec.grants.push(ApprovalGrant {
            principal: "bob".into(),
            granted_at: Utc::now(),
            authority: ApprovalAuthority::LocalAdvisory,
            envelope_id: None,
        });
        assert!(!is_first_pending_advisory(&rec));
    }

    #[test]
    fn try_notify_partial_grant_silent_when_notify_off() {
        // Default / kill-switch path must never panic and must not require a display.
        let prev_notify = std::env::var_os("LOCUS_NOTIFY");
        let prev_quiet = std::env::var_os("LOCUS_QUIET");
        // Force OFF even if CI runner or shell has LOCUS_NOTIFY=1
        std::env::set_var("LOCUS_NOTIFY", "0");
        std::env::remove_var("LOCUS_QUIET");

        assert!(
            !notifications_enabled(),
            "LOCUS_NOTIFY=0 must disable notifications"
        );

        let rec = ApprovalRecord {
            id: "appr_aabbccddeeff001122334455".into(),
            tool: "supabase.table.delete".into(),
            binding: "acme".into(),
            args_digest: "sha256:x".into(),
            created_at: Utc::now(),
            status: ApprovalStatus::Pending,
            session_id: "ses".into(),
            requester: "agent".into(),
            grants: vec![ApprovalGrant {
                principal: "alice".into(),
                granted_at: Utc::now(),
                authority: ApprovalAuthority::LocalAdvisory,
                envelope_id: None,
            }],
            expires_at: None,
            granted_at: None,
        };
        // Must be a pure no-op (no osascript) when notify is off.
        try_notify_partial_grant(&rec);
        try_notify_pending_approval(&rec);

        match prev_notify {
            Some(v) => std::env::set_var("LOCUS_NOTIFY", v),
            None => std::env::remove_var("LOCUS_NOTIFY"),
        }
        match prev_quiet {
            Some(v) => std::env::set_var("LOCUS_QUIET", v),
            None => std::env::remove_var("LOCUS_QUIET"),
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn notification_child_cannot_stall_mutation_caller() {
        use std::path::Path;
        use std::time::{Duration, Instant};

        let started = Instant::now();
        spawn_and_reap_notification(Path::new("/bin/sh"), &["-c".into(), "sleep 1".into()]);
        assert!(
            started.elapsed() < Duration::from_millis(250),
            "notification spawn blocked the mutation caller"
        );
    }

    #[test]
    fn format_grants_progress_basic() {
        assert_eq!(format_grants_progress(0, 2), "0/2");
        assert_eq!(format_grants_progress(1, 2), "1/2");
        assert_eq!(format_grants_progress(2, 2), "2/2");
    }
}

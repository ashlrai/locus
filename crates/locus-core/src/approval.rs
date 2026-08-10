//! Human approval grants for `policy.require_approval` tools.
//!
//! Records live under `$LOCUS_HOME/approvals/{id}.json`. Agents blocked by
//! policy receive a stable `approval_id`; a human runs `locus approve grant`
//! and the next matching tools/call is allowed for the grant TTL.
//!
//! Dual-control tools (`policy.dual_control` / `dual_control_all_approvals`)
//! require two distinct principals before `status` becomes `approved`.

use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Default grant lifetime after a fully-approved grant (15 minutes).
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

/// One principal's signature on an approval request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalGrant {
    pub principal: String,
    pub granted_at: DateTime<Utc>,
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
    /// Accumulated principal grants. Dual-control needs two distinct principals.
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

    /// Distinct principals that have already granted.
    pub fn grant_principals(&self) -> Vec<&str> {
        self.grants.iter().map(|g| g.principal.as_str()).collect()
    }

    /// Whether `principal` has already granted this approval.
    pub fn has_grant_from(&self, principal: &str) -> bool {
        self.grants.iter().any(|g| g.principal == principal)
    }

    /// How many distinct principal grants are still needed.
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
/// Dual-control always names the grant command and required principal count so
/// agents surface a clear human action instead of looping the tool call.
pub fn agent_approval_hint(
    approval_id: &str,
    dual_control: bool,
    required: usize,
    grants: usize,
) -> String {
    if dual_control {
        if grants == 0 {
            format!(
                "ask human: locus approve grant {approval_id} --as <principal> (needs {required} grants)"
            )
        } else {
            format!(
                "ask human: locus approve grant {approval_id} --as <other-principal> \
                 (needs {required} grants; have {grants} — dual-control)"
            )
        }
    } else {
        format!(
            "ask human: locus approve grant {approval_id} --as <principal> then re-call \
             (same args, or confirm=true + approval_id={approval_id})"
        )
    }
}

/// CLI / status line for dual-control progress after a grant.
///
/// Examples:
/// - fully approved dual: `"dual_control  grants 2/2  (alice, bob)  fully approved"`
/// - partial: `"dual_control  grants 1/2  (alice)  need 1 more distinct principal"`
/// - single: `"grants 1/1  (alice)  fully approved"`
pub fn format_dual_control_progress(
    grants: usize,
    required: usize,
    principals: &[String],
    dual_control: bool,
    fully_approved: bool,
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
    if fully_approved {
        format!("{prefix}  fully approved")
    } else {
        let remaining = required.saturating_sub(grants);
        format!("{prefix}  need {remaining} more distinct principal(s)")
    }
}

/// Next human command after a partial (or full) grant.
pub fn next_grant_command(approval_id: &str, fully_approved: bool) -> String {
    if fully_approved {
        format!("Re-call the tool with the same args (or confirm=true + approval_id={approval_id})")
    } else {
        format!("locus approve grant {approval_id} --as <other-principal>")
    }
}

/// Desktop notification body for a **new** pending approval (opt-in notify).
pub fn notification_body(rec: &ApprovalRecord) -> String {
    format!(
        "{} on {} — locus approve grant {}",
        rec.tool, rec.binding, rec.id
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
    if !rate_limit_allow(&rec.tool, &rec.binding) {
        return;
    }
    #[cfg(target_os = "macos")]
    {
        notify_macos_pending(rec);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = rec;
    }
}

/// Simple process-local rate limit (plus optional file stamp under LOCUS_HOME).
fn rate_limit_allow(tool: &str, binding: &str) -> bool {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    static LAST: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);
    let key = format!("{binding}::{tool}");
    let mut guard = match LAST.lock() {
        Ok(g) => g,
        Err(_) => return false,
    };
    let map = guard.get_or_insert_with(HashMap::new);
    let now = Instant::now();
    if let Some(prev) = map.get(&key) {
        if now.duration_since(*prev) < Duration::from_secs(60) {
            return false;
        }
    }
    map.insert(key, now);
    true
}

#[cfg(target_os = "macos")]
fn notify_macos_pending(rec: &ApprovalRecord) {
    use std::path::Path;
    use std::process::{Command, Stdio};

    let osascript = Path::new("/usr/bin/osascript");
    if !osascript.is_file() {
        return;
    }

    let title = "Locus approval";
    let body = notification_body(rec);
    // No sound name — silent banner only when user opted in
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        escape_applescript(&body),
        escape_applescript(title)
    );
    let _ = Command::new(osascript)
        .arg("-e")
        .arg(script)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
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
                },
                ApprovalGrant {
                    principal: "bob".into(),
                    granted_at: Utc::now(),
                },
            ],
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
        assert!(
            zero.contains("ask human: locus approve grant"),
            "expected agent-facing ask human: {zero}"
        );
        assert!(zero.contains(id));
        assert!(zero.contains("--as"));
        assert!(
            zero.contains("needs 2 grants"),
            "expected needs 2 grants: {zero}"
        );

        let partial = agent_approval_hint(id, true, 2, 1);
        assert!(partial.contains("needs 2 grants"));
        assert!(partial.contains("have 1"));
        assert!(partial.contains("dual-control") || partial.contains("other-principal"));
    }

    #[test]
    fn agent_approval_hint_single() {
        let id = "appr_aabbccddeeff001122334455";
        let h = agent_approval_hint(id, false, 1, 0);
        assert!(h.contains("ask human: locus approve grant"));
        assert!(h.contains(id));
        assert!(!h.contains("needs 2 grants"));
    }

    #[test]
    fn format_dual_control_progress_partial_and_full() {
        let partial = format_dual_control_progress(1, 2, &["alice".into()], true, false);
        assert!(partial.contains("dual_control"));
        assert!(partial.contains("1/2"));
        assert!(partial.contains("alice"));
        assert!(partial.contains("need 1 more"));

        let full = format_dual_control_progress(2, 2, &["alice".into(), "bob".into()], true, true);
        assert!(full.contains("2/2"));
        assert!(full.contains("fully approved"));
        assert!(full.contains("bob"));

        let single = format_dual_control_progress(1, 1, &["mason".into()], false, true);
        assert!(single.contains("grants 1/1"));
        assert!(!single.contains("dual_control"));
        assert!(single.contains("fully approved"));
    }

    #[test]
    fn next_grant_command_and_notification_body() {
        let id = "appr_aabbccddeeff001122334455";
        let next = next_grant_command(id, false);
        assert_eq!(
            next,
            format!("locus approve grant {id} --as <other-principal>")
        );
        let done = next_grant_command(id, true);
        assert!(done.contains("Re-call"));
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
            body.contains(&format!("locus approve grant {id}")),
            "notify body must name grant command: {body}"
        );
        assert!(!body.contains("locus approve list"));
    }

    #[test]
    fn format_grants_progress_basic() {
        assert_eq!(format_grants_progress(0, 2), "0/2");
        assert_eq!(format_grants_progress(1, 2), "1/2");
        assert_eq!(format_grants_progress(2, 2), "2/2");
    }
}

use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum LocusError {
    #[error("{0}")]
    Message(String),

    #[error("binding not found: {0}")]
    BindingNotFound(String),

    #[error("no active pin — run `locus pin <alias>` first")]
    NotPinned,

    #[error("invalid session seal")]
    InvalidSeal,

    #[error("legacy session seal is incomplete; human re-pin required")]
    LegacySessionSeal,

    #[error("binding authority mismatch: {0}; human re-pin required")]
    BindingAuthorityMismatch(String),

    #[error("session authority anchor unavailable: {0}; human re-pin required")]
    AuthorityAnchorUnavailable(String),

    #[error("session authority generation is stale or unknown; human re-pin required")]
    AuthorityAnchorMismatch,

    #[error("executor authority unavailable: {0}; use an explicit supervised `locus exec`, `locus run`, or `locus ci run` boundary")]
    ExecutorAuthorityUnavailable(String),

    #[error(
        "session expired at {0}; clear it with `locus leave --force`, then `locus enter <alias>`"
    )]
    SessionExpired(String),

    /// Active pin was frozen after binding drift — human must re-pin.
    #[error("session_frozen: re-pin ({0})")]
    SessionFrozen(String),

    /// The workspace's `.locus.toml` `allowed_bindings` excludes this binding.
    #[error(
        "binding `{alias}` is not allowed in this workspace ({policy} allows: {allowed}); \
         enter an allowed binding, add `{alias}` to allowed_bindings in {policy}, or pass --force",
        policy = .policy.display(),
        allowed = allowed_list(.allowed)
    )]
    BindingNotAllowed {
        alias: String,
        /// Path of the `.locus.toml` that refused it.
        policy: PathBuf,
        allowed: Vec<String>,
    },

    #[error("workspace requires an active pin (require_pin = true)")]
    PinRequired,

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("toml: {0}")]
    TomlDe(#[from] toml::de::Error),

    #[error("toml serialize: {0}")]
    TomlSer(#[from] toml::ser::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, LocusError>;

impl LocusError {
    pub fn msg(s: impl Into<String>) -> Self {
        Self::Message(s.into())
    }

    /// Allowlist refusal naming the policy file and what it does allow.
    pub fn binding_not_allowed(alias: &str, policy: &Path, allowed: &[String]) -> Self {
        Self::BindingNotAllowed {
            alias: alias.to_string(),
            policy: policy.to_path_buf(),
            allowed: allowed.to_vec(),
        }
    }
}

fn allowed_list(allowed: &[String]) -> String {
    if allowed.is_empty() {
        "none".into()
    } else {
        allowed.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_not_allowed_lists_policy_aliases_and_overrides() {
        let err = LocusError::binding_not_allowed(
            "personal",
            Path::new("/w/.locus.toml"),
            &["acme".into(), "acme-ro".into()],
        );
        assert_eq!(
            err.to_string(),
            "binding `personal` is not allowed in this workspace (/w/.locus.toml allows: acme, acme-ro); \
             enter an allowed binding, add `personal` to allowed_bindings in /w/.locus.toml, or pass --force"
        );
    }

    #[test]
    fn binding_not_allowed_with_empty_list_says_none() {
        let err = LocusError::binding_not_allowed("x", Path::new("/w/.locus.toml"), &[]);
        assert!(err.to_string().contains("allows: none"), "{err}");
    }

    #[test]
    fn session_expired_names_leave_force_then_enter() {
        let err = LocusError::SessionExpired("2026-10-16T00:00:00+00:00".into());
        assert_eq!(
            err.to_string(),
            "session expired at 2026-10-16T00:00:00+00:00; clear it with `locus leave --force`, then `locus enter <alias>`"
        );
    }
}

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

    #[error("session expired at {0}")]
    SessionExpired(String),

    /// Active pin was frozen after binding drift — human must re-pin.
    #[error("session_frozen: re-pin ({0})")]
    SessionFrozen(String),

    #[error("binding `{0}` is not allowed in this workspace")]
    BindingNotAllowed(String),

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
}

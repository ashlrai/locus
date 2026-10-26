//! Optional `$LOCUS_HOME/config.toml` — defaults, autopin remotes, credential preference.
//!
//! Absent or unreadable config is treated as empty (doctor reports "not present").
//! Never stores secrets.
//!
//! ```toml
//! [clients]
//! auto_pin = "cwd"   # cwd | none | last — shell/hook preference
//!
//! [autopin]
//! enabled = false
//!
//! [[autopin.remotes]]
//! match = "github.com/acme-corp"
//! binding = "acme"
//! ```

use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Top-level Locus home config (`config.toml`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocusConfig {
    #[serde(default)]
    pub daemon: DaemonConfig,
    #[serde(default)]
    pub clients: ClientsConfig,
    #[serde(default)]
    pub credential: CredentialConfig,
    #[serde(default)]
    pub audit: AuditConfig,
    /// Opt-in git-remote → binding auto-pin rules.
    #[serde(default)]
    pub autopin: AutopinConfig,
    /// Desktop notifications — **off by default** (agent spam is worse than silence).
    #[serde(default)]
    pub notify: NotifyConfig,
    /// Session budget caps, **off by default** (no cap unless configured).
    #[serde(default)]
    pub budget: BudgetConfig,
    /// Signed, value-free log of credential injections, **off by default**.
    #[serde(default)]
    pub credential_audit: CredentialAuditConfig,
    /// Gateway base URL passed to the isolated child, **off by default**.
    #[serde(default)]
    pub gateway: GatewayConfig,
    /// Exact parent variable names the isolated child may inherit, **none by default**.
    #[serde(default)]
    pub exec: ExecConfig,
    /// Local pin history for autopin suggestions, **on by default** (local only, hashed remotes).
    #[serde(default)]
    pub history: HistoryConfig,
}

/// `[history]`: local pin history (`pin-history.jsonl`). Override with `LOCUS_PIN_HISTORY=0|1`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Oldest entries are dropped first past this many.
    #[serde(default = "default_history_max")]
    pub max_entries: usize,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_entries: crate::pin_history::DEFAULT_MAX_ENTRIES,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_history_max() -> usize {
    crate::pin_history::DEFAULT_MAX_ENTRIES
}

/// `[exec]`: options for the isolated `locus exec` child.
///
/// ```toml
/// [exec]
/// # Exact names only, no globs. Key-shaped names cross only as phm_ placeholders.
/// inherit_env = ["ANTHROPIC_API_KEY", "ANTHROPIC_BASE_URL", "PHANTOM_PROXY_PORT"]
/// ```
///
/// Empty or absent (the default) inherits nothing. Rules live in
/// `inherit_env.rs`; see `docs/PHANTOM-USAGE.md`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inherit_env: Vec<String>,
}

/// `[gateway]`: opt-in base URL for the isolated `locus exec` child. Off and
/// absent by default, which keeps the child environment exactly as before.
/// See `docs/gateway-base-url.md`; validation lives in `gateway.rs`.
///
/// ```toml
/// [gateway]
/// enabled = true
/// base_url = "http://127.0.0.1:4000/v1"
/// child_env = ["OPENAI_BASE_URL"]
/// # Required for any host that is not 127.0.0.1 or [::1]:
/// # allowed_hosts = ["gw.example.com:443"]
/// ```
///
/// Only the URL is passed. No key, token or header is ever forwarded.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GatewayConfig {
    /// Must be `true` to opt in.
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Child variable names that receive the URL (`*_BASE_URL` or `*_API_BASE`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub child_env: Vec<String>,
    /// Exact `host:port` entries; required for a non-loopback host.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_hosts: Vec<String>,
}

/// `[credential_audit]`: opt-in, signed, value-free record of every credential
/// placed into a child environment. See `docs/credential-audit.md`.
///
/// ```toml
/// [credential_audit]
/// enabled = true
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CredentialAuditConfig {
    #[serde(default)]
    pub enabled: bool,
}

/// `[budget]`: opt-in caps that stop an agent session after N tool calls or
/// N minutes. Absent or empty means no cap and no behavior change.
///
/// ```toml
/// [budget]
/// max_actions = 200   # tool calls per session
/// max_minutes = 90    # minutes from the session's first counted call
/// ```
///
/// A cap of `0` is treated as unset (a zero budget would lock every session
/// out silently; remove the key or set a positive number).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BudgetConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_actions: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_minutes: Option<u64>,
}

impl BudgetConfig {
    /// Effective action cap (`None` when unset or zero).
    pub fn actions_cap(&self) -> Option<u64> {
        self.max_actions.filter(|n| *n > 0)
    }

    /// Effective minutes cap (`None` when unset or zero).
    pub fn minutes_cap(&self) -> Option<u64> {
        self.max_minutes.filter(|n| *n > 0)
    }

    /// True when at least one cap is configured.
    pub fn is_enabled(&self) -> bool {
        self.actions_cap().is_some() || self.minutes_cap().is_some()
    }
}

/// `[notify]` — opt-in desktop banners for pending approvals.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotifyConfig {
    /// When true, macOS may show a silent Notification Center banner on new
    /// pending approvals. Override with `LOCUS_NOTIFY=1` / `LOCUS_NOTIFY=0`.
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_start: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_ttl: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_can_pin: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_timeout: Option<String>,
}

/// Client / shell pin behaviour.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClientsConfig {
    /// `cwd` | `none` | `last` — optional; absent means unset.
    /// Used by shell hooks / `LOCUS_AUTO_ENTER` guidance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_pin: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CredentialConfig {
    /// `phantom` | `keychain` — preference only; resolution still uses CredentialRefs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// `[autopin]` — git remote substring → binding alias (opt-in).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AutopinConfig {
    /// When false (default), only workspace `default_binding` is used for bare pin.
    #[serde(default)]
    pub enabled: bool,
    /// Git remote host/path substring → binding alias.
    #[serde(default)]
    pub remotes: Vec<AutopinRemote>,
}

/// One remote match rule.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AutopinRemote {
    /// Substring matched against normalized git remote URLs.
    #[serde(rename = "match")]
    pub match_pattern: String,
    /// Binding alias to pin when the pattern hits.
    pub binding: String,
}

impl LocusConfig {
    pub fn parse(s: &str) -> Result<Self> {
        Ok(toml::from_str(s)?)
    }

    pub fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Load from a path. Missing file → `Ok(None)`. Parse errors propagate.
    pub fn load(path: &Path) -> Result<Option<Self>> {
        if !path.is_file() {
            return Ok(None);
        }
        let raw = fs::read_to_string(path)?;
        Ok(Some(Self::parse(&raw)?))
    }

    /// Example config with disabled autopin sample rule.
    pub fn example() -> Self {
        Self {
            autopin: AutopinConfig {
                enabled: false,
                remotes: vec![AutopinRemote {
                    match_pattern: "github.com/acme-corp".into(),
                    binding: "acme".into(),
                }],
            },
            ..Default::default()
        }
    }
}

/// Load `$LOCUS_HOME/config.toml`, or defaults if missing/corrupt.
pub fn load_config(home: &Path) -> LocusConfig {
    let path = home.join("config.toml");
    match fs::read_to_string(&path) {
        Ok(raw) => LocusConfig::parse(&raw).unwrap_or_default(),
        Err(_) => LocusConfig::default(),
    }
}

/// Write config (creates parent dirs as needed).
pub(crate) fn save_config(home: &Path, cfg: &LocusConfig) -> Result<PathBuf> {
    let path = home.join("config.toml");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, cfg.to_toml()?)?;
    Ok(path)
}

/// Autopin / auto_pin slice for doctor / status surfaces.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AutopinStatus {
    /// Absolute path to `config.toml`.
    pub path: String,
    /// Whether the file exists.
    pub present: bool,
    /// `clients.auto_pin` value when set (`cwd` | `none` | `last` or free-form).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_pin: Option<String>,
    /// Whether remote-based `[autopin]` is enabled.
    pub remote_autopin_enabled: bool,
    /// Number of `[[autopin.remotes]]` rules.
    pub remote_rules: usize,
    /// True when auto_pin is a known value or unset (unset is ok).
    pub ok: bool,
    /// Human note when misconfigured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl AutopinStatus {
    pub fn from_config(path: &Path, cfg: Option<&LocusConfig>) -> Self {
        let present = path.is_file();
        let auto_pin = cfg.and_then(|c| c.clients.auto_pin.clone());
        let remote_autopin_enabled = cfg.map(|c| c.autopin.enabled).unwrap_or(false);
        let remote_rules = cfg.map(|c| c.autopin.remotes.len()).unwrap_or(0);

        let mut ok = true;
        let mut notes: Vec<String> = Vec::new();

        match auto_pin.as_deref() {
            None => {
                if !present {
                    notes.push("config.toml not present (defaults: manual pin)".into());
                } else if remote_autopin_enabled {
                    notes.push(format!(
                        "clients.auto_pin unset; [autopin] enabled with {remote_rules} remote rule(s)"
                    ));
                } else {
                    notes.push("auto_pin unset (manual pin only)".into());
                }
            }
            Some("cwd" | "none" | "last") => {}
            Some(v) => {
                ok = false;
                notes.push(format!("unknown auto_pin '{v}' (expected cwd|none|last)"));
            }
        }

        if remote_autopin_enabled && remote_rules == 0 {
            ok = false;
            notes.push("autopin.enabled=true but no [[autopin.remotes]] rules".into());
        }

        Self {
            path: path.display().to_string(),
            present,
            auto_pin,
            remote_autopin_enabled,
            remote_rules,
            ok,
            note: if notes.is_empty() {
                None
            } else {
                Some(notes.join("; "))
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn parses_autopin() {
        let raw = r#"
[clients]
auto_pin = "cwd"

[daemon]
default_ttl = "8h"

[autopin]
enabled = true

[[autopin.remotes]]
match = "github.com/acme-corp"
binding = "acme"
"#;
        let cfg = LocusConfig::parse(raw).unwrap();
        assert_eq!(cfg.clients.auto_pin.as_deref(), Some("cwd"));
        assert_eq!(cfg.daemon.default_ttl.as_deref(), Some("8h"));
        assert!(cfg.autopin.enabled);
        assert_eq!(cfg.autopin.remotes[0].binding, "acme");
    }

    #[test]
    fn load_missing_is_none() {
        let dir = tempdir().unwrap();
        assert!(LocusConfig::load(&dir.path().join("config.toml"))
            .unwrap()
            .is_none());
    }

    #[test]
    fn load_present() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut f = fs::File::create(&path).unwrap();
        writeln!(f, "[clients]\nauto_pin = \"last\"").unwrap();
        let cfg = LocusConfig::load(&path).unwrap().unwrap();
        assert_eq!(cfg.clients.auto_pin.as_deref(), Some("last"));
        let st = AutopinStatus::from_config(&path, Some(&cfg));
        assert!(st.present);
        assert!(st.ok);
        assert_eq!(st.auto_pin.as_deref(), Some("last"));
    }

    #[test]
    fn unknown_autopin_not_ok() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let cfg = LocusConfig {
            clients: ClientsConfig {
                auto_pin: Some("magic".into()),
            },
            ..Default::default()
        };
        let st = AutopinStatus::from_config(&path, Some(&cfg));
        assert!(!st.ok);
    }

    #[test]
    fn example_roundtrip() {
        let cfg = LocusConfig::example();
        let s = cfg.to_toml().unwrap();
        let parsed = LocusConfig::parse(&s).unwrap();
        assert_eq!(parsed, cfg);
    }
}

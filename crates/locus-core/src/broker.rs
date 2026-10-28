//! Operator-managed engine allowlists. These constrain, never grant, tenant authority.
use std::{collections::BTreeMap, path::Path};

use crate::error::{LocusError, Result};
use serde::{Deserialize, Serialize};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerPolicy {
    pub engines: BTreeMap<String, EnginePolicy>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnginePolicy {
    #[serde(default)]
    pub cli: Vec<String>,
    #[serde(default)]
    pub mcp: Vec<String>,
}

#[derive(Serialize)]
pub struct Decision {
    pub allowed: bool,
    pub reason: &'static str,
}

impl BrokerPolicy {
    /// Tool selectors are `cli:<executable>` or `mcp:<server>`. No globs,
    /// basename matching, arguments, or implicit server-name aliases.
    pub fn check(&self, engine: &str, tool: &str) -> Decision {
        let Some(policy) = self.engines.get(engine) else {
            return Decision {
                allowed: false,
                reason: "unknown_engine",
            };
        };
        let allowed = match tool.split_once(':') {
            Some(("cli", command)) if !command.is_empty() => {
                policy.cli.iter().any(|v| v == command)
            }
            Some(("mcp", server)) if !server.is_empty() => policy.mcp.iter().any(|v| v == server),
            _ => false,
        };
        Decision {
            allowed,
            reason: if allowed {
                "allowed"
            } else {
                "not_allowlisted"
            },
        }
    }
}

pub fn load(home: &Path) -> Result<BrokerPolicy> {
    // Do not return parser diagnostics: malformed configuration may contain secrets.
    let text = std::fs::read_to_string(home.join("broker.toml"))?;
    toml::from_str(&text).map_err(|_| LocusError::msg("invalid broker policy"))
}

/// A policy file enables mandatory engine attribution. An explicit engine also
/// enables enforcement even if the policy has disappeared. Errors fail closed.
/// Audit only fixed labels: neither caller identifiers nor tool arguments.
pub fn enforce(store: &crate::Store, tool: &str) -> Result<()> {
    let engine = std::env::var_os("LOCUS_ENGINE_ID");
    enforce_engine(
        store,
        engine.as_ref().map(|id| id.to_str().unwrap_or("")),
        tool,
    )
}

fn enforce_engine(store: &crate::Store, engine: Option<&str>, tool: &str) -> Result<()> {
    // A dangling symlink or inaccessible policy is configured, not absent.
    let configured = match std::fs::symlink_metadata(store.home().join("broker.toml")) {
        Ok(_) => true,
        Err(e) => e.kind() != std::io::ErrorKind::NotFound,
    };
    if engine.is_none() && !configured {
        return Ok(());
    }
    let decision = engine.and_then(|id| load(store.home()).ok().map(|p| p.check(id, tool)));
    if decision.is_some_and(|d| d.allowed) {
        return Ok(());
    }
    store.audit(
        "broker.deny",
        "",
        Some(serde_json::json!({"reason": "engine_not_authorized"})),
    )?;
    Err(LocusError::msg(
        "broker denied access: engine not authorized",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn allow_deny_unknown_and_exact_selectors() {
        let p: BrokerPolicy =
            toml::from_str("[engines.worker]\ncli = ['gh']\nmcp = ['github']").unwrap();
        assert!(p.check("worker", "cli:gh").allowed);
        assert!(p.check("worker", "mcp:github").allowed);
        for tool in ["cli:/tmp/gh", "cli:gh auth token", "mcp:aws", "gh", "cli:"] {
            assert!(!p.check("worker", tool).allowed);
        }
        assert_eq!(p.check("unknown", "cli:gh").reason, "unknown_engine");
        assert!(!p.check("", "cli:gh").allowed);
    }
    #[test]
    fn enforcement_and_redacted_audit() {
        let home = tempfile::tempdir().unwrap();
        let store = crate::Store::open(home.path()).unwrap();
        std::fs::write(
            home.path().join("broker.toml"),
            "[engines.worker]\ncli = ['gh']\nmcp = ['github']",
        )
        .unwrap();
        assert!(enforce_engine(&store, Some("worker"), "cli:gh").is_ok());
        assert!(enforce_engine(&store, Some("worker"), "mcp:github").is_ok());
        for engine in [Some("worker"), Some("unknown-secret"), None] {
            assert!(enforce_engine(&store, engine, "cli:argument-secret").is_err());
        }
        let audit = std::fs::read_to_string(store.audit_path()).unwrap();
        assert_eq!(audit.matches("broker.deny").count(), 3);
        assert!(!audit.contains("unknown-secret"));
        assert!(!audit.contains("argument-secret"));
        std::fs::remove_file(home.path().join("broker.toml")).unwrap();
        assert!(enforce_engine(&store, Some("worker"), "cli:gh").is_err());
        assert!(enforce_engine(&store, None, "cli:gh").is_ok());
    }
    #[test]
    fn malformed_policy_fails_closed() {
        let home = tempfile::tempdir().unwrap();
        assert!(load(home.path()).is_err());
        std::fs::write(
            home.path().join("broker.toml"),
            "secret = 'never echo this'",
        )
        .unwrap();
        assert_eq!(
            load(home.path()).err().unwrap().to_string(),
            "invalid broker policy"
        );
    }
}

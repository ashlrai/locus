//! Directory-local `.locus.toml` — default pin + allowlist.

use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct WorkspaceConfig {
    #[serde(default = "default_version")]
    pub version: u32,
    /// Alias or id of the default binding for this directory tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_binding: Option<String>,
    /// If set, only these aliases/ids may be pinned here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_bindings: Vec<String>,
    /// Refuse agent/CLI work without an active pin.
    #[serde(default)]
    pub require_pin: bool,
}

fn default_version() -> u32 {
    1
}

impl WorkspaceConfig {
    pub fn allows(&self, alias_or_id: &str) -> bool {
        if self.allowed_bindings.is_empty() {
            return true;
        }
        self.allowed_bindings.iter().any(|a| a == alias_or_id)
    }

    pub fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }

    pub fn parse(s: &str) -> Result<Self> {
        Ok(toml::from_str(s)?)
    }
}

/// Walk from `start` toward root looking for `.locus.toml`.
/// Child configs win (first found).
pub fn find_workspace(start: &Path) -> Option<(PathBuf, WorkspaceConfig)> {
    let mut cur = start.to_path_buf();
    if cur.is_file() {
        cur.pop();
    }
    loop {
        let candidate = cur.join(".locus.toml");
        if candidate.is_file() {
            if let Ok(raw) = fs::read_to_string(&candidate) {
                if let Ok(cfg) = WorkspaceConfig::parse(&raw) {
                    return Some((candidate, cfg));
                }
            }
        }
        if !cur.pop() {
            break;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn finds_parent_workspace() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let nested = root.join("apps").join("web");
        fs::create_dir_all(&nested).unwrap();
        let mut f = fs::File::create(root.join(".locus.toml")).unwrap();
        writeln!(
            f,
            r#"
version = 1
default_binding = "acme"
allowed_bindings = ["acme", "acme-ro"]
require_pin = true
"#
        )
        .unwrap();
        let (path, cfg) = find_workspace(&nested).unwrap();
        assert_eq!(path, root.join(".locus.toml"));
        assert_eq!(cfg.default_binding.as_deref(), Some("acme"));
        assert!(cfg.allows("acme"));
        assert!(!cfg.allows("personal"));
    }
}

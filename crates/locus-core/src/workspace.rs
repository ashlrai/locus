//! Directory-local `.locus.toml` — default pin + allowlist.

use crate::error::{LocusError, Result};
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
pub fn find_workspace(start: &Path) -> Result<Option<(PathBuf, WorkspaceConfig)>> {
    let mut cur = start.to_path_buf();
    if cur.is_file() {
        cur.pop();
    }
    loop {
        let candidate = cur.join(".locus.toml");
        match fs::symlink_metadata(&candidate) {
            Ok(link_metadata) => {
                let metadata = if link_metadata.file_type().is_symlink() {
                    fs::metadata(&candidate).map_err(|_| {
                        LocusError::msg(format!(
                            "workspace policy link is broken or unreadable at {}",
                            candidate.display()
                        ))
                    })?
                } else {
                    link_metadata
                };
                if !metadata.is_file() {
                    return Err(LocusError::msg(format!(
                        "workspace policy is not a regular file at {}",
                        candidate.display()
                    )));
                }
                let raw = fs::read_to_string(&candidate).map_err(|e| {
                    LocusError::msg(format!(
                        "workspace policy unreadable at {}: {e}",
                        candidate.display()
                    ))
                })?;
                let cfg = WorkspaceConfig::parse(&raw).map_err(|e| {
                    LocusError::msg(format!(
                        "workspace policy malformed at {}: {e}",
                        candidate.display()
                    ))
                })?;
                return Ok(Some((candidate, cfg)));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(LocusError::msg(format!(
                    "workspace policy discovery failed at {}: {e}",
                    candidate.display()
                )));
            }
        }
        if !cur.pop() {
            break;
        }
    }
    Ok(None)
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
        let (path, cfg) = find_workspace(&nested).unwrap().unwrap();
        assert_eq!(path, root.join(".locus.toml"));
        assert_eq!(cfg.default_binding.as_deref(), Some("acme"));
        assert!(cfg.allows("acme"));
        assert!(!cfg.allows("personal"));
    }

    #[test]
    fn malformed_child_workspace_stops_walk_and_fails_closed() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let nested = root.join("apps").join("web");
        fs::create_dir_all(&nested).unwrap();
        fs::write(
            root.join(".locus.toml"),
            "default_binding = \"personal\"\nallowed_bindings = [\"personal\"]\n",
        )
        .unwrap();
        fs::write(
            root.join("apps").join(".locus.toml"),
            "default_binding = [unterminated",
        )
        .unwrap();

        let err = find_workspace(&nested).unwrap_err().to_string();
        assert!(err.contains("workspace policy malformed"));
        assert!(err.contains("apps/.locus.toml"));
        assert!(!err.contains("personal"));
    }

    #[test]
    fn non_file_workspace_stops_walk_and_fails_closed() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("repo");
        fs::create_dir_all(nested.join(".locus.toml")).unwrap();

        let err = find_workspace(&nested).unwrap_err().to_string();
        assert!(err.contains("not a regular file"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn broken_workspace_symlink_stops_walk_and_fails_closed() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let nested = dir.path().join("repo");
        fs::create_dir_all(&nested).unwrap();
        symlink("missing-policy.toml", nested.join(".locus.toml")).unwrap();

        let err = find_workspace(&nested).unwrap_err().to_string();
        assert!(err.contains("broken or unreadable"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_workspace_stops_walk_and_fails_closed_when_enforced() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let nested = dir.path().join("repo");
        fs::create_dir_all(&nested).unwrap();
        let policy = nested.join(".locus.toml");
        fs::write(&policy, "version = 1\n").unwrap();
        fs::set_permissions(&policy, fs::Permissions::from_mode(0o000)).unwrap();

        let result = find_workspace(&nested);
        fs::set_permissions(&policy, fs::Permissions::from_mode(0o600)).unwrap();
        if let Err(error) = result {
            assert!(error.to_string().contains("unreadable"));
        }
    }
}

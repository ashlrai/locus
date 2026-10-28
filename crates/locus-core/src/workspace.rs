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
    /// Per-child resident memory cap in bytes (absent = 1 GiB). Zero is invalid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_rss_cap_bytes: Option<std::num::NonZeroU64>,
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

/// True when anything occupies `path`, a broken symlink included.
///
/// `Path::exists` follows a final symlink, so a dangling `.locus.toml` link
/// reads as absent and a later `fs::write` would create the link's target
/// outside the project. Writers check with this instead.
pub fn path_entry_exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// A file written next to its destination and renamed into place by
/// [`StagedFile::commit`]. Dropped without a commit, the staged copy is removed.
#[derive(Debug)]
pub struct StagedFile {
    tmp: PathBuf,
    target: PathBuf,
    committed: bool,
}

impl StagedFile {
    /// Rename the staged copy onto the destination. A symlink at the
    /// destination is replaced itself; its target is never written.
    pub fn commit(mut self) -> Result<PathBuf> {
        fs::rename(&self.tmp, &self.target).map_err(|e| {
            LocusError::msg(format!("could not write {}: {e}", self.target.display()))
        })?;
        self.committed = true;
        Ok(self.target.clone())
    }
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.tmp);
        }
    }
}

/// Stage `contents` for `target` without following a symlink at `target`.
///
/// The caller decides whether an existing entry may be replaced (see
/// [`path_entry_exists`]); this refuses only entries a file must never replace
/// (a directory or other non-file). Staging proves the directory is writable,
/// so a caller that stages before its other writes fails before making them.
pub fn stage_file_replace(target: &Path, contents: &[u8]) -> Result<StagedFile> {
    use std::io::Write as _;
    match fs::symlink_metadata(target) {
        Ok(m) if m.is_file() || m.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(LocusError::msg(format!(
                "{} exists and is not a regular file; refusing to replace it",
                target.display()
            )));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(LocusError::msg(format!(
                "could not inspect {}: {e}",
                target.display()
            )));
        }
    }
    let dir = match target.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| LocusError::msg(format!("invalid file path {}", target.display())))?;
    let tmp = dir.join(format!(
        ".{name}.tmp-{}-{:016x}",
        std::process::id(),
        rand::random::<u64>()
    ));
    // create_new (O_EXCL) never follows a symlink planted at the temp name.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(|e| LocusError::msg(format!("could not write in {}: {e}", dir.display())))?;
    let staged = StagedFile {
        tmp,
        target: target.to_path_buf(),
        committed: false,
    };
    file.write_all(contents)?;
    file.sync_all()?;
    Ok(staged)
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

    #[cfg(unix)]
    #[test]
    fn staged_write_replaces_a_symlink_instead_of_writing_its_target() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let repo = dir.path().join("repo");
        let outside = dir.path().join("outside");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let target = repo.join(".locus.toml");
        symlink("../outside/planted.toml", &target).unwrap();
        assert!(
            !target.exists(),
            "dangling link reads as absent through exists()"
        );
        assert!(path_entry_exists(&target));

        let path = stage_file_replace(&target, b"version = 1\n")
            .unwrap()
            .commit()
            .unwrap();
        assert_eq!(path, target);
        assert!(!outside.join("planted.toml").exists());
        assert!(!fs::symlink_metadata(&target)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_to_string(&target).unwrap(), "version = 1\n");
        assert_eq!(fs::read_dir(&repo).unwrap().count(), 1, "no temp file left");
    }

    #[test]
    fn staged_write_refuses_a_directory_and_cleans_up_when_dropped() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".locus.toml")).unwrap();
        let err = stage_file_replace(&dir.path().join(".locus.toml"), b"x")
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a regular file"), "{err}");

        let other = dir.path().join("other.toml");
        drop(stage_file_replace(&other, b"x").unwrap());
        assert!(!other.exists());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn wrong_typed_fields_fail_closed() {
        // Valid TOML, wrong field types: the file is as malformed as broken
        // syntax — parse must err, never fall back to a permissive default
        // (an empty allowlist allows every binding).
        for bad in [
            "version = \"one\"\n",
            "default_binding = 42\n",
            "default_binding = [\"acme\"]\n",
            "allowed_bindings = \"acme\"\n",
            "allowed_bindings = [1, 2]\n",
            "require_pin = \"yes\"\n",
            "require_pin = 1\n",
        ] {
            assert!(WorkspaceConfig::parse(bad).is_err(), "must reject: {bad:?}");
        }
    }

    #[test]
    fn wrong_typed_workspace_stops_walk_and_fails_closed() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let nested = root.join("apps").join("web");
        fs::create_dir_all(&nested).unwrap();
        fs::write(root.join(".locus.toml"), "default_binding = \"personal\"\n").unwrap();
        fs::write(
            root.join("apps").join(".locus.toml"),
            "allowed_bindings = \"acme\"\n",
        )
        .unwrap();

        let err = find_workspace(&nested).unwrap_err().to_string();
        assert!(err.contains("workspace policy malformed"));
        assert!(err.contains("apps/.locus.toml"));
        assert!(!err.contains("personal"));
    }

    #[test]
    fn non_utf8_workspace_stops_walk_and_fails_closed() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("repo");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join(".locus.toml"), b"\xff\xfe\x00\x32").unwrap();

        let err = find_workspace(&nested).unwrap_err().to_string();
        assert!(err.contains("unreadable"), "{err}");
    }
}

#[cfg(test)]
mod rss_config_tests {
    use super::*;
    #[test]
    fn rss_cap_roundtrips_and_rejects_zero_or_invalid_values() {
        assert_eq!(
            WorkspaceConfig::parse("").unwrap().child_rss_cap_bytes,
            None
        );
        let config = WorkspaceConfig::parse("child_rss_cap_bytes = 16777216").unwrap();
        assert_eq!(config.child_rss_cap_bytes.unwrap().get(), 16777216);
        assert_eq!(
            WorkspaceConfig::parse(&config.to_toml().unwrap()).unwrap(),
            config
        );
        for invalid in ["0", "-1", "1.5", "\"unlimited\""] {
            assert!(WorkspaceConfig::parse(&format!("child_rss_cap_bytes = {invalid}")).is_err());
        }
    }
}

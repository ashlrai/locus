//! Local pin history (`$LOCUS_HOME/pin-history.jsonl`).
//!
//! Records `pin`, `leave` and `pin_mismatch` events so `locus autopin suggest`
//! can learn which binding a repo usually uses. Local only, never sent anywhere.
//! On by default; turn off with `[history] enabled = false` in config.toml or
//! `LOCUS_PIN_HISTORY=0`. Capped at `[history] max_entries` (oldest dropped first).
//!
//! Privacy: the git remote URL and workspace path are stored only as SHA-256
//! hashes (the remote after `normalize_remote_url`). Entries hold aliases only,
//! never tokens, credential references or the control capability. File mode 0600.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::autopin::{git_remote_urls, normalize_remote_url};
use crate::config::load_config;

pub const FILE_NAME: &str = "pin-history.jsonl";
pub const DEFAULT_MAX_ENTRIES: usize = 2000;
const ENV_SWITCH: &str = "LOCUS_PIN_HISTORY";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryEntry {
    pub ts: String,
    /// `pin` | `leave` | `pin_mismatch`
    pub event: String,
    pub alias: Option<String>,
    /// For `pin_mismatch`: the alias the caller expected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    pub workspace_hash: Option<String>,
    #[serde(default)]
    pub remote_hashes: Vec<String>,
}

pub fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

pub fn path(home: &Path) -> PathBuf {
    home.join(FILE_NAME)
}

/// Effective switch: env `LOCUS_PIN_HISTORY` (0/off/false or 1/on/true) wins over config.
pub fn enabled(home: &Path) -> bool {
    if let Ok(v) = std::env::var(ENV_SWITCH) {
        match v.trim().to_ascii_lowercase().as_str() {
            "0" | "off" | "false" | "no" => return false,
            "1" | "on" | "true" | "yes" => return true,
            _ => {}
        }
    }
    load_config(home).history.enabled
}

/// Build an entry for `cwd`: workspace root and remotes are hashed, never stored.
pub fn entry_for(
    event: &str,
    alias: Option<&str>,
    expected: Option<&str>,
    cwd: &Path,
) -> HistoryEntry {
    let ws_root = crate::workspace::find_workspace(cwd)
        .ok()
        .flatten()
        .map(|(p, _)| p)
        .unwrap_or_else(|| cwd.to_path_buf());
    let mut remote_hashes: Vec<String> = git_remote_urls(cwd)
        .iter()
        .map(|u| sha256_hex(&normalize_remote_url(u)))
        .collect();
    remote_hashes.sort();
    remote_hashes.dedup();
    HistoryEntry {
        ts: chrono::Utc::now().to_rfc3339(),
        event: event.to_string(),
        alias: alias.map(str::to_string),
        expected: expected.map(str::to_string),
        workspace_hash: Some(sha256_hex(&ws_root.display().to_string())),
        remote_hashes,
    }
}

/// Best-effort record: never fails the caller's pin, leave or refusal.
pub fn record(home: &Path, event: &str, alias: Option<&str>, expected: Option<&str>, cwd: &Path) {
    if !enabled(home) {
        return;
    }
    let max = load_config(home).history.max_entries;
    let _ = append(home, &entry_for(event, alias, expected, cwd), max);
}

/// Append one entry and enforce the cap, dropping the oldest lines first.
pub fn append(home: &Path, entry: &HistoryEntry, max_entries: usize) -> std::io::Result<()> {
    let p = path(home);
    let mut lines: Vec<String> = match fs::read_to_string(&p) {
        Ok(s) => s
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e),
    };
    lines.push(serde_json::to_string(entry).map_err(std::io::Error::other)?);
    let max = max_entries.max(1);
    if lines.len() > max {
        lines.drain(..lines.len() - max);
    }
    let tmp = home.join(format!(".{FILE_NAME}.tmp"));
    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        for l in &lines {
            writeln!(f, "{l}")?;
        }
        f.sync_all()?;
    }
    fs::rename(&tmp, &p)
}

/// Read all entries (skips malformed lines).
pub fn read(home: &Path) -> Vec<HistoryEntry> {
    fs::read_to_string(path(home))
        .map(|s| {
            s.lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git_repo_with_remote(url: &str) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            assert!(Command::new("git")
                .arg("-C")
                .arg(d.path())
                .args(args)
                .output()
                .unwrap()
                .status
                .success());
        };
        run(&["init", "-q"]);
        run(&["remote", "add", "origin", url]);
        d
    }

    #[test]
    fn cap_drops_oldest_first() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        for i in 0..7 {
            let e = entry_for("pin", Some(&format!("a{i}")), None, cwd.path());
            append(home.path(), &e, 3).unwrap();
        }
        let got: Vec<_> = read(home.path())
            .into_iter()
            .map(|e| e.alias.unwrap())
            .collect();
        assert_eq!(got, vec!["a4", "a5", "a6"]);
    }

    #[test]
    fn plaintext_remote_and_path_never_written() {
        let url = "git@github.com:acme-corp/secret-app.git";
        let repo = git_repo_with_remote(url);
        let home = tempfile::tempdir().unwrap();
        let e = entry_for("pin_mismatch", Some("work"), Some("acme"), repo.path());
        append(home.path(), &e, 10).unwrap();
        let raw = fs::read_to_string(path(home.path())).unwrap();
        assert!(!raw.contains("acme-corp"), "remote owner leaked: {raw}");
        assert!(!raw.contains("secret-app"), "remote repo leaked: {raw}");
        assert!(!raw.contains("github.com"), "remote host leaked: {raw}");
        assert!(
            !raw.contains(&repo.path().display().to_string()),
            "path leaked"
        );
        let back = read(home.path());
        assert_eq!(
            back[0].remote_hashes,
            vec![sha256_hex(&normalize_remote_url(url))]
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(path(home.path()))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn config_off_switch_disables_writes() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        fs::write(
            home.path().join("config.toml"),
            "[history]\nenabled = false\n",
        )
        .unwrap();
        assert!(!load_config(home.path()).history.enabled);
        // The env var can force either way; this test only covers config when unset.
        if std::env::var(ENV_SWITCH).is_err() {
            record(home.path(), "pin", Some("work"), None, cwd.path());
            assert!(!path(home.path()).exists());
        }
    }

    #[test]
    fn default_is_on_with_cap() {
        let home = tempfile::tempdir().unwrap();
        let cfg = load_config(home.path());
        assert!(cfg.history.enabled);
        assert_eq!(cfg.history.max_entries, DEFAULT_MAX_ENTRIES);
    }
}

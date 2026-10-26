//! Local pin history (`$LOCUS_HOME/pin-history.jsonl`).
//!
//! Records `pin`, `leave` and `pin_mismatch` events so `locus autopin suggest`
//! can learn which binding a repo usually uses. Local only, never sent anywhere.
//! On by default; turn off with `[history] enabled = false` in config.toml or
//! `LOCUS_PIN_HISTORY=0`. Capped at `[history] max_entries` (oldest dropped first).
//!
//! Privacy: the git remote URL and workspace path are stored only as keyed
//! hashes: HMAC-SHA256 under a random 32-byte per-install key in
//! `$LOCUS_HOME/history.key` (created 0600 on first use). The remote is hashed
//! after `normalize_remote_url`, which drops userinfo such as `user:token@`.
//! Without the key the hashes cannot be checked against guessed repo names.
//! Entries hold aliases only, never tokens, credential references or the
//! control capability. Writes take an exclusive lock on
//! `.pin-history.lock`, write a fresh `create_new` + `O_NOFOLLOW` 0600 temp file
//! and rename it over the log, so symlinks and leftover files are never followed.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::autopin::{git_remote_urls, normalize_remote_url};
use crate::config::load_config;

pub const FILE_NAME: &str = "pin-history.jsonl";
pub const KEY_FILE: &str = "history.key";
const LOCK_FILE: &str = ".pin-history.lock";
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

fn open_opts(create_new: bool) -> fs::OpenOptions {
    let mut o = fs::OpenOptions::new();
    if create_new {
        o.write(true).create_new(true);
    } else {
        o.read(true).write(true).create(true).truncate(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    o
}

fn read_nofollow(p: &Path) -> std::io::Result<String> {
    let mut o = fs::OpenOptions::new();
    o.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.custom_flags(libc::O_NOFOLLOW);
    }
    let mut s = String::new();
    o.open(p)?.read_to_string(&mut s)?;
    Ok(s)
}

/// Per-install HMAC key, created 0600 on first use (race-safe via `create_new`).
pub fn history_key(home: &Path) -> std::io::Result<Vec<u8>> {
    let p = home.join(KEY_FILE);
    let mut key = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut key);
    match open_opts(true).open(&p) {
        Ok(mut f) => {
            f.write_all(hex::encode(key).as_bytes())?;
            f.sync_all()?;
            Ok(key.to_vec())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let k = hex::decode(read_nofollow(&p)?.trim()).map_err(std::io::Error::other)?;
            if k.len() != 32 {
                return Err(std::io::Error::other("history.key is malformed"));
            }
            Ok(k)
        }
        Err(e) => Err(e),
    }
}

pub fn keyed_hash(key: &[u8], s: &str) -> String {
    let mut m = Hmac::<Sha256>::new_from_slice(key).expect("hmac accepts any key length");
    m.update(s.as_bytes());
    hex::encode(m.finalize().into_bytes())
}

pub fn path(home: &Path) -> PathBuf {
    home.join(FILE_NAME)
}

/// Effective switch: env `LOCUS_PIN_HISTORY` (0/off/false or 1/on/true) wins over config.
pub fn enabled(home: &Path) -> bool {
    enabled_with(home, std::env::var(ENV_SWITCH).ok().as_deref())
}

fn enabled_with(home: &Path, env: Option<&str>) -> bool {
    if let Some(v) = env {
        match v.trim().to_ascii_lowercase().as_str() {
            "0" | "off" | "false" | "no" => return false,
            "1" | "on" | "true" | "yes" => return true,
            _ => {}
        }
    }
    load_config(home).history.enabled
}

/// Where an event happened: hashed workspace root and remotes for `dir`.
pub fn entry_for(
    key: &[u8],
    event: &str,
    alias: Option<&str>,
    expected: Option<&str>,
    dir: Option<&Path>,
) -> HistoryEntry {
    let (workspace_hash, remote_hashes) = match dir {
        Some(cwd) => {
            let ws_root = crate::workspace::find_workspace(cwd)
                .ok()
                .flatten()
                .map(|(p, _)| p)
                .unwrap_or_else(|| cwd.to_path_buf());
            let mut r: Vec<String> = git_remote_urls(cwd)
                .iter()
                .map(|u| keyed_hash(key, &normalize_remote_url(u)))
                .collect();
            r.sort();
            r.dedup();
            (Some(keyed_hash(key, &ws_root.display().to_string())), r)
        }
        None => (None, Vec::new()),
    };
    HistoryEntry {
        ts: chrono::Utc::now().to_rfc3339(),
        event: event.to_string(),
        alias: alias.map(str::to_string),
        expected: expected.map(str::to_string),
        workspace_hash,
        remote_hashes,
    }
}

/// Best-effort record: never fails the caller's pin, leave or refusal.
pub fn record(home: &Path, event: &str, alias: Option<&str>, expected: Option<&str>, cwd: &Path) {
    let _ = try_record(home, event, alias, expected, Some(cwd));
}

/// Best-effort `leave`: hashes the session's workspace, never the caller's cwd.
/// Uses the pinned `.locus.toml` dir when known, else the latest `pin` entry
/// for the alias, else records no location.
pub fn record_leave(home: &Path, alias: &str, session_dir: Option<&Path>) {
    if session_dir.is_some() {
        let _ = try_record(home, "leave", Some(alias), None, session_dir);
        return;
    }
    if !enabled(home) {
        return;
    }
    let mut e = match history_key(home) {
        Ok(k) => entry_for(&k, "leave", Some(alias), None, None),
        Err(_) => return,
    };
    if let Some(last) = read(home)
        .into_iter()
        .rev()
        .find(|x| x.event == "pin" && x.alias.as_deref() == Some(alias))
    {
        e.workspace_hash = last.workspace_hash;
        e.remote_hashes = last.remote_hashes;
    }
    let _ = append(home, &e, load_config(home).history.max_entries);
}

fn try_record(
    home: &Path,
    event: &str,
    alias: Option<&str>,
    expected: Option<&str>,
    dir: Option<&Path>,
) -> std::io::Result<()> {
    if !enabled(home) {
        return Ok(());
    }
    let key = history_key(home)?;
    let max = load_config(home).history.max_entries;
    append(home, &entry_for(&key, event, alias, expected, dir), max)
}

/// Append one entry under an exclusive lock and enforce the cap (oldest first).
pub fn append(home: &Path, entry: &HistoryEntry, max_entries: usize) -> std::io::Result<()> {
    let lock = open_opts(false).open(home.join(LOCK_FILE))?;
    lock.lock_exclusive()?;
    let p = path(home);
    let mut lines: Vec<String> = match read_nofollow(&p) {
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
    let mut nonce = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut nonce);
    let tmp = home.join(format!(".{FILE_NAME}.{}.tmp", hex::encode(nonce)));
    let res = (|| {
        let mut f = open_opts(true).open(&tmp)?;
        let mut buf = lines.join("\n");
        buf.push('\n');
        f.write_all(buf.as_bytes())?;
        f.sync_all()?;
        fs::rename(&tmp, &p)
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res
}

/// Read all entries (skips malformed lines; never follows a symlinked log).
pub fn read(home: &Path) -> Vec<HistoryEntry> {
    read_nofollow(&path(home))
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

    const KEY: &[u8] = &[7u8; 32];

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

    #[cfg(unix)]
    fn mode(p: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn cap_drops_oldest_first() {
        let home = tempfile::tempdir().unwrap();
        for i in 0..7 {
            let e = entry_for(KEY, "pin", Some(&format!("a{i}")), None, None);
            append(home.path(), &e, 3).unwrap();
        }
        let got: Vec<_> = read(home.path())
            .into_iter()
            .map(|e| e.alias.unwrap())
            .collect();
        assert_eq!(got, vec!["a4", "a5", "a6"]);
    }

    #[test]
    fn plaintext_and_tokenized_remotes_and_path_never_written() {
        let url = "https://alice:ghp_SuperSecretToken123@github.com/acme-corp/secret-app.git";
        let repo = git_repo_with_remote(url);
        let home = tempfile::tempdir().unwrap();
        try_record(
            home.path(),
            "pin_mismatch",
            Some("work"),
            Some("acme"),
            Some(repo.path()),
        )
        .unwrap();
        let raw = fs::read_to_string(path(home.path())).unwrap();
        for leak in [
            "ghp_SuperSecretToken123",
            "alice",
            "acme-corp",
            "secret-app",
            "github.com",
        ] {
            assert!(!raw.contains(leak), "{leak} leaked: {raw}");
        }
        assert!(
            !raw.contains(&repo.path().display().to_string()),
            "path leaked"
        );
        let key = history_key(home.path()).unwrap();
        let back = read(home.path());
        assert_eq!(
            back[0].remote_hashes,
            vec![keyed_hash(&key, &normalize_remote_url(url))]
        );
        // Keyed, not a bare digest a guesser could reproduce.
        assert_ne!(
            back[0].remote_hashes[0],
            keyed_hash(&[0u8; 32], &normalize_remote_url(url))
        );
        #[cfg(unix)]
        {
            assert_eq!(mode(&path(home.path())), 0o600);
            assert_eq!(mode(&home.path().join(KEY_FILE)), 0o600);
        }
    }

    #[test]
    fn key_is_stable_per_install_and_differs_across_installs() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let k1 = history_key(a.path()).unwrap();
        assert_eq!(k1, history_key(a.path()).unwrap());
        assert_ne!(k1, history_key(b.path()).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_log_is_not_followed_and_perms_stay_0600() {
        let home = tempfile::tempdir().unwrap();
        let victim = home.path().join("victim.txt");
        fs::write(&victim, "keep\n").unwrap();
        std::os::unix::fs::symlink(&victim, path(home.path())).unwrap();
        // A leftover world-readable temp from a crash is never reused.
        let stale = home.path().join(format!(".{FILE_NAME}.tmp"));
        fs::write(&stale, "stale").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&stale, fs::Permissions::from_mode(0o644)).unwrap();
        }
        let e = entry_for(KEY, "pin", Some("work"), None, None);
        assert!(
            append(home.path(), &e, 10).is_err(),
            "must refuse a symlinked log"
        );
        assert_eq!(fs::read_to_string(&victim).unwrap(), "keep\n");
        assert_eq!(fs::read_to_string(&stale).unwrap(), "stale");
        // Without the symlink a write succeeds and lands 0600.
        fs::remove_file(path(home.path())).unwrap();
        append(home.path(), &e, 10).unwrap();
        assert_eq!(mode(&path(home.path())), 0o600);
        let leftovers = fs::read_dir(home.path())
            .unwrap()
            .filter(|d| {
                let n = d.as_ref().unwrap().file_name().into_string().unwrap();
                n.ends_with(".tmp") && n != format!(".{FILE_NAME}.tmp")
            })
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn concurrent_writers_lose_no_lines() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path().to_path_buf();
        let threads: Vec<_> = (0..4)
            .map(|t| {
                let h = h.clone();
                std::thread::spawn(move || {
                    for i in 0..25 {
                        let e = entry_for(KEY, "pin", Some(&format!("t{t}-{i}")), None, None);
                        append(&h, &e, 1000).unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let raw = fs::read_to_string(path(&h)).unwrap();
        assert_eq!(raw.lines().count(), 100);
        assert_eq!(read(&h).len(), 100, "every line parses");
    }

    #[test]
    fn config_off_switch_disables_writes() {
        let home = tempfile::tempdir().unwrap();
        fs::write(
            home.path().join("config.toml"),
            "[history]\nenabled = false\n",
        )
        .unwrap();
        assert!(!enabled_with(home.path(), None), "config off, env unset");
        assert!(!enabled_with(home.path(), Some("0")));
        assert!(enabled_with(home.path(), Some("1")), "env wins over config");
        let on = tempfile::tempdir().unwrap();
        assert!(enabled_with(on.path(), None), "default on");
        assert!(!enabled_with(on.path(), Some("off")));
    }

    #[test]
    fn leave_uses_session_workspace_not_cwd() {
        let home = tempfile::tempdir().unwrap();
        let repo = git_repo_with_remote("git@github.com:acme-corp/app.git");
        try_record(home.path(), "pin", Some("acme"), None, Some(repo.path())).unwrap();
        // No session dir known: copy the location from the alias's last pin.
        record_leave(home.path(), "acme", None);
        let h = read(home.path());
        assert_eq!(h[1].event, "leave");
        assert_eq!(h[1].workspace_hash, h[0].workspace_hash);
        assert_eq!(h[1].remote_hashes, h[0].remote_hashes);
        // Session dir known: hash that dir.
        record_leave(home.path(), "acme", Some(repo.path()));
        assert_eq!(read(home.path())[2].workspace_hash, h[0].workspace_hash);
    }

    #[test]
    fn default_is_on_with_cap() {
        let home = tempfile::tempdir().unwrap();
        let cfg = load_config(home.path());
        assert!(cfg.history.enabled);
        assert_eq!(cfg.history.max_entries, DEFAULT_MAX_ENTRIES);
    }
}

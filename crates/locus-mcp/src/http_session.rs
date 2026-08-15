//! HTTP `Mcp-Session-Id` map — in-memory cache + file-backed resume.
//!
//! Disk layout: `$LOCUS_HOME/http-sessions/<id>.json` (or `LOCUS_MCP_SESSION_DIR`).
//! Records store only session id, timestamps, and an optional values-free pin
//! summary. **Never** tokens, credential refs, or resolved secrets.

use crate::anchor::{self, AnchorDecision, SessionAnchor};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// On-disk record schema version (bump only with a reader migration).
pub const HTTP_SESSION_DISK_VERSION: u32 = 1;

/// Opaque MCP HTTP session (`Mcp-Session-Id`) — memory cache entry.
#[derive(Debug, Clone)]
pub struct HttpSessionEntry {
    pub created_at: SystemTime,
    pub last_seen: SystemTime,
    /// Values-free pin snapshot when known (alias/tenant/mode/seal_ok only).
    pub pin: Option<HttpSessionPinSummary>,
    /// Identity anchored at initialize / first healthy pinned observation.
    /// Enforced fail-closed on provider tools/call (see `crate::anchor`).
    pub anchor: Option<SessionAnchor>,
    /// In-memory only: dedupe for `mcp.anchor_mismatch` audits
    /// (anchored_session_id, current_session_id). Never persisted.
    pub last_reported_mismatch: Option<(String, String)>,
}

/// Optional pin seal summary persisted with the HTTP session (no secrets).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HttpSessionPinSummary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seal_ok: Option<bool>,
}

/// On-disk record under `$LOCUS_HOME/http-sessions/<id>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpSessionDiskRecord {
    /// Schema version ([`HTTP_SESSION_DISK_VERSION`]).
    pub v: u32,
    pub id: String,
    pub created_at_unix: u64,
    pub last_seen_unix: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<HttpSessionPinSummary>,
    /// Optional session identity anchor (aliases/ids only — never secrets).
    /// Serde-optional so v1 records round-trip both ways (old binaries ignore
    /// it; old records load as `None` and anchor at the next healthy
    /// observation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<SessionAnchor>,
}

/// In-memory map of streamable-HTTP session ids with idle TTL, hard capacity,
/// and optional file-backed resume for restarts / multi-worker.
#[derive(Debug)]
pub struct HttpSessionMap {
    sessions: HashMap<String, HttpSessionEntry>,
    ttl: Duration,
    max: usize,
    /// When set, mint/touch/remove persist under this directory.
    persist_dir: Option<PathBuf>,
    /// Optional callback for values-free pin annotation (injected from main).
    pin_summary_fn: Option<fn() -> Option<HttpSessionPinSummary>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpSessionError {
    /// Client sent a non-empty id that is unknown or past TTL (fail closed).
    Unknown,
    /// Client sent an empty / whitespace-only id.
    Invalid,
    /// Map is at capacity after purge (mint refused).
    Capacity,
}

impl HttpSessionMap {
    pub fn new(ttl: Duration, max: usize) -> Self {
        Self {
            sessions: HashMap::new(),
            ttl,
            max: max.max(1),
            persist_dir: None,
            pin_summary_fn: None,
        }
    }

    pub fn with_persist_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.persist_dir = dir;
        self
    }

    pub fn with_pin_summary_fn(mut self, f: Option<fn() -> Option<HttpSessionPinSummary>>) -> Self {
        self.pin_summary_fn = f;
        self
    }

    pub fn purge_expired(&mut self, now: SystemTime) {
        let ttl = self.ttl;
        self.sessions
            .retain(|_, e| !is_expired(e.last_seen, now, ttl));
        self.prune_disk(now);
    }

    pub fn mint(&mut self) -> Result<String, HttpSessionError> {
        let now = SystemTime::now();
        self.purge_expired(now);
        if self.live_count() >= self.max {
            return Err(HttpSessionError::Capacity);
        }
        // Opaque 128-bit id (hex). Collision retry is extremely unlikely.
        for _ in 0..8 {
            let mut bytes = [0u8; 16];
            rand::thread_rng().fill_bytes(&mut bytes);
            let id = hex::encode(bytes);
            if self.sessions.contains_key(&id) || self.disk_exists(&id) {
                continue;
            }
            let pin = self.current_pin();
            let entry = HttpSessionEntry {
                created_at: now,
                last_seen: now,
                pin,
                anchor: None,
                last_reported_mismatch: None,
            };
            self.persist_entry(&id, &entry);
            self.sessions.insert(id.clone(), entry);
            return Ok(id);
        }
        Err(HttpSessionError::Capacity)
    }

    /// Touch an existing non-expired session. Loads from disk on memory miss.
    /// Returns false if unknown/expired/corrupt (fail closed).
    pub fn touch(&mut self, id: &str) -> bool {
        let now = SystemTime::now();
        self.purge_expired(now);
        let pin = self.current_pin();
        if self.sessions.contains_key(id) {
            // Multi-worker: the disk anchor is authoritative. A sibling worker
            // may have established or reset it since this worker cached the
            // entry, so adopt the disk anchor before persisting — the
            // per-request pin refresh must never write a stale in-memory
            // anchor back over a sibling's establishment or initialize-time
            // reset. Only Established/Repinned (`observe_anchor`) and
            // `reset_anchor` author new anchor values.
            self.adopt_disk_anchor(id, now);
            let entry = self.sessions.get_mut(id).expect("checked contains_key");
            entry.last_seen = now;
            if let Some(pin) = pin {
                entry.pin = Some(pin);
            }
            let snapshot = entry.clone();
            self.persist_entry(id, &snapshot);
            return true;
        }
        // Cross-process / restart resume: load on miss.
        match self.load_disk_entry(id, now) {
            Some(mut entry) => {
                entry.last_seen = now;
                if let Some(pin) = pin {
                    entry.pin = Some(pin);
                }
                self.persist_entry(id, &entry);
                self.sessions.insert(id.to_string(), entry);
                true
            }
            None => false,
        }
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let mem = self.sessions.remove(id).is_some();
        let disk = self.remove_disk(id);
        mem || disk
    }

    /// Load an entry into memory (disk resume on miss) without refreshing the
    /// pin summary. Returns false when unknown/expired/corrupt (fail closed).
    fn ensure_loaded(&mut self, id: &str, now: SystemTime) -> bool {
        if self.sessions.contains_key(id) {
            return true;
        }
        match self.load_disk_entry(id, now) {
            Some(entry) => {
                self.sessions.insert(id.to_string(), entry);
                true
            }
            None => false,
        }
    }

    /// Adopt the on-disk anchor into the memory entry — across workers the
    /// disk record is authoritative (a sibling may have established or reset
    /// it after this worker cached the entry). No-op when persistence is off
    /// or the record cannot be read (keep the memory anchor — fail closed
    /// rather than silently un-anchoring on a transient read error).
    fn adopt_disk_anchor(&mut self, id: &str, now: SystemTime) {
        let Some(disk_entry) = self.load_disk_entry(id, now) else {
            return;
        };
        if let Some(entry) = self.sessions.get_mut(id) {
            entry.anchor = disk_entry.anchor;
        }
    }

    /// Current anchor for a session (disk-loading on memory miss; the disk
    /// anchor is adopted on a memory hit too — see [`Self::adopt_disk_anchor`]).
    pub fn anchor(&mut self, id: &str) -> Option<SessionAnchor> {
        let now = SystemTime::now();
        if !self.ensure_loaded(id, now) {
            return None;
        }
        self.adopt_disk_anchor(id, now);
        self.sessions.get(id).and_then(|e| e.anchor.clone())
    }

    /// Compare-and-set the session anchor against a healthy observation.
    ///
    /// Persists via the atomic writer only on `Established` / `Repinned`
    /// (write-once establishment — concurrent workers cannot rotate each
    /// other's anchors); `Mismatch` never writes. Returns `None` when the
    /// session id is unknown (fail closed) or no decision applies.
    pub fn observe_anchor(
        &mut self,
        id: &str,
        obs: &SessionAnchor,
        allow_establish: bool,
    ) -> Option<AnchorDecision> {
        let now = SystemTime::now();
        if !self.ensure_loaded(id, now) {
            return None;
        }
        // Cross-worker establishment race: re-read the disk anchor first so a
        // sibling's just-persisted establishment (or initialize-time reset) is
        // enforced here instead of being clobbered by a fresh establishment
        // from this worker's stale memory view.
        self.adopt_disk_anchor(id, now);
        let entry = self.sessions.get_mut(id)?;
        let decision = anchor::decide(&mut entry.anchor, obs, allow_establish)?;
        if matches!(
            decision,
            AnchorDecision::Established | AnchorDecision::Repinned
        ) {
            let snapshot = entry.clone();
            self.persist_entry(id, &snapshot);
        }
        Some(decision)
    }

    /// Initialize-only anchor overwrite (adoption path). Persisted.
    /// Returns false when the session id is unknown (fail closed).
    pub fn reset_anchor(&mut self, id: &str, new_anchor: Option<SessionAnchor>) -> bool {
        let now = SystemTime::now();
        if !self.ensure_loaded(id, now) {
            return false;
        }
        let Some(entry) = self.sessions.get_mut(id) else {
            return false;
        };
        entry.anchor = new_anchor;
        entry.last_reported_mismatch = None;
        let snapshot = entry.clone();
        self.persist_entry(id, &snapshot);
        true
    }

    /// Mismatch audit dedupe: true when this (anchored_session_id,
    /// current_session_id) pair has not been reported yet for this session.
    /// In-memory only — a restarted worker may re-report once (acceptable).
    pub fn note_mismatch(&mut self, id: &str, key: (String, String)) -> bool {
        let Some(entry) = self.sessions.get_mut(id) else {
            return false;
        };
        if entry.last_reported_mismatch.as_ref() == Some(&key) {
            return false;
        }
        entry.last_reported_mismatch = Some(key);
        true
    }

    /// Memory entries + non-expired disk files (for capacity across workers).
    pub fn live_count(&self) -> usize {
        match &self.persist_dir {
            Some(dir) => {
                let disk = count_disk_sessions(dir, self.ttl);
                self.sessions.len().max(disk)
            }
            None => self.sessions.len(),
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    #[cfg(test)]
    pub fn persist_dir(&self) -> Option<&Path> {
        self.persist_dir.as_deref()
    }

    /// Test helper: insert with an explicit last_seen (for TTL).
    #[cfg(test)]
    pub fn insert_for_test(&mut self, id: impl Into<String>, last_seen: SystemTime) {
        let id = id.into();
        let entry = HttpSessionEntry {
            created_at: last_seen,
            last_seen,
            pin: None,
            anchor: None,
            last_reported_mismatch: None,
        };
        if self.persist_dir.is_some() {
            self.persist_entry(&id, &entry);
        }
        self.sessions.insert(id, entry);
    }

    /// Drop the in-memory cache only (simulates process restart; disk remains).
    #[cfg(test)]
    pub fn clear_memory(&mut self) {
        self.sessions.clear();
    }

    fn current_pin(&self) -> Option<HttpSessionPinSummary> {
        self.pin_summary_fn.and_then(|f| f())
    }

    fn disk_path(&self, id: &str) -> Option<PathBuf> {
        let dir = self.persist_dir.as_ref()?;
        if !is_safe_http_session_id(id) {
            return None;
        }
        Some(dir.join(format!("{id}.json")))
    }

    fn disk_exists(&self, id: &str) -> bool {
        self.disk_path(id).map(|p| p.is_file()).unwrap_or(false)
    }

    fn persist_entry(&self, id: &str, entry: &HttpSessionEntry) {
        let Some(path) = self.disk_path(id) else {
            return;
        };
        let Some(dir) = self.persist_dir.as_ref() else {
            return;
        };
        let rec = HttpSessionDiskRecord {
            v: HTTP_SESSION_DISK_VERSION,
            id: id.to_string(),
            created_at_unix: system_time_to_unix(entry.created_at),
            last_seen_unix: system_time_to_unix(entry.last_seen),
            pin: entry.pin.clone(),
            anchor: entry.anchor.clone(),
        };
        if let Err(e) = write_http_session_atomic(dir, &path, &rec) {
            // Persistence is best-effort for the minting process; resume may
            // fail closed on other workers if write fails (never soft-allow).
            eprintln!("locus-mcp: http session persist failed for {id}: {e}");
        }
    }

    /// Load a non-expired disk record. Corrupt / mismatched files are removed
    /// and treated as unknown (fail closed).
    fn load_disk_entry(&self, id: &str, now: SystemTime) -> Option<HttpSessionEntry> {
        let path = self.disk_path(id)?;
        let raw = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => return None,
        };
        let rec: HttpSessionDiskRecord = match serde_json::from_str(&raw) {
            Ok(r) => r,
            Err(_) => {
                let _ = fs::remove_file(&path);
                return None;
            }
        };
        if rec.v != HTTP_SESSION_DISK_VERSION || rec.id != id || !is_safe_http_session_id(&rec.id) {
            let _ = fs::remove_file(&path);
            return None;
        }
        let created_at = unix_to_system_time(rec.created_at_unix)?;
        let last_seen = unix_to_system_time(rec.last_seen_unix)?;
        if is_expired(last_seen, now, self.ttl) {
            let _ = fs::remove_file(&path);
            return None;
        }
        // Reject obviously inverted clocks (fail closed).
        if last_seen < created_at {
            let _ = fs::remove_file(&path);
            return None;
        }
        Some(HttpSessionEntry {
            created_at,
            last_seen,
            pin: rec.pin,
            anchor: rec.anchor,
            last_reported_mismatch: None,
        })
    }

    fn remove_disk(&self, id: &str) -> bool {
        let Some(path) = self.disk_path(id) else {
            return false;
        };
        match fs::remove_file(&path) {
            Ok(()) => true,
            Err(e) if e.kind() == io::ErrorKind::NotFound => false,
            Err(_) => false,
        }
    }

    /// Scan persist dir: drop expired/corrupt files (values-free cleanup only).
    fn prune_disk(&self, now: SystemTime) {
        let Some(dir) = self.persist_dir.as_ref() else {
            return;
        };
        let entries = match fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for ent in entries.flatten() {
            let path = ent.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                // Leave unrelated files alone (including `.*.tmp` writers).
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if !is_safe_http_session_id(stem) {
                // Unexpected name under session dir — fail closed (remove).
                let _ = fs::remove_file(&path);
                continue;
            }
            let raw = match fs::read_to_string(&path) {
                Ok(s) => s,
                Err(_) => {
                    let _ = fs::remove_file(&path);
                    continue;
                }
            };
            let rec: HttpSessionDiskRecord = match serde_json::from_str(&raw) {
                Ok(r) => r,
                Err(_) => {
                    let _ = fs::remove_file(&path);
                    continue;
                }
            };
            if rec.v != HTTP_SESSION_DISK_VERSION
                || rec.id != stem
                || !is_safe_http_session_id(&rec.id)
            {
                let _ = fs::remove_file(&path);
                continue;
            }
            let Some(last_seen) = unix_to_system_time(rec.last_seen_unix) else {
                let _ = fs::remove_file(&path);
                continue;
            };
            if is_expired(last_seen, now, self.ttl) {
                let _ = fs::remove_file(&path);
            }
        }
    }
}

/// Hex-only 32-char ids (minted shape) — path-safe and collision-resistant.
pub fn is_safe_http_session_id(id: &str) -> bool {
    id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit())
}

fn is_expired(last_seen: SystemTime, now: SystemTime, ttl: Duration) -> bool {
    match now.duration_since(last_seen) {
        Ok(age) => age >= ttl,
        // Clock skew: treat as not expired so a brief jump back doesn't wipe state.
        Err(_) => false,
    }
}

pub fn system_time_to_unix(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

pub fn unix_to_system_time(secs: u64) -> Option<SystemTime> {
    UNIX_EPOCH.checked_add(Duration::from_secs(secs))
}

/// Count non-expired disk session files (best-effort).
fn count_disk_sessions(dir: &Path, ttl: Duration) -> usize {
    let now = SystemTime::now();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };
    let mut n = 0;
    for ent in entries.flatten() {
        let path = ent.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(rec) = serde_json::from_str::<HttpSessionDiskRecord>(&raw) else {
            continue;
        };
        if rec.v != HTTP_SESSION_DISK_VERSION || !is_safe_http_session_id(&rec.id) {
            continue;
        }
        let Some(last_seen) = unix_to_system_time(rec.last_seen_unix) else {
            continue;
        };
        if !is_expired(last_seen, now, ttl) {
            n += 1;
        }
    }
    n
}

/// Atomic write: temp file + `sync_all` + rename. Mode 0600 on Unix.
fn write_http_session_atomic(
    dir: &Path,
    final_path: &Path,
    rec: &HttpSessionDiskRecord,
) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let bytes =
        serde_json::to_vec(rec).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    // Unique temp name avoids clobbering concurrent writers for other ids.
    let mut nonce = [0u8; 4];
    rand::thread_rng().fill_bytes(&mut nonce);
    let tmp = dir.join(format!(".{}.{}.tmp", rec.id, hex::encode(nonce)));
    {
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, final_path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(final_path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Resolve directory for HTTP session files.
///
/// Priority: `LOCUS_MCP_SESSION_DIR` (non-empty) → `$LOCUS_HOME/http-sessions`.
/// Returns `None` only when home cannot be resolved (memory-only fallback).
pub fn resolve_http_session_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("LOCUS_MCP_SESSION_DIR") {
        let t = dir.trim();
        if !t.is_empty() {
            return Some(PathBuf::from(t));
        }
    }
    locus_core::locus_home()
        .ok()
        .map(|h| h.join("http-sessions"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_anchor(alias: &str, binding_id: &str, session_id: &str) -> SessionAnchor {
        SessionAnchor {
            binding_id: binding_id.into(),
            binding_alias: alias.into(),
            tenant: format!("{alias}-corp"),
            mode: "exclusive".into(),
            namespaces: Vec::new(),
            session_id: session_id.into(),
            backing: Some("active".into()),
            anchored_at_unix: 1,
        }
    }

    fn assert_values_free(raw: &str) {
        let lower = raw.to_ascii_lowercase();
        for banned in [
            "phm:",
            "credential",
            "token",
            "password",
            "secret",
            "authorization",
            "api_key",
            "service_role",
        ] {
            assert!(
                !lower.contains(banned),
                "disk record must not contain {banned}: {raw}"
            );
        }
    }

    #[test]
    fn mint_issues_opaque_hex_id() {
        let mut map = HttpSessionMap::new(Duration::from_secs(60), 8);
        let id = map.mint().expect("mint");
        assert_eq!(id.len(), 32, "16-byte hex id expected, got {id}");
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()), "{id}");
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn touch_reuses_and_unknown_rejects() {
        let mut map = HttpSessionMap::new(Duration::from_secs(60), 8);
        let id = map.mint().unwrap();
        assert!(map.touch(&id), "fresh id must touch");
        assert!(map.touch(&id), "second touch must succeed");
        assert!(!map.touch("deadbeefdeadbeefdeadbeefdeadbeef"));
        assert!(!map.touch("not-a-real-session"));
    }

    #[test]
    fn capacity_blocks_new_mints() {
        let mut map = HttpSessionMap::new(Duration::from_secs(60), 2);
        assert!(map.mint().is_ok());
        assert!(map.mint().is_ok());
        assert_eq!(map.mint(), Err(HttpSessionError::Capacity));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn ttl_expiry_purges_and_rejects() {
        let mut map = HttpSessionMap::new(Duration::from_millis(50), 8);
        let id = map.mint().unwrap();
        map.insert_for_test(&id, SystemTime::now() - Duration::from_secs(10));
        assert!(!map.touch(&id), "expired session must not touch");
        assert_eq!(map.len(), 0, "purge should drop expired entry");
        let id2 = map.mint().unwrap();
        assert!(map.touch(&id2));
    }

    #[test]
    fn remove_terminates_session() {
        let mut map = HttpSessionMap::new(Duration::from_secs(60), 8);
        let id = map.mint().unwrap();
        assert!(map.remove(&id));
        assert!(!map.touch(&id));
        assert!(!map.remove(&id));
    }

    #[test]
    fn persist_mint_writes_disk_without_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let mut map = HttpSessionMap::new(Duration::from_secs(60), 8)
            .with_persist_dir(Some(dir.path().to_path_buf()));
        let id = map.mint().expect("mint");
        let path = dir.path().join(format!("{id}.json"));
        assert!(path.is_file(), "mint must write {path:?}");
        let raw = fs::read_to_string(&path).unwrap();
        let rec: HttpSessionDiskRecord = serde_json::from_str(&raw).unwrap();
        assert_eq!(rec.v, HTTP_SESSION_DISK_VERSION);
        assert_eq!(rec.id, id);
        assert!(rec.created_at_unix > 0);
        assert!(rec.last_seen_unix >= rec.created_at_unix);
        let lower = raw.to_ascii_lowercase();
        for banned in [
            "phm:",
            "credential",
            "token",
            "password",
            "secret",
            "authorization",
            "api_key",
            "service_role",
        ] {
            assert!(
                !lower.contains(banned),
                "disk record must not contain {banned}: {raw}"
            );
        }

        // A record carrying an anchor stays values-free too.
        assert!(map.reset_anchor(&id, Some(sample_anchor("acme", "bnd_acme", "sess_1"))));
        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"anchor\""), "anchor must persist: {raw}");
        assert_values_free(&raw);
    }

    #[test]
    fn persist_resume_after_memory_drop() {
        let dir = tempfile::tempdir().unwrap();
        let mut map = HttpSessionMap::new(Duration::from_secs(60), 8)
            .with_persist_dir(Some(dir.path().to_path_buf()));
        let id = map.mint().unwrap();
        assert!(map.touch(&id));
        map.clear_memory();
        assert_eq!(map.len(), 0);
        assert!(map.touch(&id), "fresh map must resume session id from disk");
        assert_eq!(map.len(), 1);

        let mut map2 = HttpSessionMap::new(Duration::from_secs(60), 8)
            .with_persist_dir(Some(dir.path().to_path_buf()));
        assert!(map2.touch(&id), "sibling worker must resume from disk");
        assert_eq!(map2.len(), 1);
    }

    #[test]
    fn persist_expire_removes_disk_and_rejects() {
        let dir = tempfile::tempdir().unwrap();
        let mut map = HttpSessionMap::new(Duration::from_millis(50), 8)
            .with_persist_dir(Some(dir.path().to_path_buf()));
        let id = map.mint().unwrap();
        let path = dir.path().join(format!("{id}.json"));
        assert!(path.is_file());
        map.insert_for_test(&id, SystemTime::now() - Duration::from_secs(10));
        assert!(!map.touch(&id), "expired must not resume");
        assert!(!path.is_file(), "expired disk record must be pruned");
        let mut map2 = HttpSessionMap::new(Duration::from_millis(50), 8)
            .with_persist_dir(Some(dir.path().to_path_buf()));
        assert!(!map2.touch(&id));
    }

    #[test]
    fn persist_corrupt_file_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let mut map = HttpSessionMap::new(Duration::from_secs(60), 8)
            .with_persist_dir(Some(dir.path().to_path_buf()));
        let id = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let path = dir.path().join(format!("{id}.json"));
        fs::create_dir_all(dir.path()).unwrap();
        fs::write(&path, "{not-json").unwrap();
        assert!(!map.touch(id), "corrupt disk must fail closed");
        assert!(
            !path.is_file(),
            "corrupt file should be removed after fail-closed load"
        );

        let id2 = "cccccccccccccccccccccccccccccccc";
        let path2 = dir.path().join(format!("{id2}.json"));
        let bad = HttpSessionDiskRecord {
            v: HTTP_SESSION_DISK_VERSION,
            id: "dddddddddddddddddddddddddddddddd".into(),
            created_at_unix: system_time_to_unix(SystemTime::now()),
            last_seen_unix: system_time_to_unix(SystemTime::now()),
            pin: None,
            anchor: None,
        };
        fs::write(&path2, serde_json::to_vec(&bad).unwrap()).unwrap();
        assert!(!map.touch(id2));
        assert!(!path2.is_file());
    }

    #[test]
    fn persist_remove_deletes_disk() {
        let dir = tempfile::tempdir().unwrap();
        let mut map = HttpSessionMap::new(Duration::from_secs(60), 8)
            .with_persist_dir(Some(dir.path().to_path_buf()));
        let id = map.mint().unwrap();
        let path = dir.path().join(format!("{id}.json"));
        assert!(path.is_file());
        assert!(map.remove(&id));
        assert!(!path.is_file());
        map.clear_memory();
        assert!(!map.touch(&id), "deleted session must not resume");
    }

    #[test]
    fn with_persist_dir_none_is_memory_only() {
        let mut map = HttpSessionMap::new(Duration::from_secs(60), 8).with_persist_dir(None);
        assert!(map.persist_dir().is_none());
        let id = map.mint().unwrap();
        map.clear_memory();
        assert!(!map.touch(&id));
    }

    #[test]
    fn pin_summary_persisted_when_callback_set() {
        fn sample_pin() -> Option<HttpSessionPinSummary> {
            Some(HttpSessionPinSummary {
                binding_alias: Some("personal".into()),
                tenant: Some("home".into()),
                mode: Some("exclusive".into()),
                seal_ok: Some(true),
            })
        }
        let dir = tempfile::tempdir().unwrap();
        let mut map = HttpSessionMap::new(Duration::from_secs(60), 8)
            .with_persist_dir(Some(dir.path().to_path_buf()))
            .with_pin_summary_fn(Some(sample_pin));
        let id = map.mint().unwrap();
        let raw = fs::read_to_string(dir.path().join(format!("{id}.json"))).unwrap();
        let rec: HttpSessionDiskRecord = serde_json::from_str(&raw).unwrap();
        let pin = rec.pin.expect("pin summary");
        assert_eq!(pin.binding_alias.as_deref(), Some("personal"));
        assert_eq!(pin.tenant.as_deref(), Some("home"));
        assert_eq!(pin.seal_ok, Some(true));
        assert!(!raw.to_ascii_lowercase().contains("phm:"));

        // Anchor + pin summary together remain values-free.
        assert!(map.reset_anchor(&id, Some(sample_anchor("personal", "bnd_personal", "s1"))));
        let raw = fs::read_to_string(dir.path().join(format!("{id}.json"))).unwrap();
        assert!(raw.contains("\"anchor\""));
        assert_values_free(&raw);
    }

    #[test]
    fn touch_preserves_anchor_while_refreshing_pin() {
        fn sample_pin() -> Option<HttpSessionPinSummary> {
            Some(HttpSessionPinSummary {
                binding_alias: Some("personal".into()),
                tenant: Some("home".into()),
                mode: Some("exclusive".into()),
                seal_ok: Some(true),
            })
        }
        let dir = tempfile::tempdir().unwrap();
        let mut map = HttpSessionMap::new(Duration::from_secs(60), 8)
            .with_persist_dir(Some(dir.path().to_path_buf()))
            .with_pin_summary_fn(Some(sample_pin));
        let id = map.mint().unwrap();
        let anchor = sample_anchor("acme", "bnd_acme", "sess_1");
        assert!(map.reset_anchor(&id, Some(anchor.clone())));

        // Every touch refreshes the informational pin — the anchor must survive
        // both the memory-hit persist and the disk resume path.
        assert!(map.touch(&id));
        assert!(map.touch(&id));
        let raw = fs::read_to_string(dir.path().join(format!("{id}.json"))).unwrap();
        let rec: HttpSessionDiskRecord = serde_json::from_str(&raw).unwrap();
        let disk_anchor = rec.anchor.expect("touch clobbered the anchor");
        assert!(disk_anchor.same_identity(&anchor));

        map.clear_memory();
        assert!(map.touch(&id), "resume from disk");
        assert!(map
            .anchor(&id)
            .expect("anchor survives resume")
            .same_identity(&anchor));

        // Sibling worker resumes with the anchor enforced from disk.
        let mut map2 = HttpSessionMap::new(Duration::from_secs(60), 8)
            .with_persist_dir(Some(dir.path().to_path_buf()));
        assert!(map2
            .anchor(&id)
            .expect("sibling sees anchor")
            .same_identity(&anchor));
    }

    /// Regression (multi-worker): a sibling worker's initialize-time anchor
    /// reset must never be clobbered by another worker's touch() re-persisting
    /// its stale in-memory anchor on the next request.
    #[test]
    fn touch_never_clobbers_sibling_anchor_reset() {
        let dir = tempfile::tempdir().unwrap();
        let mut worker1 = HttpSessionMap::new(Duration::from_secs(60), 8)
            .with_persist_dir(Some(dir.path().to_path_buf()));
        let mut worker2 = HttpSessionMap::new(Duration::from_secs(60), 8)
            .with_persist_dir(Some(dir.path().to_path_buf()));

        let id = worker1.mint().unwrap();
        let anchor_a = sample_anchor("acme", "bnd_acme", "sess_a");
        assert!(worker1.reset_anchor(&id, Some(anchor_a.clone())));

        // worker2 caches the entry (anchor A in memory).
        assert!(worker2.touch(&id));
        assert!(worker2.anchor(&id).unwrap().same_identity(&anchor_a));

        // worker1 handles a re-initialize: anchor reset to B on disk.
        let anchor_b = sample_anchor("beta", "bnd_beta", "sess_b");
        assert!(worker1.reset_anchor(&id, Some(anchor_b.clone())));

        // worker2's next request touches — it must adopt B, never write A back.
        assert!(worker2.touch(&id));
        let raw = fs::read_to_string(dir.path().join(format!("{id}.json"))).unwrap();
        let rec: HttpSessionDiskRecord = serde_json::from_str(&raw).unwrap();
        let disk_anchor = rec.anchor.expect("anchor must survive");
        assert!(
            disk_anchor.same_identity(&anchor_b),
            "touch clobbered the sibling's reset: disk={} expected beta",
            disk_anchor.binding_alias
        );
        assert!(
            worker2.anchor(&id).unwrap().same_identity(&anchor_b),
            "worker2 memory must converge to the disk anchor"
        );

        // Reset to None (unpinned re-initialize) is adopted too.
        assert!(worker1.reset_anchor(&id, None));
        assert!(worker2.touch(&id));
        let raw = fs::read_to_string(dir.path().join(format!("{id}.json"))).unwrap();
        let rec: HttpSessionDiskRecord = serde_json::from_str(&raw).unwrap();
        assert!(rec.anchor.is_none(), "cleared anchor must stay cleared");
    }

    /// Regression (multi-worker): a worker observing with a stale anchorless
    /// memory entry must enforce the sibling's just-persisted anchor —
    /// Mismatch, never a clobbering fresh establishment.
    #[test]
    fn observe_anchor_enforces_sibling_established_anchor() {
        let dir = tempfile::tempdir().unwrap();
        let mut worker1 = HttpSessionMap::new(Duration::from_secs(60), 8)
            .with_persist_dir(Some(dir.path().to_path_buf()));
        let mut worker2 = HttpSessionMap::new(Duration::from_secs(60), 8)
            .with_persist_dir(Some(dir.path().to_path_buf()));

        let id = worker1.mint().unwrap();
        // worker2 loads the entry while no anchor exists yet.
        assert!(worker2.touch(&id));
        assert!(worker2.anchor(&id).is_none());

        // worker1 establishes acme on disk.
        let obs_a = sample_anchor("acme", "bnd_acme", "sess_a");
        assert_eq!(
            worker1.observe_anchor(&id, &obs_a, true),
            Some(AnchorDecision::Established)
        );

        // worker2 observes a different identity: must see the sibling's anchor
        // and refuse — not establish beta over it.
        let obs_b = sample_anchor("beta", "bnd_beta", "sess_b");
        match worker2.observe_anchor(&id, &obs_b, true) {
            Some(AnchorDecision::Mismatch { anchored }) => {
                assert_eq!(anchored.binding_alias, "acme");
            }
            other => panic!("expected mismatch against sibling anchor, got {other:?}"),
        }
        let rec: HttpSessionDiskRecord = serde_json::from_str(
            &fs::read_to_string(dir.path().join(format!("{id}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(rec.anchor.unwrap().binding_alias, "acme");

        // Same identity from worker2 is a Match (adopted, not re-established).
        assert_eq!(
            worker2.observe_anchor(&id, &sample_anchor("acme", "bnd_acme", "sess_a"), true),
            Some(AnchorDecision::Match)
        );
    }

    #[test]
    fn legacy_record_without_anchor_loads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let id = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
        let now = system_time_to_unix(SystemTime::now());
        // Hand-written v1 record predating the anchor field.
        let legacy = format!(
            "{{\"v\":1,\"id\":\"{id}\",\"created_at_unix\":{now},\"last_seen_unix\":{now}}}"
        );
        fs::create_dir_all(dir.path()).unwrap();
        fs::write(dir.path().join(format!("{id}.json")), legacy).unwrap();
        let mut map = HttpSessionMap::new(Duration::from_secs(60), 8)
            .with_persist_dir(Some(dir.path().to_path_buf()));
        assert!(map.touch(id), "legacy record must resume");
        assert!(map.anchor(id).is_none(), "legacy record anchors later");

        // Adopt-once at the next healthy observation.
        let obs = sample_anchor("acme", "bnd_acme", "sess_1");
        assert_eq!(
            map.observe_anchor(id, &obs, true),
            Some(AnchorDecision::Established)
        );
        let raw = fs::read_to_string(dir.path().join(format!("{id}.json"))).unwrap();
        assert!(raw.contains("\"anchor\""), "establishment must persist");
    }

    #[test]
    fn observe_anchor_persists_only_established_and_repinned() {
        let dir = tempfile::tempdir().unwrap();
        let mut map = HttpSessionMap::new(Duration::from_secs(60), 8)
            .with_persist_dir(Some(dir.path().to_path_buf()));
        let id = map.mint().unwrap();
        let path = dir.path().join(format!("{id}.json"));

        let obs = sample_anchor("acme", "bnd_acme", "sess_1");
        // allow_establish=false never establishes (and never writes an anchor).
        assert_eq!(map.observe_anchor(&id, &obs, false), None);
        assert!(map.anchor(&id).is_none());

        assert_eq!(
            map.observe_anchor(&id, &obs, true),
            Some(AnchorDecision::Established)
        );
        let after_establish = fs::read_to_string(&path).unwrap();
        assert!(after_establish.contains("\"anchor\""));

        // Match does not rewrite the record.
        assert_eq!(
            map.observe_anchor(&id, &obs, true),
            Some(AnchorDecision::Match)
        );

        // Same identity, new session id → Repinned, persisted session_id.
        let repin = sample_anchor("acme", "bnd_acme", "sess_2");
        assert_eq!(
            map.observe_anchor(&id, &repin, true),
            Some(AnchorDecision::Repinned)
        );
        let rec: HttpSessionDiskRecord =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(rec.anchor.as_ref().unwrap().session_id, "sess_2");

        // Mismatch never rotates or rewrites the anchor.
        let evil = sample_anchor("beta", "bnd_beta", "sess_3");
        match map.observe_anchor(&id, &evil, true) {
            Some(AnchorDecision::Mismatch { anchored }) => {
                assert_eq!(anchored.binding_alias, "acme");
            }
            other => panic!("expected mismatch, got {other:?}"),
        }
        let rec: HttpSessionDiskRecord =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(rec.anchor.as_ref().unwrap().binding_alias, "acme");

        // Unknown session id → fail closed.
        assert_eq!(
            map.observe_anchor("ffffffffffffffffffffffffffffffff", &obs, true),
            None
        );
    }

    #[test]
    fn note_mismatch_dedupes_per_session_pair() {
        let mut map = HttpSessionMap::new(Duration::from_secs(60), 8);
        let id = map.mint().unwrap();
        let key = ("anchored-sess".to_string(), "current-sess".to_string());
        assert!(map.note_mismatch(&id, key.clone()), "first report");
        assert!(!map.note_mismatch(&id, key), "repeat suppressed");
        let key2 = ("anchored-sess".to_string(), "another-sess".to_string());
        assert!(map.note_mismatch(&id, key2), "new pair reports again");
        assert!(!map.note_mismatch("ffffffffffffffffffffffffffffffff", ("a".into(), "b".into())));
    }
}

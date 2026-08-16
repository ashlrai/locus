//! Shared disk-layout rule for HTTP `Mcp-Session-Id` records.
//!
//! `locus-mcp` owns the record format and lifecycle; this module owns only the
//! **path rule** and default idle TTL so operator surfaces (`locus mcp list` /
//! `locus mcp revoke`) reconcile against the exact directories the server
//! writes — including the `LOCUS_MCP_SESSION_DIR` override and the hard `-mt`
//! partition for multi-tenant records.

use std::path::PathBuf;
use std::time::Duration;

/// Default idle TTL for HTTP sessions. The server map and any operator-side
/// liveness math must agree on this value.
pub const DEFAULT_HTTP_SESSION_TTL: Duration = Duration::from_secs(30 * 60);

/// Single-tenant session dir.
///
/// Priority: `LOCUS_MCP_SESSION_DIR` (non-empty) → `$LOCUS_HOME/http-sessions`.
/// Returns `None` only when home cannot be resolved.
pub fn http_session_dir() -> Option<PathBuf> {
    resolve_session_dir(env_override().as_deref(), false)
}

/// Multi-tenant session dir — a HARD partition from the single-tenant dir so
/// a single-tenant binary can never resume tenant-bound records and vice
/// versa. `LOCUS_MCP_SESSION_DIR` is honored with an `-mt` suffix.
pub fn http_session_dir_mt() -> Option<PathBuf> {
    resolve_session_dir(env_override().as_deref(), true)
}

/// Is a disk record past the idle TTL, judged on unix seconds? Clock skew
/// (`last_seen` in the future) reads as *not* expired — same rule as the
/// server's in-memory map.
pub fn http_session_record_expired(last_seen_unix: u64, now_unix: u64, ttl: Duration) -> bool {
    now_unix
        .checked_sub(last_seen_unix)
        .is_some_and(|age| age >= ttl.as_secs())
}

fn env_override() -> Option<String> {
    std::env::var("LOCUS_MCP_SESSION_DIR").ok()
}

fn resolve_session_dir(override_dir: Option<&str>, mt: bool) -> Option<PathBuf> {
    if let Some(dir) = override_dir {
        let t = dir.trim();
        if !t.is_empty() {
            return Some(if mt {
                PathBuf::from(format!("{t}-mt"))
            } else {
                PathBuf::from(t)
            });
        }
    }
    crate::store::locus_home().ok().map(|h| {
        h.join(if mt {
            "http-sessions-mt"
        } else {
            "http-sessions"
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_dir_wins_and_mt_gets_suffix() {
        assert_eq!(
            resolve_session_dir(Some("/tmp/x"), false),
            Some(PathBuf::from("/tmp/x"))
        );
        assert_eq!(
            resolve_session_dir(Some("/tmp/x"), true),
            Some(PathBuf::from("/tmp/x-mt"))
        );
    }

    #[test]
    fn blank_override_falls_back_to_home_partitions() {
        for raw in [None, Some("  ")] {
            let st = resolve_session_dir(raw, false);
            let mt = resolve_session_dir(raw, true);
            if let (Some(st), Some(mt)) = (st, mt) {
                assert!(st.ends_with("http-sessions"));
                assert!(mt.ends_with("http-sessions-mt"));
                assert_ne!(st, mt);
            }
        }
    }

    #[test]
    fn expiry_is_ttl_on_unix_seconds_with_skew_grace() {
        let ttl = Duration::from_secs(60);
        assert!(!http_session_record_expired(1000, 1059, ttl));
        assert!(http_session_record_expired(1000, 1060, ttl));
        // Clock skew: last_seen in the future is not expired.
        assert!(!http_session_record_expired(2000, 1000, ttl));
    }
}

//! Opt-in session budget caps (`[budget]` in `config.toml`).
//!
//! Stops an agent session after `max_actions` tool calls or `max_minutes`
//! minutes. Off by default: with no `[budget]` configured nothing is read from
//! or written to disk and every call behaves as before.
//!
//! Scope and limits, stated plainly:
//! - It counts MCP `tools/call` requests that reach the policy gate for a
//!   session (denied and approval-pending calls count too, because a looping
//!   agent is what a cap is for). Control-plane operations a human runs in the
//!   CLI are not counted.
//! - The minutes window starts at the session's first counted call, not at pin
//!   time (use `max_ttl` for a wall-clock pin expiry).
//! - Exhaustion is sticky for that session and is recorded once in the audit
//!   log as `budget.exhausted`. Nothing is auto-applied or auto-undone: the
//!   session keeps its pin and seal, calls are simply refused. A human lifts the
//!   stop by raising or removing the cap, or by pinning a fresh session.
//! - It never touches credentials, sealing, or approval authority.

use crate::config::BudgetConfig;
use crate::error::{LocusError, Result};
use crate::store::Store;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;

/// Persisted per-session counters (`$LOCUS_HOME/budgets/<session_id>.json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BudgetState {
    pub session_id: String,
    /// Time of the first counted call.
    pub started_at: DateTime<Utc>,
    /// Calls counted so far.
    pub actions: u64,
    /// Set once a cap trips; later calls are refused without further counting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped: Option<String>,
}

/// Result of charging one call against the budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetVerdict {
    /// No cap configured, or the call is within budget.
    Allowed,
    /// A cap tripped (now or earlier); the call must be refused.
    Exhausted { reason: String },
}

fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn state_path(store: &Store, session_id: &str) -> PathBuf {
    store
        .home()
        .join("budgets")
        .join(format!("{session_id}.json"))
}

/// Count one call for `session_id` and say whether it may proceed.
///
/// `now` is injected so tests do not sleep. Returns `Allowed` immediately, with
/// no I/O, when `cfg` has no cap. Fails closed (refuses the call) when a cap is
/// configured but the counter cannot be read or written, because an unreadable
/// counter is not evidence of budget left.
pub fn charge(
    store: &Store,
    cfg: &BudgetConfig,
    session_id: &str,
    now: DateTime<Utc>,
) -> Result<BudgetVerdict> {
    if !cfg.is_enabled() {
        return Ok(BudgetVerdict::Allowed);
    }
    if !valid_session_id(session_id) {
        return Ok(BudgetVerdict::Exhausted {
            reason: "session id is not usable for budget tracking".into(),
        });
    }
    let dir = store.home().join("budgets");
    fs::create_dir_all(&dir)?;
    let path = state_path(store, session_id);

    // One lock per session file serializes concurrent calls.
    let mut f = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)?;
    f.lock()?;
    let mut raw = String::new();
    f.read_to_string(&mut raw)?;
    let mut state: BudgetState = if raw.trim().is_empty() {
        BudgetState {
            session_id: session_id.to_string(),
            started_at: now,
            actions: 0,
            stopped: None,
        }
    } else {
        serde_json::from_str(&raw).map_err(|e| {
            LocusError::msg(format!("budget state for {session_id} is unreadable: {e}"))
        })?
    };

    let mut newly_stopped = false;
    if state.stopped.is_none() {
        if let Some(cap) = cfg.actions_cap() {
            if state.actions >= cap {
                state.stopped = Some(format!("action budget of {cap} tool calls used"));
            }
        }
        if state.stopped.is_none() {
            if let Some(mins) = cfg.minutes_cap() {
                let elapsed = now.signed_duration_since(state.started_at);
                if elapsed >= chrono::Duration::minutes(i64::try_from(mins).unwrap_or(i64::MAX)) {
                    state.stopped = Some(format!("time budget of {mins} minutes used"));
                }
            }
        }
        if state.stopped.is_some() {
            newly_stopped = true;
        } else {
            state.actions += 1;
        }
    }

    let bytes = serde_json::to_vec_pretty(&state)
        .map_err(|e| LocusError::msg(format!("budget state encode failed: {e}")))?;
    f.set_len(0)?;
    std::io::Seek::seek(&mut f, std::io::SeekFrom::Start(0))?;
    f.write_all(&bytes)?;
    f.sync_all()?;
    drop(f);

    match &state.stopped {
        None => Ok(BudgetVerdict::Allowed),
        Some(reason) => {
            if newly_stopped {
                // Best effort: an audit write failure must not turn a refusal
                // into an allow.
                let _ = store.audit(
                    "budget.exhausted",
                    "",
                    Some(serde_json::json!({
                        "session_id": session_id,
                        "actions": state.actions,
                        "reason": reason,
                    })),
                );
            }
            Ok(BudgetVerdict::Exhausted {
                reason: reason.clone(),
            })
        }
    }
}

/// Read the current counters for a session without charging (`None` when the
/// session has none).
pub fn peek(store: &Store, session_id: &str) -> Option<BudgetState> {
    if !valid_session_id(session_id) {
        return None;
    }
    let raw = fs::read_to_string(state_path(store, session_id)).ok()?;
    serde_json::from_str(&raw).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use tempfile::tempdir;

    fn cfg(actions: Option<u64>, minutes: Option<u64>) -> BudgetConfig {
        BudgetConfig {
            max_actions: actions,
            max_minutes: minutes,
        }
    }

    #[test]
    fn no_cap_means_no_io_and_always_allowed() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let t = Utc::now();
        for _ in 0..50 {
            assert_eq!(
                charge(&store, &cfg(None, None), "ses_a", t).unwrap(),
                BudgetVerdict::Allowed
            );
        }
        assert!(!dir.path().join("budgets").exists());
    }

    #[test]
    fn zero_is_treated_as_unset() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let c = cfg(Some(0), Some(0));
        assert!(!c.is_enabled());
        assert_eq!(
            charge(&store, &c, "ses_a", Utc::now()).unwrap(),
            BudgetVerdict::Allowed
        );
    }

    #[test]
    fn stops_after_n_actions_and_stays_stopped() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let c = cfg(Some(3), None);
        let t = Utc::now();
        for _ in 0..3 {
            assert_eq!(
                charge(&store, &c, "ses_a", t).unwrap(),
                BudgetVerdict::Allowed
            );
        }
        for _ in 0..2 {
            assert!(matches!(
                charge(&store, &c, "ses_a", t).unwrap(),
                BudgetVerdict::Exhausted { .. }
            ));
        }
        // Refused calls are not counted.
        assert_eq!(peek(&store, "ses_a").unwrap().actions, 3);
    }

    #[test]
    fn stops_after_n_minutes_from_first_call() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let c = cfg(None, Some(10));
        let t0 = Utc::now();
        assert_eq!(
            charge(&store, &c, "ses_a", t0).unwrap(),
            BudgetVerdict::Allowed
        );
        assert_eq!(
            charge(&store, &c, "ses_a", t0 + Duration::minutes(9)).unwrap(),
            BudgetVerdict::Allowed
        );
        let v = charge(&store, &c, "ses_a", t0 + Duration::minutes(10)).unwrap();
        assert!(
            matches!(v, BudgetVerdict::Exhausted { ref reason } if reason.contains("10 minutes"))
        );
        // Sticky even if the clock is read as earlier again.
        assert!(matches!(
            charge(&store, &c, "ses_a", t0).unwrap(),
            BudgetVerdict::Exhausted { .. }
        ));
    }

    #[test]
    fn sessions_are_independent() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let c = cfg(Some(1), None);
        let t = Utc::now();
        assert_eq!(
            charge(&store, &c, "ses_a", t).unwrap(),
            BudgetVerdict::Allowed
        );
        assert!(matches!(
            charge(&store, &c, "ses_a", t).unwrap(),
            BudgetVerdict::Exhausted { .. }
        ));
        assert_eq!(
            charge(&store, &c, "ses_b", t).unwrap(),
            BudgetVerdict::Allowed
        );
    }

    #[test]
    fn a_tripped_stop_is_sticky_even_if_the_cap_is_raised_later() {
        // Documented behavior: a tripped stop is sticky for that session.
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let t = Utc::now();
        assert_eq!(
            charge(&store, &cfg(Some(1), None), "ses_a", t).unwrap(),
            BudgetVerdict::Allowed
        );
        assert!(matches!(
            charge(&store, &cfg(Some(1), None), "ses_a", t).unwrap(),
            BudgetVerdict::Exhausted { .. }
        ));
        assert!(matches!(
            charge(&store, &cfg(Some(100), None), "ses_a", t).unwrap(),
            BudgetVerdict::Exhausted { .. }
        ));
    }

    #[test]
    fn unusable_session_id_fails_closed_without_touching_paths() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        for bad in ["", "../x", "a/b", "ses a"] {
            assert!(matches!(
                charge(&store, &cfg(Some(5), None), bad, Utc::now()).unwrap(),
                BudgetVerdict::Exhausted { .. }
            ));
        }
        assert!(!dir.path().join("x.json").exists());
    }

    #[test]
    fn corrupt_state_fails_closed() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        fs::create_dir_all(dir.path().join("budgets")).unwrap();
        fs::write(dir.path().join("budgets").join("ses_a.json"), "{not json").unwrap();
        assert!(charge(&store, &cfg(Some(5), None), "ses_a", Utc::now()).is_err());
    }

    #[test]
    fn exhaustion_is_audited_once() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let c = cfg(Some(1), None);
        let t = Utc::now();
        let _ = charge(&store, &c, "ses_a", t).unwrap();
        for _ in 0..3 {
            let _ = charge(&store, &c, "ses_a", t).unwrap();
        }
        let log = fs::read_to_string(store.audit_path()).unwrap_or_default();
        assert_eq!(log.matches("budget.exhausted").count(), 1);
    }
}

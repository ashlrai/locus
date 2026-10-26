//! `locus autopin suggest`: learn a repo's usual binding from local pin history.
//!
//! The current repo is matched by keyed hash only: each live git remote is
//! normalized with [`normalize_remote_url`] (which drops `user:token@`) and
//! hashed with the per-install history key, exactly as [`pin_history`] records
//! it. The raw remote URL is never stored, printed or returned.
//!
//! Ranking is deterministic and model-free. For each alias seen in matching
//! entries: `score = pins - mismatches` where `pins` counts `pin` events with
//! that alias and `mismatches` counts `pin_mismatch` refusals where that alias
//! was the one pinned (the wrong account for this repo). A suggestion is made
//! only when the top score is at least [`MIN_SCORE`] and strictly greater than
//! the runner-up; thin or tied evidence suggests nothing.
//!
//! Entries whose alias fails binding-name validation (for example a refusal
//! recorded as `<invalid alias>`) are dropped and never rank.
//!
//! Remote spoofing: a repo controls its own remotes, so a clone that adds a
//! second remote mimicking `acme-corp/app` could borrow acme's history. When
//! the repo has an `origin` remote, only `origin` is matched; other remotes
//! are ignored. Without `origin`, every remote is matched. A repo whose
//! `origin` itself mimics another repo still matches that repo's history:
//! remotes are not authenticated. `--apply` needs the operator control
//! capability and prints the binding and the file it wrote, so check both.
//!
//! Read-only: `suggest` never creates `history.key`. With no key there is no
//! history that could match, so it reports `no_history` and writes nothing.
//!
//! Writing the suggestion (`--apply`) lives on [`crate::store::Store`] behind
//! the control capability gate.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

use crate::autopin::{git_remote_urls, normalize_remote_url};
use crate::binding::validate_name_component;
use crate::pin_history::{self, HistoryEntry};

/// Stable JSON schema id for `locus autopin suggest --json`.
pub const SCHEMA: &str = "locus.autopin.suggest.v1";
/// Minimum net score (pins minus mismatches) before anything is suggested.
pub const MIN_SCORE: i64 = 3;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Candidate {
    pub alias: String,
    pub pins: u64,
    pub mismatches: u64,
    pub score: i64,
    /// RFC 3339 timestamp of the latest `pin` or `pin_mismatch` for this alias.
    pub last_seen: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SuggestReport {
    pub schema: &'static str,
    pub history_enabled: bool,
    /// True when the current directory has at least one git remote.
    pub repo_detected: bool,
    /// Matching history entries considered (all events).
    pub matched_entries: u64,
    pub suggestion: Option<String>,
    /// Machine-readable reason: `suggested` | `history_disabled` | `no_remote`
    /// | `no_history` | `thin_evidence` | `tied`.
    pub reason: &'static str,
    pub min_score: i64,
    pub candidates: Vec<Candidate>,
}

impl SuggestReport {
    fn empty(history_enabled: bool, repo_detected: bool, reason: &'static str) -> Self {
        Self {
            schema: SCHEMA,
            history_enabled,
            repo_detected,
            matched_entries: 0,
            suggestion: None,
            reason,
            min_score: MIN_SCORE,
            candidates: Vec::new(),
        }
    }
}

/// The URL of the `origin` remote of `cwd`, if any.
fn git_origin_url(cwd: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let u = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!u.is_empty()).then_some(u)
}

/// Keyed hashes of the live remotes of `cwd` used for matching, computed now:
/// `origin` alone when it exists, else every remote (see module docs).
pub fn live_remote_hashes(key: &[u8], cwd: &Path) -> Vec<String> {
    let urls = match git_origin_url(cwd) {
        Some(u) => vec![u],
        None => git_remote_urls(cwd),
    };
    let mut r: Vec<String> = urls
        .iter()
        .map(|u| pin_history::keyed_hash(key, &normalize_remote_url(u)))
        .collect();
    r.sort();
    r.dedup();
    r
}

/// Suggest a binding for `cwd` from `$LOCUS_HOME` pin history.
pub fn suggest(home: &Path, cwd: &Path) -> std::io::Result<SuggestReport> {
    if !pin_history::enabled(home) {
        return Ok(SuggestReport::empty(false, false, "history_disabled"));
    }
    if git_remote_urls(cwd).is_empty() {
        return Ok(SuggestReport::empty(true, false, "no_remote"));
    }
    let Some(key) = pin_history::existing_history_key(home)? else {
        return Ok(SuggestReport::empty(true, true, "no_history"));
    };
    let hashes = live_remote_hashes(&key, cwd);
    Ok(rank(&pin_history::read(home), &hashes))
}

/// Pure ranker over history entries for a repo identified by `remote_hashes`.
pub fn rank(entries: &[HistoryEntry], remote_hashes: &[String]) -> SuggestReport {
    let matching: Vec<&HistoryEntry> = entries
        .iter()
        .filter(|e| e.remote_hashes.iter().any(|h| remote_hashes.contains(h)))
        .collect();
    let mut by_alias: BTreeMap<String, Candidate> = BTreeMap::new();
    for e in &matching {
        let Some(alias) = e.alias.as_deref() else {
            continue;
        };
        if validate_name_component("alias", alias).is_err() {
            continue;
        }
        let c = by_alias
            .entry(alias.to_string())
            .or_insert_with(|| Candidate {
                alias: alias.to_string(),
                pins: 0,
                mismatches: 0,
                score: 0,
                last_seen: None,
            });
        match e.event.as_str() {
            "pin" => c.pins += 1,
            "pin_mismatch" => c.mismatches += 1,
            _ => continue,
        }
        if c.last_seen.as_deref().is_none_or(|t| e.ts.as_str() > t) {
            c.last_seen = Some(e.ts.clone());
        }
    }
    let mut candidates: Vec<Candidate> = by_alias
        .into_values()
        .filter(|c| c.pins + c.mismatches > 0)
        .map(|mut c| {
            c.score = c.pins as i64 - c.mismatches as i64;
            c
        })
        .collect();
    candidates.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then(b.pins.cmp(&a.pins))
            .then(b.last_seen.cmp(&a.last_seen))
            .then(a.alias.cmp(&b.alias))
    });
    let mut report = SuggestReport::empty(true, true, "no_history");
    report.matched_entries = matching.len() as u64;
    if let Some(top) = candidates.first() {
        let runner_up = candidates.get(1).map(|c| c.score);
        report.reason = if top.score < MIN_SCORE {
            "thin_evidence"
        } else if runner_up == Some(top.score) {
            "tied"
        } else {
            report.suggestion = Some(top.alias.clone());
            "suggested"
        };
    }
    report.candidates = candidates;
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(event: &str, alias: &str, ts: &str, hash: &str) -> HistoryEntry {
        HistoryEntry {
            ts: ts.into(),
            event: event.into(),
            alias: Some(alias.into()),
            expected: None,
            workspace_hash: None,
            remote_hashes: vec![hash.into()],
        }
    }

    fn here() -> Vec<String> {
        vec!["h1".into()]
    }

    #[test]
    fn suggests_clear_winner_with_evidence() {
        let mut v = Vec::new();
        for i in 0..4 {
            v.push(ev("pin", "acme", &format!("2026-10-2{i}T00:00:00Z"), "h1"));
        }
        v.push(ev("pin", "personal", "2026-10-25T00:00:00Z", "h1"));
        v.push(ev("pin", "personal", "2026-10-25T00:00:00Z", "other"));
        let r = rank(&v, &here());
        assert_eq!(r.suggestion.as_deref(), Some("acme"));
        assert_eq!(r.reason, "suggested");
        assert_eq!(r.matched_entries, 5);
        assert_eq!(r.candidates[0].pins, 4);
        assert_eq!(
            r.candidates[0].last_seen.as_deref(),
            Some("2026-10-23T00:00:00Z")
        );
        assert_eq!(r.candidates[1].alias, "personal");
        assert_eq!(r.candidates[1].pins, 1);
    }

    #[test]
    fn thin_evidence_suggests_nothing() {
        let v = vec![
            ev("pin", "acme", "2026-10-20T00:00:00Z", "h1"),
            ev("pin", "acme", "2026-10-21T00:00:00Z", "h1"),
        ];
        let r = rank(&v, &here());
        assert_eq!(r.suggestion, None);
        assert_eq!(r.reason, "thin_evidence");
    }

    #[test]
    fn tie_suggests_nothing() {
        let mut v = Vec::new();
        for a in ["acme", "personal"] {
            for i in 0..3 {
                v.push(ev("pin", a, &format!("2026-10-2{i}T00:00:00Z"), "h1"));
            }
        }
        let r = rank(&v, &here());
        assert_eq!(r.suggestion, None);
        assert_eq!(r.reason, "tied");
        // Deterministic order on ties: alias ascending.
        assert_eq!(r.candidates[0].alias, "acme");
    }

    #[test]
    fn mismatches_count_against_the_pinned_alias() {
        let mut v = Vec::new();
        for i in 0..4 {
            v.push(ev(
                "pin",
                "personal",
                &format!("2026-10-2{i}T00:00:00Z"),
                "h1",
            ));
        }
        for i in 0..3 {
            v.push(ev("pin", "acme", &format!("2026-10-2{i}T01:00:00Z"), "h1"));
        }
        for i in 0..2 {
            let mut m = ev(
                "pin_mismatch",
                "personal",
                &format!("2026-10-2{i}T02:00:00Z"),
                "h1",
            );
            m.expected = Some("acme".into());
            v.push(m);
        }
        let r = rank(&v, &here());
        assert_eq!(r.suggestion.as_deref(), Some("acme"));
        let p = r.candidates.iter().find(|c| c.alias == "personal").unwrap();
        assert_eq!((p.pins, p.mismatches, p.score), (4, 2, 2));
    }

    #[test]
    fn invalid_aliases_never_rank() {
        let mut v = Vec::new();
        for i in 0..5 {
            let ts = format!("2026-10-2{i}T00:00:00Z");
            v.push(ev("pin", "<invalid alias>", &ts, "h1"));
            v.push(ev("pin", "../etc", &ts, "h1"));
        }
        let r = rank(&v, &here());
        assert_eq!(r.suggestion, None);
        assert_eq!(r.reason, "no_history");
        assert!(r.candidates.is_empty());
    }

    #[test]
    fn leave_events_and_other_repos_are_ignored() {
        let mut v = Vec::new();
        for i in 0..5 {
            v.push(ev(
                "leave",
                "acme",
                &format!("2026-10-2{i}T00:00:00Z"),
                "h1",
            ));
            v.push(ev(
                "pin",
                "acme",
                &format!("2026-10-2{i}T00:00:00Z"),
                "nope",
            ));
        }
        let r = rank(&v, &here());
        assert_eq!(r.suggestion, None);
        assert_eq!(r.reason, "no_history");
        assert!(r.candidates.is_empty());
    }
}

//! Value-free command outcomes: what recent `exec` / `ci run` calls did.
//!
//! Read-only like `fleet-denials`: resolves the audit log and approvals
//! directory without opening the store, so `LOCUS_HOME` is never initialized.
//! Only fixed kinds, exit codes and numeric positions are emitted — never
//! commands, bindings, timestamps or spawn error text.
//!
//! Missing data is explicit, not guessed: no log means `"log": "missing"` and
//! empty lists; an outcome without an exit code (killed by a signal, or an
//! old record) is `"unknown"`; `pending_approvals` is `null` when the
//! approvals directory cannot be read.
use locus_core::{audit_events_path, locus_home, ApprovalStatus, AuditEvent};
use serde::Serialize;
use std::collections::VecDeque;
use std::io::BufRead;
use std::path::Path;

/// Parsed audit records inspected per call (the most recent ones).
const WINDOW: usize = 100;

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
enum Result {
    Succeeded,
    Failed,
    SpawnFailed,
    Unknown,
}

#[derive(Serialize)]
struct Outcome {
    position: usize,
    kind: &'static str,
    result: Result,
    exit: Option<i64>,
}

#[derive(Serialize, Default)]
struct Counts {
    succeeded: usize,
    failed: usize,
    spawn_failed: usize,
    unknown: usize,
}

#[derive(Serialize)]
struct Feed {
    schema: &'static str,
    log: &'static str,
    outcomes: Vec<Outcome>,
    counts: Counts,
    pending_approvals: Option<usize>,
}

fn kind(op: &str) -> Option<&'static str> {
    match op {
        "session.exec" => Some("exec"),
        "ci.run" => Some("ci"),
        _ => None,
    }
}

fn result_of(event: &AuditEvent) -> (Result, Option<i64>) {
    let detail = event.detail.as_ref();
    let spawn_failed = detail
        .and_then(|d| d.get("spawn_error"))
        .is_some_and(|v| !v.is_null());
    let exit = detail.and_then(|d| d.get("exit")).and_then(|v| v.as_i64());
    let result = match (spawn_failed, exit) {
        (true, _) => Result::SpawnFailed,
        (false, Some(0)) => Result::Succeeded,
        (false, Some(_)) => Result::Failed,
        (false, None) => Result::Unknown,
    };
    (result, exit)
}

/// Pending approval records, or `None` when the directory cannot be listed.
/// A missing directory is a real zero, not unknown data.
fn pending_approvals(home: &Path) -> Option<usize> {
    let dir = home.join("approvals");
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(0),
        Err(_) => return None,
    };
    Some(
        entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("json"))
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .filter_map(|raw| serde_json::from_str::<locus_core::ApprovalRecord>(&raw).ok())
            .filter(|r| r.status == ApprovalStatus::Pending)
            .count(),
    )
}

/// One pass: keep outcomes among the last `WINDOW` parsed records.
fn scan(path: &Path) -> anyhow::Result<Option<Vec<Outcome>>> {
    let unavailable = || anyhow::anyhow!("outcome feed unavailable");
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(unavailable()),
    };
    let mut parsed = 0usize;
    let mut recent = VecDeque::with_capacity(WINDOW);
    for line in std::io::BufReader::new(file).lines() {
        let line = line.map_err(|_| unavailable())?;
        let Ok(event) = serde_json::from_str::<AuditEvent>(line.trim()) else {
            continue;
        };
        parsed += 1;
        let floor = parsed.saturating_sub(WINDOW);
        while recent
            .front()
            .is_some_and(|o: &Outcome| o.position <= floor)
        {
            recent.pop_front();
        }
        if let Some(kind) = kind(&event.op) {
            let (result, exit) = result_of(&event);
            recent.push_back(Outcome {
                position: parsed,
                kind,
                result,
                exit,
            });
        }
    }
    Ok(Some(recent.into()))
}

pub(crate) fn run() -> anyhow::Result<()> {
    let home = locus_home().map_err(|_| anyhow::anyhow!("outcome feed unavailable"))?;
    let scanned = scan(&audit_events_path(&home))?;
    let log = if scanned.is_some() {
        "present"
    } else {
        "missing"
    };
    let outcomes = scanned.unwrap_or_default();
    let mut counts = Counts::default();
    for o in &outcomes {
        match o.result {
            Result::Succeeded => counts.succeeded += 1,
            Result::Failed => counts.failed += 1,
            Result::SpawnFailed => counts.spawn_failed += 1,
            Result::Unknown => counts.unknown += 1,
        }
    }
    println!(
        "{}",
        serde_json::to_string(&Feed {
            schema: super::json_out::FLEET_OUTCOMES,
            log,
            outcomes,
            counts,
            pending_approvals: pending_approvals(&home),
        })?
    );
    Ok(())
}

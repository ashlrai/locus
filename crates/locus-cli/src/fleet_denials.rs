//! Value-free projection: never serialize audit strings, including timestamps.
//!
//! Read-only: the log path is resolved without opening the store, so this
//! feed never initializes `LOCUS_HOME` (no directories, no seal key). The
//! log is scanned once — corrupt lines skipped exactly like
//! `Store::read_audit_events` — retaining only fixed denial kinds and their
//! 1-based positions among parsed records.
use locus_core::{audit_events_path, locus_home, AuditEvent};
use serde::Serialize;
use std::io::BufRead;

/// Parsed audit records scanned per call (default tail window; `--cursor` page).
const WINDOW: usize = 100;

#[derive(Serialize)]
struct Denial {
    position: usize,
    kind: &'static str,
}

#[derive(Serialize)]
struct Feed {
    schema: &'static str,
    denials: Vec<Denial>,
    next_cursor: usize,
    has_more: bool,
}

fn classify(op: &str) -> Option<&'static str> {
    if op.contains("scope_freeze") {
        Some("scope_freeze")
    } else if op.ends_with(".deny")
        || op.contains("denied")
        || op.ends_with(".refused")
        || op.ends_with(".require_approval")
    {
        Some("command_refused")
    } else {
        None
    }
}

/// One pass over the log: count parsed records, keep only denial positions.
/// With a cursor, stop once the page is provably full — later lines change
/// nothing the feed reports.
fn scan(cursor: Option<usize>) -> anyhow::Result<(usize, Vec<Denial>)> {
    let unavailable = || anyhow::anyhow!("denial feed unavailable");
    let path = audit_events_path(&locus_home().map_err(|_| unavailable())?);
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((0, Vec::new())),
        Err(_) => return Err(unavailable()),
    };
    let page_end = cursor.map(|c| c.saturating_add(WINDOW));
    let mut parsed = 0usize;
    let mut denials = Vec::new();
    for line in std::io::BufReader::new(file).lines() {
        let line = line.map_err(|_| unavailable())?;
        let Ok(event) = serde_json::from_str::<AuditEvent>(line.trim()) else {
            continue;
        };
        parsed += 1;
        if let Some(kind) = classify(&event.op) {
            denials.push(Denial {
                position: parsed,
                kind,
            });
        }
        if page_end.is_some_and(|end| parsed > end) {
            break;
        }
    }
    Ok((parsed, denials))
}

pub(crate) fn run(cursor: Option<usize>) -> anyhow::Result<()> {
    // Suppress underlying paths and parse details on this value-free surface.
    let (parsed, mut denials) = scan(cursor)?;
    let start = cursor.unwrap_or_else(|| parsed.saturating_sub(WINDOW));
    anyhow::ensure!(
        start <= parsed,
        "denial cursor out of range; restart without cursor"
    );
    let end = start.saturating_add(WINDOW).min(parsed);
    denials.retain(|d| start < d.position && d.position <= end);
    println!(
        "{}",
        serde_json::to_string(&Feed {
            schema: super::json_out::FLEET_DENIALS,
            denials,
            next_cursor: end,
            has_more: end < parsed,
        })?
    );
    Ok(())
}

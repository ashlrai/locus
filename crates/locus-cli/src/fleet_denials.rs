//! Value-free projection: never serialize audit strings, including timestamps.
use serde::Serialize;

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

pub(crate) fn run(cursor: Option<usize>) -> anyhow::Result<()> {
    // Suppress underlying paths and parse details on this value-free surface.
    let events = super::store()
        .and_then(|store| store.read_audit_events().map_err(Into::into))
        .map_err(|_| anyhow::anyhow!("denial feed unavailable"))?;
    let start = cursor.unwrap_or_else(|| events.len().saturating_sub(100));
    anyhow::ensure!(
        start <= events.len(),
        "denial cursor out of range; restart without cursor"
    );
    let end = start.saturating_add(100).min(events.len());
    let denials = events[start..end]
        .iter()
        .enumerate()
        .filter_map(|(index, event)| {
            let kind = if event.op.contains("scope_freeze") {
                "scope_freeze"
            } else if event.op.ends_with(".deny")
                || event.op.contains("denied")
                || event.op.ends_with(".refused")
                || event.op.ends_with(".require_approval")
            {
                "command_refused"
            } else {
                return None;
            };
            Some(Denial {
                position: start + index + 1,
                kind,
            })
        })
        .collect();
    println!(
        "{}",
        serde_json::to_string(&Feed {
            schema: super::json_out::FLEET_DENIALS,
            denials,
            next_cursor: end,
            has_more: end < events.len(),
        })?
    );
    Ok(())
}

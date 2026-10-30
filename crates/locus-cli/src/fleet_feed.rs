//! Read-only workbench resource observation. Never serialize sessions, bindings,
//! error messages, or caller-controlled identifiers into this surface.
use locus_core::Store;
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum State {
    Pinned,
    Unpinned,
    Unhealthy,
}

#[derive(Serialize)]
struct Feed {
    schema: &'static str,
    kind: &'static str,
    state: State,
    session_ok: bool,
}

pub(crate) fn run() -> anyhow::Result<()> {
    let state = match super::store() {
        Ok(store) => observe(&store),
        Err(_) => State::Unhealthy,
    };
    let feed = Feed {
        schema: super::json_out::FLEET_FEED,
        kind: "locus_session",
        session_ok: matches!(state, State::Pinned),
        state,
    };
    // Success means the observation was emitted, even for unhealthy sessions.
    println!("{}", serde_json::to_string(&feed)?);
    Ok(())
}

fn observe(store: &Store) -> State {
    match store.resolve_active_session() {
        Ok(None) if super::exact_session_selected() => State::Unhealthy,
        Ok(None) => State::Unpinned,
        Ok(Some(_)) => {
            // require_active rejects expiry/freeze; whoami also requires the
            // binding to exist. Both verify the seal and authority anchor.
            if store
                .require_active_resolved()
                .and_then(|resolved| store.whoami_for(&resolved))
                .is_ok()
            {
                State::Pinned
            } else {
                State::Unhealthy
            }
        }
        Err(_) => State::Unhealthy,
    }
}

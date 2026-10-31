//! Versioned `--json` envelopes for check / status / plan output.
//!
//! Every object these commands print starts with `"schema": "locus.<name>.vN"`.
//! The `.vN` suffix changes only when the shape breaks (a field removed,
//! renamed or retyped); new fields are additive and keep the id. The rest of
//! the document is the command's existing payload, field order unchanged, so
//! the stamp can never introduce a value the payload did not already carry.
//! MCP tool payloads and the dashboard API are not stamped.
//!
//! Catalogue: `docs/agent-contract.md` § JSON schemas (a test keeps it in sync).

use anyhow::{bail, Result};
use serde::Serialize;

pub(crate) const DOCTOR: &str = "locus.doctor.v1";
pub(crate) const DOCTOR_FIX: &str = "locus.doctor.fix.v1";
pub(crate) const FLEET_DENIALS: &str = "locus.fleet_denials.v1";
pub(crate) const FLEET_FEED: &str = "locus.fleet_feed.v1";
pub(crate) const WHOAMI: &str = "locus.whoami.v1";
pub(crate) const STATUS: &str = "locus.status.v1";
pub(crate) const VERIFY_SESSION: &str = "locus.verify.session.v1";
pub(crate) const VERIFY_CLAIM: &str = "locus.verify.claim.v1";
pub(crate) const AGENT_DOCTOR: &str = "locus.agent.doctor.v1";
pub(crate) const AGENT_REPORT: &str = "locus.agent.report.v1";
pub(crate) const WATCH: &str = "locus.watch.v1";
pub(crate) const GOAL_STATUS: &str = "locus.goal.status.v1";
pub(crate) const CAPABILITY_STATUS: &str = "locus.capability.status.v1";
pub(crate) const NOTIFY_STATUS: &str = "locus.notify.status.v1";
pub(crate) const APPROVE_STATUS: &str = "locus.approve.status.v1";
pub(crate) const AUTOPIN_SUGGEST: &str = locus_core::autopin_suggest::SCHEMA;
pub(crate) const OFFBOARD: &str = "locus.offboard.v1";
pub(crate) const ADAPTER_VERIFY: &str = "locus.adapter.verify.v1";
pub(crate) const ADAPTER_VERIFY_MANIFEST: &str = "locus.adapter.verify_manifest.v1";
pub(crate) const VERSION: &str = "locus.version.v1";
pub(crate) const PLAN: &str = "locus.plan.v1";
pub(crate) const ERROR: &str = "locus.error.v1";
pub(crate) const CONTRACT: &str = "locus.contract.v1";
pub(crate) const ASHLR_HANDSHAKE: &str = "locus.ashlr.handshake.v1";

/// `(command, schema id)` for every stamped surface, in docs order.
pub(crate) const CATALOGUE: &[(&str, &str)] = &[
    ("locus doctor", DOCTOR),
    ("locus doctor --fix [--apply]", DOCTOR_FIX),
    ("locus whoami", WHOAMI),
    ("locus fleet-feed (always JSON)", FLEET_FEED),
    ("locus fleet-denials (always JSON)", FLEET_DENIALS),
    ("locus status", STATUS),
    ("locus verify session", VERIFY_SESSION),
    ("locus verify claim", VERIFY_CLAIM),
    ("locus agent doctor", AGENT_DOCTOR),
    ("locus agent report", AGENT_REPORT),
    ("locus watch (one NDJSON line per tick)", WATCH),
    ("locus goal status", GOAL_STATUS),
    ("locus capability status", CAPABILITY_STATUS),
    ("locus notify status", NOTIFY_STATUS),
    ("locus approve status", APPROVE_STATUS),
    ("locus autopin suggest", AUTOPIN_SUGGEST),
    ("locus offboard --apply", OFFBOARD),
    ("locus adapter verify", ADAPTER_VERIFY),
    ("locus adapter verify-manifest", ADAPTER_VERIFY_MANIFEST),
    ("locus version", VERSION),
    ("any mutating command with --dry-run", PLAN),
    ("any command failing under --json (stderr)", ERROR),
    ("locus topic agent-contract", CONTRACT),
    ("locus --ashlr-handshake", ASHLR_HANDSHAKE),
];

#[derive(Serialize)]
struct Stamped<'a, T: Serialize + ?Sized> {
    schema: &'a str,
    #[serde(flatten)]
    body: &'a T,
}

/// `body` (a struct or JSON object) with `"schema"` as its first key.
///
/// Errors when `body` is not an object or already carries a `schema` key, so
/// a stamped document never has a duplicate or a silently replaced field.
pub(crate) fn stamp<T: Serialize + ?Sized>(schema: &str, body: &T) -> Result<serde_json::Value> {
    let probe = serde_json::to_value(body)?;
    let Some(obj) = probe.as_object() else {
        bail!("{schema}: JSON payload is not an object");
    };
    if obj.contains_key("schema") {
        bail!("{schema}: payload already has a schema key");
    }
    Ok(serde_json::to_value(Stamped { schema, body })?)
}

/// Serialize `body` with its schema stamp, pretty or as one line.
pub(crate) fn render<T: Serialize + ?Sized>(
    schema: &str,
    body: &T,
    pretty: bool,
) -> Result<String> {
    // Validate (object, no schema key) first; then serialize the wrapper
    // directly so struct field order survives (serde_json::Value would sort).
    stamp(schema, body)?;
    let wrapper = Stamped { schema, body };
    Ok(if pretty {
        serde_json::to_string_pretty(&wrapper)?
    } else {
        serde_json::to_string(&wrapper)?
    })
}

/// Print the stamped document on stdout (pretty).
pub(crate) fn print<T: Serialize + ?Sized>(schema: &str, body: &T) -> Result<()> {
    println!("{}", render(schema, body, true)?);
    Ok(())
}

/// Print the stamped document on stdout as one line (status, NDJSON ticks).
pub(crate) fn print_line<T: Serialize + ?Sized>(schema: &str, body: &T) -> Result<()> {
    println!("{}", render(schema, body, false)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct Report {
        zeta: u8,
        alpha: &'static str,
    }

    #[test]
    fn schema_is_first_and_struct_order_is_kept() {
        let out = render(
            DOCTOR,
            &Report {
                zeta: 1,
                alpha: "a",
            },
            false,
        )
        .unwrap();
        assert_eq!(out, r#"{"schema":"locus.doctor.v1","zeta":1,"alpha":"a"}"#);
    }

    #[test]
    fn stamp_never_invents_or_duplicates_a_value() {
        let canary = "sk-schema-canary-should-stay-put";
        let body = serde_json::json!({ "note": canary, "ok": false });
        let out = render(STATUS, &body, true).unwrap();
        assert_eq!(out.matches(canary).count(), 1);
        assert_eq!(out.matches("\"schema\"").count(), 1);
    }

    #[test]
    fn non_objects_and_existing_schema_keys_are_refused() {
        assert!(stamp(PLAN, &serde_json::json!([1, 2])).is_err());
        assert!(stamp(PLAN, &serde_json::json!({ "schema": "x" })).is_err());
    }

    #[test]
    fn catalogue_ids_are_unique_versioned_and_namespaced() {
        let mut ids: Vec<_> = CATALOGUE.iter().map(|(_, id)| *id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), CATALOGUE.len());
        for id in ids {
            assert!(id.starts_with("locus."), "{id}");
            let (_, version) = id.rsplit_once('.').unwrap();
            assert!(
                version.starts_with('v') && version[1..].parse::<u32>().is_ok(),
                "{id}"
            );
        }
    }

    #[test]
    fn every_catalogued_command_has_a_schema_field() {
        for (command, id) in CATALOGUE {
            let output = render(id, &serde_json::json!({ "ok": true }), false).unwrap();
            let value: serde_json::Value = serde_json::from_str(&output).unwrap();
            assert_eq!(value["schema"], *id, "{command}");
        }
    }
}

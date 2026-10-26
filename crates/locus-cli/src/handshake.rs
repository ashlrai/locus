//! Ashlr companion handshake v1 (`locus --ashlr-handshake`): one JSON object
//! naming the CLI, its protocols and capabilities, local health and the files
//! it may edit, so `phm companions` / `phm doctor` stop guessing from
//! `--help`. Contract: docs/ashlr-handshake.md (shared with ashlr-stack and
//! ashlr-plugin).
//!
//! Answered before clap parses anything and before the store is opened: no
//! network, no seal key, no bindings or credential values are read. Health is
//! two existence checks.

use serde_json::{json, Value};
use std::path::PathBuf;

pub const FLAG: &str = "--ashlr-handshake";

/// `$LOCUS_HOME`, else `~/.locus`.
fn locus_home() -> Option<PathBuf> {
    match std::env::var_os("LOCUS_HOME") {
        Some(h) if !h.is_empty() => Some(PathBuf::from(h)),
        _ => dirs::home_dir().map(|h| h.join(".locus")),
    }
}

pub fn build(version: &str, home_initialized: bool, phantom_on_path: bool) -> Value {
    let status = if home_initialized { "ok" } else { "degraded" };
    json!({
        "schema": crate::json_out::ASHLR_HANDSHAKE,
        "kind": "ashlr.handshake",
        "schema_version": 1,
        "name": "locus",
        "command": "locus",
        "version": version,
        "protocols": { "ashlr.handshake": 1, "phantom.agent.preflight": 1 },
        "capabilities": [
            "workspace-identity-pinning",
            "credential-bindings",
            "phm-references",
            "mcp-server",
            "agent-setup",
            "agent-preflight",
            "doctor-json",
            "doctor-fix"
        ],
        "health": {
            "status": status,
            "checks": [
                { "id": "locus-home", "status": if home_initialized { "ok" } else { "missing" } },
                // Only bindings that use phm: references need it.
                { "id": "phantom-on-path", "status": if phantom_on_path { "ok" } else { "absent" } }
            ]
        },
        "edits": [
            "~/.locus/",
            ".locus.toml",
            ".mcp.json",
            "~/.cursor/mcp.json",
            "~/.codex/config.toml",
            "~/.grok/config.toml",
            "~/.gemini/settings.json"
        ],
        "receipts": []
    })
}

/// Print the handshake when `args` (without argv[0]) contain the flag.
pub fn answer_if_requested(args: &[String], version: &str) -> bool {
    if !args.iter().any(|a| a == FLAG) {
        return false;
    }
    let home = locus_home().is_some_and(|h| h.join("seal.key").exists() || h.is_dir());
    let phantom = crate::find_on_path("phantom").is_some();
    println!("{}", build(version, home, phantom));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_the_contract() {
        let h = build("1.2.3", true, true);
        assert_eq!(h["kind"], "ashlr.handshake");
        assert_eq!(h["schema"], crate::json_out::ASHLR_HANDSHAKE);
        assert_eq!(h["schema_version"], 1);
        assert_eq!(h["name"], "locus");
        assert_eq!(h["version"], "1.2.3");
        assert_eq!(h["protocols"]["phantom.agent.preflight"], 1);
        assert_eq!(h["health"]["status"], "ok");
    }

    #[test]
    fn uninitialized_home_degrades_and_phantom_is_informational() {
        let h = build("1", false, true);
        assert_eq!(h["health"]["status"], "degraded");
        assert_eq!(h["health"]["checks"][0]["status"], "missing");
        let h = build("1", true, false);
        assert_eq!(h["health"]["status"], "ok");
        assert_eq!(h["health"]["checks"][1]["status"], "absent");
    }

    #[test]
    fn ids_are_kebab_case_and_edits_are_relative_or_home() {
        let h = build("1", true, true);
        let kebab = |s: &str| {
            !s.is_empty()
                && !s.starts_with('-')
                && !s.ends_with('-')
                && s.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        };
        for c in h["capabilities"].as_array().unwrap() {
            assert!(kebab(c.as_str().unwrap()), "{c}");
        }
        for c in h["health"]["checks"].as_array().unwrap() {
            assert!(kebab(c["id"].as_str().unwrap()));
        }
        for e in h["edits"].as_array().unwrap() {
            assert!(!e.as_str().unwrap().starts_with('/'));
        }
    }

    #[test]
    fn only_the_exact_flag_answers() {
        assert!(!answer_if_requested(&["doctor".into()], "1"));
        assert!(!answer_if_requested(&["--ashlr-handshake-x".into()], "1"));
    }
}

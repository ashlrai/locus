//! Opt-in Phantom agent preflight for `locus doctor`, following phantom-secrets
//! docs/agent-preflight-contract.md (schema_version 1). Runs
//! `phantom agent doctor --json --strict`, which reads only project files and
//! environment variables (no vault, no keychain). `--strict` exits 0 ready,
//! 2 degraded, 1 blocked, 3 could not evaluate.
//!
//! Off unless `LOCUS_PHANTOM_PREFLIGHT` is set: `1`/`true`/`on` refuses only on
//! `blocked`; `strict` refuses on anything but `ready`. A missing phantom, or
//! one without `agent doctor --json`, is only a warning. Output carries the
//! verdict and kebab-case check ids, never names, values or phantom's text.

use serde::Serialize;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Off,
    On,
    Strict,
}

pub fn mode_from(value: Option<&str>) -> Mode {
    match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Some("strict") => Mode::Strict,
        Some("1") | Some("true") | Some("on") => Mode::On,
        _ => Mode::Off,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Preflight {
    /// `ready`, `degraded`, `blocked`, `unknown`, or null when not evaluated.
    pub verdict: Option<String>,
    pub blocked_by: Vec<String>,
    pub refuse: bool,
    pub message: Option<String>,
}

/// Raw result of running phantom: exit status (None if killed or not run),
/// stdout, and whether the binary was missing.
pub struct Run {
    pub status: Option<i32>,
    pub stdout: String,
    pub missing: bool,
}

pub fn run_phantom() -> Run {
    match Command::new("phantom")
        .args(["agent", "doctor", "--json", "--strict"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    {
        Ok(out) => Run {
            status: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            missing: false,
        },
        Err(e) => Run {
            status: None,
            stdout: String::new(),
            missing: e.kind() == std::io::ErrorKind::NotFound,
        },
    }
}

fn is_id(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && !s.ends_with('-')
        && !s.contains("--")
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

pub fn evaluate(mode: Mode, run: impl FnOnce() -> Run) -> Option<Preflight> {
    if mode == Mode::Off {
        return None;
    }
    let r = run();
    let skipped = |m: &str| Preflight {
        verdict: None,
        blocked_by: Vec::new(),
        refuse: false,
        message: Some(m.to_string()),
    };
    if r.missing {
        return Some(skipped(
            "phantom preflight skipped: phantom is not installed",
        ));
    }
    let parsed = serde_json::from_str::<serde_json::Value>(&r.stdout)
        .ok()
        .and_then(|doc| {
            if doc.get("kind")?.as_str()? != "phantom.agent.preflight"
                || doc.get("schema_version")?.as_i64()? != 1
            {
                return None;
            }
            let verdict = doc.get("verdict")?.as_str()?.to_string();
            if !["ready", "degraded", "blocked", "unknown"].contains(&verdict.as_str()) {
                return None;
            }
            let ids = doc
                .get("wrap")
                .and_then(|w| w.get("blocked_by"))
                .and_then(|b| b.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .filter(|s| is_id(s))
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            Some((verdict, ids))
        });
    let Some((verdict, blocked_by)) = parsed.filter(|_| matches!(r.status, Some(0..=3))) else {
        return Some(skipped(
            "phantom preflight skipped: this phantom has no `agent doctor --json` (schema 1)",
        ));
    };
    let refuse = verdict == "blocked" || (mode == Mode::Strict && verdict != "ready");
    let message = match verdict.as_str() {
        "ready" => None,
        "blocked" if !blocked_by.is_empty() => Some(format!(
            "phantom preflight: blocked ({}). Run `phantom agent doctor` for details.",
            blocked_by.join(", ")
        )),
        v => Some(format!(
            "phantom preflight: {v}{}. Run `phantom agent doctor` for details.",
            if refuse { "" } else { ", continuing" }
        )),
    };
    Some(Preflight {
        verdict: Some(verdict),
        blocked_by,
        refuse,
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(verdict: &str, ids: &[&str]) -> String {
        serde_json::json!({
            "schema_version": 1, "kind": "phantom.agent.preflight", "verdict": verdict,
            "readiness": "protected", "risk_level": "medium", "vault_probed": false,
            "checks": [], "wrap": {"exec_ready": ids.is_empty(), "blocked_by": ids}
        })
        .to_string()
    }
    fn run(status: Option<i32>, stdout: String) -> impl FnOnce() -> Run {
        move || Run {
            status,
            stdout,
            missing: false,
        }
    }

    #[test]
    fn mode_is_off_by_default() {
        assert_eq!(mode_from(None), Mode::Off);
        assert_eq!(mode_from(Some("0")), Mode::Off);
        assert_eq!(mode_from(Some("yes")), Mode::Off);
        assert_eq!(mode_from(Some("1")), Mode::On);
        assert_eq!(mode_from(Some("Strict")), Mode::Strict);
    }

    #[test]
    fn off_never_runs_phantom() {
        assert!(evaluate(Mode::Off, || panic!("ran phantom")).is_none());
    }

    #[test]
    fn exit_codes_map_per_mode() {
        for (mode, code, verdict, refuse) in [
            (Mode::On, 0, "ready", false),
            (Mode::On, 2, "degraded", false),
            (Mode::On, 1, "blocked", true),
            (Mode::On, 3, "unknown", false),
            (Mode::Strict, 0, "ready", false),
            (Mode::Strict, 2, "degraded", true),
            (Mode::Strict, 1, "blocked", true),
            (Mode::Strict, 3, "unknown", true),
        ] {
            let p = evaluate(mode, run(Some(code), doc(verdict, &[]))).unwrap();
            assert_eq!(p.verdict.as_deref(), Some(verdict));
            assert_eq!(p.refuse, refuse, "{mode:?} {verdict}");
        }
    }

    #[test]
    fn blocked_reports_ids_only() {
        let p = evaluate(
            Mode::On,
            run(
                Some(1),
                doc(
                    "blocked",
                    &["unprotected-env-file", "OPENAI_API_KEY=sk-live"],
                ),
            ),
        )
        .unwrap();
        assert_eq!(p.blocked_by, vec!["unprotected-env-file".to_string()]);
        assert!(!p.message.unwrap().contains("sk-live"));
    }

    #[test]
    fn missing_or_old_phantom_only_warns() {
        let p = evaluate(Mode::Strict, || Run {
            status: None,
            stdout: String::new(),
            missing: true,
        })
        .unwrap();
        assert!(!p.refuse && p.verdict.is_none());
        for (code, out) in [
            (
                Some(2),
                "error: unrecognized subcommand sk-live-x".to_string(),
            ),
            (
                Some(0),
                r#"{"schema_version":2,"kind":"phantom.agent.preflight","verdict":"ready"}"#
                    .to_string(),
            ),
            (Some(101), doc("ready", &[])),
            (None, doc("blocked", &[])),
        ] {
            let p = evaluate(Mode::Strict, run(code, out)).unwrap();
            assert!(!p.refuse && p.verdict.is_none());
            assert!(!p.message.unwrap().contains("sk-live"));
        }
    }
}

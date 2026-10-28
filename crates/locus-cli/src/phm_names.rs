//! Value-free Phantom name listing for `locus doctor --fix` only.
//!
//! Runs exactly `phantom list --json` and keeps only the `name` strings, so
//! `doctor_fix::plan_fixes` can report a `phm:` ref whose name Phantom does
//! not list as `dangling_phm_ref`. Rules (design-locus-phm-check.md):
//! - Only when the current directory has a `.phantom.toml` (`phantom list`
//!   reads that project's vault); otherwise `no_phantom_project`.
//! - Never `reveal`, `exec` or any other subcommand; no `PHANTOM_BIN`
//!   override. `env_clear()`, then PATH, HOME, USERPROFILE, SYSTEMROOT,
//!   TMPDIR, TEMP, LANG and `NO_COLOR=1` only: no `LOCUS_*`, `PHANTOM_*` or
//!   token reaches Phantom.
//! - stdin null, stderr discarded (never echoed), stdout capped at 1 MiB,
//!   3 s wall clock then kill.
//! - Exit 0 and a JSON array of objects each with a valid `name`, else
//!   unavailable. Any failure passes `None` to the planner (fail closed).
//! - Names are never printed: callers report the status and a count only.
//!
//! Called only from `cmd_doctor_fix`; binding resolution never runs it
//! (pinned by tests/doctor_fix_phm_names.rs).

use serde::Serialize;
use std::collections::BTreeSet;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const TIMEOUT: Duration = Duration::from_secs(3);
const STDOUT_CAP: usize = 1024 * 1024;
const ENV_KEEP: &[&str] = &[
    "PATH",
    "HOME",
    "USERPROFILE",
    "SYSTEMROOT",
    "TMPDIR",
    "TEMP",
    "LANG",
];

/// Probe result as reported in `--json` (`phantom_names`). Never carries names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum NamesStatus {
    Listed { count: usize },
    Unavailable { reason: &'static str },
}

pub struct Probe {
    pub names: Option<BTreeSet<String>>,
    pub status: NamesStatus,
}

/// A valid but empty list while bindings hold `phm:` refs: phantom-secrets
/// 0.6.0 to 0.7.9 print `[]` with exit 0 for a locked vault, so an empty
/// answer cannot prove a ref is dangling. Callers keep every ref unavailable.
pub fn empty_list() -> Probe {
    unavailable("empty_list")
}

fn unavailable(reason: &'static str) -> Probe {
    Probe {
        names: None,
        status: NamesStatus::Unavailable { reason },
    }
}

/// Parse `phantom list --json` stdout into names; any bad entry rejects it all.
pub fn parse_names(stdout: &str) -> Option<BTreeSet<String>> {
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).ok()?;
    let mut out = BTreeSet::new();
    for entry in v.as_array()? {
        let name = entry.as_object()?.get("name")?.as_str()?;
        if !locus_core::credential::valid_phantom_name(name) {
            return None;
        }
        out.insert(name.to_string());
    }
    Some(out)
}

pub fn probe(cwd: &Path) -> Probe {
    probe_with_timeout(cwd, TIMEOUT)
}

pub fn probe_with_timeout(cwd: &Path, timeout: Duration) -> Probe {
    if !cwd.join(".phantom.toml").is_file() {
        return unavailable("no_phantom_project");
    }
    let mut cmd = Command::new("phantom");
    cmd.args(["list", "--json"])
        .current_dir(cwd)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for k in ENV_KEEP {
        if let Some(v) = std::env::var_os(k) {
            cmd.env(k, v);
        }
    }
    cmd.env("NO_COLOR", "1");
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return unavailable("not_installed"),
        Err(_) => return unavailable("exit_nonzero"),
    };
    let Some(mut out) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return unavailable("bad_output");
    };
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = (&mut out).take(STDOUT_CAP as u64 + 1).read_to_end(&mut buf);
        let _ = sender.send(buf);
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if start.elapsed() >= timeout => break None,
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => break None,
        }
    };
    let Some(status) = status else {
        let _ = child.kill();
        let _ = child.wait();
        // The reader ends when the pipe closes; a grandchild holding it open
        // only keeps that thread alive, never this command.
        return unavailable("timeout");
    };
    // A successful parent can leave a descendant holding stdout open. The
    // original wall-clock deadline applies to the pipe as well as the process.
    let remaining = timeout.saturating_sub(start.elapsed());
    let Ok(buf) = receiver.recv_timeout(remaining) else {
        return unavailable("timeout");
    };
    if !status.success() {
        return unavailable("exit_nonzero");
    }
    if buf.len() > STDOUT_CAP {
        return unavailable("bad_output");
    }
    match parse_names(&String::from_utf8_lossy(&buf)) {
        Some(names) => Probe {
            status: NamesStatus::Listed { count: names.len() },
            names: Some(names),
        },
        None => unavailable("bad_output"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names_only_and_rejects_bad_shapes() {
        let ok =
            parse_names(r#"[{"name":"GITHUB_TOKEN","detected_service":"github"},{"name":"_X1"}]"#)
                .unwrap();
        assert_eq!(ok.len(), 2);
        assert!(parse_names("[]").unwrap().is_empty());
        for bad in [
            "{}",
            "junk",
            r#"[{"name":"lower"}]"#,
            r#"[{"nam":"X"}]"#,
            r#"["X"]"#,
            r#"[{"name":1}]"#,
            r#"[{"name":"1ABC"}]"#,
        ] {
            assert!(parse_names(bad).is_none(), "{bad}");
        }
        let long = format!(r#"[{{"name":"{}"}}]"#, "A".repeat(300));
        assert!(parse_names(&long).is_none());
    }

    #[test]
    fn no_phantom_project_without_phantom_toml() {
        let d = tempfile::tempdir().unwrap();
        let p = probe(d.path());
        assert!(p.names.is_none());
        assert_eq!(
            p.status,
            NamesStatus::Unavailable {
                reason: "no_phantom_project"
            }
        );
    }
}

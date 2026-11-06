//! `locus events export --format ndjson --since <ts>` — the SIEM export
//! contract: every stdout line parses as one `locus.siem.v1` event, `--since`
//! bounds the slice, and a seeded credential value never survives the export.

use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::{Command, Output};

/// Minimal env: a temporary `LOCUS_HOME` holding a hand-written audit log.
/// `events export` only reads `audit/events.jsonl` — no init, pin or
/// capability is needed, so lines with raw canaries can be seeded directly.
struct Env {
    root: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        Self {
            root: tempfile::tempdir().unwrap(),
        }
    }

    fn home(&self) -> PathBuf {
        self.root.path().join("locus")
    }

    fn seed(&self, events: &[Value]) {
        let audit = self.home().join("audit");
        std::fs::create_dir_all(&audit).unwrap();
        let mut body = events
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        body.push('\n');
        std::fs::write(audit.join("events.jsonl"), body).unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_locus"))
            .args(args)
            .env("LOCUS_HOME", self.home())
            .env("HOME", self.root.path())
            .env("NO_COLOR", "1")
            .env_remove("LOCUS_SESSION_ID")
            .env_remove("LOCUS_CONTROL_CAPABILITY")
            .env_remove("LOCUS_EXECUTOR_CAPABILITY")
            .env_remove("LOCUS_AUDIT_WEBHOOK_URL")
            .env_remove("LOCUS_NON_INTERACTIVE")
            .current_dir(self.root.path())
            .output()
            .expect("locus binary runs")
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn ev(ts: &str, op: &str, binding: &str, detail: Value) -> Value {
    json!({"ts": ts, "op": op, "binding": binding, "detail": detail})
}

fn sample() -> Vec<Value> {
    vec![
        ev(
            "2026-08-09T12:00:00Z",
            "session.pin",
            "acme",
            json!({"session_id": "ses_1"}),
        ),
        ev(
            "2026-08-09T13:00:00Z",
            "mcp.scope_freeze",
            "acme",
            json!({"tool": "supabase.scope", "args_digest": "sha256:9f"}),
        ),
        ev(
            "2026-08-09T14:00:00Z",
            "binding.save",
            "personal",
            Value::Null,
        ),
    ]
}

fn parsed_lines(o: &Output) -> Vec<Value> {
    stdout(o)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("every line is a JSON object"))
        .collect()
}

fn ops(lines: &[Value]) -> Vec<&str> {
    lines.iter().map(|v| v["op"].as_str().unwrap()).collect()
}

#[test]
fn ndjson_lines_parse_with_stable_fields() {
    let env = Env::new();
    env.seed(&sample());
    let out = env.run(&["events", "export", "--format", "ndjson"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let lines = parsed_lines(&out);
    assert_eq!(lines.len(), 3);
    for v in &lines {
        assert_eq!(v["schema"], "locus.siem.v1");
        assert_eq!(v["kind"], "audit");
        for key in [
            "schema",
            "ts",
            "op",
            "binding",
            "severity",
            "kind",
            "detail",
            "exported_at",
            "locus_version",
        ] {
            assert!(v.get(key).is_some(), "missing stable field {key}: {v}");
        }
    }
    assert_eq!(lines[1]["severity"], "WARN");
    assert_eq!(lines[0]["severity"], "INFO");

    // `--out` writes the same line-oriented body for file handoff.
    let file = env.root.path().join("siem.ndjson");
    let out = env.run(&[
        "events",
        "export",
        "--format",
        "ndjson",
        "--out",
        file.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let written = std::fs::read_to_string(&file).unwrap();
    assert_eq!(written.lines().filter(|l| !l.is_empty()).count(), 3);
    for line in written.lines() {
        serde_json::from_str::<Value>(line).expect("file line is a JSON object");
    }
}

#[test]
fn since_bounds_the_exported_slice() {
    let env = Env::new();
    env.seed(&sample());

    // Inclusive lower bound keeps the boundary event.
    let out = env.run(&[
        "events",
        "export",
        "--format",
        "ndjson",
        "--since",
        "2026-08-09T13:00:00Z",
    ]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(
        ops(&parsed_lines(&out)),
        vec!["mcp.scope_freeze", "binding.save"]
    );

    // A bare date means start of day UTC.
    let out = env.run(&[
        "events",
        "export",
        "--format",
        "ndjson",
        "--since",
        "2026-08-10",
    ]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        parsed_lines(&out).is_empty(),
        "no events on/after 2026-08-10"
    );

    // --since composes with --binding and --last.
    let out = env.run(&[
        "events",
        "export",
        "--format",
        "ndjson",
        "--since",
        "2026-08-09",
        "--binding",
        "acme",
        "--last",
        "1",
    ]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(ops(&parsed_lines(&out)), vec!["mcp.scope_freeze"]);

    // An unparseable bound fails closed — no silently wrong slice.
    let out = env.run(&[
        "events",
        "export",
        "--format",
        "ndjson",
        "--since",
        "not-a-time",
    ]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("--since"),
        "stderr names --since: {}",
        stderr(&out)
    );
}

#[test]
fn seeded_credential_never_appears_in_output() {
    // Plainly fake canary; key-shaped prefixes are assembled from fragments
    // so this file never embeds a complete token-looking literal.
    const CANARY: &str = "synthetic-siem-canary";
    let pat_prefix = ["gh", "p_"].concat();
    let aws_prefix = ["AK", "IA"].concat();
    let env = Env::new();
    env.seed(&[ev(
        "2026-08-09T12:00:00Z",
        "probe",
        "acme",
        json!({
            "credential_ref": format!("phm:{CANARY}"),
            "api_key": CANARY,
            "session_key": CANARY,
            "note": format!("pasted {pat_prefix}{CANARY} into args"),
            "aws": format!("{aws_prefix}{CANARY}"),
            "nested": [
                {"access_token": CANARY},
                format!("--ref=env:{CANARY}"),
                format!("prefix test:{CANARY}"),
            ],
            "safe": {"session_id": "ses_1"},
        }),
    )]);
    let out = env.run(&["events", "export", "--format", "ndjson"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let body = stdout(&out);
    assert_eq!(body.lines().filter(|l| !l.is_empty()).count(), 1);
    assert!(!body.contains(CANARY), "seeded credential leaked: {body}");
    // Every line still parses and safe fields survive.
    let v: Value = serde_json::from_str(body.lines().next().unwrap()).unwrap();
    assert_eq!(v["detail"]["safe"]["session_id"], "ses_1");
    assert_eq!(v["detail"]["api_key_redacted"], "<redacted>");
}

#[test]
fn fleet_and_otlp_formats_still_work() {
    let env = Env::new();
    env.seed(&sample());

    for args in [
        vec!["events", "export"],
        vec!["events", "export", "--format", "fleet"],
    ] {
        let out = env.run(&args);
        assert!(out.status.success(), "{args:?}: {}", stderr(&out));
        let lines = parsed_lines(&out);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0]["schema"], "locus.audit.v1");
    }

    for args in [
        vec!["events", "export", "--otlp"],
        vec!["events", "export", "--format", "otlp"],
    ] {
        let out = env.run(&args);
        assert!(out.status.success(), "{args:?}: {}", stderr(&out));
        let doc: Value = serde_json::from_str(&stdout(&out)).unwrap();
        assert!(
            doc["resourceLogs"].is_array(),
            "{args:?} missing OTLP shape"
        );
    }

    // --format and the --otlp alias are mutually exclusive.
    let out = env.run(&["events", "export", "--format", "otlp", "--otlp"]);
    assert!(!out.status.success());
}

#[test]
fn since_also_bounds_the_default_fleet_format() {
    let env = Env::new();
    env.seed(&sample());
    let out = env.run(&["events", "export", "--since", "2026-08-09T13:00:00Z"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let lines = parsed_lines(&out);
    assert_eq!(ops(&lines), vec!["mcp.scope_freeze", "binding.save"]);
    assert_eq!(lines[0]["schema"], "locus.audit.v1");
}

#[test]
fn invalid_since_preserves_existing_output_file() {
    let env = Env::new();
    env.seed(&sample());
    let file = env.root.path().join("siem.ndjson");
    std::fs::write(&file, "synthetic-siem-canary").unwrap();
    let out = env.run(&[
        "events",
        "export",
        "--format",
        "ndjson",
        "--since",
        "invalid",
        "--out",
        file.to_str().unwrap(),
    ]);
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    assert_eq!(
        std::fs::read_to_string(file).unwrap(),
        "synthetic-siem-canary"
    );
}

#[test]
fn since_compares_instants_before_limiting_events() {
    let env = Env::new();
    env.seed(&[
        ev("2026-08-09T14:30:00+02:30", "boundary", "acme", Value::Null),
        ev("invalid", "corrupt", "acme", Value::Null),
        ev("2026-08-09T06:59:59-05:00", "older", "acme", Value::Null),
    ]);
    let out = env.run(&[
        "events",
        "export",
        "--format",
        "ndjson",
        "--since",
        "2026-08-09T12:00:00Z",
        "--last",
        "1",
    ]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(ops(&parsed_lines(&out)), vec!["boundary"]);
}

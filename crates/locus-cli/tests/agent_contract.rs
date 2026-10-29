//! Agent contract (docs/agent-contract.md): versioned `--json` envelopes,
//! the single exit-code table, `--dry-run` plans that write nothing, and the
//! non-interactive default when stdout is not a terminal (always the case
//! here: the test harness captures it).

use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A throwaway operator: `LOCUS_HOME`, a fake `$HOME` (agent/client setup
/// paths) and a project directory, all temporary.
struct Env {
    root: tempfile::TempDir,
    /// Outside the snapshot: state that third-party CLIs probed during
    /// detection (for example `gh`'s device id) keep for themselves.
    tool_state: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("user")).unwrap();
        std::fs::create_dir_all(root.path().join("project")).unwrap();
        Self {
            root,
            tool_state: tempfile::tempdir().unwrap(),
        }
    }

    fn home(&self) -> PathBuf {
        self.root.path().join("locus")
    }

    fn user(&self) -> PathBuf {
        self.root.path().join("user")
    }

    fn project(&self) -> PathBuf {
        self.root.path().join("project")
    }

    fn cap(&self) -> Option<String> {
        std::fs::read_to_string(self.home().join("control_capability"))
            .ok()
            .map(|c| c.trim().to_string())
    }

    fn run_with(&self, args: &[&str], cap: Option<&str>, extra: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_locus"));
        cmd.args(args)
            .env("LOCUS_HOME", self.home())
            .env("HOME", self.user())
            .env("NO_COLOR", "1")
            .env("XDG_STATE_HOME", self.tool_state.path())
            .env_remove("LOCUS_SESSION_ID")
            .env_remove("LOCUS_CONTROL_CAPABILITY")
            .env_remove("LOCUS_GRAPH_PASSPHRASE")
            .env_remove("LOCUS_NON_INTERACTIVE")
            .env_remove("LOCUS_PHANTOM_PREFLIGHT")
            .current_dir(self.project());
        if let Some(c) = cap {
            cmd.env("LOCUS_CONTROL_CAPABILITY", c);
        }
        for (k, v) in extra {
            cmd.env(k, v);
        }
        cmd.output().expect("locus binary runs")
    }

    /// Run with the persisted control capability (when one exists).
    fn run(&self, args: &[&str]) -> Output {
        let cap = self.cap();
        self.run_with(args, cap.as_deref(), &[])
    }

    fn quickstart(&self) {
        let out = self.run(&["quickstart"]);
        assert!(out.status.success(), "quickstart: {}", stderr(&out));
    }

    /// Every file under the locus home, the fake user home and the project
    /// (path → bytes). `runtime/` holds the live authority broker's state,
    /// which that background process may touch on its own.
    fn snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut out = BTreeMap::new();
        walk(self.root.path(), self.root.path(), &mut out);
        out
    }
}

fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let rel = path.strip_prefix(root).unwrap().to_path_buf();
        if rel.starts_with("locus/runtime") {
            continue;
        }
        let meta = std::fs::symlink_metadata(&path).unwrap();
        if meta.is_dir() {
            out.insert(rel, b"<dir>".to_vec());
            walk(root, &path, out);
        } else {
            out.insert(rel, std::fs::read(&path).unwrap_or_default());
        }
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// Exactly one JSON document on stdout, carrying `schema`.
fn doc(o: &Output, schema: &str) -> Value {
    let text = stdout(o);
    let v: Value = serde_json::from_str(text.trim()).unwrap_or_else(|e| {
        panic!(
            "stdout is one JSON document ({e}): {text:?} / {}",
            stderr(o)
        )
    });
    assert_eq!(v["schema"], schema, "{v}");
    // `schema` is the first key in the printed text.
    let first_key = text
        .trim_start_matches(['{', ' ', '\n'])
        .split(':')
        .next()
        .unwrap();
    assert_eq!(first_key.trim(), "\"schema\"", "{text}");
    v
}

fn assert_plan(o: &Output, command_prefix: &str) -> Value {
    assert_eq!(o.status.code(), Some(0), "{} / {}", stdout(o), stderr(o));
    let v = doc(o, "locus.plan.v1");
    assert_eq!(v["ok"], true);
    assert_eq!(v["dry_run"], true);
    assert!(v["changes"].is_array(), "{v}");
    assert!(v["notes"].is_array(), "{v}");
    assert!(v["requires_control_capability"].is_boolean(), "{v}");
    let command = v["command"].as_str().unwrap();
    assert!(
        command.starts_with(command_prefix),
        "{command} vs {command_prefix}"
    );
    for c in v["changes"].as_array().unwrap() {
        assert!(c["action"].is_string() && c["target"].is_string(), "{c}");
    }
    v
}

fn error_doc(o: &Output, code: i32, name: &str) -> Value {
    assert_eq!(o.status.code(), Some(code), "{}", stderr(o));
    let v: Value = serde_json::from_str(stderr(o).trim())
        .unwrap_or_else(|e| panic!("stderr is one JSON error ({e}): {:?}", stderr(o)));
    assert_eq!(v["schema"], "locus.error.v1", "{v}");
    assert_eq!(v["ok"], false);
    assert_eq!(v["exit_code"], code);
    assert_eq!(v["exit_code_name"], name);
    assert!(v["error"].is_string());
    v
}

// ── Docs ↔ code ──────────────────────────────────────────────────────────

fn read_repo(rel: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::fs::read_to_string(root.join(rel)).unwrap()
}

#[test]
fn docs_exit_code_table_and_schema_catalogue_match_the_binary() {
    let env = Env::new();
    let out = env.run(&["topic", "agent-contract", "--json"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let contract = doc(&out, "locus.contract.v1");
    let docs = read_repo("docs/agent-contract.md");

    let mut table = String::from("| Code | Name | Meaning |\n| --- | --- | --- |\n");
    for c in contract["exit_codes"].as_array().unwrap() {
        table.push_str(&format!(
            "| {} | `{}` | {} |\n",
            c["code"],
            c["name"].as_str().unwrap(),
            c["meaning"].as_str().unwrap()
        ));
    }
    let begin = docs.find("<!-- exit-codes:begin -->\n").unwrap() + 26;
    let end = docs.find("<!-- exit-codes:end -->").unwrap();
    assert_eq!(
        &docs[begin..end],
        table,
        "docs/agent-contract.md exit-code table drifted"
    );
    let stable_docs = read_repo("docs/exit-codes.md");
    let begin = stable_docs.find("<!-- exit-codes:begin -->\n").unwrap() + 26;
    let end = stable_docs.find("<!-- exit-codes:end -->").unwrap();
    assert_eq!(
        &stable_docs[begin..end],
        table,
        "docs/exit-codes.md exit-code table drifted"
    );

    for s in contract["schemas"].as_array().unwrap() {
        let id = s["schema"].as_str().unwrap();
        assert!(docs.contains(&format!("`{id}`")), "docs miss schema {id}");
    }
    // Human topic prints the same table.
    let human = stdout(&env.run(&["topic", "agent-contract"]));
    assert!(human.contains(&table), "{human}");
    // And `locus help topic` reaches it too.
    let via_help = env.run(&["help", "topic", "agent-contract", "--json"]);
    doc(&via_help, "locus.contract.v1");
}

#[test]
fn contract_exit_codes_are_the_documented_values() {
    let env = Env::new();
    let v = doc(
        &env.run(&["topic", "agent-contract", "--json"]),
        "locus.contract.v1",
    );
    let codes: Vec<(i64, String)> = v["exit_codes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["code"].as_i64().unwrap(),
                c["name"].as_str().unwrap().into(),
            )
        })
        .collect();
    let expected = [
        (0, "success"),
        (1, "failure"),
        (2, "unsafe"),
        (2, "usage"),
        (3, "refused"),
        (4, "repin_required"),
        (5, "expired"),
        (6, "misconfig"),
        (126, "cannot_execute"),
        (127, "not_found"),
        (141, "stdout_closed"),
    ];
    assert_eq!(
        codes,
        expected
            .iter()
            .map(|(c, n)| (*c, n.to_string()))
            .collect::<Vec<_>>()
    );
}

// ── Check / status JSON shapes and exit codes ────────────────────────────

#[test]
fn status_and_identity_checks_unpinned() {
    let env = Env::new();
    let init = env.run(&["init", "--with-samples"]);
    assert!(init.status.success(), "{}", stderr(&init));

    let v = doc(&env.run(&["version", "--json"]), "locus.version.v1");
    assert!(v["version"].is_string());

    let out = env.run(&["status", "--json"]);
    assert_eq!(out.status.code(), Some(0));
    let v = doc(&out, "locus.status.v1");
    assert_eq!(v["pinned"], false);
    assert!(
        !stdout(&out).trim().contains('\n'),
        "status prints one line"
    );

    // whoami unpinned is a command error: exit 1, JSON on stderr, stdout clean.
    let out = env.run(&["whoami", "--json"]);
    assert!(out.stdout.is_empty());
    error_doc(&out, 1, "failure");

    // verify session: report on stdout, then exit 1 when session_ok is false.
    let out = env.run(&["verify", "session", "--json"]);
    let v = doc(&out, "locus.verify.session.v1");
    assert_eq!(v["session_ok"], false);
    assert_eq!(out.status.code(), Some(1));

    let out = env.run(&["verify", "claim", "--text", "deploys always pass", "--json"]);
    assert_eq!(out.status.code(), Some(0));
    let v = doc(&out, "locus.verify.claim.v1");
    assert!(v["confidence"].is_string() && v["needs_tool"].is_boolean());
}

#[test]
fn doctor_advisories_succeed_and_agent_reports_exit_with_their_status() {
    let env = Env::new();
    env.quickstart();

    let out = env.run(&["doctor", "--json"]);
    let v = doc(&out, "locus.doctor.v1");
    assert_eq!(v["schema_version"], 1, "older envelope fields stay");
    assert!(
        matches!(v["verdict"].as_str(), Some("SAFE" | "WARN")),
        "{v}"
    );
    assert_eq!(out.status.code(), Some(0), "fresh setup advisories: {v}");

    for (sub, schema) in [
        ("doctor", "locus.agent.doctor.v1"),
        ("report", "locus.agent.report.v1"),
    ] {
        let out = env.run(&["agent", sub, "--json"]);
        let v = doc(&out, schema);
        let code = match v["status"].as_str().unwrap() {
            "ready" => 0,
            "protected" => 1,
            "unsafe" => 2,
            other => panic!("status {other}"),
        };
        assert_eq!(out.status.code(), Some(code), "{v}");
        assert_eq!(v["exit_code"], code, "report.exit_code matches the process");
    }

    let out = env.run(&["doctor", "--fix", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let v = doc(&out, "locus.doctor.fix.v1");
    assert_eq!(v["mode"], "dry_run");
}

#[test]
fn pinned_identity_and_status_json() {
    let env = Env::new();
    env.quickstart();

    let out = env.run(&["whoami", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let v = doc(&out, "locus.whoami.v1");
    assert_eq!(v["binding_alias"], "personal");

    let out = env.run(&["status", "--json"]);
    let v = doc(&out, "locus.status.v1");
    assert_eq!(v["pinned"], true);
    let code = if v["ok"] == true { 0 } else { 2 };
    assert_eq!(out.status.code(), Some(code), "{v}");

    let out = env.run(&["watch", "--once", "--json"]);
    let line = stdout(&out);
    assert_eq!(line.trim().lines().count(), 1, "one NDJSON tick: {line}");
    let v = doc(&out, "locus.watch.v1");
    assert!(v["session_ok"].is_boolean());
}

#[test]
fn whoami_json_reports_environment_credential_availability() {
    const VAR: &str = "LOCUS_GH_TOKEN_PERSONAL";
    const CANARY: &str = "synthetic-whoami-credential-canary";
    let env = Env::new();
    env.quickstart();
    let cap = env.cap().unwrap();

    // Each invocation gets its own controlled environment, without mutating
    // the test process or inheriting an operator's credential.
    let whoami = |value: Option<&str>| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_locus"));
        cmd.args(["whoami", "--json"])
            .env_clear()
            .env("HOME", env.user())
            .env("LOCUS_HOME", env.home())
            .env("LOCUS_CONTROL_CAPABILITY", &cap)
            .current_dir(env.project());
        if let Some(value) = value {
            cmd.env(VAR, value);
        }
        let out = cmd.output().expect("locus runs");
        assert!(out.status.success());
        assert!(!out
            .stdout
            .windows(CANARY.len())
            .any(|w| w == CANARY.as_bytes()));
        assert!(!out
            .stderr
            .windows(CANARY.len())
            .any(|w| w == CANARY.as_bytes()));
        let result: Value = serde_json::from_slice(&out.stdout).unwrap();
        let provider = result["providers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["provider"] == "github")
            .unwrap();
        assert_eq!(provider["credential"]["source"], "environment");
        assert!(!String::from_utf8_lossy(&out.stdout).contains(VAR));
        provider["credential"]["present"].as_bool().unwrap()
    };

    assert!(!whoami(None));
    assert!(whoami(Some(CANARY)));
    assert!(!whoami(None));
}

#[test]
fn whoami_reports_a_set_binding_credential_without_exposing_it() {
    const VAR: &str = "LOCUS_TEST_WHOAMI_CUSTOM_CREDENTIAL";
    const CANARY: &str = "synthetic-custom-credential-canary";
    let env = Env::new();
    env.quickstart();

    let out = env.run(&[
        "binding",
        "add",
        "credential-test",
        "--tenant",
        "test-tenant",
        "--provider",
        "github",
        "--account",
        "test-account",
        "--credential-ref",
        "env:LOCUS_TEST_WHOAMI_CUSTOM_CREDENTIAL",
        "--non-interactive",
    ]);
    assert!(out.status.success(), "binding add failed");
    assert!(env.run(&["enter", "credential-test"]).status.success());
    let cap = env.cap().unwrap();

    // Set the credential only in the whoami process, after the pin was made.
    // An unrelated credential must not count as the binding's credential.
    for (value, expected) in [(None, false), (Some(CANARY), true), (None, false)] {
        for json in [true, false] {
            let mut cmd = Command::new(env!("CARGO_BIN_EXE_locus"));
            cmd.arg("whoami")
                .env_clear()
                .env("HOME", env.user())
                .env("LOCUS_HOME", env.home())
                .env("LOCUS_CONTROL_CAPABILITY", &cap)
                .env("LOCUS_TEST_WHOAMI_UNRELATED_CREDENTIAL", CANARY)
                .current_dir(env.project());
            if json {
                cmd.arg("--json");
            }
            if let Some(value) = value {
                cmd.env(VAR, value);
            }
            let out = cmd.output().expect("locus runs");
            assert!(out.status.success(), "whoami failed");
            for stream in [&out.stdout, &out.stderr] {
                assert!(!stream.windows(CANARY.len()).any(|w| w == CANARY.as_bytes()));
                assert!(!stream.windows(VAR.len()).any(|w| w == VAR.as_bytes()));
            }
            if json {
                let result: Value = serde_json::from_slice(&out.stdout).unwrap();
                assert_eq!(result["binding_alias"], "credential-test");
                let providers = result["providers"].as_array().unwrap();
                assert_eq!(providers.len(), 1);
                assert_eq!(providers[0]["provider"], "github");
                assert_eq!(providers[0]["credential"]["source"], "environment");
                assert_eq!(providers[0]["credential"]["present"], expected);
            }
        }
    }
}

#[test]
fn setup_and_settings_status_json() {
    let env = Env::new();
    env.quickstart();

    let v = doc(
        &env.run(&["capability", "status", "--json"]),
        "locus.capability.status.v1",
    );
    assert!(v["posture"].is_string());
    let cap = env.cap().unwrap();
    assert!(!stdout(&env.run(&["capability", "status", "--json"])).contains(&cap));

    let v = doc(
        &env.run(&["notify", "status", "--json"]),
        "locus.notify.status.v1",
    );
    assert_eq!(v["default"], "off");

    let v = doc(
        &env.run(&["goal", "status", "--json"]),
        "locus.goal.status.v1",
    );
    assert!(v["milestones"].is_array());

    let out = env.run(&["autopin", "suggest", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let v = doc(&out, "locus.autopin.suggest.v1");
    assert_eq!(v["applied"], false);

    let out = env.run(&["adapter", "verify", "--json"]);
    let v = doc(&out, "locus.adapter.verify.v1");
    let code = if v["ok"] == true { 0 } else { 1 };
    assert_eq!(out.status.code(), Some(code));

    let manifest = env.project().join("release.json");
    let export = env.run(&[
        "adapter",
        "registry",
        "export",
        "--out",
        manifest.to_str().unwrap(),
    ]);
    assert!(export.status.success(), "{}", stderr(&export));
    let out = env.run(&[
        "adapter",
        "verify-manifest",
        manifest.to_str().unwrap(),
        "--allow-unsigned",
        "--json",
    ]);
    let v = doc(&out, "locus.adapter.verify_manifest.v1");
    assert_eq!(out.status.code(), Some(if v["ok"] == true { 0 } else { 1 }));

    // Unknown approval id: command error under the error envelope.
    error_doc(
        &env.run(&["approve", "status", "appr_nope", "--json"]),
        1,
        "failure",
    );
}

#[test]
fn unsigned_release_manifest_requires_explicit_drift_only_verification() {
    let env = Env::new();
    let path = env.project().join("release.json");
    let path_arg = path.to_str().unwrap();
    let export = env.run(&["adapter", "registry", "export", "--out", path_arg]);
    assert!(export.status.success(), "{}", stderr(&export));

    let mut manifest: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(manifest.get("signature").is_none());
    assert!(manifest.get("signed_by").is_none());

    let rejected = env.run(&["adapter", "verify-manifest", path_arg, "--json"]);
    assert_eq!(rejected.status.code(), Some(1));
    let report = doc(&rejected, "locus.adapter.verify_manifest.v1");
    assert_eq!(report["signature"], "unsigned");
    assert_eq!(report["ok"], false);

    let verify = || {
        env.run(&[
            "adapter",
            "verify-manifest",
            path_arg,
            "--allow-unsigned",
            "--json",
        ])
    };
    let matching = verify();
    assert_eq!(matching.status.code(), Some(0));
    let report = doc(&matching, "locus.adapter.verify_manifest.v1");
    assert_eq!(report["signature"], "unsigned");
    assert_eq!(report["ok"], true);

    // Empty signatures are unsigned; a nonempty malformed signature must not
    // be accepted by the drift-only opt-in.
    manifest["signature"] = Value::String("  ".into());
    std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let empty = verify();
    assert_eq!(empty.status.code(), Some(0));
    assert_eq!(
        doc(&empty, "locus.adapter.verify_manifest.v1")["signature"],
        "unsigned"
    );

    manifest["signature"] = Value::String("not-a-signature".into());
    std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let malformed = verify();
    assert_eq!(malformed.status.code(), Some(1));
    let report = doc(&malformed, "locus.adapter.verify_manifest.v1");
    assert_eq!(report["signature"], "malformed");
    assert_eq!(report["ok"], false);

    manifest.as_object_mut().unwrap().remove("signature");
    manifest["locus_version"] = Value::String("0.0.0-drift".into());
    std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let drifted = verify();
    assert_eq!(drifted.status.code(), Some(1));
    let report = doc(&drifted, "locus.adapter.verify_manifest.v1");
    assert_eq!(report["signature"], "unsigned");
    assert_eq!(report["ok"], false);
    assert!(!report["drift"].as_array().unwrap().is_empty());
}

#[test]
fn usage_errors_exit_2_with_the_error_envelope() {
    let env = Env::new();
    let out = env.run(&["whoami", "--json", "--no-such-flag"]);
    assert!(out.stdout.is_empty());
    error_doc(&out, 2, "usage");
    // Human usage errors keep clap's prose and exit 2.
    let out = env.run(&["whoami", "--no-such-flag"]);
    assert_eq!(out.status.code(), Some(2));
}

// ── --dry-run ────────────────────────────────────────────────────────────

/// Run a dry run and assert it printed a plan and changed no file.
fn dry(env: &Env, args: &[&str], prefix: &str) -> Value {
    let before = env.snapshot();
    let mut full = vec!["--json", "--dry-run"];
    full.extend_from_slice(args);
    let out = env.run(&full);
    let v = assert_plan(&out, prefix);
    let after = env.snapshot();
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>(),
        "`locus {}` --dry-run created or removed files",
        args.join(" ")
    );
    for (path, bytes) in &before {
        assert_eq!(
            Some(bytes),
            after.get(path),
            "`locus {}` --dry-run changed {}",
            args.join(" "),
            path.display()
        );
    }
    // The human form plans the same and writes nothing either.
    let mut human = vec!["--dry-run"];
    human.extend_from_slice(args);
    let out = env.run(&human);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(env.snapshot(), after);
    v
}

#[test]
fn init_and_quickstart_dry_runs_never_create_the_home() {
    let env = Env::new();
    let v = dry(&env, &["init", "--with-samples"], "locus init");
    assert!(!env.home().exists(), "init --dry-run created LOCUS_HOME");
    let targets: Vec<&str> = v["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["target"].as_str().unwrap())
        .collect();
    assert!(targets.iter().any(|t| t.ends_with("personal.toml")), "{v}");
    assert!(
        targets.iter().any(|t| t.ends_with("control_capability")),
        "{v}"
    );

    let v = dry(&env, &["quickstart"], "locus quickstart");
    assert!(!env.home().exists());
    assert_eq!(v["pin"], "personal", "{v}");

    // Commands that need a store refuse to plan against a missing one.
    let out = env.run(&["--json", "--dry-run", "pin", "personal"]);
    let e = error_doc(&out, 1, "failure");
    assert!(
        e["error"].as_str().unwrap().contains("init --dry-run"),
        "{e}"
    );
    assert!(!env.home().exists());
}

#[test]
fn pin_family_plans_resolve_like_the_real_pin() {
    let env = Env::new();
    env.quickstart();

    let v = dry(&env, &["pin", "acme"], "locus pin acme");
    assert_eq!(v["binding"], "acme");
    assert_eq!(v["replaces"], "personal");
    assert_eq!(v["requires_control_capability"], true);
    assert_eq!(v["control_capability_in_env"], true);
    let actions: Vec<&str> = v["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["action"].as_str().unwrap())
        .collect();
    assert_eq!(actions, ["unpin", "pin", "overwrite"], "{v}");

    // TTL is capped by policy.max_ttl exactly like the real pin (acme: 8h).
    let v = dry(&env, &["enter", "acme", "--ttl", "20h"], "locus enter acme");
    assert_eq!(v["ttl_secs"], 8 * 3600);
    assert_eq!(v["ttl_capped"], true);

    dry(&env, &["switch", "acme"], "locus switch acme");
    dry(
        &env,
        &["pin", "--ns", "personal,acme"],
        "locus pin --ns personal",
    );
    dry(&env, &["leave"], "locus leave");
    dry(&env, &["leave", "--force"], "locus leave --force");

    // An unknown alias fails the plan like the real pin, with suggestions.
    let out = env.run(&["--json", "--dry-run", "pin", "acmee"]);
    let e = error_doc(&out, 1, "failure");
    assert!(e["error"].as_str().unwrap().contains("acme"), "{e}");

    // A workspace allowlist refusal is the real pin's refusal.
    let ws = env.run(&["workspace", "--default", "personal"]);
    assert!(ws.status.success(), "{}", stderr(&ws));
    error_doc(
        &env.run(&["--json", "--dry-run", "pin", "acme"]),
        1,
        "failure",
    );
    dry(&env, &["pin", "acme", "--force"], "locus pin acme");
}

#[test]
fn binding_and_workspace_plans_write_nothing() {
    let env = Env::new();
    env.quickstart();

    let v = dry(
        &env,
        &[
            "binding",
            "add",
            "newco",
            "--tenant",
            "newco",
            "--provider",
            "github",
            "--account",
            "newco",
            "--credential-ref",
            "env:NEWCO_GH",
            "--non-interactive",
        ],
        "locus binding add newco",
    );
    assert!(v["toml"].as_str().unwrap().contains("newco"));
    assert_eq!(v["changes"][0]["action"], "create");
    dry(
        &env,
        &[
            "client",
            "add",
            "newco",
            "--tenant",
            "newco",
            "--provider",
            "github",
            "--account",
            "newco",
            "--credential-ref",
            "env:NEWCO_GH",
            "--non-interactive",
        ],
        "locus binding add newco",
    );
    let v = dry(&env, &["binding", "rm", "acme"], "locus binding rm acme");
    assert_eq!(v["changes"][0]["action"], "delete");
    dry(
        &env,
        &["binding", "migrate-credential-refs", "acme"],
        "locus binding migrate-credential-refs acme",
    );
    error_doc(
        &env.run(&["--json", "--dry-run", "binding", "rm", "ghost"]),
        1,
        "failure",
    );

    let v = dry(
        &env,
        &["workspace", "--default", "acme", "--allow", "ghost"],
        "locus workspace",
    );
    assert_eq!(v["unknown_bindings"], serde_json::json!(["ghost"]));
    dry(
        &env,
        &[
            "engagement",
            "init",
            "globex",
            "--tenant",
            "globex-corp",
            "--workspace",
        ],
        "locus engagement init globex",
    );
    dry(
        &env,
        &["engagement", "close", "acme", "--archive"],
        "locus engagement close acme",
    );
    // No suggestion yet: --apply would refuse, so the plan refuses too.
    let out = env.run(&["--json", "--dry-run", "autopin", "suggest", "--apply"]);
    error_doc(&out, 1, "failure");
}

#[test]
fn credential_and_grant_plans_never_print_a_capability_or_token() {
    let env = Env::new();
    env.quickstart();
    let cap = env.cap().unwrap();

    for (args, prefix) in [
        (&["capability", "persist"][..], "locus capability persist"),
        (
            &["capability", "unpersist"][..],
            "locus capability unpersist",
        ),
        (&["ci", "mint", "-b", "acme"][..], "locus ci mint -b acme"),
        (&["ci", "env", "-b", "acme"][..], "locus ci env -b acme"),
        (&["mcp", "mint", "-b", "acme"][..], "locus mcp mint -b acme"),
        (&["mcp", "revoke", "--all"][..], "locus mcp revoke"),
    ] {
        let v = dry(&env, args, prefix);
        assert!(
            !v.to_string().contains(&cap),
            "{prefix} leaked the capability"
        );
    }
    // No session or grant was minted.
    let sessions: Vec<_> = std::fs::read_dir(env.home().join("sessions"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("ci-"))
        .collect();
    assert!(sessions.is_empty(), "{sessions:?}");
}

#[test]
fn settings_export_and_client_plans_write_nothing() {
    let env = Env::new();
    env.quickstart();
    let out_file = env.project().join("events.jsonl");
    let v = dry(
        &env,
        &["events", "export", "--out", out_file.to_str().unwrap()],
        "locus events export",
    );
    assert_eq!(v["changes"][0]["action"], "create");
    assert!(!out_file.exists());
    // Webhook: names scheme + host only, never userinfo, path or query.
    let v = dry(
        &env,
        &[
            "events",
            "export",
            "--sink",
            "webhook",
            "--url",
            "https://user:tok3n@hooks.example.com/p?key=s3cret",
        ],
        "locus events export",
    );
    let text = v.to_string();
    assert!(text.contains("https://hooks.example.com"), "{text}");
    assert!(
        !text.contains("tok3n") && !text.contains("s3cret"),
        "{text}"
    );

    dry(
        &env,
        &["forensics", "export", "--out", "pack.json"],
        "locus forensics export",
    );
    assert!(!env.project().join("pack.json").exists());
    dry(&env, &["notify", "on"], "locus notify on");
    dry(&env, &["notify", "off"], "locus notify off");
    dry(
        &env,
        &["setup", "--client", "claude"],
        "locus setup --client claude",
    );
    dry(
        &env,
        &["setup", "--client", "codex"],
        "locus setup --client codex",
    );
    let v = dry(
        &env,
        &["agent", "setup", "--client", "all"],
        "locus agent setup",
    );
    assert!(
        v["actions"].is_array(),
        "agent setup fields ride along: {v}"
    );
    dry(&env, &["doctor", "--fix"], "locus doctor --fix");
    dry(&env, &["onboard"], "locus onboard");
    dry(
        &env,
        &["graph", "export", "--out", "team.locusgraph"],
        "locus graph export",
    );
    dry(
        &env,
        &["adapter", "registry", "export", "--out", "m.json"],
        "locus adapter registry export",
    );
    dry(
        &env,
        &[
            "adapter",
            "trust",
            "add",
            "--id",
            "root",
            // RFC 8032 test vector 1 public key.
            "--ed25519-pub",
            "11qYAYKxCrfVS/7TyWQHOg7hcvPaoiMlrwIaaPcHURo=",
        ],
        "locus adapter trust add --id root",
    );
    dry(
        &env,
        &["adapter", "uninstall", "nope"],
        "locus adapter uninstall nope",
    );
    dry(
        &env,
        &["adapter", "registry", "index", "remove", "nope"],
        "locus adapter registry index",
    );
    error_doc(
        &env.run(&["--json", "--dry-run", "approve", "grant", "appr_nope"]),
        1,
        "failure",
    );
    error_doc(
        &env.run(&["--json", "--dry-run", "approve", "deny", "appr_nope"]),
        1,
        "failure",
    );
}

#[test]
fn graph_import_dry_run_decrypts_and_lists_without_writing() {
    let env = Env::new();
    env.quickstart();
    let pass = [("LOCUS_GRAPH_PASSPHRASE", "correct horse")];
    let cap = env.cap().unwrap();
    let export = env.run_with(
        &["graph", "export", "--out", "g.locusgraph"],
        Some(&cap),
        &pass,
    );
    assert!(export.status.success(), "{}", stderr(&export));

    let before = env.snapshot();
    let out = env.run_with(
        &["--json", "--dry-run", "graph", "import", "g.locusgraph"],
        Some(&cap),
        &pass,
    );
    let v = assert_plan(&out, "locus graph import");
    assert_eq!(v["bindings_skipped"].as_array().unwrap().len(), 2, "{v}");
    assert_eq!(env.snapshot(), before);

    // Wrong passphrase fails the plan like the real import.
    let out = env.run_with(
        &["--json", "--dry-run", "graph", "import", "g.locusgraph"],
        Some(&cap),
        &[("LOCUS_GRAPH_PASSPHRASE", "wrong")],
    );
    error_doc(&out, 1, "failure");
    // Non-interactive: no passphrase and no terminal never prompts.
    let out = env.run_with(
        &["--json", "--dry-run", "graph", "import", "g.locusgraph"],
        Some(&cap),
        &[],
    );
    let e = error_doc(&out, 1, "failure");
    assert!(e["error"]
        .as_str()
        .unwrap()
        .contains("LOCUS_GRAPH_PASSPHRASE"));
}

#[test]
fn read_only_commands_accept_dry_run_and_unplannable_ones_refuse() {
    let env = Env::new();
    env.quickstart();
    let out = env.run(&["--dry-run", "status", "--json"]);
    doc(&out, "locus.status.v1");
    let out = env.run(&["--dry-run", "binding", "list"]);
    assert_eq!(out.status.code(), Some(0));
    for args in [
        &["graph", "list"][..],
        &["capability", "status"][..],
        &["notify", "status"][..],
    ] {
        let mut full = vec!["--dry-run"];
        full.extend_from_slice(args);
        let out = env.run(&full);
        assert_eq!(out.status.code(), Some(0), "{args:?}: {}", stderr(&out));
    }
    // doctor --fix --dry-run against a pre-made, empty home creates nothing.
    let empty = Env::new();
    std::fs::create_dir(empty.home()).unwrap();
    let out = empty.run(&["--json", "--dry-run", "doctor", "--fix"]);
    assert_plan(&out, "locus doctor --fix");
    assert_eq!(std::fs::read_dir(empty.home()).unwrap().count(), 0);

    // --apply and --dry-run together: a usage error on doctor --fix (new).
    let out = env.run(&["--json", "--dry-run", "doctor", "--fix", "--apply"]);
    error_doc(&out, 2, "usage");
}

// ── 0.5.1 contract pins (RQ-CLI-1-FIX) ───────────────────────────────────
// Each test below pins behaviour that shipped before the global --dry-run.

#[test]
fn legacy_agent_setup_dry_run_json_keeps_every_field() {
    let env = Env::new();
    let out = env.run(&["--json", "agent", "setup", "--dry-run"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let v: Value = serde_json::from_str(stdout(&out).trim()).unwrap();
    for key in [
        "ok",
        "apply",
        "dry_run",
        "clients",
        "home",
        "mcp_bin",
        "mcp_bin_fallback",
        "actions",
        "verified",
        "verify_failures",
        "paste_server_entry",
        "paste_server_toml",
        "env",
    ] {
        assert!(v.get(key).is_some(), "missing legacy field {key}: {v}");
    }
    assert_eq!(v["ok"], true);
    assert_eq!(v["apply"], false);
    assert_eq!(v["dry_run"], true);
    assert!(v["clients"].is_array() && v["actions"].is_array(), "{v}");
    assert!(v["home"].is_string() && v["mcp_bin"].is_string(), "{v}");
    assert!(v["mcp_bin_fallback"].is_boolean(), "{v}");
    assert!(v["verified"].is_null(), "{v}");
    assert_eq!(v["verify_failures"], serde_json::json!([]));
    assert_eq!(v["env"]["LOCUS_AUTO_PIN"], "cwd");
    assert_eq!(v["env"]["LOCUS_CLIENT"], "<client>");
    assert!(v["env"]["LOCUS_NOTIFY"].is_null(), "{v}");
    // Plan fields sit alongside; --apply needs the control capability.
    assert_eq!(v["requires_control_capability"], true, "{v}");
}

#[test]
fn legacy_agent_setup_apply_with_dry_run_exits_one() {
    let env = Env::new();
    for args in [
        &["agent", "setup", "--apply", "--dry-run"][..],
        &["--dry-run", "agent", "setup", "--apply"][..],
    ] {
        let out = env.run(args);
        assert_eq!(out.status.code(), Some(1), "{args:?}: {}", stderr(&out));
        assert!(
            stderr(&out).contains("pass only one of --apply or --dry-run"),
            "{}",
            stderr(&out)
        );
    }
    let out = env.run(&["--json", "agent", "setup", "--apply", "--dry-run"]);
    let e = error_doc(&out, 1, "failure");
    assert_eq!(e["error"], "pass only one of --apply or --dry-run");
}

#[test]
fn legacy_exec_dry_run_is_execs_own_argument_error() {
    let env = Env::new();
    let out = env.run(&["exec", "--dry-run", "--", "true"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(out.stdout.is_empty(), "{}", stdout(&out));
    assert!(
        stderr(&out).contains(
            "`--dry-run` is not a `locus exec` option, and a command cannot start with `-`; nothing ran."
        ),
        "{}",
        stderr(&out)
    );
    let out = env.run(&["--json", "exec", "--dry-run", "--", "true"]);
    error_doc(&out, 1, "failure");
}

#[test]
fn legacy_unplannable_commands_reject_dry_run_like_clap_did() {
    let env = Env::new();
    for args in [
        &["--dry-run", "exec", "--", "true"][..],
        &["--dry-run", "run", "-b", "personal", "--", "true"][..],
        &["--dry-run", "ci", "run", "-b", "personal", "--", "true"][..],
        &["--dry-run", "mcp"][..],
        &["mcp", "--dry-run"][..],
        &["--dry-run", "serve"][..],
        &["serve", "--dry-run"][..],
        &["--dry-run", "dashboard", "--no-open"][..],
        &["dashboard", "--no-open", "--dry-run"][..],
    ] {
        let out = env.run(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", stderr(&out));
        assert!(out.stdout.is_empty(), "{args:?}: {}", stdout(&out));
        assert!(
            stderr(&out).contains("unexpected argument '--dry-run' found"),
            "{args:?}: {}",
            stderr(&out)
        );
    }
    // `mcp mint/list/revoke` still take the global flag (they plan or read).
    let out = env.run(&["mcp", "list", "--dry-run"]);
    assert!(
        !stderr(&out).contains("unexpected argument"),
        "{}",
        stderr(&out)
    );
    // `run`/`ci run` usage text never advertises the flag.
    let out = env.run(&["run", "--dry-run", "--", "true"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(
        !stderr(&out).contains("--dry-run <CMD>"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn legacy_binding_add_dry_run_text_ends_with_ok_line() {
    let env = Env::new();
    env.quickstart();
    let out = env.run(&[
        "binding",
        "add",
        "dryx",
        "--tenant",
        "t",
        "--provider",
        "github",
        "--account",
        "a",
        "--credential-ref",
        "env:LOCUS_TEST_UNUSED",
        "--dry-run",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with("[binding]"), "{text}");
    assert_eq!(
        text.trim_end().lines().last().unwrap(),
        "ok dry run — nothing written",
        "{text}"
    );
}

#[test]
fn registry_index_add_dry_run_does_not_fetch_and_redacts_the_url() {
    let env = Env::new();
    env.quickstart();
    let before = env.snapshot();
    // Port 9 on loopback refuses connections: a fetch would fail the plan.
    let out = env.run(&[
        "--json",
        "--dry-run",
        "adapter",
        "registry",
        "index",
        "add",
        "https://127.0.0.1:9/index.json?token=PLANSECRET",
        "--name",
        "probe",
    ]);
    let v = assert_plan(&out, "locus adapter registry index");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(
        !text.contains("PLANSECRET") && !text.contains("PLANFRAG"),
        "{text}"
    );
    assert_eq!(v["url"], "https://127.0.0.1:9/index.json?<redacted>", "{v}");
    assert_eq!(v["name"], "probe");
    assert_eq!(env.snapshot(), before, "a plan wrote something");
    // Credentials or a fragment in the URL are refused before planning
    // (the real run's own URL check), and never echoed.
    let out = env.run(&[
        "--dry-run",
        "adapter",
        "registry",
        "index",
        "add",
        "https://user:PLANSECRET@127.0.0.1:9/index.json",
    ]);
    assert_ne!(out.status.code(), Some(0));
    assert!(!stderr(&out).contains("PLANSECRET"), "{}", stderr(&out));
}

// ── Non-interactive default ──────────────────────────────────────────────

#[test]
fn captured_stdout_never_prompts() {
    let env = Env::new();
    env.quickstart();
    // Missing flags fail closed instead of prompting (stdout is captured).
    let out = env.run(&["binding", "add", "x", "--guided"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("--provider"), "{}", stderr(&out));
    // Uninstall without --yes declines rather than waiting on a prompt.
    let out = env.run(&["adapter", "uninstall", "nope"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("declined"), "{}", stderr(&out));
}

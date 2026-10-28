//! `locus doctor --fix` checks `phm:` refs against `phantom list --json`
//! (design-locus-phm-check.md). A stand-in `phantom` script on PATH records
//! every argv and its environment; no real vault is touched.
//!
//! Pinned here:
//! - only `list --json` ever reaches phantom (never `reveal`, `exec`, ...);
//! - the probe runs only for `doctor --fix` in a `.phantom.toml` directory;
//! - env isolation, fail closed on every bad answer, no ref names or
//!   stand-in values in stdout, stderr or `--json`;
//! - resolution and other commands never run the probe.
#![cfg(unix)]

use locus_core::{Binding, BindingBody, Policy, ProviderBinding, Scope, Store};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

const CANARY: &str = "sk-canary-phm-names";

fn provider(name: &str, r: &str) -> ProviderBinding {
    ProviderBinding {
        provider: name.into(),
        account: format!("{name}-acct"),
        credential_ref: r.into(),
        scope: Scope::default(),
        upstream: None,
    }
}

struct Env {
    home: TempDir,
    proj: TempDir,
    bin: TempDir,
    log: PathBuf,
}

/// `body` is the stand-in's behaviour after it logs argv and env.
fn setup(body: &str, phantom_project: bool) -> Env {
    let home = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    let bin = tempfile::tempdir().unwrap();
    let store = Store::open(home.path()).unwrap();
    let b = Binding::from_body(BindingBody {
        id: "bnd_acme".into(),
        alias: "acme".into(),
        tenant: "acme".into(),
        principal: None,
        description: None,
        policy: Policy::default(),
        providers: vec![
            provider("github", "phm:GITHUB_TOKEN"),
            provider("openai", "phm:OPENAI_API_KEY"),
        ],
    });
    fs::write(store.bindings_dir().join("acme.toml"), b.to_toml().unwrap()).unwrap();
    if phantom_project {
        fs::write(
            proj.path().join(".phantom.toml"),
            "[phantom]\nproject_id = \"t\"\n",
        )
        .unwrap();
    }
    let log = bin.path().join("calls.log");
    let script = format!(
        "#!/bin/sh\necho \"ARGV $*\" >> '{log}'\nenv | sed 's/^/ENV /' >> '{log}'\n{body}\n",
        log = log.display()
    );
    let p = bin.path().join("phantom");
    fs::write(&p, script).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    Env {
        home,
        proj,
        bin,
        log,
    }
}

impl Env {
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_locus"))
            .args(args)
            .current_dir(self.proj.path())
            .env_clear()
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.bin.path().display()),
            )
            .env("HOME", self.home.path())
            .env("LOCUS_HOME", self.home.path())
            .env("NO_COLOR", "1")
            // Planted: none of these may reach phantom.
            .env("LOCUS_TEST_SECRET", CANARY)
            .env("PHANTOM_PROXY_TOKEN", CANARY)
            .env("GITHUB_TOKEN", CANARY)
            .output()
            .expect("locus runs")
    }
    fn calls(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }
    fn argvs(&self) -> Vec<String> {
        self.calls()
            .lines()
            .filter_map(|l| l.strip_prefix("ARGV "))
            .map(str::to_string)
            .collect()
    }
    fn fix_json(&self) -> (serde_json::Value, String) {
        let out = self.run(&["--json", "doctor", "--fix"]);
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(out.status.success(), "{stdout}{stderr}");
        let all = format!("{stdout}{stderr}");
        (serde_json::from_str(&stdout).unwrap(), all)
    }
}

fn codes(v: &serde_json::Value) -> Vec<(String, String)> {
    v["plan"]["report_only"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["code"].as_str().unwrap().contains("phm"))
        .map(|r| {
            (
                r["provider"].as_str().unwrap_or("").to_string(),
                r["code"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// Fails if any argv other than exactly `list --json` reached the stand-in.
fn assert_only_list_json(e: &Env) {
    for a in e.argvs() {
        assert_eq!(a, "list --json", "phantom was called with: {a}");
        assert!(!a.contains("reveal") && !a.contains("exec"));
    }
}

fn assert_no_leak(all: &str) {
    for s in [CANARY, "GITHUB_TOKEN", "OPENAI_API_KEY"] {
        assert!(!all.contains(s), "output contains {s}: {all}");
    }
}

#[test]
fn lists_one_name_reports_the_other_as_dangling() {
    let e = setup(
        r#"printf '[{"name":"GITHUB_TOKEN","detected_service":"github"}]'"#,
        true,
    );
    let (v, all) = e.fix_json();
    assert_eq!(
        v["phantom_names"],
        serde_json::json!({"status":"listed","count":1})
    );
    assert_eq!(
        codes(&v),
        vec![("openai".to_string(), "dangling_phm_ref".to_string())]
    );
    assert_eq!(e.argvs(), vec!["list --json".to_string()]);
    assert_no_leak(&all);
}

#[test]
fn lists_both_names_no_phm_findings() {
    let e = setup(
        r#"printf '[{"name":"GITHUB_TOKEN"},{"name":"OPENAI_API_KEY"}]'"#,
        true,
    );
    let (v, _) = e.fix_json();
    assert_eq!(v["phantom_names"]["status"], "listed");
    assert!(codes(&v).is_empty(), "{v}");
}

#[test]
fn every_bad_answer_fails_closed_with_its_reason() {
    let cases: &[(&str, bool, &str)] = &[
        ("printf '[]'", false, "no_phantom_project"),
        ("printf '[]'", true, "empty_list"),
        ("exit 1", true, "exit_nonzero"),
        ("printf 'junk'", true, "bad_output"),
        ("printf '{}'", true, "bad_output"),
        (r#"printf '[{"name":"lower_case"}]'"#, true, "bad_output"),
        ("sleep 6", true, "timeout"),
    ];
    for (body, project, reason) in cases {
        let e = setup(body, *project);
        let (v, all) = e.fix_json();
        assert_eq!(
            v["phantom_names"],
            serde_json::json!({"status":"unavailable","reason":reason}),
            "{body}"
        );
        let c = codes(&v);
        assert_eq!(c.len(), 2, "{body}: {v}");
        assert!(
            c.iter().all(|(_, code)| code == "phm_unavailable"),
            "{body}: {v}"
        );
        assert_only_list_json(&e);
        assert_no_leak(&all);
        if !*project {
            assert!(e.argvs().is_empty(), "probe ran without .phantom.toml");
        }
    }
    let long = format!(r#"printf '[{{"name":"{}"}}]'"#, "A".repeat(300));
    let e = setup(&long, true);
    assert_eq!(e.fix_json().0["phantom_names"]["reason"], "bad_output");
}

#[test]
fn phantom_not_installed() {
    let e = setup("true", true);
    fs::remove_file(e.bin.path().join("phantom")).unwrap();
    let (v, _) = e.fix_json();
    assert_eq!(v["phantom_names"]["reason"], "not_installed");
}

#[test]
fn argv_and_env_are_isolated() {
    let e = setup(r#"printf '[{"name":"GITHUB_TOKEN"}]'"#, true);
    e.fix_json();
    assert_only_list_json(&e);
    let calls = e.calls();
    let env: Vec<&str> = calls
        .lines()
        .filter_map(|l| l.strip_prefix("ENV "))
        .collect();
    assert!(!env.is_empty());
    for line in &env {
        assert!(!line.contains(CANARY), "{line}");
        assert!(!line.starts_with("PHANTOM_"), "{line}");
        assert!(!line.starts_with("LOCUS_"), "{line}");
        assert!(!line.starts_with("GITHUB_TOKEN"), "{line}");
    }
}

#[test]
fn stand_in_values_and_stderr_never_reach_output() {
    let e = setup(
        &format!(
            r#"echo "{CANARY} OPENAI_API_KEY" >&2; printf '[{{"name":"GITHUB_TOKEN","value":"{CANARY}"}}]'"#
        ),
        true,
    );
    let (v, all) = e.fix_json();
    assert_eq!(v["phantom_names"]["status"], "listed");
    assert_no_leak(&all);
    let human = e.run(&["doctor", "--fix"]);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&human.stdout),
        String::from_utf8_lossy(&human.stderr)
    );
    assert!(text.contains("phantom names: listed (1)"), "{text}");
    assert_no_leak(&text);
}

#[test]
fn apply_with_a_dangling_ref_changes_nothing() {
    let e = setup(r#"printf '[{"name":"GITHUB_TOKEN"}]'"#, true);
    let snap = |d: &Path| {
        let mut v: Vec<_> = walk(d)
            .into_iter()
            .map(|p| (p.clone(), fs::read(&p).unwrap()))
            .collect();
        v.sort();
        v
    };
    let before = (snap(e.home.path()), snap(e.proj.path()));
    let out = e.run(&["doctor", "--fix", "--apply"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!((snap(e.home.path()), snap(e.proj.path())), before);
}

fn walk(d: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(d) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else if p.extension().and_then(|ext| ext.to_str()) != Some("profraw") {
                // An instrumented binary writes default_*.profraw into its
                // working directory once the child env drops LLVM_PROFILE_FILE.
                out.push(p);
            }
        }
    }
    out
}

/// No hot-path spawn: other commands, including the plain doctor and
/// binding reads, never run `phantom list --json`, even in a Phantom project.
#[test]
fn other_commands_never_run_the_probe() {
    let e = setup(r#"printf '[{"name":"GITHUB_TOKEN"}]'"#, true);
    for args in [
        vec!["status"],
        vec!["binding", "list"],
        vec!["binding", "show", "acme"],
        vec!["doctor"],
        vec!["--json", "doctor"],
    ] {
        let _ = e.run(&args);
    }
    assert!(
        !e.argvs().iter().any(|a| a == "list --json"),
        "{:?}",
        e.argvs()
    );
}

/// Structural check: the probe has exactly one call site, inside
/// `cmd_doctor_fix`, and locus-core (where bindings resolve) never spawns
/// `phantom list`.
#[test]
fn probe_has_one_call_site_in_doctor_fix() {
    let main = include_str!("../src/main.rs");
    assert_eq!(main.matches("phm_names::probe(").count(), 1);
    let start = main.find("fn cmd_doctor_fix(").unwrap();
    let end = start + main[start..].find("\n}\n").unwrap();
    assert!(main[start..end].contains("phm_names::probe("));
    let core = Path::new(env!("CARGO_MANIFEST_DIR")).join("../locus-core/src");
    for f in walk(&core) {
        let text = fs::read_to_string(&f).unwrap_or_default();
        assert!(
            !text.contains("\"list\", \"--json\""),
            "{} spawns phantom list",
            f.display()
        );
    }
}

/// An empty list with no phm refs is a valid answer, not `empty_list`.
#[test]
fn empty_list_without_phm_refs_is_listed() {
    let e = setup("printf '[]'", true);
    let b = Binding::from_body(BindingBody {
        id: "bnd_acme".into(),
        alias: "acme".into(),
        tenant: "acme".into(),
        principal: None,
        description: None,
        policy: Policy::default(),
        providers: vec![provider("github", "env:LOCUS_EMPTY_LIST_TOKEN")],
    });
    fs::write(
        e.home.path().join("bindings").join("acme.toml"),
        b.to_toml().unwrap(),
    )
    .unwrap();
    let (v, _) = e.fix_json();
    assert_eq!(
        v["phantom_names"],
        serde_json::json!({"status":"listed","count":0})
    );
    assert!(codes(&v).is_empty(), "{v}");
}

#[test]
fn exited_parent_with_inherited_stdout_still_obeys_deadline() {
    let e = setup("sleep 8 &\nprintf '[]'\nexit 0", true);
    let started = std::time::Instant::now();
    let (v, _) = e.fix_json();
    assert_eq!(v["phantom_names"]["reason"], "timeout");
    assert!(started.elapsed() < std::time::Duration::from_secs(6));
}

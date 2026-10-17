//! E2E: a `phm:` credential ref that Phantom lists but refuses to reveal must
//! be visible. `locus doctor` flags it (with its cause) instead of reporting
//! "credentials ok", plain `locus exec` warns naming the binding and env vars,
//! and `locus exec --strict-creds` fails. Uses a fake `phantom` on PATH.
//! Never prints values: the fake reveal writes a canary to stdout that must
//! not appear anywhere.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output};

const CANARY: &str = "phm-canary-value-do-not-print";
const LISTED: &str = "SUPABASE_PERSONAL GH_TOKEN_PERSONAL VERCEL_TOKEN_PERSONAL \
                      SUPABASE_ACME GH_TOKEN_ACME VERCEL_TOKEN_ACME";

struct Fixture {
    home: tempfile::TempDir,
    cwd: tempfile::TempDir,
    _shim: tempfile::TempDir,
    path_env: String,
    cap: String,
}

/// `list_names`: what `phantom list` prints. `reveal_ok`: whether reveal
/// succeeds (prints the canary, exit 0) or refuses (canary on stdout, exit 1,
/// like a refusing binary that still writes something).
fn fixture(list_names: &str, reveal_ok: bool) -> Fixture {
    let shim = tempfile::tempdir().unwrap();
    let script = format!(
        "#!/bin/sh\n\
         case \"$1\" in\n\
           --version) echo 'phantom 0.7.9'; exit 0;;\n\
           list) for n in {list_names}; do echo \"$n\"; done; exit 0;;\n\
           reveal) echo '{CANARY}'; echo 'reveal requires an attached terminal' >&2; exit {code};;\n\
         esac\n\
         exit 2\n",
        code = if reveal_ok { 0 } else { 1 },
    );
    let bin = shim.path().join("phantom");
    std::fs::write(&bin, script).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path_env = format!(
        "{}:{}",
        shim.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let mut f = Fixture {
        home,
        cwd,
        _shim: shim,
        path_env,
        cap: String::new(),
    };
    let qs = f.locus(&["quickstart"]);
    assert!(qs.status.success(), "quickstart failed: {qs:?}");
    f.cap = std::fs::read_to_string(f.home.path().join("control_capability"))
        .expect("quickstart persists the control capability")
        .trim()
        .to_string();
    f
}

impl Fixture {
    fn locus(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_locus"));
        cmd.args(args)
            .env("LOCUS_HOME", self.home.path())
            .env("PATH", &self.path_env)
            .env("NO_COLOR", "1")
            .env_remove("LOCUS_PHANTOM_PROJECT")
            .env_remove("LOCUS_CONTROL_CAPABILITY")
            .current_dir(self.cwd.path());
        if !self.cap.is_empty() {
            cmd.env("LOCUS_CONTROL_CAPABILITY", &self.cap);
        }
        cmd.output().expect("locus binary runs")
    }
}

fn text(out: &Output) -> (String, String) {
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn assert_no_secret_material(all: &str) {
    assert!(!all.contains(CANARY), "secret value leaked: {all}");
    for name in LISTED.split_whitespace() {
        assert!(!all.contains(name), "locator {name} leaked: {all}");
    }
}

fn doctor_json(f: &Fixture) -> serde_json::Value {
    let out = f.locus(&["doctor", "--json"]);
    let (stdout, stderr) = text(&out);
    assert_no_secret_material(&format!("{stdout}{stderr}"));
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("doctor json: {e}: {stdout}"))
}

fn codes(report: &serde_json::Value) -> Vec<String> {
    report["unresolved_phm"]
        .as_array()
        .expect("unresolved_phm array")
        .iter()
        .map(|i| i["code"].as_str().unwrap_or_default().to_string())
        .collect()
}

const CHILD: &str = "if [ -n \"$GH_TOKEN\" ]; then echo GH_TOKEN=set; else echo GH_TOKEN=unset; fi";

#[test]
fn refused_reveal_is_flagged_by_doctor_and_exec() {
    let f = fixture(LISTED, false);

    // doctor --json: flagged with cause code, not SAFE.
    let report = doctor_json(&f);
    let codes = codes(&report);
    assert!(
        !codes.is_empty() && codes.iter().all(|c| c == "reveal-failed"),
        "refused reveal must be flagged as reveal-failed: {report}"
    );
    assert_ne!(report["verdict"], "SAFE", "{report}");
    assert!(
        report["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["code"] == "unresolved_phm"),
        "{report}"
    );

    // Human doctor: no "credentials  ok", cause line present.
    let out = f.locus(&["doctor"]);
    let (stdout, stderr) = text(&out);
    assert_no_secret_material(&format!("{stdout}{stderr}"));
    assert!(!stdout.contains("credentials  ok"), "{stdout}");
    assert!(
        stdout.contains("cause reveal-failed: phantom refused or failed `reveal`"),
        "{stdout}"
    );

    // Plain exec: child runs (exit 0) without the var, stderr names binding,
    // env vars and cause.
    let out = f.locus(&["exec", "--", "sh", "-c", CHILD]);
    let (stdout, stderr) = text(&out);
    assert_no_secret_material(&format!("{stdout}{stderr}"));
    assert!(out.status.success(), "non-strict exec still runs: {stderr}");
    assert!(stdout.contains("GH_TOKEN=unset"), "{stdout}");
    assert!(
        stderr.contains("binding=personal provider=github env=GH_TOKEN,GITHUB_TOKEN"),
        "{stderr}"
    );
    assert!(stderr.contains("code=reveal-failed"), "{stderr}");

    // Strict exec: fails before the child runs.
    let out = f.locus(&["exec", "--strict-creds", "--", "sh", "-c", CHILD]);
    let (stdout, stderr) = text(&out);
    assert_no_secret_material(&format!("{stdout}{stderr}"));
    assert!(!out.status.success(), "strict exec must fail: {stdout}");
    assert!(
        !stdout.contains("GH_TOKEN="),
        "child must not run: {stdout}"
    );
    assert!(
        stderr.contains("--strict-creds") && stderr.contains("env=GH_TOKEN"),
        "{stderr}"
    );
}

#[test]
fn absent_name_is_reported_as_not_listed() {
    let f = fixture("", false);

    let report = doctor_json(&f);
    let codes = codes(&report);
    assert!(
        !codes.is_empty() && codes.iter().all(|c| c == "unavailable"),
        "{report}"
    );

    let out = f.locus(&["doctor"]);
    let (stdout, _) = text(&out);
    assert!(stdout.contains("not listed by `phantom list`"), "{stdout}");

    let out = f.locus(&["exec", "--", "sh", "-c", CHILD]);
    let (stdout, stderr) = text(&out);
    assert!(out.status.success(), "{stderr}");
    assert!(
        stderr.contains("env=GH_TOKEN") && stderr.contains("not listed by `phantom list`"),
        "{stderr}"
    );
    assert_no_secret_material(&format!("{stdout}{stderr}"));
}

#[test]
fn working_reveal_stays_clean() {
    let f = fixture(LISTED, true);

    let report = doctor_json(&f);
    assert!(codes(&report).is_empty(), "{report}");

    let out = f.locus(&["doctor"]);
    let (stdout, _) = text(&out);
    assert!(stdout.contains("credentials  ok"), "{stdout}");

    let out = f.locus(&["exec", "--strict-creds", "--", "sh", "-c", CHILD]);
    let (stdout, stderr) = text(&out);
    assert!(out.status.success(), "{stderr}");
    assert!(stdout.contains("GH_TOKEN=set"), "{stdout}");
    assert!(!stderr.contains("unresolved credential"), "{stderr}");
}

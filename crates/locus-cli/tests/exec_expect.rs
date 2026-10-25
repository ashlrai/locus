//! E2E: `locus exec --expect <alias>` refuses (exit 1, the documented
//! pre-spawn refusal code) unless the pinned binding is that alias. The child
//! never starts on refusal, and the refusal names aliases only.

#![cfg(unix)]

use std::path::PathBuf;
use std::process::{Command, Output};

struct Fixture {
    home: tempfile::TempDir,
    cwd: tempfile::TempDir,
    cap: String,
}

fn fixture() -> Fixture {
    let mut f = Fixture {
        home: tempfile::tempdir().unwrap(),
        cwd: tempfile::tempdir().unwrap(),
        cap: String::new(),
    };
    let qs = f.locus(&["quickstart"]);
    assert!(qs.status.success(), "quickstart failed: {qs:?}");
    f.cap = std::fs::read_to_string(f.home.path().join("control_capability"))
        .unwrap()
        .trim()
        .to_string();
    f
}

impl Fixture {
    fn locus(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_locus"));
        cmd.args(args)
            .env("LOCUS_HOME", self.home.path())
            .env("NO_COLOR", "1")
            .env_remove("LOCUS_PHANTOM_PROJECT")
            .env_remove("LOCUS_CONTROL_CAPABILITY")
            .current_dir(self.cwd.path());
        if !self.cap.is_empty() {
            cmd.env("LOCUS_CONTROL_CAPABILITY", &self.cap);
        }
        cmd.output().expect("locus binary runs")
    }
    fn marker(&self) -> PathBuf {
        self.cwd.path().join("child-ran")
    }
    fn exec_expect(&self, alias: &str, json: bool) -> Output {
        let touch = format!("touch '{}'", self.marker().display());
        let mut args = vec![];
        if json {
            args.push("--json");
        }
        args.extend(["exec", "--expect", alias, "--", "sh", "-c", touch.as_str()]);
        self.locus(&args)
    }
    fn pinned(&self) -> String {
        let out = self.locus(&["status", "--oneline"]);
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn no_refs(t: &str) {
    for needle in ["env:", "phm:", "keychain:", "credential_ref"] {
        assert!(!t.contains(needle), "refusal shows {needle}: {t}");
    }
}

#[test]
fn matching_alias_runs_the_child() {
    let f = fixture();
    let pinned = f.pinned();
    let alias = pinned.split(':').next().unwrap().to_string();
    let out = f.exec_expect(&alias, false);
    assert!(out.status.success(), "pinned={pinned} {}", text(&out));
    assert!(f.marker().exists());
}

#[test]
fn mismatch_refuses_with_exit_1_and_never_runs_the_child() {
    let f = fixture();
    let alias = f.pinned().split(':').next().unwrap().to_string();
    let other = if alias == "acme" { "personal" } else { "acme" };
    let out = f.exec_expect(other, false);
    let t = text(&out);
    assert_eq!(out.status.code(), Some(1), "{t}");
    assert!(!f.marker().exists(), "child ran on refusal");
    assert!(
        t.contains(&format!(
            "expected binding '{other}' but '{alias}' is pinned"
        )),
        "{t}"
    );
    assert!(t.contains(&format!("locus enter {other}")), "{t}");
    no_refs(&t);
}

#[test]
fn no_pin_refuses_and_never_runs_the_child() {
    let f = fixture();
    let left = f.locus(&["leave", "--force"]);
    assert!(left.status.success(), "{}", text(&left));
    let out = f.exec_expect("acme", false);
    let t = text(&out);
    assert_eq!(out.status.code(), Some(1), "{t}");
    assert!(!f.marker().exists());
    assert!(t.contains("no binding is pinned"), "{t}");
    no_refs(&t);
}

#[test]
fn unknown_alias_names_known_aliases_and_a_suggestion() {
    let f = fixture();
    let out = f.exec_expect("acm", false);
    let t = text(&out);
    assert_eq!(out.status.code(), Some(1), "{t}");
    assert!(!f.marker().exists());
    assert!(t.contains("'acm' is not a known binding"), "{t}");
    assert!(t.contains("did you mean `acme`?"), "{t}");
    no_refs(&t);
}

#[test]
fn junk_alias_is_not_echoed() {
    let f = fixture();
    let out = f.exec_expect("$(rm -rf x)\u{1b}[31m", false);
    let t = text(&out);
    assert_eq!(out.status.code(), Some(1), "{t}");
    assert!(!f.marker().exists());
    assert!(t.contains("<invalid alias>"), "{t}");
    assert!(!t.contains("rm -rf"), "{t}");
}

#[test]
fn json_refusal_is_an_error_document() {
    let f = fixture();
    let alias = f.pinned().split(':').next().unwrap().to_string();
    let other = if alias == "acme" { "personal" } else { "acme" };
    let out = f.exec_expect(other, true);
    assert_eq!(out.status.code(), Some(1));
    assert!(!f.marker().exists());
    let all = text(&out);
    let line = all
        .lines()
        .find(|l| l.trim_start().starts_with('{'))
        .expect(&all);
    let v: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!(v["ok"], false);
    assert!(
        v["error"].as_str().unwrap().starts_with("pin_mismatch:"),
        "{v}"
    );
    no_refs(&all);
}

#[test]
fn frozen_pin_refuses_even_when_the_alias_matches() {
    let f = fixture();
    let alias = f.pinned().split(':').next().unwrap().to_string();
    // Edit the pinned binding under the pin: drift freezes the session.
    let path = f.home.path().join("bindings").join(format!("{alias}.toml"));
    let mut toml = std::fs::read_to_string(&path).unwrap();
    toml.push_str("\n# drift\n");
    toml = toml.replacen(&format!("tenant = \"{alias}\""), "tenant = \"drifted\"", 1);
    std::fs::write(&path, toml).unwrap();
    let out = f.exec_expect(&alias, false);
    let t = text(&out);
    assert_eq!(out.status.code(), Some(1), "{t}");
    assert!(!f.marker().exists(), "child ran under a frozen pin");
    no_refs(&t);
}

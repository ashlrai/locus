//! Shared phantom+locus integration test (merge queue, Tier-1 credential path).
//!
//! Runs only when `PHANTOM_BIN` points at a phantom binary (CI uses a released,
//! sha256-pinned one). Without it the test prints a skip note and passes, so
//! local `cargo nextest` keeps working. It never falls back to a `phantom` on
//! PATH, which may be a self-updating shim rather than the pinned release.
//!
//! Every secret here is a DUMMY canary generated per run. `phantom reveal` is
//! never called; a logging shim in front of phantom records every argv locus
//! or this test passes to it, and the test fails if `reveal` ever appears.
#![cfg(unix)]

#[path = "../../crates/locus-cli/tests/handshake_conformance.rs"]
#[allow(dead_code)]
mod conformance;

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct World {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    proj: PathBuf,
    bin: PathBuf,
    shim_log: PathBuf,
    canary: String,
    cap: String,
    outputs: String,
}

fn canary() -> String {
    let seed = format!("{:?}-{}", std::time::SystemTime::now(), std::process::id());
    let h: String = Sha256::digest(seed.as_bytes())
        .iter()
        .take(12)
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("canary-FAKE-not-a-secret-{h}")
}

impl World {
    fn path_var(&self) -> String {
        format!("{}:/usr/bin:/bin", self.bin.display())
    }

    fn base(&self, program: &Path) -> Command {
        let mut c = Command::new(program);
        c.current_dir(&self.proj)
            .env_clear()
            .env("PATH", self.path_var())
            .env("HOME", &self.home)
            .env("LOCUS_HOME", self.home.join(".locus"))
            .env("NO_COLOR", "1");
        if !self.cap.is_empty() {
            c.env("LOCUS_CONTROL_CAPABILITY", &self.cap);
        }
        c
    }

    fn record(&mut self, out: &Output) -> (String, String) {
        let so = String::from_utf8_lossy(&out.stdout).to_string();
        let se = String::from_utf8_lossy(&out.stderr).to_string();
        self.outputs.push_str(&so);
        self.outputs.push_str(&se);
        (so, se)
    }

    fn locus(&mut self, args: &[&str]) -> (Output, String, String) {
        let out = self
            .base(Path::new(env!("CARGO_BIN_EXE_locus")))
            .args(args)
            .output()
            .expect("locus runs");
        let (so, se) = self.record(&out);
        (out, so, se)
    }

    /// `phantom exec -- locus <args>`, through the logging shim.
    fn phantom_exec_locus(&mut self, args: &[&str]) -> (Output, String, String) {
        let out = self
            .base(&self.bin.join("phantom"))
            .arg("exec")
            .arg("--")
            .arg(env!("CARGO_BIN_EXE_locus"))
            .args(args)
            .output()
            .expect("phantom exec runs");
        let (so, se) = self.record(&out);
        (out, so, se)
    }
}

fn write_binding(home: &Path, alias: &str, credential_ref: &str) {
    let body = format!(
        "id = \"bnd_{alias}\"\nalias = \"{alias}\"\ntenant = \"{alias}\"\n\n\
         [[providers]]\nprovider = \"openai\"\naccount = \"{alias}\"\n\
         credential_ref = \"{credential_ref}\"\n"
    );
    fs::write(
        home.join(".locus")
            .join("bindings")
            .join(format!("{alias}.toml")),
        body,
    )
    .unwrap();
}

fn walk(d: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(d) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(m) = fs::symlink_metadata(&p) else {
            continue;
        };
        if m.is_dir() {
            walk(&p, out);
        } else if m.is_file() {
            out.push(p);
        }
    }
}

fn setup(phantom: &Path) -> World {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let proj = tmp.path().join("proj");
    let bin = tmp.path().join("bin");
    for d in [&home, &proj, &bin] {
        fs::create_dir_all(d).unwrap();
    }
    let shim_log = tmp.path().join("phantom-argv.log");
    let shim = format!(
        "#!/bin/sh\nprintf 'ARGV %s\\n' \"$*\" >> '{}'\nexec '{}' \"$@\"\n",
        shim_log.display(),
        phantom.display()
    );
    let shim_path = bin.join("phantom");
    fs::write(&shim_path, shim).unwrap();
    fs::set_permissions(&shim_path, fs::Permissions::from_mode(0o755)).unwrap();
    World {
        _tmp: tmp,
        home,
        proj,
        bin,
        shim_log,
        canary: canary(),
        cap: String::new(),
        outputs: String::new(),
    }
}

#[test]
fn phantom_and_locus_compose_without_leaking_the_canary() {
    let Some(phantom) = std::env::var_os("PHANTOM_BIN").map(PathBuf::from) else {
        eprintln!("skip: PHANTOM_BIN is not set (merge-queue job sets a pinned phantom)");
        return;
    };
    if !phantom.is_file() {
        eprintln!("skip: PHANTOM_BIN={} does not exist", phantom.display());
        return;
    }
    let mut w = setup(&phantom);
    let canary = w.canary.clone();

    // 1. phantom init imports a DUMMY canary from .env and rewrites it.
    let _ = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&w.proj)
        .status();
    fs::write(w.proj.join(".env"), format!("OPENAI_API_KEY={canary}\n")).unwrap();
    let out = w
        .base(&w.bin.join("phantom"))
        .args(["init", "--json"])
        .output()
        .unwrap();
    let (so, se) = w.record(&out);
    assert!(out.status.success(), "phantom init: {so}{se}");
    let env_after = fs::read_to_string(w.proj.join(".env")).unwrap();
    assert!(
        !env_after.contains(&canary),
        "phantom left the canary in .env"
    );

    // 2. locus quickstart, then a phm: binding and an env: binding.
    let (out, so, se) = w.locus(&["quickstart"]);
    assert!(out.status.success(), "quickstart: {so}{se}");
    w.cap = fs::read_to_string(w.home.join(".locus").join("control_capability"))
        .unwrap()
        .trim()
        .to_string();
    assert!(!w.cap.is_empty());
    write_binding(&w.home, "phmbind", "phm:OPENAI_API_KEY");
    write_binding(&w.home, "envbind", "env:OPENAI_API_KEY");

    // 3. doctor --fix --json lists phantom names and finds the phm ref live.
    let (out, so, se) = w.locus(&["--json", "doctor", "--fix"]);
    assert!(out.status.success(), "doctor --fix: {so}{se}");
    let d: Value = serde_json::from_str(&so).unwrap();
    assert_eq!(d["phantom_names"]["status"], "listed", "{so}");
    assert!(
        d["phantom_names"]["count"].as_u64().unwrap_or(0) >= 1,
        "{so}"
    );
    let phm_findings: Vec<&Value> = d["plan"]["report_only"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["code"].as_str().unwrap_or("").contains("phm"))
        .collect();
    assert!(phm_findings.is_empty(), "phm findings: {phm_findings:?}");

    // 4. Wrong --expect refuses before any child, reveal or credential read.
    let (out, _, _) = w.locus(&["pin", "envbind"]);
    assert!(out.status.success());
    let marker = w.proj.join("child-ran");
    let touch = format!("touch '{}'", marker.display());
    let (out, so, se) =
        w.phantom_exec_locus(&["exec", "--expect", "personal", "--", "sh", "-c", &touch]);
    assert!(!out.status.success(), "wrong --expect ran: {so}{se}");
    assert!(se.contains("pin_mismatch"), "{se}");
    assert!(!marker.exists(), "child ran despite a wrong --expect");

    // 5. Right --expect under phantom exec: the child sees a placeholder only.
    let dump = "env; echo ARGV; tr '\\0' ' ' < /proc/self/cmdline; echo; \
                ps -eo args 2>/dev/null || true";
    let (out, so, se) =
        w.phantom_exec_locus(&["exec", "--expect", "envbind", "--", "sh", "-c", dump]);
    assert!(out.status.success(), "envbind exec: {so}{se}");
    let key = so
        .lines()
        .find_map(|l| l.strip_prefix("OPENAI_API_KEY="))
        .unwrap_or_else(|| panic!("child has no OPENAI_API_KEY: {so}"));
    assert!(
        key.starts_with("phm_"),
        "child key is not a phantom placeholder"
    );

    // 6. A phm: ref fails closed in locus exec: no reveal, no child.
    let (out, _, _) = w.locus(&["pin", "phmbind"]);
    assert!(out.status.success());
    let (out, so, se) =
        w.phantom_exec_locus(&["exec", "--expect", "phmbind", "--", "sh", "-c", &touch]);
    assert!(!out.status.success(), "phm exec ran: {so}{se}");
    assert!(se.contains("no Phantom credential was retrieved"), "{se}");
    assert!(!marker.exists());

    // 7. Handshakes against the vendored ashlr-stack contract.
    let (out, so, _) = w.locus(&["--ashlr-handshake", "--json"]);
    assert!(out.status.success());
    let h: Value = serde_json::from_str(&so).unwrap();
    assert_eq!(
        conformance::check(&h, &conformance::contract()),
        Vec::<String>::new()
    );
    let out = w
        .base(&w.bin.join("phantom"))
        .args(["--ashlr-handshake", "--json"])
        .output()
        .unwrap();
    let (so, _) = w.record(&out);
    if out.status.success() {
        let h: Value = serde_json::from_str(&so).unwrap();
        assert_eq!(
            conformance::check(&h, &conformance::contract()),
            Vec::<String>::new()
        );
    } else if std::env::var_os("PHANTOM_HANDSHAKE_REQUIRED").is_some() {
        panic!("phantom has no --ashlr-handshake and PHANTOM_HANDSHAKE_REQUIRED is set");
    } else {
        eprintln!("note: this phantom has no --ashlr-handshake; phantom handshake not checked");
    }

    // 8. The canary is absent everywhere locus could see it.
    let argv = fs::read_to_string(&w.shim_log).unwrap_or_default();
    assert!(
        !argv.contains("reveal"),
        "phantom reveal was called: {argv}"
    );
    assert!(!argv.contains(&canary), "canary in a phantom argv");
    assert!(!w.outputs.contains(&canary), "canary in a command output");
    let mut files = Vec::new();
    walk(&w.home.join(".locus"), &mut files);
    walk(&w.proj, &mut files);
    assert!(!files.is_empty());
    for f in files {
        let bytes = fs::read(&f).unwrap_or_default();
        assert!(
            !String::from_utf8_lossy(&bytes).contains(&canary),
            "canary found in {}",
            f.display()
        );
    }
}

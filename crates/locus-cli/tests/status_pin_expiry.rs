//! `locus status` on a real pinned session: the expiry countdown, the yellow
//! renew hint under 15 minutes, and `expires_at` / `expires_in_secs` in
//! `--json`. Pins use explicit `--ttl` so the remaining time is known; the
//! asserted bounds leave minutes of slack, so the tests are deterministic.

use std::path::Path;
use std::process::{Command, Output};

fn locus(home: &Path, cwd: &Path, cap: Option<&str>, color: bool, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_locus"));
    cmd.args(args)
        .env("LOCUS_HOME", home)
        .env_remove("LOCUS_CONTROL_CAPABILITY")
        .env_remove("NO_COLOR")
        .env_remove("CLICOLOR_FORCE")
        .current_dir(cwd);
    if color {
        cmd.env("CLICOLOR_FORCE", "1");
    } else {
        cmd.env("NO_COLOR", "1");
    }
    if let Some(cap) = cap {
        cmd.env("LOCUS_CONTROL_CAPABILITY", cap);
    }
    cmd.output().expect("locus binary runs")
}

/// Quickstart a temp home and pin `acme` for `ttl`; returns (home, cwd, cap).
fn pinned(ttl: &str) -> (tempfile::TempDir, tempfile::TempDir, String) {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let qs = locus(home.path(), cwd.path(), None, false, &["quickstart"]);
    assert!(qs.status.success(), "quickstart failed: {qs:?}");
    let cap = std::fs::read_to_string(home.path().join("control_capability"))
        .unwrap()
        .trim()
        .to_string();
    let enter = locus(
        home.path(),
        cwd.path(),
        Some(&cap),
        false,
        &["enter", "acme", "--ttl", ttl],
    );
    assert!(enter.status.success(), "enter failed: {enter:?}");
    (home, cwd, cap)
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn long_pin_shows_countdown_without_renew_hint() {
    let (home, cwd, cap) = pinned("2h");
    let out = stdout(&locus(
        home.path(),
        cwd.path(),
        Some(&cap),
        false,
        &["status"],
    ));
    assert!(
        out.contains("expires  in 1h 5") || out.contains("expires  in 2h 00m"),
        "countdown missing: {out}"
    );
    assert!(
        !out.contains("renew: locus enter"),
        "no hint expected: {out}"
    );

    let colored = stdout(&locus(
        home.path(),
        cwd.path(),
        Some(&cap),
        true,
        &["status"],
    ));
    let line = colored.lines().find(|l| l.contains("expires")).unwrap();
    assert!(
        !line.contains("\u{1b}[33m"),
        "long pin must not be yellow: {line:?}"
    );
}

#[test]
fn short_pin_is_yellow_with_renew_hint() {
    let (home, cwd, cap) = pinned("10m");
    let out = stdout(&locus(
        home.path(),
        cwd.path(),
        Some(&cap),
        false,
        &["status"],
    ));
    assert!(
        out.contains("expires  in 9m") || out.contains("expires  in 10m"),
        "countdown missing: {out}"
    );
    assert!(
        out.contains("(renew: locus enter acme)"),
        "hint missing: {out}"
    );

    let colored = stdout(&locus(
        home.path(),
        cwd.path(),
        Some(&cap),
        true,
        &["status"],
    ));
    let line = colored.lines().find(|l| l.contains("expires")).unwrap();
    assert!(
        line.contains("\u{1b}[33m"),
        "short pin must be yellow: {line:?}"
    );
}

#[test]
fn json_carries_expires_at_and_expires_in_secs() {
    let (home, cwd, cap) = pinned("10m");
    let before = chrono::Utc::now();
    let o = locus(
        home.path(),
        cwd.path(),
        Some(&cap),
        false,
        &["--json", "status"],
    );
    let v: serde_json::Value = serde_json::from_str(stdout(&o).trim()).expect("json");
    assert_eq!(v["schema"], "locus.status.v1");
    assert_eq!(v["pinned"], true);
    let secs = v["expires_in_secs"].as_i64().expect("expires_in_secs");
    assert!((480..=600).contains(&secs), "expires_in_secs {secs}");
    let at = chrono::DateTime::parse_from_rfc3339(v["expires_at"].as_str().unwrap()).unwrap();
    let delta = (at.with_timezone(&chrono::Utc) - before).num_seconds();
    assert!((480..=600).contains(&delta), "expires_at delta {delta}");
}

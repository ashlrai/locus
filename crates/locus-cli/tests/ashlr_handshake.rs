//! `locus --ashlr-handshake` prints one JSON line and exits 0, without a
//! subcommand and without touching LOCUS_HOME.
use std::process::Command;

#[test]
fn prints_one_json_line_and_creates_nothing() {
    let home = tempfile::tempdir().unwrap();
    let locus_home = home.path().join("never-created");
    let out = Command::new(env!("CARGO_BIN_EXE_locus"))
        .arg("--ashlr-handshake")
        .env("LOCUS_HOME", &locus_home)
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(stdout.trim().lines().count(), 1);
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(v["kind"], "ashlr.handshake");
    assert_eq!(v["command"], "locus");
    assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(v["health"]["status"], "degraded");
    assert!(!locus_home.exists());
}

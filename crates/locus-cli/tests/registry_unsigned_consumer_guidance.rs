//! Exercise the unsigned consumer guide's command and its fail-closed limits.

use std::process::Command;

const GUIDE: &str = include_str!("../../../docs/registry-unsigned-consumer-guidance.md");

#[test]
fn documented_command_checks_catalog_without_accepting_bad_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_locus"))
            .args(args)
            .current_dir(dir.path())
            .env("HOME", dir.path())
            .env("LOCUS_HOME", dir.path().join("locus-home"))
            .env_remove("LOCUS_ADAPTER_TRUST_KEYS")
            .env_remove("LOCUS_REGISTRY_SIGNING_KEY")
            .output()
            .unwrap()
    };
    let example = GUIDE
        .split("```bash\n")
        .nth(1)
        .unwrap()
        .split("\n```")
        .next()
        .unwrap();
    let args: Vec<_> = example.split_whitespace().collect();
    assert_eq!(args.len(), 5);
    assert_eq!(&args[..3], &["locus", "adapter", "verify-manifest"]);
    assert_eq!(args[4], "--allow-unsigned");
    let path = dir.path().join(args[3]);
    assert!(run(&["adapter", "registry", "export", "--out", args[3]])
        .status
        .success());

    let matching = run(&args[1..]);
    assert!(matching.status.success());
    let report = String::from_utf8(matching.stdout).unwrap();
    assert!(report.contains("unsigned") && report.contains("none"));
    assert!(!run(&args[1..4]).status.success());

    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    manifest["signature"] = "invalid".into();
    std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(!run(&args[1..]).status.success());
    manifest.as_object_mut().unwrap().remove("signature");
    manifest["adapters"][0]["name"] = "Drifted".into();
    std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(!run(&args[1..]).status.success());
    std::fs::write(&path, b"invalid JSON").unwrap();
    assert!(!run(&args[1..]).status.success());
}

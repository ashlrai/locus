//! Exercise the consumer command documented for the unsigned CI export.

use std::path::Path;
use std::process::Command;

const DOC: &str = include_str!("../../../docs/registry-manifest-ci-role.md");

#[test]
fn sources_and_follow_up_guides_exist() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for (link, file) in [
        ("./registry-trust.md", "docs/registry-trust.md"),
        ("../CHANGELOG.md#050--2026-08-16", "CHANGELOG.md"),
        (
            "./registry-manifest-verification.md",
            "docs/registry-manifest-verification.md",
        ),
        (
            "./registry-signing-open-questions.md",
            "docs/registry-signing-open-questions.md",
        ),
    ] {
        assert!(DOC.contains(&format!("]({link})")), "missing {link}");
        assert!(root.join(file).is_file(), "missing {file}");
    }
}

#[test]
fn documented_check_accepts_only_matching_unsigned_catalog() {
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
    assert!(
        run(&["adapter", "registry", "export", "--out", "release.json"])
            .status
            .success()
    );
    let path = dir.path().join("release.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(manifest.get("signature").is_none());
    assert!(manifest.get("signed_by").is_none());

    let example = DOC
        .split("```bash\n")
        .nth(1)
        .unwrap()
        .split("\n```")
        .next()
        .unwrap();
    let args: Vec<_> = example.split_whitespace().collect();
    assert_eq!(args.first(), Some(&"locus"));
    let output = run(&args[1..]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("unsigned") && stdout.contains("none"));
    assert!(!run(&["adapter", "verify-manifest", "release.json"])
        .status
        .success());

    manifest["locus_version"] = "0.0.0-drift".into();
    std::fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(!run(&args[1..]).status.success());
}

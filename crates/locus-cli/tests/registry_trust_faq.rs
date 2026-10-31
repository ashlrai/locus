//! Check the FAQ's source links and run its unsigned verification example.

use std::path::Path;
use std::process::Command;

const FAQ: &str = include_str!("../../../docs/registry-trust-faq.md");

#[test]
fn faq_links_to_registry_trust_and_the_changelog_note() {
    for (link, path) in [
        ("./registry-trust.md", "docs/registry-trust.md"),
        ("../CHANGELOG.md#050--2026-08-16", "CHANGELOG.md"),
    ] {
        assert!(FAQ.contains(&format!("]({link})")), "missing {link}");
        assert!(Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(path)
            .is_file());
    }
    assert!(include_str!("../../../CHANGELOG.md")
        .contains("**Signed adapter-registry release manifests**"));
}

#[test]
fn faq_unsigned_example_runs_and_rejects_drift() {
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
    let example = FAQ
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
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("unsigned") && text.contains("none"));
    assert!(!run(&["adapter", "verify-manifest", "release.json"])
        .status
        .success());

    let path = dir.path().join("release.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    manifest["locus_version"] = "0.0.0-drift".into();
    std::fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(!run(&args[1..]).status.success());
}

#[test]
fn verification_questions_example_is_drift_only_and_fails_closed() {
    let questions = include_str!("../../../docs/registry-manifest-verification-questions.md");
    for link in [
        "./registry-trust.md",
        "../CHANGELOG.md#050--2026-08-16",
        "./registry-manifest-verification.md",
    ] {
        assert!(questions.contains(&format!("]({link})")), "missing {link}");
    }
    let example = questions
        .split("```bash\n")
        .nth(1)
        .unwrap()
        .split("\n```")
        .next()
        .unwrap();
    let args: Vec<_> = example.split_whitespace().collect();
    assert_eq!(args.first(), Some(&"locus"));

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
    assert!(!run(&["adapter", "verify-manifest", "release.json"])
        .status
        .success());
    let output = run(&args[1..]);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("unsigned") && text.contains("none"));

    let path = dir.path().join("release.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    manifest["signature"] = "not-a-signature".into();
    std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(!run(&args[1..]).status.success());
    manifest.as_object_mut().unwrap().remove("signature");
    manifest["locus_version"] = "0.0.0-drift".into();
    std::fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(!run(&args[1..]).status.success());
}

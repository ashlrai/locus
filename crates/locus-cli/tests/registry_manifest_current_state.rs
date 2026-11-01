//! Exercise the current-state guide's unsigned verification example.

use std::path::Path;
use std::process::Command;

const GUIDE: &str = include_str!("../../../docs/registry-manifest-current-state.md");
const SIGNING_STATUS: &str = include_str!("../../../docs/registry-signing-status.md");

#[test]
fn signing_status_cites_sources_and_distinguishes_release_from_capability() {
    let trust = include_str!("../../../docs/registry-trust.md");
    let changelog = include_str!("../../../CHANGELOG.md");
    assert!(SIGNING_STATUS.contains("](./registry-trust.md#current-release-status)"));
    assert!(trust.contains("## Current release status"));
    assert!(SIGNING_STATUS.contains("](../CHANGELOG.md#050--2026-08-16)"));
    assert!(changelog.contains("## [0.5.0] — 2026-08-16"));
    assert!(trust.contains("](./registry-signing-status.md)"));
    for phrase in [
        "**without `--sign`**",
        "**unsigned**",
        "`--allow-unsigned`",
        "optional operator-managed process",
        "**future work**",
    ] {
        assert!(SIGNING_STATUS.contains(phrase), "missing {phrase:?}");
    }
}

#[test]
fn guide_links_to_sources_and_labels_release_signing_unimplemented() {
    for (link, path) in [
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
        assert!(GUIDE.contains(&format!("]({link})")), "missing {link}");
        assert!(Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(path)
            .is_file());
    }
    assert!(GUIDE.contains("**unsigned**"));
    assert!(GUIDE.contains("**unimplemented future work**"));
}

#[test]
fn documented_unsigned_check_rejects_unsigned_by_default_and_detects_drift() {
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
    let examples: Vec<Vec<&str>> = [GUIDE, SIGNING_STATUS]
        .iter()
        .map(|guide| {
            let example = guide
                .split("```bash\n")
                .nth(1)
                .unwrap()
                .split("\n```")
                .next()
                .unwrap();
            let args: Vec<_> = example.split_whitespace().collect();
            assert_eq!(args.first(), Some(&"locus"));
            args
        })
        .collect();
    for args in &examples {
        let output = run(&args[1..]);
        assert!(output.status.success());
        let report = String::from_utf8(output.stdout).unwrap();
        assert!(report.contains("unsigned") && report.contains("none"));
    }
    assert!(!run(&["adapter", "verify-manifest", "release.json"])
        .status
        .success());

    let path = dir.path().join("release.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    manifest["signature"] = "invalid-signature".into();
    std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    for args in &examples {
        assert!(!run(&args[1..]).status.success());
    }
    manifest.as_object_mut().unwrap().remove("signature");
    manifest["locus_version"] = "0.0.0-drift".into();
    std::fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    for args in &examples {
        assert!(!run(&args[1..]).status.success());
    }
}

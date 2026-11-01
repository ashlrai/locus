//! Check the documented release export against the CLI's built-in catalog.

use std::path::Path;
use std::process::Command;

const GUIDE: &str = include_str!("../../../docs/registry-manifest-provenance.md");

#[test]
fn provenance_guide_links_to_its_sources() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for (link, file) in [
        ("./registry-trust.md", "docs/registry-trust.md"),
        ("../adapters/manifest.toml", "adapters/manifest.toml"),
        (
            "./registry-manifest-ci-role.md",
            "docs/registry-manifest-ci-role.md",
        ),
        ("./registry-trust.md#trust-model", "docs/registry-trust.md"),
        (
            "./registry-manifest-verification.md",
            "docs/registry-manifest-verification.md",
        ),
    ] {
        assert!(GUIDE.contains(&format!("]({link})")), "missing {link}");
        assert!(root.join(file).is_file(), "missing {file}");
    }
    assert!(include_str!("../../../docs/registry-trust.md")
        .contains("](./registry-manifest-provenance.md)"));
}

#[test]
fn release_style_export_is_repeatable_unsigned_catalog_metadata() {
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
    for file in ["first.json", "second.json"] {
        assert!(run(&["adapter", "registry", "export", "--out", file])
            .status
            .success());
    }
    let first = std::fs::read(dir.path().join("first.json")).unwrap();
    let second = std::fs::read(dir.path().join("second.json")).unwrap();
    assert_eq!(first, second);
    let manifest: serde_json::Value = serde_json::from_slice(&first).unwrap();
    assert!(manifest.get("signature").is_none());
    assert!(manifest.get("signed_by").is_none());
    assert_eq!(
        manifest,
        serde_json::to_value(locus_core::build_release_manifest().unwrap()).unwrap()
    );
    assert!(!run(&["adapter", "verify-manifest", "first.json"])
        .status
        .success());
    assert!(run(&[
        "adapter",
        "verify-manifest",
        "first.json",
        "--allow-unsigned"
    ])
    .status
    .success());
}

//! Check the signing open-questions doc: its source links resolve, it
//! distinguishes the landed CLI signing capability from the unconfigured
//! release deployment, it never claims release signing exists, and its
//! current-state example runs as a drift-only, fail-closed check.

use std::path::Path;
use std::process::Command;

const DOC: &str = include_str!("../../../docs/registry-signing-open-questions.md");

#[test]
fn doc_links_resolve() {
    for (link, path) in [
        ("./registry-trust.md", "docs/registry-trust.md"),
        (
            "./registry-manifest-verification.md",
            "docs/registry-manifest-verification.md",
        ),
        (
            "./registry-manifest-verification-questions.md",
            "docs/registry-manifest-verification-questions.md",
        ),
        (
            "./adapter-sdk.md#signed-registry-roadmap",
            "docs/adapter-sdk.md",
        ),
        ("../CHANGELOG.md#050--2026-08-16", "CHANGELOG.md"),
    ] {
        assert!(DOC.contains(&format!("]({link})")), "missing {link}");
        assert!(Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(path)
            .is_file());
    }
    assert!(include_str!("../../../CHANGELOG.md")
        .contains("**Signed adapter-registry release manifests**"));
}

#[test]
fn doc_distinguishes_capability_from_deployment() {
    // Names the landed CLI capability.
    for needle in [
        "--sign",
        "--allow-unsigned",
        "LOCUS_REGISTRY_SIGNING_KEY",
        "ed25519",
        "verify-manifest",
    ] {
        assert!(
            DOC.contains(needle),
            "doc must name the signing capability: missing {needle:?}"
        );
    }
    // States the unsigned release deployment.
    for needle in [
        "locus-adapters-<tag>.json",
        "**without** `--sign`",
        "unsigned",
        "TODO(registry-signing)",
        "no `LOCUS_REGISTRY_SIGNING_KEY`",
        "no registry root public key is published",
    ] {
        assert!(
            DOC.contains(needle),
            "doc must state the unsigned deployment: missing {needle:?}"
        );
    }
    // Frames the transition as unresolved future work.
    for needle in ["unresolved", "future", "open questions", "Open questions"] {
        assert!(
            DOC.contains(needle),
            "doc must frame signing as unresolved: missing {needle:?}"
        );
    }
    // Never claims that release signing exists today.
    for claim in [
        "CI signs",
        "CI-signed",
        "signed by CI",
        "root is published",
        "releases are signed",
        "is signed today",
        "signing is enabled",
    ] {
        assert!(
            !DOC.contains(claim),
            "doc must not claim release signing exists: found {claim:?}"
        );
    }
}

#[test]
fn doc_example_is_drift_only_and_fails_closed() {
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

    // The doc's fenced example verifies the unsigned export.
    let example = DOC
        .split("```bash\n")
        .nth(1)
        .unwrap()
        .split("\n```")
        .next()
        .unwrap();
    let args: Vec<_> = example.split_whitespace().collect();
    assert_eq!(args.first(), Some(&"locus"));

    // Without --allow-unsigned the unsigned asset fails even when it matches.
    assert!(!run(&["adapter", "verify-manifest", "release.json"])
        .status
        .success());
    let output = run(&args[1..]);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("unsigned") && text.contains("none"));

    // A nonempty malformed signature still fails under --allow-unsigned.
    let path = dir.path().join("release.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    manifest["signature"] = "not-a-signature".into();
    std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(!run(&args[1..]).status.success());

    // Drift still fails under --allow-unsigned.
    manifest.as_object_mut().unwrap().remove("signature");
    manifest["locus_version"] = "0.0.0-drift".into();
    std::fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(!run(&args[1..]).status.success());
}

//! Check the consumer checklist's links and required facts (unsigned status,
//! intended use, verification limits), and run its verification example end
//! to end.

use std::path::Path;
use std::process::Command;

const CHECKLIST: &str = include_str!("../../../docs/registry-manifest-consumer-checklist.md");

#[test]
fn checklist_links_resolve() {
    for (link, path) in [
        ("./registry-trust.md", "docs/registry-trust.md"),
        (
            "./registry-manifest-verification.md",
            "docs/registry-manifest-verification.md",
        ),
        ("./registry-trust-faq.md", "docs/registry-trust-faq.md"),
    ] {
        assert!(CHECKLIST.contains(&format!("]({link})")), "missing {link}");
        assert!(Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(path)
            .is_file());
    }
}

#[test]
fn checklist_identifies_unsigned_manifest_use_and_limits() {
    // Identifies the unsigned release manifest.
    for needle in [
        "locus-adapters-<tag>.json",
        "unsigned",
        "--sign",
        "no published registry root",
    ] {
        assert!(
            CHECKLIST.contains(needle),
            "checklist must identify the unsigned manifest: missing {needle:?}"
        );
    }
    // States its intended use: a drift check of the catalog against the
    // running binary.
    for needle in ["catalog", "drift", "snapshot"] {
        assert!(
            CHECKLIST.contains(needle),
            "checklist must state the manifest's intended use: missing {needle:?}"
        );
    }
    // States the current verification limits.
    for needle in [
        "--allow-unsigned",
        "does not authenticate",
        "does not hash",
        "No freshness",
    ] {
        assert!(
            CHECKLIST.contains(needle),
            "checklist must state verification limits: missing {needle:?}"
        );
    }
    // Does not claim that release signing exists today.
    for claim in [
        "is signed",
        "signature valid",
        "signed release",
        "CI signs",
        "root is published",
    ] {
        assert!(
            !CHECKLIST.contains(claim),
            "checklist must not claim signing exists: found {claim:?}"
        );
    }
}

#[test]
fn checklist_verify_example_runs_and_rejects_drift() {
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

    // The checklist's fenced example verifies the unsigned asset.
    let example = CHECKLIST
        .split("```bash\n")
        .skip(1)
        .map(|b| b.split("\n```").next().unwrap())
        .find(|b| b.contains("verify-manifest"))
        .expect("checklist has a verify-manifest example");
    let cmdline = example
        .lines()
        .find(|l| l.contains("verify-manifest"))
        .unwrap();
    let args: Vec<String> = cmdline
        .split_whitespace()
        .map(|t| {
            if t.ends_with(".json") {
                "release.json".to_string()
            } else {
                t.to_string()
            }
        })
        .collect();
    assert_eq!(args.first().map(String::as_str), Some("locus"));
    let output = run(&args[1..].iter().map(String::as_str).collect::<Vec<_>>());
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("unsigned") && text.contains("none"));

    // Without --allow-unsigned the unsigned asset fails even when it matches.
    let strict_args: Vec<&str> = args[1..]
        .iter()
        .map(String::as_str)
        .filter(|a| *a != "--allow-unsigned")
        .collect();
    assert!(!run(&strict_args).status.success());

    // Drift still fails under --allow-unsigned.
    let path = dir.path().join("release.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    manifest["adapters"][0]["name"] = "Drifted Name".into();
    std::fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let drifted = run(&args[1..].iter().map(String::as_str).collect::<Vec<_>>());
    assert!(!drifted.status.success());
    let report = format!(
        "{}{}",
        String::from_utf8_lossy(&drifted.stdout),
        String::from_utf8_lossy(&drifted.stderr)
    );
    assert!(report.contains("drift"), "expected drift report: {report}");
}

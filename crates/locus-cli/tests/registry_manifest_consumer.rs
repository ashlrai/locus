//! Check the consumer guide's links and required facts, and run its
//! verification example against a real export.

use std::path::Path;
use std::process::Command;

const GUIDE: &str = include_str!("../../../docs/registry-manifest-consumer.md");

#[test]
fn guide_links_resolve() {
    for (link, path) in [
        ("./registry-trust.md", "docs/registry-trust.md"),
        (
            "./registry-manifest-consumer-checklist.md",
            "docs/registry-manifest-consumer-checklist.md",
        ),
        ("./registry-trust-faq.md", "docs/registry-trust-faq.md"),
    ] {
        assert!(GUIDE.contains(&format!("]({link})")), "missing {link}");
        assert!(Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(path)
            .is_file());
    }
}

#[test]
fn guide_states_unsigned_status_and_limits_without_claiming_signing() {
    for needle in [
        "unsigned",
        "no published registry root",
        "--allow-unsigned",
        "What you can verify",
        "What you cannot verify",
        "No freshness guarantee",
        "catalog metadata",
    ] {
        assert!(GUIDE.contains(needle), "missing {needle:?}");
    }
    for claim in [
        "is signed",
        "signature valid",
        "CI signs",
        "root is published",
    ] {
        assert!(!GUIDE.contains(claim), "must not claim signing: {claim:?}");
    }
}

#[test]
fn guide_example_verifies_unsigned_export_and_rejects_without_flag() {
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
    let cmdline = GUIDE
        .split("```bash\n")
        .skip(1)
        .map(|b| b.split("\n```").next().unwrap())
        .flat_map(str::lines)
        .find(|l| l.contains("verify-manifest"))
        .expect("guide has a verify-manifest example");
    let args: Vec<String> = cmdline
        .split_whitespace()
        .map(|t| {
            if t.ends_with(".json") {
                "release.json".into()
            } else {
                t.to_string()
            }
        })
        .collect();
    assert_eq!(args.first().map(String::as_str), Some("locus"));
    let out = run(&args[1..].iter().map(String::as_str).collect::<Vec<_>>());
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("unsigned") && text.contains("none"));
    let strict: Vec<&str> = args[1..]
        .iter()
        .map(String::as_str)
        .filter(|a| *a != "--allow-unsigned")
        .collect();
    assert!(!run(&strict).status.success());
}

//! Run the unsigned manifest FAQ's example against the real CLI.

use std::process::Command;

const FAQ: &str = include_str!("../../../docs/registry-unsigned-manifest-faq.md");

#[test]
fn faq_example_compares_catalog_but_does_not_accept_bad_signatures_or_drift() {
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
    let example = FAQ
        .split("```bash\n")
        .nth(1)
        .unwrap()
        .split("\n```")
        .next()
        .unwrap();
    let args: Vec<_> = example.split_whitespace().collect();
    assert_eq!(
        args,
        [
            "locus",
            "adapter",
            "verify-manifest",
            "release.json",
            "--allow-unsigned"
        ]
    );
    assert!(run(&["adapter", "registry", "export", "--out", args[3]])
        .status
        .success());

    assert!(!run(&args[1..4]).status.success());
    let matching = run(&args[1..]);
    assert!(matching.status.success());
    let report = String::from_utf8(matching.stdout).unwrap();
    assert!(report.contains("unsigned") && report.contains("none"));

    let path = dir.path().join(args[3]);
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    manifest["signed_by"] = "unverified-claim".into();
    std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(run(&args[1..]).status.success());

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

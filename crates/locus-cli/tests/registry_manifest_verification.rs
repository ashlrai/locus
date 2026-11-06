//! Run the verification walkthrough against a locally exported manifest.

use std::process::Command;

const GUIDE: &str = include_str!("../../../docs/registry-manifest-verification.md");
const TRUST_GUIDE: &str = include_str!("../../../docs/registry-trust.md");

#[test]
fn trust_guide_introduces_the_unsigned_release_boundary() {
    let boundary = TRUST_GUIDE
        .split("## Trust boundary at a glance\n")
        .nth(1)
        .unwrap()
        .split("\n## ")
        .next()
        .unwrap();
    for phrase in [
        "**unsigned**",
        "without `--sign`",
        "](../CHANGELOG.md#050--2026-08-16)",
        "future work",
        "rejects the unsigned asset",
        "`--allow-unsigned`",
        "binary running the",
        "does not accept a bad signature or drift",
        "operator can sign a separate manifest today",
    ] {
        assert!(boundary.contains(phrase), "missing {phrase:?}");
    }
}

#[test]
fn documented_unsigned_verification_fails_closed_on_invalid_input_and_drift() {
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
    for guide in [GUIDE, TRUST_GUIDE] {
        let example = guide
            .split("```bash\n")
            .nth(1)
            .unwrap()
            .split("```")
            .next()
            .unwrap();
        let args: Vec<_> = example.split_whitespace().collect();
        assert_eq!(args[0..3], ["locus", "adapter", "verify-manifest"]);
        assert_eq!(args.len(), 5);
        assert_eq!(args[4], "--allow-unsigned");
        let filename = args[3];
        assert!(run(&["adapter", "registry", "export", "--out", filename])
            .status
            .success());

        let matching = run(&args[1..]);
        assert!(matching.status.success());
        let report = String::from_utf8(matching.stdout).unwrap();
        assert!(report.contains("unsigned") && report.contains("none"));
        assert!(!run(&args[1..4]).status.success());

        let path = dir.path().join(filename);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        manifest["signature"] = "not-a-signature".into();
        std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(!run(&args[1..]).status.success());

        manifest.as_object_mut().unwrap().remove("signature");
        manifest["locus_version"] = "0.0.0-drift".into();
        std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(!run(&args[1..]).status.success());

        std::fs::write(&path, b"invalid JSON").unwrap();
        assert!(!run(&args[1..]).status.success());
    }
}

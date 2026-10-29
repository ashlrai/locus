//! Keep the documented minimum and verification command aligned with the manifests.

#[test]
fn changelog_rust_version_matches_workspace_and_member_manifests() {
    let workspace: toml::Value = toml::from_str(include_str!("../../../Cargo.toml")).unwrap();
    let version = workspace["workspace"]["package"]["rust-version"]
        .as_str()
        .expect("workspace declares rust-version");
    let version = if version.matches('.').count() == 1 {
        format!("{version}.0")
    } else {
        version.to_owned()
    };

    for manifest in [
        include_str!("../Cargo.toml"),
        include_str!("../../locus-cli/Cargo.toml"),
        include_str!("../../locus-mcp/Cargo.toml"),
    ] {
        let package: toml::Value = toml::from_str(manifest).unwrap();
        assert_eq!(
            package["package"]["rust-version"]["workspace"].as_bool(),
            Some(true)
        );
    }

    let changelog = include_str!("../../../CHANGELOG.md");
    let entry = changelog
        .split_once("**Rust version check.**")
        .expect("Rust version check is documented")
        .1
        .split("\n\n")
        .next()
        .unwrap();
    assert!(entry.contains(&format!("minimum supported Rust version is {version}")));
    assert!(entry.contains(&format!(
        "rustup toolchain install {version} --profile minimal"
    )));
    assert!(entry.contains(&format!("cargo +{version} check --workspace --locked")));
}

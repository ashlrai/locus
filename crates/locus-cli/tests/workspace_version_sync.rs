use std::path::Path;

fn section_line<'a>(manifest: &'a str, section: &str, key: &str) -> &'a str {
    let header = format!("[{section}]");
    let mut lines = manifest.lines().skip_while(|line| line.trim() != header);
    lines
        .next()
        .unwrap_or_else(|| panic!("missing {header} section"));
    lines
        .take_while(|line| !line.trim_start().starts_with('['))
        .find(|line| line.trim_start().starts_with(key))
        .unwrap_or_else(|| panic!("missing {key} in {header}"))
}

fn quoted_version(line: &str) -> &str {
    let (_, value) = line.split_once("version").expect("missing version");
    let (_, value) = value.split_once('"').expect("missing opening quote");
    value.split_once('"').expect("missing closing quote").0
}

#[test]
fn locus_core_version_matches_workspace_package() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    let manifest = std::fs::read_to_string(root).expect("read root Cargo.toml");
    let package_version = section_line(&manifest, "workspace.package", "version =");
    let core_dependency = section_line(&manifest, "workspace.dependencies", "locus-core =");

    assert_eq!(
        quoted_version(package_version),
        quoted_version(core_dependency),
        "locus-core dependency version must match workspace package version"
    );
}

#[test]
fn locus_mcp_version_matches_workspace_package() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    let workspace = std::fs::read_to_string(root).expect("read root Cargo.toml");
    let cli = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .expect("read CLI Cargo.toml");
    let package_version = section_line(&workspace, "workspace.package", "version =");
    let mcp_dependency = section_line(&cli, "dependencies", "locus-mcp =");

    assert_eq!(
        quoted_version(package_version),
        quoted_version(mcp_dependency),
        "locus-mcp dependency version must match workspace package version"
    );
}

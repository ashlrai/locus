use locus_core::adapters::AnthropicAdapter;
use std::path::Path;

fn version_occurrences(root: &Path) -> Vec<std::path::PathBuf> {
    let mut occurrences = Vec::new();
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            occurrences.extend(version_occurrences(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let source = std::fs::read_to_string(&path).unwrap();
            for _ in source.match_indices(AnthropicAdapter::API_VERSION) {
                occurrences.push(path.clone());
            }
        }
    }
    occurrences
}

#[test]
fn anthropic_api_version_has_one_workspace_source() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let crates = crate_root.parent().unwrap();
    assert_eq!(
        version_occurrences(crates),
        vec![crate_root.join("src/adapters/anthropic.rs")],
        "Use AnthropicAdapter::API_VERSION instead of repeating its literal"
    );
}

#[test]
fn guard_detects_duplicate_literals_in_nested_crates() {
    let fixture = tempfile::tempdir().unwrap();
    let nested = fixture.path().join("another-crate/src");
    std::fs::create_dir_all(&nested).unwrap();
    let source = nested.join("client.rs");
    std::fs::write(
        &source,
        format!(
            "{:?}\n{:?}",
            AnthropicAdapter::API_VERSION,
            AnthropicAdapter::API_VERSION
        ),
    )
    .unwrap();
    assert_eq!(
        version_occurrences(fixture.path()),
        vec![source.clone(), source]
    );
}

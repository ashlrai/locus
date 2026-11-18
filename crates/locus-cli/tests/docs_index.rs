//! Docs index. Every `docs/*.md` page must be linked from `docs/README.md`,
//! and every relative `.md` link in the index must resolve to a file.

use std::collections::BTreeSet;
use std::path::Path;

fn docs_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs")
}

/// Relative `.md` link targets in `index`, without anchors or directories.
fn linked_pages(index: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for part in index.split("](").skip(1) {
        let Some(target) = part.split(')').next() else {
            continue;
        };
        let target = target.split('#').next().unwrap_or("");
        if target.ends_with(".md") && !target.contains("://") && !target.contains('/') {
            out.insert(target.to_string());
        }
    }
    out
}

fn pages_on_disk() -> BTreeSet<String> {
    std::fs::read_dir(docs_dir())
        .expect("read docs dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".md") && n != "README.md")
        .collect()
}

#[test]
fn every_doc_page_is_linked_from_docs_readme() {
    let index = std::fs::read_to_string(docs_dir().join("README.md")).expect("docs/README.md");
    let linked = linked_pages(&index);
    let missing: Vec<_> = pages_on_disk().difference(&linked).cloned().collect();
    assert!(
        missing.is_empty(),
        "docs/README.md does not link: {missing:?}. Add each to the Reference by topic table."
    );
}

#[test]
fn docs_readme_links_resolve() {
    let index = std::fs::read_to_string(docs_dir().join("README.md")).expect("docs/README.md");
    let on_disk = pages_on_disk();
    let dangling: Vec<_> = linked_pages(&index).difference(&on_disk).cloned().collect();
    assert!(
        dangling.is_empty(),
        "docs/README.md links missing pages: {dangling:?}"
    );
}

#[test]
fn linked_pages_ignores_anchors_urls_and_parents() {
    let set = linked_pages("[a](a.md#x) [b](https://x/b.md) [c](../c.md) [d](d.md)");
    assert_eq!(
        set,
        ["a.md", "d.md"].iter().map(|s| s.to_string()).collect()
    );
}

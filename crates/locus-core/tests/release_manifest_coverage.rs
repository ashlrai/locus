use locus_core::adapter_registry::{
    build_release_manifest_from, builtin_manifest, release_manifest_json,
    verify_release_manifest_against,
};
use serde_json::Value;

#[test]
fn unsigned_export_covers_catalog_metadata_but_not_description_or_entry_signatures() {
    let catalog = builtin_manifest().unwrap();
    let version = "0.5.2-test";
    let original = build_release_manifest_from(&catalog, version);
    let json: Value = serde_json::from_str(&release_manifest_json(&original).unwrap()).unwrap();
    assert_eq!(json["locus_version"], version);
    assert_eq!(
        json["adapters"].as_array().unwrap().len(),
        catalog.providers.len()
    );
    assert!(json.get("signature").is_none());
    assert!(json.get("signed_by").is_none());
    for row in json["adapters"].as_array().unwrap() {
        let mut keys: Vec<_> = row
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["digest", "id", "name", "tools", "version"]);
    }

    for field in [
        "id",
        "name",
        "status",
        "synthetic",
        "capabilities",
        "frozen_selectors",
        "tools",
        "destructive_tools",
    ] {
        let mut changed = catalog.clone();
        let entry = &mut changed.providers[0];
        match field {
            "id" => entry.id.push_str("-changed"),
            "name" => entry.name.push_str(" changed"),
            "status" => entry.status.push_str("-changed"),
            "synthetic" => entry.synthetic = !entry.synthetic,
            "capabilities" => entry.capabilities.push("changed".into()),
            "frozen_selectors" => entry.frozen_selectors.push("changed".into()),
            "tools" => entry.tools.push("changed.tool".into()),
            "destructive_tools" => entry.destructive_tools.push("changed.tool".into()),
            _ => unreachable!(),
        }
        let modified = build_release_manifest_from(&changed, version);
        assert_ne!(modified, original, "{field} must affect the export");
        // An ID change is reported as an added and a missing entry; other
        // covered metadata changes are also observable through digest drift.
        let report = verify_release_manifest_against(&modified, &original, &[], true);
        assert!(!report.ok, "{field} must cause drift");
        assert!(!report.drift.is_empty(), "{field} must be reported");
        if field != "id" {
            assert!(
                report
                    .drift
                    .iter()
                    .any(|item| item.contains("digest drift")),
                "{field}"
            );
        }
    }

    let mut excluded = catalog.clone();
    let entry = &mut excluded.providers[0];
    entry.description.push_str(" changed");
    entry.signature = Some("unverified per-entry signature".into());
    entry.signed_by = Some("unverified signer".into());
    let modified = build_release_manifest_from(&excluded, version);
    assert_eq!(modified, original);
    assert!(verify_release_manifest_against(&modified, &original, &[], true).ok);
}

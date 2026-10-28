//! Regression check for issue #168: `cargo install` builds each package in
//! its own scratch target dir, so installing `locus-cli` and `locus-mcp` as
//! two packages compiled `locus-core` twice. The fix ships the `locus-mcp`
//! binary inside the `locus-cli` package: one install, one build session,
//! `locus-core` compiled once.
//!
//! These assertions pin the workspace install surface that guarantees that:
//! `locus-cli` must emit both `locus` and `locus-mcp`, and no other package
//! may emit a `locus-mcp` bin (a duplicate output filename would warn on every
//! workspace build and would let a second install overwrite the binary).

use serde_json::Value;
use std::process::Command;

fn workspace_metadata() -> Value {
    let manifest = format!("{}/../../Cargo.toml", env!("CARGO_MANIFEST_DIR"));
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .args([
            "metadata",
            "--no-deps",
            "--offline",
            "--format-version",
            "1",
            "--manifest-path",
            &manifest,
        ])
        .output()
        .expect("spawn cargo metadata");
    assert!(
        out.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("cargo metadata JSON")
}

fn bin_names(pkg: &Value) -> Vec<String> {
    pkg["targets"]
        .as_array()
        .expect("targets")
        .iter()
        .filter(|t| {
            t["kind"]
                .as_array()
                .expect("target kind")
                .iter()
                .any(|k| k == "bin")
        })
        .map(|t| t["name"].as_str().expect("target name").to_string())
        .collect()
}

#[test]
fn locus_cli_installs_locus_and_locus_mcp() {
    let meta = workspace_metadata();
    let packages = meta["packages"].as_array().expect("workspace packages");

    let cli = packages
        .iter()
        .find(|p| p["name"] == "locus-cli")
        .expect("locus-cli package");
    let cli_bins = bin_names(cli);
    for want in ["locus", "locus-mcp"] {
        assert!(
            cli_bins.iter().any(|b| b == want),
            "locus-cli must install `{want}` (bins: {cli_bins:?}); \
             a second package install recompiles locus-core (#168)"
        );
    }

    let other_mcp_owners: Vec<String> = packages
        .iter()
        .filter(|p| p["name"] != "locus-cli" && bin_names(p).iter().any(|b| b == "locus-mcp"))
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect();
    assert!(
        other_mcp_owners.is_empty(),
        "locus-mcp bin must come only from locus-cli; duplicate owners: {other_mcp_owners:?}"
    );
}

#[test]
fn locus_mcp_package_is_library_only() {
    let meta = workspace_metadata();
    let packages = meta["packages"].as_array().expect("workspace packages");
    let mcp = packages
        .iter()
        .find(|p| p["name"] == "locus-mcp")
        .expect("locus-mcp package");
    assert!(
        bin_names(mcp).is_empty(),
        "locus-mcp must ship no binary; it is the server library the \
         locus-cli `locus-mcp` bin calls into"
    );
    let has_lib = mcp["targets"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["kind"].as_array().unwrap().iter().any(|k| k == "lib"));
    assert!(has_lib, "locus-mcp must remain a library crate");
}

//! Offline contract for the root MCP Registry manifest. Reuse core's existing
//! schema validator so this check runs in the workspace suite without tooling
//! downloads or changes to publication configuration.

use serde_json::{json, Value};

const MANIFEST: &str = include_str!("../../../server.json");
const SCHEMA: &str = include_str!("fixtures/mcp-registry/server.schema.json");

fn manifest() -> Value {
    serde_json::from_str(MANIFEST).expect("server.json must be JSON")
}

fn validator() -> jsonschema::Validator {
    let schema: Value = serde_json::from_str(SCHEMA).expect("vendored schema must be JSON");
    jsonschema::validator_for(&schema).expect("published registry schema must compile offline")
}

#[test]
fn registry_manifest_matches_published_schema() {
    let schema: Value = serde_json::from_str(SCHEMA).unwrap();
    let manifest = manifest();
    assert_eq!(manifest["$schema"], schema["$id"]);
    let errors: Vec<_> = validator()
        .iter_errors(&manifest)
        .map(|error| format!("{}: {error}", error.instance_path))
        .collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

#[test]
fn registry_versions_match_workspace_source_of_truth() {
    let workspace: toml::Value = toml::from_str(include_str!("../../../Cargo.toml")).unwrap();
    let mcp_crate: toml::Value =
        toml::from_str(include_str!("../../locus-mcp/Cargo.toml")).unwrap();
    let npm: Value = serde_json::from_str(include_str!("../../../npm-mcp/package.json")).unwrap();
    let version = workspace["workspace"]["package"]["version"]
        .as_str()
        .unwrap();
    assert_eq!(
        mcp_crate["package"]["version"]["workspace"].as_bool(),
        Some(true)
    );
    assert_eq!(manifest()["version"], version);
    assert_eq!(npm["version"], version);
    for package in manifest()["packages"].as_array().unwrap() {
        assert_eq!(package["version"], version);
    }
}

#[test]
fn registry_package_matches_existing_npm_stdio_entrypoint() {
    let manifest = manifest();
    let npm: Value = serde_json::from_str(include_str!("../../../npm-mcp/package.json")).unwrap();
    assert_eq!(manifest["name"], npm["mcpName"]);
    assert_eq!(manifest["repository"]["url"], npm["repository"]["url"]);
    assert_eq!(manifest["repository"]["source"], "github");
    assert_eq!(manifest["websiteUrl"], npm["homepage"]);
    assert_eq!(npm["bin"], json!({"locus-mcp": "bin/locus-mcp.js"}));

    // A direct npm launch needs no arguments or credentials. Do not advertise
    // authority inputs or remote endpoints that bypass the supervised gate.
    assert!(manifest.get("remotes").is_none());
    assert_eq!(
        manifest["packages"],
        json!([{
            "registryType": "npm",
            "registryBaseUrl": "https://registry.npmjs.org",
            "identifier": npm["name"],
            "version": npm["version"],
            "runtimeHint": "npx",
            "transport": {"type": "stdio"}
        }])
    );
    assert!(!MANIFEST.contains('\u{2014}'));
}

#[test]
fn registry_schema_rejects_missing_required_metadata() {
    let validator = validator();
    for field in ["name", "description", "version"] {
        let mut invalid = manifest();
        invalid.as_object_mut().unwrap().remove(field);
        assert!(!validator.is_valid(&invalid), "schema must require {field}");
    }
    let mut invalid = manifest();
    invalid["packages"][0]
        .as_object_mut()
        .unwrap()
        .remove("transport");
    assert!(!validator.is_valid(&invalid));
}

#[test]
fn registry_schema_rejects_invalid_version_and_transport() {
    let validator = validator();
    let mut invalid = manifest();
    invalid["version"] = json!(52);
    assert!(!validator.is_valid(&invalid));
    let mut invalid = manifest();
    invalid["packages"][0]["transport"]["type"] = json!("unsupported");
    assert!(!validator.is_valid(&invalid));
}

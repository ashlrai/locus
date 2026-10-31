//! Issue #167: the supported Rust version is declared once, in
//! `[workspace.package].rust-version`, and every surface a user meets must
//! agree with it:
//!
//! - each member crate manifest inherits it (`rust-version.workspace = true`),
//! - `cargo metadata` exposes it on every workspace package,
//! - the documented install requirements (README, npm READMEs, Homebrew
//!   notes) name the same floor,
//! - the npm wrappers' cargo-fallback messages name the same floor.

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

/// File contents with all whitespace runs collapsed, so a phrase that wraps
/// across lines still matches.
fn normalized(rel: &str) -> String {
    read(rel).split_whitespace().collect::<Vec<_>>().join(" ")
}

fn workspace_manifest() -> toml::Value {
    read("Cargo.toml")
        .parse()
        .expect("workspace Cargo.toml parses")
}

/// The declared minimum: `[workspace.package].rust-version`.
fn msrv() -> String {
    workspace_manifest()["workspace"]["package"]["rust-version"]
        .as_str()
        .expect("[workspace.package].rust-version must be declared (issue #167)")
        .to_string()
}

/// Relative directory of every workspace member.
fn members(manifest: &toml::Value) -> Vec<String> {
    manifest["workspace"]["members"]
        .as_array()
        .expect("[workspace].members is a list")
        .iter()
        .map(|m| m.as_str().expect("member path is a string").to_string())
        .collect()
}

fn member_manifest(member: &str) -> toml::Value {
    let rel = format!("{member}/Cargo.toml");
    read(&rel)
        .parse()
        .unwrap_or_else(|e| panic!("parse {rel}: {e}"))
}

#[test]
fn workspace_declares_a_supported_rust_version() {
    let msrv = msrv();
    // `rust-version` accepts a bare `major.minor` or a full semver triple.
    let parts: Vec<&str> = msrv.split('.').collect();
    assert!(
        (2..=3).contains(&parts.len()) && parts.iter().all(|p| p.parse::<u64>().is_ok()),
        "workspace rust-version must be a semver version, got {msrv:?}"
    );
}

#[test]
fn every_member_crate_declares_the_same_floor() {
    let manifest = workspace_manifest();
    let msrv = msrv();
    for member in members(&manifest) {
        let declared = &member_manifest(&member)["package"]["rust-version"];
        let inherited = declared.get("workspace").and_then(|v| v.as_bool()) == Some(true);
        let pinned = declared.as_str() == Some(msrv.as_str());
        assert!(
            inherited || pinned,
            "{member}/Cargo.toml: [package].rust-version must be `workspace = true` or {msrv:?}"
        );
    }
}

#[test]
fn cargo_metadata_exposes_the_floor_on_every_package() {
    let manifest = workspace_manifest();
    let msrv = msrv();
    let cargo = option_env!("CARGO").unwrap_or("cargo");
    let out = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps", "--locked"])
        .current_dir(repo_root())
        .output()
        .expect("cargo metadata runs");
    assert!(
        out.status.success(),
        "cargo metadata --no-deps failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let meta: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("cargo metadata prints JSON");
    let packages: Vec<&serde_json::Value> = meta["packages"]
        .as_array()
        .expect("metadata has a packages array")
        .iter()
        .collect();
    assert!(
        !packages.is_empty(),
        "cargo metadata --no-deps must list the workspace packages"
    );

    // Every package metadata reports is a declared member exposing the floor.
    for pkg in &packages {
        let name = pkg["name"].as_str().expect("package has a name");
        assert_eq!(
            pkg["rust_version"].as_str(),
            Some(msrv.as_str()),
            "cargo metadata reports rust_version {:?} for {name}; expected {msrv:?}",
            pkg["rust_version"]
        );
    }

    // And every declared member produced a package in the report.
    for member in members(&manifest) {
        let pkg_manifest = member_manifest(&member);
        let name = pkg_manifest["package"]["name"]
            .as_str()
            .expect("member declares package.name");
        assert!(
            packages
                .iter()
                .any(|pkg| pkg["name"].as_str() == Some(name)),
            "member {member} ({name}) is missing from `cargo metadata --no-deps`"
        );
    }
}

#[test]
fn documented_install_requirements_agree() {
    let msrv = msrv();
    let phrase = format!("Rust {msrv} or newer");

    // README states the floor inside the source-install section.
    let readme = normalized("README.md");
    let install = readme
        .split("### Install from source")
        .nth(1)
        .and_then(|s| s.split(" ## ").next())
        .expect("README has an `### Install from source` section");
    assert!(
        install.contains(&phrase),
        "README.md source-install section must state `{phrase}`"
    );

    for rel in [
        "npm/README.md",
        "npm-mcp/README.md",
        "integrations/homebrew/README.md",
    ] {
        assert!(
            normalized(rel).contains(&phrase),
            "{rel} must state the same floor: `{phrase}`"
        );
    }

    // The npm wrappers carry the floor in their cargo-fallback messages; it
    // must be the same constant the workspace declares.
    for rel in ["npm/bin/locus.js", "npm-mcp/bin/locus-mcp.js"] {
        let needle = format!("MIN_RUST_VERSION = \"{msrv}\"");
        assert!(
            read(rel).contains(&needle),
            "{rel}: cargo-fallback minimum must be the workspace floor ({needle})"
        );
    }
}

#[test]
fn contributor_instructions_check_all_targets_on_the_declared_toolchain() {
    let msrv = msrv();
    let instructions = read("CONTRIBUTING.md");
    let section = instructions
        .split("### Minimum supported Rust version")
        .nth(1)
        .and_then(|s| s.split("\n## ").next())
        .expect("CONTRIBUTING.md has an MSRV section");
    assert!(
        section.contains(&format!("Rust {msrv} ")),
        "CONTRIBUTING.md must state the declared MSRV"
    );
    let toolchain = if msrv.split('.').count() == 2 {
        format!("{msrv}.0")
    } else {
        msrv
    };
    assert!(
        section.contains(&format!(
            "cargo +{toolchain} check --workspace --all-targets --locked"
        )),
        "CONTRIBUTING.md must show how to check every target against the declared MSRV and lockfile"
    );
}

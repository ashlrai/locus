//! Community adapter marketplace: signed, registry-agnostic adapter distribution.
//!
//! Locus ships built-in provider adapters with a signed registry manifest and a
//! trust store ([`crate::adapter_registry`], [`crate::adapter_trust`]). The
//! marketplace extends that trust machinery to **community adapters** — the
//! long tail of providers (Linear, Notion, Salesforce, …) that can't all be
//! built-in.
//!
//! ## Trust model (v1)
//!
//! - Adapters are **declarative manifests, not code**: the same canonical
//!   [`AdapterManifestEntry`](crate::adapter_registry::AdapterManifestEntry)
//!   JSON the built-in registry exports, plus an optional upstream MCP server
//!   spec ([`UpstreamSpec`](crate::binding::UpstreamSpec)) that the existing
//!   worker machinery spawns and scopes. No new execution primitive.
//! - Publishers sign manifests with ed25519 (preferred) or HMAC-SHA256
//!   (backcompat) using the exact canonical material from
//!   [`canonical_entry_material`](crate::adapter_registry::canonical_entry_material).
//! - Operators add publisher keys via `locus adapter trust add`
//!   (per-publisher, per-adapter, or per-version pinning). Install is
//!   **fail-closed**: unsigned, unknown-key, or invalid manifests are refused.
//! - Discovery is **registry-agnostic**: any HTTPS URL can serve a static
//!   index JSON. The index itself may carry a signature, but trust never
//!   depends on the server being honest — every manifest is verified against
//!   the operator's trust store on install.
//! - Tool-surface widening requires explicit re-approval: updating to a
//!   manifest whose tool list grew is refused unless the operator confirms.
//! - Installed adapters run through the same isolation pipeline as built-ins:
//!   isolated env, scope freeze, `require_approval` policy. A malicious
//!   manifest can at worst expose its own provider's tools.
//!
//! ## Layout under `$LOCUS_HOME`
//!
//! - `adapter-indexes.toml` — registered index sources (`{name, url}`).
//! - `adapters/<id>.json` — installed community manifests (mode 0600).
//! - `adapters/installed.toml` — install ledger (version, publisher,
//!   signed_by, digest, tool surface at install time).
//!
//! Explicitly out of scope for v1: executing third-party code (WASM/native),
//! a hosted registry service with accounts/billing, and auto-update.

use crate::adapter_registry::{
    canonical_entry_material, entry_digest, verify_entry_with_keys, AdapterManifestEntry,
    EntryVerifyStatus, RegistryTrustKey,
};
use crate::binding::UpstreamSpec;
use crate::error::{LocusError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Index sources file under `$LOCUS_HOME`.
pub const INDEXES_FILE: &str = "adapter-indexes.toml";
/// Installed community manifests directory under `$LOCUS_HOME`.
pub const ADAPTERS_DIR: &str = "adapters";
/// Install ledger file under `$LOCUS_HOME/adapters/`.
pub const INSTALLED_FILE: &str = "installed.toml";

/// One entry in a community index (summary; the full signed manifest is
/// fetched from `manifest_url` at install time).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommunityIndexEntry {
    /// Stable adapter id (`linear`, `notion`, …).
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Publisher label (display only; trust comes from the signature).
    #[serde(default)]
    pub publisher: String,
    /// Publisher's adapter version (semver-ish, informational).
    #[serde(default)]
    pub version: String,
    /// HTTPS URL of the full signed manifest JSON.
    pub manifest_url: String,
    /// Declared tool surface (checked against the manifest at install).
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// A static, registry-agnostic community index (JSON over HTTPS).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunityIndex {
    pub version: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub adapters: Vec<CommunityIndexEntry>,
    /// Optional whole-index signature (`ed25519:<base64>` / `hmac-sha256:<hex>`);
    /// verified when the `signed_by` key is trusted, ignored otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed_by: Option<String>,
}

/// Full installable community adapter manifest.
///
/// The `entry` is the signed distribution unit — identical canonical JSON to
/// built-in registry entries. `upstream` lets the existing MCP stdio worker
/// machinery spawn the provider's server with the binding's isolated env;
/// `credential_env` names the env var the worker maps the resolved
/// credential ref into.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunityAdapterManifest {
    #[serde(default = "manifest_schema_version")]
    pub manifest_version: u32,
    pub entry: AdapterManifestEntry,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<UpstreamSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_env: Option<String>,
    /// Publisher label (display only).
    #[serde(default)]
    pub publisher: String,
    /// Publisher's adapter version.
    #[serde(default)]
    pub version: String,
}

fn manifest_schema_version() -> u32 {
    1
}

impl CommunityAdapterManifest {
    /// The canonical bytes the publisher's `entry.signature` covers.
    pub fn signing_material(&self) -> String {
        canonical_entry_material(&self.entry)
    }
}

/// A registered index source.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexSource {
    pub name: String,
    pub url: String,
}

/// Load `$LOCUS_HOME/adapter-indexes.toml` (empty when missing).
pub fn load_index_sources(home: &Path) -> Result<Vec<IndexSource>> {
    #[derive(Deserialize, Default)]
    struct File {
        #[serde(default)]
        source: Vec<IndexSource>,
    }
    let path = home.join(INDEXES_FILE);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| LocusError::msg(format!("read {}: {e}", path.display())))?;
    let file: File = toml::from_str(&text)
        .map_err(|e| LocusError::msg(format!("parse {}: {e}", path.display())))?;
    Ok(file.source)
}

/// Persist index sources (mode 0600).
pub fn save_index_sources(home: &Path, sources: &[IndexSource]) -> Result<()> {
    #[derive(Serialize)]
    struct File<'a> {
        source: &'a [IndexSource],
    }
    let text = toml::to_string_pretty(&File { source: sources })
        .map_err(|e| LocusError::msg(format!("serialize index sources: {e}")))?;
    let path = home.join(INDEXES_FILE);
    std::fs::write(&path, text)
        .map_err(|e| LocusError::msg(format!("write {}: {e}", path.display())))?;
    restrict_0600(&path);
    Ok(())
}

fn restrict_0600(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
}

/// True for `http://localhost*` / `http://127.0.0.1*` (tests + local indexes).
fn is_loopback_http(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("http://localhost")
        || lower.starts_with("http://localhost:")
        || lower.starts_with("http://127.0.0.1")
        || lower.starts_with("http://[::1]")
}

/// Fetch a URL body. HTTPS only in production; plain HTTP is allowed for
/// loopback so tests and local indexes stay hermetic.
pub fn fetch_url(url: &str) -> Result<String> {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("https://") || is_loopback_http(url)) {
        return Err(LocusError::msg(format!(
            "refusing to fetch non-HTTPS marketplace URL: {url}"
        )));
    }
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(20))
        .user_agent(&format!("locus-marketplace/{}", crate::VERSION))
        .build();
    let resp = agent
        .get(url)
        .call()
        .map_err(|e| LocusError::msg(format!("fetch {url}: {e}")))?;
    if !(200..300).contains(&resp.status()) {
        return Err(LocusError::msg(format!(
            "fetch {url}: HTTP {}",
            resp.status()
        )));
    }
    resp.into_string()
        .map_err(|e| LocusError::msg(format!("read {url}: {e}")))
}

/// Fetch and parse a community index.
pub fn fetch_index(url: &str) -> Result<CommunityIndex> {
    let body = fetch_url(url)?;
    serde_json::from_str(&body).map_err(|e| LocusError::msg(format!("parse index {url}: {e}")))
}

/// Validate an index entry's shape before install (fail fast on garbage).
pub fn validate_index_entry(entry: &CommunityIndexEntry) -> Result<()> {
    if entry.id.trim().is_empty() {
        return Err(LocusError::msg("index entry has an empty id"));
    }
    if entry.manifest_url.trim().is_empty() {
        return Err(LocusError::msg(format!(
            "index entry `{}` has an empty manifest_url",
            entry.id
        )));
    }
    let lower = entry.manifest_url.to_ascii_lowercase();
    if !(lower.starts_with("https://") || is_loopback_http(&entry.manifest_url)) {
        return Err(LocusError::msg(format!(
            "index entry `{}` manifest_url must be HTTPS",
            entry.id
        )));
    }
    Ok(())
}

/// One search hit: which index it came from + the entry.
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub index_name: String,
    pub index_url: String,
    pub entry: CommunityIndexEntry,
}

/// Search all registered indexes for `query` (case-insensitive substring over
/// id, name, description, publisher). Indexes that fail to fetch are skipped
/// with a warning carried in `warnings`, never fatal.
pub fn search_indexes(sources: &[IndexSource], query: &str) -> (Vec<SearchHit>, Vec<String>) {
    let q = query.to_ascii_lowercase();
    let mut hits = Vec::new();
    let mut warnings = Vec::new();
    for src in sources {
        let index = match fetch_index(&src.url) {
            Ok(i) => i,
            Err(e) => {
                warnings.push(format!("index `{}` unreachable: {e}", src.name));
                continue;
            }
        };
        for entry in index.adapters {
            let haystack = format!(
                "{} {} {} {}",
                entry.id, entry.name, entry.description, entry.publisher
            )
            .to_ascii_lowercase();
            if q.is_empty() || haystack.contains(&q) {
                hits.push(SearchHit {
                    index_name: src.name.clone(),
                    index_url: src.url.clone(),
                    entry,
                });
            }
        }
    }
    (hits, warnings)
}

/// Install ledger record for one community adapter.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstalledAdapter {
    pub id: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub publisher: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed_by: Option<String>,
    /// sha256 of the canonical entry material at install time.
    pub digest: String,
    /// Tool surface at install/update time (widening needs re-approval).
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub installed_at: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct InstalledFile {
    #[serde(default)]
    adapter: Vec<InstalledAdapter>,
}

fn installed_path(home: &Path) -> PathBuf {
    home.join(ADAPTERS_DIR).join(INSTALLED_FILE)
}

fn manifest_path(home: &Path, id: &str) -> PathBuf {
    home.join(ADAPTERS_DIR).join(format!("{id}.json"))
}

/// Load the install ledger (empty when missing).
pub fn load_installed(home: &Path) -> Result<Vec<InstalledAdapter>> {
    let path = installed_path(home);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| LocusError::msg(format!("read {}: {e}", path.display())))?;
    let file: InstalledFile = toml::from_str(&text)
        .map_err(|e| LocusError::msg(format!("parse {}: {e}", path.display())))?;
    Ok(file.adapter)
}

fn save_installed(home: &Path, adapters: &[InstalledAdapter]) -> Result<()> {
    let dir = home.join(ADAPTERS_DIR);
    std::fs::create_dir_all(&dir)
        .map_err(|e| LocusError::msg(format!("create {}: {e}", dir.display())))?;
    let text = toml::to_string_pretty(&InstalledFile {
        adapter: adapters.to_vec(),
    })
    .map_err(|e| LocusError::msg(format!("serialize install ledger: {e}")))?;
    let path = installed_path(home);
    std::fs::write(&path, text)
        .map_err(|e| LocusError::msg(format!("write {}: {e}", path.display())))?;
    restrict_0600(&path);
    Ok(())
}

/// Load one installed community manifest.
pub fn load_installed_manifest(home: &Path, id: &str) -> Result<CommunityAdapterManifest> {
    let path = manifest_path(home, id);
    let text = std::fs::read_to_string(&path)
        .map_err(|_| LocusError::msg(format!("adapter `{id}` is not installed")))?;
    serde_json::from_str(&text)
        .map_err(|e| LocusError::msg(format!("parse installed adapter `{id}`: {e}")))
}

/// All installed community manifests (for `locus adapter list` merging).
pub fn installed_manifests(home: &Path) -> Vec<(InstalledAdapter, CommunityAdapterManifest)> {
    let mut out = Vec::new();
    for rec in load_installed(home).unwrap_or_default() {
        if let Ok(m) = load_installed_manifest(home, &rec.id) {
            out.push((rec, m));
        }
    }
    out
}

/// Outcome of [`install_adapter`].
#[derive(Debug, Clone)]
pub struct InstallReport {
    pub id: String,
    pub version: String,
    pub publisher: String,
    pub signed_by: Option<String>,
    /// True when this replaced a previous install.
    pub updated: bool,
    /// Tools added by this install/update vs the previous one.
    pub added_tools: Vec<String>,
    /// Tools removed vs the previous one.
    pub removed_tools: Vec<String>,
}

/// Install (or update) a community adapter from a fetched manifest.
///
/// Fail-closed: the manifest entry must carry a valid signature from a key in
/// `trust_keys`. A tool-surface *widening* (new tools vs the installed record)
/// is refused unless `approve_widen` — the operator must explicitly approve
/// new capabilities. Narrowing is always allowed.
pub fn install_adapter(
    home: &Path,
    manifest: &CommunityAdapterManifest,
    trust_keys: &[RegistryTrustKey],
    approve_widen: bool,
) -> Result<InstallReport> {
    install_adapter_inner(home, manifest, trust_keys, approve_widen, None)
}

fn install_adapter_inner(
    home: &Path,
    manifest: &CommunityAdapterManifest,
    trust_keys: &[RegistryTrustKey],
    approve_widen: bool,
    now: Option<String>,
) -> Result<InstallReport> {
    let id = manifest.entry.id.trim();
    if id.is_empty() {
        return Err(LocusError::msg(
            "community manifest has an empty adapter id",
        ));
    }
    if id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err(LocusError::msg(format!(
            "community adapter id `{id}` is not a safe file name"
        )));
    }

    // 1. Signature verification against the operator's trust store (fail closed).
    let report = verify_entry_with_keys(&manifest.entry, trust_keys);
    match report.status {
        EntryVerifyStatus::Valid => {}
        other => {
            return Err(LocusError::msg(format!(
                "refusing to install `{id}`: manifest signature {}",
                other.as_str(),
            )));
        }
    }
    let signed_by = manifest.entry.signed_by.clone();

    // 2. Tool-surface diff vs the installed record.
    let mut installed = load_installed(home)?;
    let previous = installed.iter().find(|a| a.id == id);
    let prev_tools: BTreeMap<&str, ()> = previous
        .map(|p| p.tools.iter().map(|t| (t.as_str(), ())).collect())
        .unwrap_or_default();
    let mut new_tools: Vec<String> = manifest.entry.tools.clone();
    new_tools.sort();
    new_tools.dedup();
    let added: Vec<String> = new_tools
        .iter()
        .filter(|t| !prev_tools.contains_key(t.as_str()))
        .cloned()
        .collect();
    let new_set: BTreeMap<&str, ()> = new_tools.iter().map(|t| (t.as_str(), ())).collect();
    let removed: Vec<String> = previous
        .map(|p| {
            p.tools
                .iter()
                .filter(|t| !new_set.contains_key(t.as_str()))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if previous.is_some() && !added.is_empty() && !approve_widen {
        return Err(LocusError::msg(format!(
            "refusing to widen tool surface for `{id}` without explicit approval: \
             new tools: {}. Re-run with approval to accept.",
            added.join(", ")
        )));
    }

    // 3. Persist manifest + ledger.
    let dir = home.join(ADAPTERS_DIR);
    std::fs::create_dir_all(&dir)
        .map_err(|e| LocusError::msg(format!("create {}: {e}", dir.display())))?;
    let body = serde_json::to_string_pretty(manifest)
        .map_err(|e| LocusError::msg(format!("serialize manifest: {e}")))?;
    let mpath = manifest_path(home, id);
    std::fs::write(&mpath, body)
        .map_err(|e| LocusError::msg(format!("write {}: {e}", mpath.display())))?;
    restrict_0600(&mpath);

    let record = InstalledAdapter {
        id: id.to_string(),
        version: manifest.version.clone(),
        publisher: manifest.publisher.clone(),
        signed_by,
        digest: entry_digest(&manifest.entry),
        tools: new_tools,
        installed_at: now.unwrap_or_else(current_timestamp),
    };
    let updated = previous.is_some();
    if let Some(slot) = installed.iter_mut().find(|a| a.id == id) {
        *slot = record;
    } else {
        installed.push(record);
    }
    save_installed(home, &installed)?;

    Ok(InstallReport {
        id: id.to_string(),
        version: manifest.version.clone(),
        publisher: manifest.publisher.clone(),
        signed_by: manifest.entry.signed_by.clone(),
        updated,
        added_tools: added,
        removed_tools: removed,
    })
}

fn current_timestamp() -> String {
    // chrono is available in locus-core; keep the ledger human-readable.
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Remove an installed community adapter (manifest + ledger entry).
pub fn uninstall_adapter(home: &Path, id: &str) -> Result<bool> {
    let mut installed = load_installed(home)?;
    let before = installed.len();
    installed.retain(|a| a.id != id);
    if installed.len() == before {
        return Ok(false);
    }
    let mpath = manifest_path(home, id);
    if mpath.exists() {
        std::fs::remove_file(&mpath)
            .map_err(|e| LocusError::msg(format!("remove {}: {e}", mpath.display())))?;
    }
    save_installed(home, &installed)?;
    Ok(true)
}

/// Fetch a manifest from an index entry and validate its shape.
pub fn fetch_manifest(entry: &CommunityIndexEntry) -> Result<CommunityAdapterManifest> {
    validate_index_entry(entry)?;
    let body = fetch_url(&entry.manifest_url)?;
    let manifest: CommunityAdapterManifest = serde_json::from_str(&body)
        .map_err(|e| LocusError::msg(format!("parse manifest for `{}`: {e}", entry.id)))?;
    if manifest.entry.id.trim() != entry.id.trim() {
        return Err(LocusError::msg(format!(
            "manifest id mismatch: index says `{}` but manifest says `{}`",
            entry.id, manifest.entry.id
        )));
    }
    // Declared tool surface in the index must be a subset of the manifest's.
    let manifest_tools: BTreeMap<&str, ()> = manifest
        .entry
        .tools
        .iter()
        .map(|t| (t.as_str(), ()))
        .collect();
    for t in &entry.tools {
        if !manifest_tools.contains_key(t.as_str()) {
            return Err(LocusError::msg(format!(
                "index for `{}` declares tool `{t}` missing from the signed manifest",
                entry.id
            )));
        }
    }
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter_registry::{ed25519_public_key_b64, sign_entry_ed25519, RegistryTrustKey};
    use ed25519_dalek::SigningKey;

    fn tmp_home(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locus-marketplace-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn test_keypair() -> (SigningKey, RegistryTrustKey) {
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let trust = RegistryTrustKey::ed25519_public(
            "test-publisher",
            ed25519_public_key_b64(&signing.verifying_key()),
        );
        (signing, trust)
    }

    fn signed_manifest(id: &str, tools: &[&str], signing: &SigningKey) -> CommunityAdapterManifest {
        let mut entry = AdapterManifestEntry {
            id: id.to_string(),
            name: format!("{id} adapter"),
            status: "community".to_string(),
            synthetic: false,
            capabilities: vec!["issues".to_string()],
            frozen_selectors: vec!["workspace".to_string()],
            tools: tools.iter().map(|s| s.to_string()).collect(),
            destructive_tools: vec![],
            description: format!("Community adapter for {id}"),
            signature: None,
            signed_by: None,
        };
        let sig = sign_entry_ed25519(&entry, signing);
        entry.signature = Some(sig);
        entry.signed_by = Some("test-publisher".to_string());
        CommunityAdapterManifest {
            manifest_version: 1,
            entry,
            upstream: Some(UpstreamSpec {
                command: "npx".to_string(),
                args: vec!["-y".to_string(), format!("mcp-{id}")],
                ..UpstreamSpec::new("npx")
            }),
            credential_env: Some(format!("{}_API_KEY", id.to_ascii_uppercase())),
            publisher: "Test Publisher".to_string(),
            version: "1.2.0".to_string(),
        }
    }

    #[test]
    fn install_verifies_signature_fail_closed() {
        let home = tmp_home("sig");
        let (signing, trust) = test_keypair();

        // Unsigned → refused.
        let mut unsigned = signed_manifest("linear", &["linear.issue"], &signing);
        unsigned.entry.signature = None;
        unsigned.entry.signed_by = None;
        assert!(install_adapter(&home, &unsigned, std::slice::from_ref(&trust), true).is_err());

        // Signed by an unknown key → refused.
        let other = SigningKey::from_bytes(&[9u8; 32]);
        let wrong_key = signed_manifest("linear", &["linear.issue"], &other);
        assert!(install_adapter(&home, &wrong_key, std::slice::from_ref(&trust), true).is_err());

        // Tampered after signing → refused.
        let mut tampered = signed_manifest("linear", &["linear.issue"], &signing);
        tampered.entry.tools.push("linear.admin".to_string());
        assert!(install_adapter(&home, &tampered, std::slice::from_ref(&trust), true).is_err());

        // Valid → installed.
        let good = signed_manifest("linear", &["linear.issue"], &signing);
        let report = install_adapter(&home, &good, std::slice::from_ref(&trust), true).unwrap();
        assert_eq!(report.id, "linear");
        assert!(!report.updated);
        assert_eq!(report.added_tools, vec!["linear.issue".to_string()]);
        let recs = load_installed(&home).unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].signed_by.as_deref(), Some("test-publisher"));
        // Manifest persisted with mode-safe write; ledger has digest.
        assert!(!recs[0].digest.is_empty());
        let loaded = load_installed_manifest(&home, "linear").unwrap();
        assert_eq!(loaded.credential_env.as_deref(), Some("LINEAR_API_KEY"));
    }

    #[test]
    fn install_rejects_path_traversal_ids() {
        let home = tmp_home("traversal");
        let (signing, trust) = test_keypair();
        let mut m = signed_manifest("../evil", &["x"], &signing);
        // sign_entry covers the id, so re-sign after mutating.
        m.entry.signature = Some(sign_entry_ed25519(&m.entry, &signing));
        assert!(install_adapter(&home, &m, &[trust], true).is_err());
        assert!(!home.join(ADAPTERS_DIR).join("evil.json").exists());
    }

    #[test]
    fn update_widening_requires_explicit_approval() {
        let home = tmp_home("widen");
        let (signing, trust) = test_keypair();
        install_adapter(
            &home,
            &signed_manifest("notion", &["notion.read"], &signing),
            std::slice::from_ref(&trust),
            true,
        )
        .unwrap();

        // Narrowing is always allowed.
        let narrower = signed_manifest("notion", &[], &signing);
        let r = install_adapter(&home, &narrower, std::slice::from_ref(&trust), true).unwrap();
        assert!(r.updated);
        assert_eq!(r.removed_tools, vec!["notion.read".to_string()]);

        // Widening without approval → refused.
        let wider = signed_manifest("notion", &["notion.read", "notion.write"], &signing);
        let err = install_adapter(&home, &wider, std::slice::from_ref(&trust), false).unwrap_err();
        assert!(err.to_string().contains("without explicit approval"));
        // …and with approval → accepted, diff reported.
        let r2 = install_adapter(&home, &wider, std::slice::from_ref(&trust), true).unwrap();
        assert_eq!(
            r2.added_tools,
            vec!["notion.read".to_string(), "notion.write".to_string()]
        );
    }

    #[test]
    fn uninstall_removes_manifest_and_ledger() {
        let home = tmp_home("uninstall");
        let (signing, trust) = test_keypair();
        install_adapter(
            &home,
            &signed_manifest("figma", &["figma.file"], &signing),
            &[trust],
            true,
        )
        .unwrap();
        assert!(uninstall_adapter(&home, "figma").unwrap());
        assert!(!uninstall_adapter(&home, "figma").unwrap());
        assert!(load_installed(&home).unwrap().is_empty());
    }

    #[test]
    fn index_sources_round_trip() {
        let home = tmp_home("indexes");
        assert!(load_index_sources(&home).unwrap().is_empty());
        save_index_sources(
            &home,
            &[IndexSource {
                name: "curated".into(),
                url: "https://adapters.example.com/index.json".into(),
            }],
        )
        .unwrap();
        let back = load_index_sources(&home).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].name, "curated");
    }

    #[test]
    fn fetch_url_rejects_plain_http() {
        assert!(fetch_url("http://example.com/index.json").is_err());
    }

    #[test]
    fn validate_index_entry_guards() {
        let mut e = CommunityIndexEntry {
            id: "x".into(),
            name: String::new(),
            description: String::new(),
            publisher: String::new(),
            version: String::new(),
            manifest_url: "https://example.com/x.json".into(),
            tools: vec![],
            capabilities: vec![],
        };
        assert!(validate_index_entry(&e).is_ok());
        e.manifest_url = "http://example.com/x.json".into();
        assert!(validate_index_entry(&e).is_err());
        e.manifest_url = String::new();
        assert!(validate_index_entry(&e).is_err());
    }
}

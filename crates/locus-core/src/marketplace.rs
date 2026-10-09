//! Community adapter marketplace: signed, registry-agnostic adapter distribution.
//!
//! Locus ships built-in provider adapters with a signed registry manifest and a
//! trust store ([`crate::adapter_registry`], [`crate::adapter_trust`]). The
//! marketplace extends that trust machinery to **community adapters** — the
//! long tail of providers (Linear, Notion, Salesforce, …) that can't all be
//! built-in.
//!
//! ## Trust model (v2)
//!
//! The full installable envelope is signed with an unambiguous typed JSON
//! encoding and a community-specific domain prefix. Commands, ordered args,
//! sandbox flags, credential mapping, tools and frozen selectors are covered.
//! Legacy entry-only signatures are refused. An upstream spec executes code:
//! a signature proves authorization by a configured key; it does not establish publisher identity or make code safe.
//! Installed bytes and current trust are rechecked before use.

use crate::adapter_registry::{
    verify_material_signature, AdapterManifestEntry, EntryVerifyStatus, RegistryTrustKey,
};
use crate::binding::UpstreamSpec;
use crate::error::{LocusError, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use url::{Host, Url};

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
    /// Reserved index signature metadata. Indexes remain untrusted discovery;
    /// only full installable envelopes are verified and admitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed_by: Option<String>,
}

/// Signed installable envelope. An upstream command is executable code.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CommunityAdapterManifest {
    pub manifest_version: u32,
    pub entry: AdapterManifestEntry,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<UpstreamSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_env: Option<String>,
    #[serde(default)]
    pub publisher: String,
    #[serde(default)]
    pub version: String,
}

pub const COMMUNITY_MANIFEST_VERSION: u32 = 2;
const COMMUNITY_SIGNATURE_DOMAIN: &str = "locus-community-adapter-envelope-v2\0";
const MAX_COMMUNITY_BYTES: usize = 1024 * 1024;

impl CommunityAdapterManifest {
    /// Exact typed JSON wire encoding, including signed_by, excluding only the
    /// detached signature. Ordered arrays remain arrays, never joined strings.
    pub fn signing_material(&self) -> String {
        let mut unsigned = self.clone();
        unsigned.entry.signature = None;
        format!(
            "{COMMUNITY_SIGNATURE_DOMAIN}{}",
            serde_json::to_string(&unsigned)
                .expect("typed community manifest contains serializable fields")
        )
    }
}

fn valid_adapter_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Verify the complete v2 envelope against the operator's current explicit keys.
pub fn verify_community_manifest_with_keys(
    manifest: &CommunityAdapterManifest,
    keys: &[RegistryTrustKey],
) -> Result<()> {
    if manifest.manifest_version != COMMUNITY_MANIFEST_VERSION {
        return Err(LocusError::msg("community manifests require full-envelope schema 2; legacy entry-only signatures are unsupported"));
    }
    if !valid_adapter_id(&manifest.entry.id)
        || crate::adapter_registry::builtin_manifest()?
            .providers
            .iter()
            .any(|entry| entry.id.eq_ignore_ascii_case(&manifest.entry.id))
    {
        return Err(LocusError::msg(
            "community adapter id is invalid or reserved by a built-in provider",
        ));
    }
    if manifest
        .upstream
        .as_ref()
        .is_some_and(|upstream| upstream.community_adapter.is_some())
    {
        return Err(LocusError::msg(
            "nested community adapter envelopes are unsupported",
        ));
    }
    if manifest.signing_material().len() > MAX_COMMUNITY_BYTES {
        return Err(LocusError::msg(
            "invalid or oversized community adapter envelope",
        ));
    }
    if let Some(name) = manifest.credential_env.as_deref() {
        if name.is_empty()
            || name.len() > 128
            || !name
                .bytes()
                .enumerate()
                .all(|(i, b)| b == b'_' || b.is_ascii_alphabetic() || i > 0 && b.is_ascii_digit())
        {
            return Err(LocusError::msg(
                "invalid community credential environment key",
            ));
        }
    }
    let (status, _, _) = verify_material_signature(
        &manifest.signing_material(),
        manifest.entry.signature.as_deref(),
        manifest.entry.signed_by.as_deref(),
        keys,
    );
    if status != EntryVerifyStatus::Valid {
        return Err(LocusError::msg(format!(
            "community manifest signature {}",
            status.as_str()
        )));
    }
    Ok(())
}

pub fn community_manifest_digest(manifest: &CommunityAdapterManifest) -> String {
    hex::encode(Sha256::digest(manifest.signing_material().as_bytes()))
}

/// A registered index source.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexSource {
    pub name: String,
    pub url: String,
}

fn read_bounded_file(path: &Path) -> Result<Vec<u8>> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .map_err(|_| LocusError::msg("community metadata read failed"))?;
    let metadata = file
        .metadata()
        .map_err(|_| LocusError::msg("community metadata stat failed"))?;
    if !metadata.is_file() || metadata.len() > MAX_COMMUNITY_BYTES as u64 {
        return Err(LocusError::msg(
            "community metadata must be a bounded regular file",
        ));
    }
    let mut bytes = Vec::new();
    file.take((MAX_COMMUNITY_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| LocusError::msg("community metadata read failed"))?;
    if bytes.len() > MAX_COMMUNITY_BYTES {
        return Err(LocusError::msg("community metadata exceeds 1 MiB"));
    }
    Ok(bytes)
}

fn atomic_metadata_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| LocusError::msg("invalid community metadata path"))?;
    let metadata = std::fs::symlink_metadata(parent)
        .map_err(|_| LocusError::msg("community metadata directory unavailable"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(LocusError::msg(
            "community metadata directory must be physical",
        ));
    }
    let tmp = parent.join(format!(".community-{:016x}.tmp", rand::random::<u64>()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(|_| LocusError::msg("community metadata persistence failed"))
}

fn parse_community_manifest(bytes: &[u8]) -> Result<CommunityAdapterManifest> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|_| LocusError::msg("invalid community manifest JSON"))?;
    for (field, allowed) in [
        (
            "entry",
            &[
                "id",
                "name",
                "status",
                "synthetic",
                "capabilities",
                "frozen_selectors",
                "tools",
                "destructive_tools",
                "description",
                "signature",
                "signed_by",
            ][..],
        ),
        (
            "upstream",
            &[
                "command",
                "args",
                "recipe",
                "resolve_secrets",
                "sandbox",
                "sandbox_no_network",
            ][..],
        ),
    ] {
        if let Some(object) = value.get(field).and_then(|v| v.as_object()) {
            if object.keys().any(|key| !allowed.contains(&key.as_str())) {
                return Err(LocusError::msg("unknown community manifest contract field"));
            }
        }
    }
    serde_json::from_value(value).map_err(|_| LocusError::msg("invalid community manifest schema"))
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
    let text = String::from_utf8(read_bounded_file(&path)?)
        .map_err(|_| LocusError::msg("invalid community metadata text"))?;
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
    atomic_metadata_write(&path, text.as_bytes())?;
    Ok(())
}

/// Parse the URL rather than accepting host prefixes or userinfo lookalikes.
fn marketplace_url(raw: &str) -> Result<Url> {
    let parsed = Url::parse(raw).map_err(|_| LocusError::msg("invalid marketplace URL"))?;
    if !parsed.username().is_empty() || parsed.password().is_some() || parsed.fragment().is_some() {
        return Err(LocusError::msg(
            "marketplace URLs cannot contain credentials or fragments",
        ));
    }
    let loopback = match parsed.host() {
        Some(Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    if parsed.host().is_none()
        || !(parsed.scheme() == "https" || parsed.scheme() == "http" && loopback)
    {
        return Err(LocusError::msg(
            "marketplace URLs require HTTPS or exact loopback HTTP",
        ));
    }
    Ok(parsed)
}

/// No redirects: even an HTTPS/loopback index cannot redirect to another host.
pub fn fetch_url(raw: &str) -> Result<String> {
    let parsed = marketplace_url(raw)?;
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout(Duration::from_secs(20))
        .user_agent(&format!("locus-marketplace/{}", crate::VERSION))
        .build();
    let response = agent
        .get(parsed.as_str())
        .call()
        .map_err(|_| LocusError::msg("marketplace fetch failed"))?;
    if !(200..300).contains(&response.status()) {
        return Err(LocusError::msg(
            "marketplace redirects or non-success responses are refused",
        ));
    }
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take((MAX_COMMUNITY_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| LocusError::msg("marketplace response read failed"))?;
    if bytes.len() > MAX_COMMUNITY_BYTES {
        return Err(LocusError::msg("marketplace response exceeds 1 MiB"));
    }
    String::from_utf8(bytes).map_err(|_| LocusError::msg("marketplace response is not UTF-8"))
}

/// Fetch and parse a community index.
pub fn fetch_index(url: &str) -> Result<CommunityIndex> {
    let body = fetch_url(url)?;
    let index: CommunityIndex =
        serde_json::from_str(&body).map_err(|_| LocusError::msg("invalid community index JSON"))?;
    if index.version != 1 {
        return Err(LocusError::msg("unsupported community index schema"));
    }
    Ok(index)
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
    if !valid_adapter_id(&entry.id) {
        return Err(LocusError::msg("invalid community index adapter id"));
    }
    marketplace_url(&entry.manifest_url)?;
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
    /// sha256 of the domain-separated full envelope at install time.
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
    let text = String::from_utf8(read_bounded_file(&path)?)
        .map_err(|_| LocusError::msg("invalid community metadata text"))?;
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
    atomic_metadata_write(&path, text.as_bytes())?;
    Ok(())
}

/// Load one installed community manifest.
pub fn load_installed_manifest(home: &Path, id: &str) -> Result<CommunityAdapterManifest> {
    let keys = crate::adapter_trust::load_merged_trust_keys(home);
    load_installed_manifest_with_keys(home, id, &keys)
}

pub fn load_installed_manifest_with_keys(
    home: &Path,
    id: &str,
    keys: &[RegistryTrustKey],
) -> Result<CommunityAdapterManifest> {
    if !valid_adapter_id(id) {
        return Err(LocusError::msg("invalid installed adapter id"));
    }
    let path = manifest_path(home, id);
    let bytes = read_bounded_file(&path)?;
    let manifest = parse_community_manifest(&bytes)?;
    verify_community_manifest_with_keys(&manifest, keys)?;
    let ledger = load_installed(home)?;
    let mut entries = ledger.iter().filter(|record| record.id == id);
    let record = entries
        .next()
        .ok_or_else(|| LocusError::msg("community adapter ledger is missing"))?;
    let mut tools = manifest.entry.tools.clone();
    tools.sort();
    tools.dedup();
    if entries.next().is_some()
        || manifest.entry.id != id
        || record.digest != community_manifest_digest(&manifest)
        || record.version != manifest.version
        || record.publisher != manifest.publisher
        || record.signed_by != manifest.entry.signed_by
        || record.tools != tools
    {
        return Err(LocusError::msg(
            "installed community envelope does not match its ledger",
        ));
    }
    Ok(manifest)
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

    // 1. Verify every installable field before any persistence.
    verify_community_manifest_with_keys(manifest, trust_keys)?;
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
    atomic_metadata_write(&mpath, body.as_bytes())?;

    let record = InstalledAdapter {
        id: id.to_string(),
        version: manifest.version.clone(),
        publisher: manifest.publisher.clone(),
        signed_by,
        digest: community_manifest_digest(manifest),
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
    let manifest = parse_community_manifest(body.as_bytes())?;
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
    use crate::adapter_registry::{
        ed25519_public_key_b64, sign_entry_ed25519, sign_entry_material_ed25519, RegistryTrustKey,
    };
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
        let entry = AdapterManifestEntry {
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
            signed_by: Some("test-publisher".to_string()),
        };
        let mut manifest = CommunityAdapterManifest {
            manifest_version: COMMUNITY_MANIFEST_VERSION,
            entry,
            upstream: Some(UpstreamSpec {
                command: "npx".to_string(),
                args: vec!["-y".to_string(), format!("mcp-{id}")],
                ..UpstreamSpec::new("npx")
            }),
            credential_env: Some(format!("{}_API_KEY", id.to_ascii_uppercase())),
            publisher: "Test Publisher".to_string(),
            version: "1.2.0".to_string(),
        };
        manifest.entry.signature = Some(sign_entry_material_ed25519(
            &manifest.signing_material(),
            signing,
        ));
        manifest
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
        let loaded =
            load_installed_manifest_with_keys(&home, "linear", std::slice::from_ref(&trust))
                .unwrap();
        assert_eq!(loaded.credential_env.as_deref(), Some("LINEAR_API_KEY"));
    }

    #[test]
    fn install_rejects_path_traversal_ids() {
        let home = tmp_home("traversal");
        let (signing, trust) = test_keypair();
        let mut m = signed_manifest("../evil", &["x"], &signing);
        // sign_entry covers the id, so re-sign after mutating.
        m.entry.signature = Some(sign_entry_material_ed25519(&m.signing_material(), &signing));
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
            &signed_manifest("customfigma", &["customfigma.file"], &signing),
            &[trust],
            true,
        )
        .unwrap();
        assert!(uninstall_adapter(&home, "customfigma").unwrap());
        assert!(!uninstall_adapter(&home, "customfigma").unwrap());
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
    #[test]
    fn full_envelope_rejects_each_executable_and_authority_field_tamper() {
        let (signing, trust) = test_keypair();
        let good = signed_manifest("linear", &["linear.read", "linear.write"], &signing);
        let mut mutations = Vec::new();
        macro_rules! tamper {
            ($statement:expr) => {{
                let mut m = good.clone();
                $statement(&mut m);
                mutations.push(m);
            }};
        }
        tamper!(
            |m: &mut CommunityAdapterManifest| m.upstream.as_mut().unwrap().command =
                "attacker".into()
        );
        tamper!(|m: &mut CommunityAdapterManifest| m.upstream.as_mut().unwrap().args.reverse());
        tamper!(
            |m: &mut CommunityAdapterManifest| m.upstream.as_mut().unwrap().recipe =
                Some("attacker".into())
        );
        tamper!(
            |m: &mut CommunityAdapterManifest| m.upstream.as_mut().unwrap().resolve_secrets =
                !m.upstream.as_ref().unwrap().resolve_secrets
        );
        tamper!(
            |m: &mut CommunityAdapterManifest| m.upstream.as_mut().unwrap().sandbox =
                Some(!m.upstream.as_ref().unwrap().sandbox.unwrap_or(false))
        );
        tamper!(|m: &mut CommunityAdapterManifest| m
            .upstream
            .as_mut()
            .unwrap()
            .sandbox_no_network =
            !m.upstream.as_ref().unwrap().sandbox_no_network);
        tamper!(|m: &mut CommunityAdapterManifest| m.credential_env = Some("OTHER_KEY".into()));
        tamper!(|m: &mut CommunityAdapterManifest| m.publisher.push('x'));
        tamper!(|m: &mut CommunityAdapterManifest| m.version.push('x'));
        tamper!(|m: &mut CommunityAdapterManifest| m.entry.signed_by = Some("another-key".into()));
        tamper!(|m: &mut CommunityAdapterManifest| m.entry.tools.push("linear.admin".into()));
        tamper!(|m: &mut CommunityAdapterManifest| m
            .entry
            .destructive_tools
            .push("linear.write".into()));
        tamper!(|m: &mut CommunityAdapterManifest| m.entry.frozen_selectors.clear());
        for mutation in mutations {
            assert!(
                verify_community_manifest_with_keys(&mutation, std::slice::from_ref(&trust))
                    .is_err()
            );
        }
        verify_community_manifest_with_keys(&good, &[trust]).unwrap();
    }

    #[test]
    fn legacy_partial_signatures_and_builtin_ids_are_refused() {
        let (signing, trust) = test_keypair();
        let mut legacy = signed_manifest("linear", &["linear.read"], &signing);
        legacy.entry.signature = Some(sign_entry_ed25519(&legacy.entry, &signing));
        assert!(
            verify_community_manifest_with_keys(&legacy, std::slice::from_ref(&trust)).is_err()
        );
        legacy.manifest_version = 1;
        assert!(
            verify_community_manifest_with_keys(&legacy, std::slice::from_ref(&trust)).is_err()
        );
        let builtin = signed_manifest("github", &["github.scope"], &signing);
        assert!(verify_community_manifest_with_keys(&builtin, &[trust]).is_err());
    }

    #[test]
    fn envelope_encoding_has_no_joined_array_or_field_collision() {
        let (signing, _) = test_keypair();
        let one = signed_manifest("linear", &["a,b"], &signing);
        let two = signed_manifest("linear", &["a", "b"], &signing);
        assert_ne!(one.signing_material(), two.signing_material());
        let mut left = one.clone();
        left.publisher = "a|b".into();
        left.version = "c".into();
        let mut right = one;
        right.publisher = "a".into();
        right.version = "b|c".into();
        assert_ne!(left.signing_material(), right.signing_material());
    }

    #[test]
    fn installed_envelope_rechecks_current_trust_and_ledger() {
        let home = tmp_home("installed-recheck");
        let (signing, trust) = test_keypair();
        let good = signed_manifest("linear", &["linear.read"], &signing);
        install_adapter(&home, &good, std::slice::from_ref(&trust), true).unwrap();
        assert!(load_installed_manifest_with_keys(&home, "linear", &[]).is_err());
        let mut tampered = good.clone();
        tampered.upstream.as_mut().unwrap().command = "attacker".into();
        std::fs::write(
            manifest_path(&home, "linear"),
            serde_json::to_vec(&tampered).unwrap(),
        )
        .unwrap();
        assert!(
            load_installed_manifest_with_keys(&home, "linear", std::slice::from_ref(&trust))
                .is_err()
        );
        tampered.entry.signature = Some(sign_entry_material_ed25519(
            &tampered.signing_material(),
            &signing,
        ));
        std::fs::write(
            manifest_path(&home, "linear"),
            serde_json::to_vec(&tampered).unwrap(),
        )
        .unwrap();
        assert!(
            load_installed_manifest_with_keys(&home, "linear", &[trust]).is_err(),
            "valid signature cannot replace ledger-bound bytes silently"
        );
    }

    #[test]
    fn parsed_urls_reject_host_prefix_and_userinfo_lookalikes() {
        for url in [
            "http://localhost.attacker.example/index",
            "http://127.0.0.1.attacker.example/index",
            "http://localhost@attacker.example/index",
            "https://user:pass@example.com/index",
            "file:///tmp/index",
            "https://example.com/index#fragment",
        ] {
            assert!(marketplace_url(url).is_err(), "{url}");
        }
        for url in [
            "http://localhost:1234/index",
            "http://127.0.0.1:1234/index",
            "http://[::1]:1234/index",
            "https://example.com/index",
        ] {
            assert!(marketplace_url(url).is_ok());
        }
    }

    #[test]
    fn actual_fetch_refuses_loopback_redirect_without_contacting_destination() {
        use std::net::TcpListener;
        let destination = TcpListener::bind("127.0.0.1:0").unwrap();
        destination.set_nonblocking(true).unwrap();
        let origin = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/index", origin.local_addr().unwrap());
        let target = destination.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = origin.accept().unwrap();
            let mut request = [0; 4096];
            let count = stream.read(&mut request).unwrap();
            assert!(count > 0);
            write!(stream, "HTTP/1.1 302 Found\r\nLocation: http://{target}/redirected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        });
        assert!(fetch_url(&url).is_err());
        server.join().unwrap();
        assert!(
            matches!(destination.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
    }

    #[test]
    fn unknown_runtime_fields_are_not_silently_accepted() {
        let (signing, _) = test_keypair();
        let good = signed_manifest("linear", &["linear.read"], &signing);
        let mut value = serde_json::to_value(good).unwrap();
        value["upstream"]["env"] = serde_json::json!({"INVENTED":"value"});
        assert!(parse_community_manifest(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn installed_manifest_symlinks_are_refused() {
        let home = tmp_home("no-follow");
        let (signing, trust) = test_keypair();
        let manifest = signed_manifest("linear", &["linear.read"], &signing);
        install_adapter(&home, &manifest, std::slice::from_ref(&trust), true).unwrap();
        let path = manifest_path(&home, "linear");
        let outside = home.join("outside.json");
        std::fs::rename(&path, &outside).unwrap();
        std::os::unix::fs::symlink(&outside, &path).unwrap();
        assert!(load_installed_manifest_with_keys(&home, "linear", &[trust]).is_err());
    }
    #[test]
    fn weak_ed25519_key_cannot_authorize_an_arbitrary_executable_envelope() {
        use base64::{engine::general_purpose::STANDARD, Engine};
        use ed25519_dalek::{Signature, Verifier, VerifyingKey};
        let (signing, _) = test_keypair();
        let mut manifest = signed_manifest("linear", &["linear.read"], &signing);
        let mut identity = [0u8; 32];
        identity[0] = 1;
        let weak = VerifyingKey::from_bytes(&identity).unwrap();
        assert!(weak.is_weak());
        let mut forged = [0u8; 64];
        forged[0] = 1;
        manifest.entry.signature = Some(format!("ed25519:{}", STANDARD.encode(forged)));
        let key = RegistryTrustKey::ed25519_public("test-publisher", STANDARD.encode(identity));
        // Ordinary Dalek verification accepts this forgery for any material.
        assert!(weak
            .verify(
                manifest.signing_material().as_bytes(),
                &Signature::from_bytes(&forged)
            )
            .is_ok());
        assert!(key.ed25519_verifying_key().is_err());
        assert!(verify_community_manifest_with_keys(&manifest, &[key]).is_err());
    }
}

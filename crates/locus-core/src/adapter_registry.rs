//! Built-in adapter registry (manifest catalog + optional signatures).
//!
//! Source of truth: repo `adapters/manifest.toml` (embedded at compile time).
//! This is the **discovery / verification** surface for the adapter catalog —
//! not a plugin loader. See [docs/adapter-sdk.md](../../../docs/adapter-sdk.md).
//!
//! ## Signatures (v0)
//!
//! Per-entry optional fields:
//! - `signature` — `hmac-sha256:<hex>` over the entry's canonical material
//! - `signed_by` — key id from the local trust store
//!
//! Verification uses HMAC-SHA256 with published registry verify keys (symmetric
//! stand-in for a future ed25519 root). Production builds ship **no** trust
//! keys by default; tests pass keys via [`verify_entry_with_keys`] /
//! [`verify_manifest_with_keys`]. `--require-signed` is fail-closed: unsigned
//! or invalid → error.

use crate::error::{LocusError, Result};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::sync::OnceLock;

type HmacSha256 = Hmac<Sha256>;

/// Embedded built-in catalog (repo `adapters/manifest.toml`).
const MANIFEST_TOML: &str = include_str!("../../../adapters/manifest.toml");

/// Signature scheme prefix used in `signature` fields.
pub const SIG_SCHEME_HMAC_SHA256: &str = "hmac-sha256";

/// Canonical material version (bumped when signing fields change).
const CANONICAL_VERSION: &str = "v1";

/// One trusted registry key (id + 32-byte secret, hex-encoded).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryTrustKey {
    /// Key id recorded in `signed_by`.
    pub id: String,
    /// 32-byte HMAC key as lowercase hex (64 chars).
    pub secret_hex: String,
}

impl RegistryTrustKey {
    /// Parse and return the raw 32-byte key.
    pub fn secret_bytes(&self) -> Result<[u8; 32]> {
        let bytes = hex::decode(self.secret_hex.trim())
            .map_err(|e| LocusError::msg(format!("registry trust key `{}` hex: {e}", self.id)))?;
        if bytes.len() != 32 {
            return Err(LocusError::msg(format!(
                "registry trust key `{}` must be 32 bytes (64 hex chars), got {}",
                self.id,
                bytes.len()
            )));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(arr)
    }
}

/// Process-wide trust keys (optional; production default is empty).
static TRUST_KEYS: OnceLock<Vec<RegistryTrustKey>> = OnceLock::new();

/// Default production trust store: empty (unsigned catalog is expected until
/// a real registry root is published).
fn default_trust_keys() -> Vec<RegistryTrustKey> {
    Vec::new()
}

fn trust_keys() -> &'static [RegistryTrustKey] {
    TRUST_KEYS.get_or_init(default_trust_keys)
}

/// Install trust keys for the process (primarily tests / tooling).
///
/// Returns `Err` if keys were already installed (OnceLock semantics).
pub fn install_trust_keys(keys: Vec<RegistryTrustKey>) -> Result<()> {
    TRUST_KEYS.set(keys).map_err(|_| {
        LocusError::msg("adapter registry trust keys already installed for this process")
    })
}

/// One provider row in the adapter registry catalog.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdapterManifestEntry {
    /// Stable provider id (`supabase`, `github`, …).
    pub id: String,
    /// Human title.
    #[serde(default)]
    pub name: String,
    /// Lifecycle: `built-in`, `experimental`, `deprecated`, …
    #[serde(default = "default_status")]
    pub status: String,
    /// True when tools are synthetic identity stubs (not upstream MCP).
    #[serde(default)]
    pub synthetic: bool,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub frozen_selectors: Vec<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub destructive_tools: Vec<String>,
    #[serde(default)]
    pub description: String,
    /// Optional detached signature: `hmac-sha256:<hex>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// Key id that produced `signature`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed_by: Option<String>,
}

fn default_status() -> String {
    "built-in".into()
}

/// Parsed adapter registry file (`adapters/manifest.toml` shape).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdapterManifest {
    /// Schema version (currently `1`).
    pub version: u32,
    #[serde(default)]
    pub providers: Vec<AdapterManifestEntry>,
    /// Optional whole-file signature (roadmap; not required in v0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed_by: Option<String>,
}

/// Per-entry verification outcome.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EntryVerifyStatus {
    /// Valid signature against a trusted key.
    Valid,
    /// No signature present.
    Unsigned,
    /// Signature present but key id unknown / not in trust store.
    UnknownKey,
    /// Signature present but does not match canonical material.
    Invalid,
    /// Signature field malformed (bad scheme / hex).
    Malformed,
}

impl EntryVerifyStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::Unsigned => "unsigned",
            Self::UnknownKey => "unknown_key",
            Self::Invalid => "invalid",
            Self::Malformed => "malformed",
        }
    }

    /// True when the entry is cryptographically trusted.
    pub fn is_trusted(&self) -> bool {
        matches!(self, Self::Valid)
    }
}

/// Verification report for one catalog entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EntryVerifyReport {
    pub id: String,
    pub status: EntryVerifyStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signed_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Full registry verify result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestVerifyReport {
    pub version: u32,
    pub provider_count: usize,
    pub trusted: usize,
    pub unsigned: usize,
    pub failed: usize,
    pub require_signed: bool,
    pub ok: bool,
    pub entries: Vec<EntryVerifyReport>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

/// Parse an adapter manifest TOML string.
pub fn parse_manifest(toml_src: &str) -> Result<AdapterManifest> {
    let m: AdapterManifest = toml::from_str(toml_src)
        .map_err(|e| LocusError::msg(format!("adapter manifest parse error: {e}")))?;
    validate_manifest(&m)?;
    Ok(m)
}

/// Load the built-in embedded catalog.
pub fn builtin_manifest() -> Result<AdapterManifest> {
    parse_manifest(MANIFEST_TOML)
}

/// All providers from the built-in catalog (sorted by id).
pub fn list_adapters() -> Result<Vec<AdapterManifestEntry>> {
    let mut m = builtin_manifest()?;
    m.providers.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(m.providers)
}

fn validate_manifest(m: &AdapterManifest) -> Result<()> {
    if m.version == 0 {
        return Err(LocusError::msg(
            "adapter manifest version must be >= 1 (got 0)",
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for p in &m.providers {
        let id = p.id.trim();
        if id.is_empty() {
            return Err(LocusError::msg(
                "adapter manifest entry has empty provider id",
            ));
        }
        if !seen.insert(id.to_ascii_lowercase()) {
            return Err(LocusError::msg(format!(
                "adapter manifest duplicate provider id `{id}`"
            )));
        }
        for tool in &p.tools {
            if tool.trim().is_empty() {
                return Err(LocusError::msg(format!(
                    "adapter `{id}` has an empty tool name"
                )));
            }
        }
    }
    Ok(())
}

/// Canonical signing material for one entry (excludes signature fields).
///
/// Format (stable, newline-free):
/// `v1|{id}|{name}|{status}|{synthetic}|caps…|selectors…|tools…|destructive…`
/// Lists are sorted ASCII-case-insensitive for determinism.
pub fn canonical_entry_material(entry: &AdapterManifestEntry) -> String {
    fn sorted_join(items: &[String]) -> String {
        let mut v: Vec<&str> = items.iter().map(|s| s.as_str()).collect();
        v.sort_by_key(|a| a.to_ascii_lowercase());
        v.join(",")
    }
    format!(
        "{CANONICAL_VERSION}|{}|{}|{}|{}|{}|{}|{}|{}",
        entry.id.trim(),
        entry.name.trim(),
        entry.status.trim(),
        if entry.synthetic { "1" } else { "0" },
        sorted_join(&entry.capabilities),
        sorted_join(&entry.frozen_selectors),
        sorted_join(&entry.tools),
        sorted_join(&entry.destructive_tools),
    )
}

/// Sign entry material with a trust key → `hmac-sha256:<hex>`.
pub fn sign_entry_material(material: &str, key: &RegistryTrustKey) -> Result<String> {
    let secret = key.secret_bytes()?;
    let mut mac = HmacSha256::new_from_slice(&secret).expect("HMAC accepts any key length");
    mac.update(material.as_bytes());
    let result = mac.finalize().into_bytes();
    Ok(format!("{SIG_SCHEME_HMAC_SHA256}:{}", hex::encode(result)))
}

/// Sign a full entry (uses [`canonical_entry_material`]).
pub fn sign_entry(entry: &AdapterManifestEntry, key: &RegistryTrustKey) -> Result<String> {
    sign_entry_material(&canonical_entry_material(entry), key)
}

fn parse_signature(sig: &str) -> Result<Vec<u8>> {
    let s = sig.trim();
    let prefix = format!("{SIG_SCHEME_HMAC_SHA256}:");
    let hex_part = s.strip_prefix(&prefix).ok_or_else(|| {
        LocusError::msg(format!(
            "signature must start with `{SIG_SCHEME_HMAC_SHA256}:`"
        ))
    })?;
    let bytes =
        hex::decode(hex_part).map_err(|e| LocusError::msg(format!("signature hex decode: {e}")))?;
    if bytes.len() != 32 {
        return Err(LocusError::msg(format!(
            "signature must be 32 bytes, got {}",
            bytes.len()
        )));
    }
    Ok(bytes)
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Verify one entry against the process trust store.
pub fn verify_entry(entry: &AdapterManifestEntry) -> EntryVerifyReport {
    verify_entry_with_keys(entry, trust_keys())
}

/// Verify one entry against an explicit key set (tests / offline).
pub fn verify_entry_with_keys(
    entry: &AdapterManifestEntry,
    keys: &[RegistryTrustKey],
) -> EntryVerifyReport {
    let signed_by = entry.signed_by.clone();
    let Some(sig) = entry
        .signature
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return EntryVerifyReport {
            id: entry.id.clone(),
            status: EntryVerifyStatus::Unsigned,
            signed_by,
            detail: Some("no signature field".into()),
        };
    };

    let expected_bytes = match parse_signature(sig) {
        Ok(b) => b,
        Err(e) => {
            return EntryVerifyReport {
                id: entry.id.clone(),
                status: EntryVerifyStatus::Malformed,
                signed_by,
                detail: Some(e.to_string()),
            };
        }
    };

    let key_id = entry
        .signed_by
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let Some(key_id) = key_id else {
        return EntryVerifyReport {
            id: entry.id.clone(),
            status: EntryVerifyStatus::Malformed,
            signed_by: None,
            detail: Some("signature present but signed_by missing".into()),
        };
    };

    let Some(key) = keys.iter().find(|k| k.id.eq_ignore_ascii_case(key_id)) else {
        return EntryVerifyReport {
            id: entry.id.clone(),
            status: EntryVerifyStatus::UnknownKey,
            signed_by: Some(key_id.to_string()),
            detail: Some(format!("key id `{key_id}` not in trust store")),
        };
    };

    let material = canonical_entry_material(entry);
    let Ok(computed) = sign_entry_material(&material, key) else {
        return EntryVerifyReport {
            id: entry.id.clone(),
            status: EntryVerifyStatus::Malformed,
            signed_by: Some(key_id.to_string()),
            detail: Some("failed to compute expected signature (bad trust key)".into()),
        };
    };
    let Ok(computed_bytes) = parse_signature(&computed) else {
        return EntryVerifyReport {
            id: entry.id.clone(),
            status: EntryVerifyStatus::Malformed,
            signed_by: Some(key_id.to_string()),
            detail: Some("internal signature format error".into()),
        };
    };

    if ct_eq(&expected_bytes, &computed_bytes) {
        EntryVerifyReport {
            id: entry.id.clone(),
            status: EntryVerifyStatus::Valid,
            signed_by: Some(key_id.to_string()),
            detail: None,
        }
    } else {
        EntryVerifyReport {
            id: entry.id.clone(),
            status: EntryVerifyStatus::Invalid,
            signed_by: Some(key_id.to_string()),
            detail: Some("signature mismatch for canonical material".into()),
        }
    }
}

/// Verify an entire manifest.
///
/// When `require_signed` is true, any non-`Valid` entry makes `ok = false`
/// and populates `errors` (fail closed).
pub fn verify_manifest(manifest: &AdapterManifest, require_signed: bool) -> ManifestVerifyReport {
    verify_manifest_with_keys(manifest, require_signed, trust_keys())
}

/// Verify with an explicit trust key set.
pub fn verify_manifest_with_keys(
    manifest: &AdapterManifest,
    require_signed: bool,
    keys: &[RegistryTrustKey],
) -> ManifestVerifyReport {
    let entries: Vec<EntryVerifyReport> = manifest
        .providers
        .iter()
        .map(|p| verify_entry_with_keys(p, keys))
        .collect();

    let trusted = entries
        .iter()
        .filter(|e| e.status == EntryVerifyStatus::Valid)
        .count();
    let unsigned = entries
        .iter()
        .filter(|e| e.status == EntryVerifyStatus::Unsigned)
        .count();
    let failed = entries
        .iter()
        .filter(|e| {
            !matches!(
                e.status,
                EntryVerifyStatus::Valid | EntryVerifyStatus::Unsigned
            )
        })
        .count();

    let mut errors = Vec::new();
    if require_signed {
        for e in &entries {
            if e.status != EntryVerifyStatus::Valid {
                errors.push(format!(
                    "provider `{}`: {}{}",
                    e.id,
                    e.status.as_str(),
                    e.detail
                        .as_ref()
                        .map(|d| format!(" ({d})"))
                        .unwrap_or_default()
                ));
            }
        }
    } else {
        for e in &entries {
            if matches!(
                e.status,
                EntryVerifyStatus::Invalid | EntryVerifyStatus::Malformed
            ) {
                errors.push(format!(
                    "provider `{}`: {}{}",
                    e.id,
                    e.status.as_str(),
                    e.detail
                        .as_ref()
                        .map(|d| format!(" ({d})"))
                        .unwrap_or_default()
                ));
            }
        }
    }

    let ok = if require_signed {
        errors.is_empty() && entries.iter().all(|e| e.status == EntryVerifyStatus::Valid)
    } else {
        // Soft mode: invalid/malformed signatures still fail (tamper), but
        // unsigned + unknown_key are informational only.
        !entries.iter().any(|e| {
            matches!(
                e.status,
                EntryVerifyStatus::Invalid | EntryVerifyStatus::Malformed
            )
        })
    };

    ManifestVerifyReport {
        version: manifest.version,
        provider_count: entries.len(),
        trusted,
        unsigned,
        failed,
        require_signed,
        ok,
        entries,
        errors,
    }
}

/// Verify the built-in embedded catalog.
pub fn verify_builtin(require_signed: bool) -> Result<ManifestVerifyReport> {
    let m = builtin_manifest()?;
    Ok(verify_manifest(&m, require_signed))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixed mock registry key for unit tests (not a production secret).
    const MOCK_KEY_ID: &str = "locus-registry-mock";
    const MOCK_SECRET_HEX: &str =
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn mock_key() -> RegistryTrustKey {
        RegistryTrustKey {
            id: MOCK_KEY_ID.into(),
            secret_hex: MOCK_SECRET_HEX.into(),
        }
    }

    fn sample_entry(id: &str) -> AdapterManifestEntry {
        AdapterManifestEntry {
            id: id.into(),
            name: id.to_ascii_uppercase(),
            status: "built-in".into(),
            synthetic: true,
            capabilities: vec!["scope".into(), "identity".into()],
            frozen_selectors: vec!["account_id".into()],
            tools: vec![format!("{id}.scope"), format!("{id}.whoami")],
            destructive_tools: vec![],
            description: format!("{id} test adapter"),
            signature: None,
            signed_by: None,
        }
    }

    #[test]
    fn builtin_manifest_parses() {
        let m = builtin_manifest().expect("builtin manifest");
        assert_eq!(m.version, 1);
        assert!(
            m.providers.len() >= 5,
            "expected several providers, got {}",
            m.providers.len()
        );
        let ids: Vec<_> = m.providers.iter().map(|p| p.id.as_str()).collect();
        assert!(ids.contains(&"github"));
        assert!(ids.contains(&"supabase"));
        assert!(ids.contains(&"vercel"));
    }

    #[test]
    fn list_adapters_sorted() {
        let list = list_adapters().unwrap();
        let mut sorted = list.clone();
        sorted.sort_by(|a, b| a.id.cmp(&b.id));
        assert_eq!(list, sorted);
    }

    #[test]
    fn parse_rejects_duplicate_ids() {
        let toml = r#"
version = 1
[[providers]]
id = "github"
name = "A"
[[providers]]
id = "GitHub"
name = "B"
"#;
        let err = parse_manifest(toml).unwrap_err().to_string();
        assert!(err.contains("duplicate"), "{err}");
    }

    #[test]
    fn parse_rejects_version_zero() {
        let toml = r#"
version = 0
[[providers]]
id = "x"
"#;
        assert!(parse_manifest(toml).is_err());
    }

    #[test]
    fn parse_rejects_empty_id() {
        let toml = r#"
version = 1
[[providers]]
id = "  "
name = "Empty"
"#;
        assert!(parse_manifest(toml).is_err());
    }

    #[test]
    fn canonical_material_is_stable_under_list_reorder() {
        let mut a = sample_entry("demo");
        a.tools = vec!["demo.b".into(), "demo.a".into()];
        let mut b = sample_entry("demo");
        b.tools = vec!["demo.a".into(), "demo.b".into()];
        assert_eq!(canonical_entry_material(&a), canonical_entry_material(&b));
    }

    #[test]
    fn sign_and_verify_roundtrip() {
        let key = mock_key();
        let mut entry = sample_entry("stripe");
        let sig = sign_entry(&entry, &key).unwrap();
        entry.signature = Some(sig);
        entry.signed_by = Some(MOCK_KEY_ID.into());

        let report = verify_entry_with_keys(&entry, &[key]);
        assert_eq!(report.status, EntryVerifyStatus::Valid);
        assert!(report.status.is_trusted());
    }

    #[test]
    fn verify_detects_tamper() {
        let key = mock_key();
        let mut entry = sample_entry("aws");
        entry.signature = Some(sign_entry(&entry, &key).unwrap());
        entry.signed_by = Some(MOCK_KEY_ID.into());
        // Tamper after signing.
        entry.tools.push("aws.evil".into());

        let report = verify_entry_with_keys(&entry, &[key]);
        assert_eq!(report.status, EntryVerifyStatus::Invalid);
    }

    #[test]
    fn verify_unknown_key() {
        let key = mock_key();
        let mut entry = sample_entry("resend");
        entry.signature = Some(sign_entry(&entry, &key).unwrap());
        entry.signed_by = Some("not-a-real-key".into());

        let report = verify_entry_with_keys(&entry, &[key]);
        assert_eq!(report.status, EntryVerifyStatus::UnknownKey);
    }

    #[test]
    fn verify_malformed_signature() {
        let mut entry = sample_entry("cloudflare");
        entry.signature = Some("not-a-sig".into());
        entry.signed_by = Some(MOCK_KEY_ID.into());
        let report = verify_entry_with_keys(&entry, &[mock_key()]);
        assert_eq!(report.status, EntryVerifyStatus::Malformed);
    }

    #[test]
    fn require_signed_fail_closed_on_unsigned() {
        let m = AdapterManifest {
            version: 1,
            providers: vec![sample_entry("github")],
            signature: None,
            signed_by: None,
        };
        let soft = verify_manifest_with_keys(&m, false, &[]);
        assert!(soft.ok, "unsigned ok without --require-signed");
        assert_eq!(soft.unsigned, 1);

        let hard = verify_manifest_with_keys(&m, true, &[]);
        assert!(!hard.ok, "unsigned must fail with --require-signed");
        assert!(!hard.errors.is_empty());
    }

    #[test]
    fn require_signed_passes_when_all_valid() {
        let key = mock_key();
        let mut e1 = sample_entry("github");
        e1.signature = Some(sign_entry(&e1, &key).unwrap());
        e1.signed_by = Some(MOCK_KEY_ID.into());
        let mut e2 = sample_entry("vercel");
        e2.signature = Some(sign_entry(&e2, &key).unwrap());
        e2.signed_by = Some(MOCK_KEY_ID.into());

        let m = AdapterManifest {
            version: 1,
            providers: vec![e1, e2],
            signature: None,
            signed_by: None,
        };
        let report = verify_manifest_with_keys(&m, true, &[key]);
        assert!(report.ok);
        assert_eq!(report.trusted, 2);
        assert_eq!(report.unsigned, 0);
        assert!(report.errors.is_empty());
    }

    #[test]
    fn soft_mode_fails_on_invalid_signature() {
        let key = mock_key();
        let mut entry = sample_entry("supabase");
        entry.signature = Some(sign_entry(&entry, &key).unwrap());
        entry.signed_by = Some(MOCK_KEY_ID.into());
        entry.name = "TAMPERED".into();

        let m = AdapterManifest {
            version: 1,
            providers: vec![entry],
            signature: None,
            signed_by: None,
        };
        let report = verify_manifest_with_keys(&m, false, &[key]);
        assert!(!report.ok);
        assert_eq!(report.failed, 1);
    }

    #[test]
    fn builtin_verify_soft_ok_when_unsigned() {
        // Built-in catalog ships unsigned in v0; soft verify must pass.
        let report = verify_builtin(false).unwrap();
        assert!(report.ok, "builtin soft verify failed: {:?}", report.errors);
        assert_eq!(report.failed, 0);
    }

    #[test]
    fn json_schema_shape_roundtrips() {
        let m = builtin_manifest().unwrap();
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["version"], 1);
        assert!(v["providers"].as_array().unwrap().len() >= 5);
        let report = verify_manifest(&m, false);
        let rv = serde_json::to_value(&report).unwrap();
        assert!(rv["ok"].as_bool().unwrap());
        assert!(rv["provider_count"].as_u64().unwrap() >= 5);
    }

    #[test]
    fn builtin_manifest_matches_json_schema() {
        let schema_raw = include_str!("../../../schema/adapter-manifest.schema.json");
        let schema: serde_json::Value = serde_json::from_str(schema_raw).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let m = builtin_manifest().unwrap();
        let instance = serde_json::to_value(&m).unwrap();
        let errors: Vec<String> = validator
            .iter_errors(&instance)
            .map(|e| e.to_string())
            .collect();
        assert!(
            errors.is_empty(),
            "builtin manifest failed schema: {errors:?}"
        );
    }
}

//! Built-in adapter registry (manifest catalog + optional signatures).
//!
//! Source of truth: repo `adapters/manifest.toml` (embedded at compile time).
//! This is the **discovery / verification** surface for the adapter catalog —
//! not a plugin loader. See [docs/adapter-sdk.md](../../../docs/adapter-sdk.md).
//!
//! ## Signatures
//!
//! Per-entry optional fields:
//! - `signature` — `ed25519:<base64>` (preferred) or `hmac-sha256:<hex>` (backcompat)
//! - `signed_by` — key id from the local trust store
//!
//! Verification accepts either scheme when the matching key is trusted.
//! Production builds ship **no** baked-in trust keys; load keys via:
//!
//! 1. **File store** — `$LOCUS_HOME/trust/adapter-keys.toml` (mode `0600`; see [`crate::adapter_trust`])
//! 2. **Env** — [`LOCUS_ADAPTER_TRUST_KEYS`] (overlays file on same id)
//! 3. Tests / tooling — [`install_trust_keys`] / explicit [`verify_entry_with_keys`]
//!
//! `--require-signed` is fail-closed: unsigned or invalid → error.
//!
//! CLI: `locus adapter trust list` · `locus adapter trust add --id root --ed25519-pub <b64>`
//!
//! ### `LOCUS_ADAPTER_TRUST_KEYS`
//!
//! Comma- or semicolon-separated entries:
//!
//! ```text
//! <id>:ed25519:<base64-public-key>
//! <id>:hmac-sha256:<64-hex-secret>
//! ```
//!
//! Example:
//! `LOCUS_ADAPTER_TRUST_KEYS=root:ed25519:BASE64PUB,mock:hmac-sha256:0123…abcdef`

use crate::error::{LocusError, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::sync::OnceLock;

type HmacSha256 = Hmac<Sha256>;

/// Environment variable for process-wide adapter registry trust keys.
pub const LOCUS_ADAPTER_TRUST_KEYS_ENV: &str = "LOCUS_ADAPTER_TRUST_KEYS";

/// Embedded built-in catalog (repo `adapters/manifest.toml`).
const MANIFEST_TOML: &str = include_str!("../../../adapters/manifest.toml");

/// HMAC-SHA256 signature scheme prefix (symmetric stand-in / backcompat).
pub const SIG_SCHEME_HMAC_SHA256: &str = "hmac-sha256";

/// Ed25519 detached signature scheme prefix (preferred).
pub const SIG_SCHEME_ED25519: &str = "ed25519";

/// Canonical material version (bumped when signing fields change).
const CANONICAL_VERSION: &str = "v1";

/// Scheme-specific verification material for one trusted registry key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryKeyMaterial {
    /// 32-byte HMAC secret as lowercase hex (64 chars). Symmetric stand-in.
    HmacSha256 {
        /// 32-byte secret, hex-encoded.
        secret_hex: String,
    },
    /// 32-byte ed25519 public key as standard base64.
    Ed25519Public {
        /// Verifying key bytes, standard base64.
        public_key_b64: String,
    },
}

/// One trusted registry key (id + scheme material).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryTrustKey {
    /// Key id recorded in `signed_by`.
    pub id: String,
    /// Public (or symmetric) material used to verify signatures.
    pub key: RegistryKeyMaterial,
}

impl RegistryTrustKey {
    /// Construct an HMAC-SHA256 trust key (`secret_hex` = 64 hex chars).
    pub fn hmac_sha256(id: impl Into<String>, secret_hex: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            key: RegistryKeyMaterial::HmacSha256 {
                secret_hex: secret_hex.into(),
            },
        }
    }

    /// Construct an ed25519 public trust key (`public_key_b64` = standard base64 of 32 bytes).
    pub fn ed25519_public(id: impl Into<String>, public_key_b64: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            key: RegistryKeyMaterial::Ed25519Public {
                public_key_b64: public_key_b64.into(),
            },
        }
    }

    /// Scheme label for this key (`hmac-sha256` or `ed25519`).
    pub fn scheme(&self) -> &'static str {
        match &self.key {
            RegistryKeyMaterial::HmacSha256 { .. } => SIG_SCHEME_HMAC_SHA256,
            RegistryKeyMaterial::Ed25519Public { .. } => SIG_SCHEME_ED25519,
        }
    }

    /// Parse and return the raw 32-byte HMAC secret (HMAC keys only).
    pub fn secret_bytes(&self) -> Result<[u8; 32]> {
        match &self.key {
            RegistryKeyMaterial::HmacSha256 { secret_hex } => {
                let bytes = hex::decode(secret_hex.trim()).map_err(|e| {
                    LocusError::msg(format!("registry trust key `{}` hex: {e}", self.id))
                })?;
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
            RegistryKeyMaterial::Ed25519Public { .. } => Err(LocusError::msg(format!(
                "registry trust key `{}` is ed25519 (no HMAC secret)",
                self.id
            ))),
        }
    }

    /// Parse the ed25519 verifying key (ed25519 keys only).
    pub fn ed25519_verifying_key(&self) -> Result<VerifyingKey> {
        match &self.key {
            RegistryKeyMaterial::Ed25519Public { public_key_b64 } => {
                let bytes = B64.decode(public_key_b64.trim()).map_err(|e| {
                    LocusError::msg(format!(
                        "registry trust key `{}` ed25519 public base64: {e}",
                        self.id
                    ))
                })?;
                if bytes.len() != 32 {
                    return Err(LocusError::msg(format!(
                        "registry trust key `{}` ed25519 public must be 32 bytes, got {}",
                        self.id,
                        bytes.len()
                    )));
                }
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&bytes);
                VerifyingKey::from_bytes(&arr).map_err(|e| {
                    LocusError::msg(format!(
                        "registry trust key `{}` invalid ed25519 public key: {e}",
                        self.id
                    ))
                })
            }
            RegistryKeyMaterial::HmacSha256 { .. } => Err(LocusError::msg(format!(
                "registry trust key `{}` is hmac-sha256 (no ed25519 public key)",
                self.id
            ))),
        }
    }
}

/// Process-wide trust keys (optional; production default is empty unless file/env set).
static TRUST_KEYS: OnceLock<Vec<RegistryTrustKey>> = OnceLock::new();

/// Default production trust store: `$LOCUS_HOME/trust/adapter-keys.toml` then env overlay.
fn default_trust_keys() -> Vec<RegistryTrustKey> {
    crate::adapter_trust::load_merged_trust_keys_default()
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

/// Parse `LOCUS_ADAPTER_TRUST_KEYS` value into trust keys.
///
/// Format (comma or semicolon separated):
/// `<id>:ed25519:<base64-pubkey>` or `<id>:hmac-sha256:<64-hex-secret>`
pub fn parse_trust_keys_env(raw: &str) -> Result<Vec<RegistryTrustKey>> {
    let mut out = Vec::new();
    for part in raw.split([',', ';']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        // id:scheme:material — material may contain ':' in theory; split carefully.
        let (id, rest) = part.split_once(':').ok_or_else(|| {
            LocusError::msg(format!(
                "trust key entry must be `id:scheme:material`, got `{part}`"
            ))
        })?;
        let id = id.trim();
        if id.is_empty() {
            return Err(LocusError::msg("trust key entry has empty id"));
        }
        let (scheme, material) = rest.split_once(':').ok_or_else(|| {
            LocusError::msg(format!(
                "trust key `{id}` must be `id:scheme:material`, got `{part}`"
            ))
        })?;
        let scheme = scheme.trim().to_ascii_lowercase();
        let material = material.trim();
        if material.is_empty() {
            return Err(LocusError::msg(format!(
                "trust key `{id}` has empty material"
            )));
        }
        let key = match scheme.as_str() {
            SIG_SCHEME_ED25519 => {
                // Validate early so bad env fails closed at parse time.
                let k = RegistryTrustKey::ed25519_public(id, material);
                k.ed25519_verifying_key()?;
                k
            }
            SIG_SCHEME_HMAC_SHA256 => {
                let k = RegistryTrustKey::hmac_sha256(id, material);
                k.secret_bytes()?;
                k
            }
            other => {
                return Err(LocusError::msg(format!(
                    "trust key `{id}`: unknown scheme `{other}` (want `{SIG_SCHEME_ED25519}` or `{SIG_SCHEME_HMAC_SHA256}`)"
                )));
            }
        };
        out.push(key);
    }
    Ok(out)
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
    /// Optional detached signature: `ed25519:<base64>` or `hmac-sha256:<hex>`.
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
    /// Signature field malformed (bad scheme / encoding).
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

/// Parsed wire signature.
enum ParsedSignature {
    HmacSha256([u8; 32]),
    Ed25519(Signature),
}

/// Parse `hmac-sha256:<hex>` or `ed25519:<base64>`.
fn parse_signature(sig: &str) -> Result<ParsedSignature> {
    let s = sig.trim();
    if let Some(hex_part) = s.strip_prefix(&format!("{SIG_SCHEME_HMAC_SHA256}:")) {
        let bytes = hex::decode(hex_part.trim())
            .map_err(|e| LocusError::msg(format!("hmac-sha256 signature hex decode: {e}")))?;
        if bytes.len() != 32 {
            return Err(LocusError::msg(format!(
                "hmac-sha256 signature must be 32 bytes, got {}",
                bytes.len()
            )));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        return Ok(ParsedSignature::HmacSha256(arr));
    }
    if let Some(b64_part) = s.strip_prefix(&format!("{SIG_SCHEME_ED25519}:")) {
        let bytes = B64
            .decode(b64_part.trim())
            .map_err(|e| LocusError::msg(format!("ed25519 signature base64 decode: {e}")))?;
        if bytes.len() != Signature::BYTE_SIZE {
            return Err(LocusError::msg(format!(
                "ed25519 signature must be {} bytes, got {}",
                Signature::BYTE_SIZE,
                bytes.len()
            )));
        }
        let mut arr = [0u8; 64];
        arr.copy_from_slice(&bytes);
        let signature = Signature::from_bytes(&arr);
        return Ok(ParsedSignature::Ed25519(signature));
    }
    Err(LocusError::msg(format!(
        "signature must start with `{SIG_SCHEME_ED25519}:` or `{SIG_SCHEME_HMAC_SHA256}:`"
    )))
}

/// Sign entry material with an HMAC trust key → `hmac-sha256:<hex>`.
pub fn sign_entry_material(material: &str, key: &RegistryTrustKey) -> Result<String> {
    let secret = key.secret_bytes()?;
    let mut mac = HmacSha256::new_from_slice(&secret).expect("HMAC accepts any key length");
    mac.update(material.as_bytes());
    let result = mac.finalize().into_bytes();
    Ok(format!("{SIG_SCHEME_HMAC_SHA256}:{}", hex::encode(result)))
}

/// Sign a full entry with an HMAC trust key (uses [`canonical_entry_material`]).
pub fn sign_entry(entry: &AdapterManifestEntry, key: &RegistryTrustKey) -> Result<String> {
    sign_entry_material(&canonical_entry_material(entry), key)
}

/// Sign entry material with an ed25519 signing key → `ed25519:<base64>`.
pub fn sign_entry_material_ed25519(material: &str, signing_key: &SigningKey) -> String {
    let signature: Signature = signing_key.sign(material.as_bytes());
    format!("{SIG_SCHEME_ED25519}:{}", B64.encode(signature.to_bytes()))
}

/// Sign a full entry with an ed25519 signing key.
pub fn sign_entry_ed25519(entry: &AdapterManifestEntry, signing_key: &SigningKey) -> String {
    sign_entry_material_ed25519(&canonical_entry_material(entry), signing_key)
}

/// Encode an ed25519 verifying key as standard base64 (trust-store material).
pub fn ed25519_public_key_b64(verifying_key: &VerifyingKey) -> String {
    B64.encode(verifying_key.as_bytes())
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
    let material = canonical_entry_material(entry);
    let (status, signed_by, detail) = verify_material_signature(
        &material,
        entry.signature.as_deref(),
        entry.signed_by.as_deref(),
        keys,
    );
    EntryVerifyReport {
        id: entry.id.clone(),
        status,
        signed_by,
        detail,
    }
}

/// Verify a detached signature over canonical `material` against a key set.
///
/// Shared by per-entry catalog verification and release-manifest verification.
/// Returns `(status, signed_by, detail)`.
fn verify_material_signature(
    material: &str,
    signature: Option<&str>,
    signed_by: Option<&str>,
    keys: &[RegistryTrustKey],
) -> (EntryVerifyStatus, Option<String>, Option<String>) {
    let declared_by = signed_by
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);
    let Some(sig) = signature.map(str::trim).filter(|s| !s.is_empty()) else {
        return (
            EntryVerifyStatus::Unsigned,
            declared_by,
            Some("no signature field".into()),
        );
    };

    let parsed = match parse_signature(sig) {
        Ok(p) => p,
        Err(e) => {
            return (
                EntryVerifyStatus::Malformed,
                declared_by,
                Some(e.to_string()),
            );
        }
    };

    let Some(key_id) = declared_by else {
        return (
            EntryVerifyStatus::Malformed,
            None,
            Some("signature present but signed_by missing".into()),
        );
    };

    let Some(key) = keys.iter().find(|k| k.id.eq_ignore_ascii_case(&key_id)) else {
        let detail = format!("key id `{key_id}` not in trust store");
        return (EntryVerifyStatus::UnknownKey, Some(key_id), Some(detail));
    };

    match (&parsed, &key.key) {
        (ParsedSignature::HmacSha256(expected), RegistryKeyMaterial::HmacSha256 { .. }) => {
            let Ok(computed) = sign_entry_material(material, key) else {
                return (
                    EntryVerifyStatus::Malformed,
                    Some(key_id),
                    Some("failed to compute expected signature (bad trust key)".into()),
                );
            };
            let Ok(ParsedSignature::HmacSha256(computed_bytes)) = parse_signature(&computed) else {
                return (
                    EntryVerifyStatus::Malformed,
                    Some(key_id),
                    Some("internal signature format error".into()),
                );
            };
            if ct_eq(expected, &computed_bytes) {
                (EntryVerifyStatus::Valid, Some(key_id), None)
            } else {
                (
                    EntryVerifyStatus::Invalid,
                    Some(key_id),
                    Some("signature mismatch for canonical material".into()),
                )
            }
        }
        (ParsedSignature::Ed25519(signature), RegistryKeyMaterial::Ed25519Public { .. }) => {
            let Ok(vk) = key.ed25519_verifying_key() else {
                return (
                    EntryVerifyStatus::Malformed,
                    Some(key_id),
                    Some("failed to parse ed25519 trust key".into()),
                );
            };
            match vk.verify(material.as_bytes(), signature) {
                Ok(()) => (EntryVerifyStatus::Valid, Some(key_id), None),
                Err(_) => (
                    EntryVerifyStatus::Invalid,
                    Some(key_id),
                    Some("ed25519 signature verification failed".into()),
                ),
            }
        }
        (ParsedSignature::HmacSha256(_), RegistryKeyMaterial::Ed25519Public { .. }) => (
            EntryVerifyStatus::Malformed,
            Some(key_id),
            Some(format!(
                "signature scheme `{SIG_SCHEME_HMAC_SHA256}` does not match trust key scheme `{SIG_SCHEME_ED25519}`"
            )),
        ),
        (ParsedSignature::Ed25519(_), RegistryKeyMaterial::HmacSha256 { .. }) => (
            EntryVerifyStatus::Malformed,
            Some(key_id),
            Some(format!(
                "signature scheme `{SIG_SCHEME_ED25519}` does not match trust key scheme `{SIG_SCHEME_HMAC_SHA256}`"
            )),
        ),
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

// ---------------------------------------------------------------------------
// Signed release manifests (`locus adapter registry export` / `verify-manifest`)
// ---------------------------------------------------------------------------

/// Schema id stamped into exported release manifests.
pub const RELEASE_MANIFEST_SCHEMA: &str = "locus-adapter-registry/v1";

/// Env var holding the operator ed25519 signing key (base64 or 64-hex of the
/// 32-byte seed). Read by `locus adapter registry export --sign`; never
/// echoed back in output or logs.
pub const LOCUS_REGISTRY_SIGNING_KEY_ENV: &str = "LOCUS_REGISTRY_SIGNING_KEY";

/// One adapter row in a release manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseManifestAdapter {
    /// Stable provider id (`supabase`, `github`, …).
    pub id: String,
    /// Human title from the catalog.
    pub name: String,
    /// Locus workspace version this adapter shipped in.
    pub version: String,
    /// Tool names (sorted for determinism).
    pub tools: Vec<String>,
    /// `sha256:<hex>` over [`canonical_entry_material`] — covers name, status,
    /// synthetic flag, capabilities, frozen selectors, tools, destructive tools.
    pub digest: String,
}

/// Canonical registry release manifest (JSON, one per release tag).
///
/// Signature fields are excluded from [`canonical_release_manifest_material`],
/// so signing after building does not invalidate the material.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseManifest {
    /// Always [`RELEASE_MANIFEST_SCHEMA`].
    pub schema: String,
    /// Locus workspace version the manifest was exported from.
    pub locus_version: String,
    /// Built-in adapter set (sorted by id).
    pub adapters: Vec<ReleaseManifestAdapter>,
    /// Optional detached signature — `ed25519:<base64>` only. `hmac-sha256`
    /// is rejected for release manifests: HMAC is symmetric, so every
    /// verifier's trust store would hold the forging secret and the release
    /// attestation would degrade to the weakest key in the store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// Trust-store key id that produced `signature`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed_by: Option<String>,
}

/// `sha256:<hex>` digest of one catalog entry's canonical material.
pub fn entry_digest(entry: &AdapterManifestEntry) -> String {
    use sha2::Digest;
    let mut h = Sha256::new();
    h.update(canonical_entry_material(entry).as_bytes());
    format!("sha256:{}", hex::encode(h.finalize()))
}

/// Build the (unsigned) release manifest for the embedded built-in catalog.
pub fn build_release_manifest() -> Result<ReleaseManifest> {
    Ok(build_release_manifest_from(
        &builtin_manifest()?,
        env!("CARGO_PKG_VERSION"),
    ))
}

/// Build an unsigned release manifest from an explicit catalog + version.
pub fn build_release_manifest_from(
    manifest: &AdapterManifest,
    locus_version: &str,
) -> ReleaseManifest {
    let mut adapters: Vec<ReleaseManifestAdapter> = manifest
        .providers
        .iter()
        .map(|p| {
            let mut tools = p.tools.clone();
            tools.sort_by_key(|t| t.to_ascii_lowercase());
            ReleaseManifestAdapter {
                id: p.id.trim().to_string(),
                name: p.name.trim().to_string(),
                version: locus_version.trim().to_string(),
                tools,
                digest: entry_digest(p),
            }
        })
        .collect();
    adapters.sort_by(|a, b| a.id.cmp(&b.id));
    ReleaseManifest {
        schema: RELEASE_MANIFEST_SCHEMA.into(),
        locus_version: locus_version.trim().to_string(),
        adapters,
        signature: None,
        signed_by: None,
    }
}

/// Canonical signing material for a release manifest (excludes signature fields).
///
/// Stable line format:
/// `locus-registry|v1|{schema}|{locus_version}|{adapter_count}` then one line
/// per adapter (sorted by id): `{id}|{name}|{version}|{tools,…}|{digest}`.
pub fn canonical_release_manifest_material(m: &ReleaseManifest) -> String {
    let mut sorted: Vec<&ReleaseManifestAdapter> = m.adapters.iter().collect();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));
    let mut lines = vec![format!(
        "locus-registry|{CANONICAL_VERSION}|{}|{}|{}",
        m.schema.trim(),
        m.locus_version.trim(),
        sorted.len()
    )];
    for a in sorted {
        let mut tools: Vec<&str> = a.tools.iter().map(|s| s.as_str()).collect();
        tools.sort_by_key(|t| t.to_ascii_lowercase());
        lines.push(format!(
            "{}|{}|{}|{}|{}",
            a.id.trim(),
            a.name.trim(),
            a.version.trim(),
            tools.join(","),
            a.digest.trim(),
        ));
    }
    lines.join("\n")
}

/// Sign a release manifest in place with an operator ed25519 key.
pub fn sign_release_manifest(m: &mut ReleaseManifest, key_id: &str, signing_key: &SigningKey) {
    let material = canonical_release_manifest_material(m);
    m.signature = Some(sign_entry_material_ed25519(&material, signing_key));
    m.signed_by = Some(key_id.trim().to_string());
}

/// Parse an operator-provided ed25519 signing key: standard base64 or 64-hex
/// of the 32-byte seed. Error messages never echo the key material.
pub fn parse_ed25519_signing_key(raw: &str) -> Result<SigningKey> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(LocusError::msg("ed25519 signing key is empty"));
    }
    let bytes = if s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        hex::decode(s).map_err(|_| LocusError::msg("ed25519 signing key: bad hex"))?
    } else {
        B64.decode(s).map_err(|_| {
            LocusError::msg(
                "ed25519 signing key must be standard base64 or 64-hex of the 32-byte seed",
            )
        })?
    };
    if bytes.len() != 32 {
        return Err(LocusError::msg(format!(
            "ed25519 signing key must decode to 32 bytes, got {}",
            bytes.len()
        )));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(SigningKey::from_bytes(&arr))
}

/// Locate a canonical-material separator (`|`, `,`, CR, LF) inside a
/// release-manifest field.
///
/// [`canonical_release_manifest_material`] joins fields with `|`, tools with
/// `,`, and lines with `\n` **without escaping**, so any of those bytes
/// inside a field would make two distinct manifests share one canonical
/// material — and therefore one valid signature (e.g. tools `["a","b"]` vs a
/// single tool literally named `"a,b"`). Fail closed: no real catalog field
/// uses these characters.
fn release_manifest_separator_ambiguity(m: &ReleaseManifest) -> Option<String> {
    fn bad(value: &str) -> bool {
        value.contains(['|', ',', '\r', '\n'])
    }
    let describe = |id: &str, what: &str| {
        format!(
            "release manifest adapter `{id}`: {what} contains a canonical-material \
             separator (`|`, `,`, CR, or LF) — signature would be ambiguous; refusing (fail closed)"
        )
    };
    if bad(&m.locus_version) {
        return Some(
            "release manifest locus_version contains a canonical-material separator \
             (`|`, `,`, CR, or LF) — signature would be ambiguous; refusing (fail closed)"
                .into(),
        );
    }
    for a in &m.adapters {
        if bad(&a.id) {
            return Some(describe(a.id.trim(), "id"));
        }
        if bad(&a.name) {
            return Some(describe(a.id.trim(), "name"));
        }
        if bad(&a.version) {
            return Some(describe(a.id.trim(), "version"));
        }
        if bad(&a.digest) {
            return Some(describe(a.id.trim(), "digest"));
        }
        for tool in &a.tools {
            if bad(tool) {
                return Some(describe(a.id.trim(), "tool name"));
            }
        }
    }
    None
}

/// Parse + validate a release manifest JSON document.
pub fn parse_release_manifest(json_src: &str) -> Result<ReleaseManifest> {
    let m: ReleaseManifest = serde_json::from_str(json_src)
        .map_err(|e| LocusError::msg(format!("release manifest parse error: {e}")))?;
    if m.schema != RELEASE_MANIFEST_SCHEMA {
        return Err(LocusError::msg(format!(
            "release manifest schema must be `{RELEASE_MANIFEST_SCHEMA}`, got `{}`",
            m.schema
        )));
    }
    if let Some(ambiguity) = release_manifest_separator_ambiguity(&m) {
        return Err(LocusError::msg(ambiguity));
    }
    let mut seen = std::collections::BTreeSet::new();
    for a in &m.adapters {
        let id = a.id.trim();
        if id.is_empty() {
            return Err(LocusError::msg("release manifest adapter has empty id"));
        }
        if !seen.insert(id.to_ascii_lowercase()) {
            return Err(LocusError::msg(format!(
                "release manifest duplicate adapter id `{id}`"
            )));
        }
    }
    Ok(m)
}

/// Serialize a release manifest as pretty JSON (trailing newline).
pub fn release_manifest_json(m: &ReleaseManifest) -> Result<String> {
    let body = serde_json::to_string_pretty(m)
        .map_err(|e| LocusError::msg(format!("serialize release manifest: {e}")))?;
    Ok(format!("{body}\n"))
}

/// Result of verifying a release manifest against a trust store + the running
/// binary's adapter set.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseManifestVerifyReport {
    /// Signature verdict (same taxonomy as per-entry catalog verification).
    pub signature: EntryVerifyStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signed_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature_detail: Option<String>,
    /// Version the manifest was exported from.
    pub manifest_version: String,
    /// Version of the verifying binary.
    pub binary_version: String,
    pub adapter_count: usize,
    /// True when an unsigned manifest was explicitly permitted.
    pub allow_unsigned: bool,
    /// Human-readable drift findings (empty = adapter sets match).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drift: Vec<String>,
    /// Overall verdict (fail closed).
    pub ok: bool,
}

/// Verify a release manifest against the running binary's built-in adapter set.
///
/// Fail closed: unsigned (unless `allow_unsigned`), unknown key, invalid or
/// malformed signature, and **any** adapter-set drift all make `ok = false`.
/// `allow_unsigned` only excuses a *missing* signature — a present-but-bad
/// signature still fails.
pub fn verify_release_manifest_with_keys(
    manifest: &ReleaseManifest,
    keys: &[RegistryTrustKey],
    allow_unsigned: bool,
) -> Result<ReleaseManifestVerifyReport> {
    let current = build_release_manifest()?;
    Ok(verify_release_manifest_against(
        manifest,
        &current,
        keys,
        allow_unsigned,
    ))
}

/// Verify against an explicit "current" manifest (tests / offline).
pub fn verify_release_manifest_against(
    manifest: &ReleaseManifest,
    current: &ReleaseManifest,
    keys: &[RegistryTrustKey],
    allow_unsigned: bool,
) -> ReleaseManifestVerifyReport {
    let declared_by = manifest
        .signed_by
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);
    let declared_sig = manifest.signature.as_deref().map(str::trim);
    let (signature, signed_by, signature_detail) = if let Some(ambiguity) =
        release_manifest_separator_ambiguity(manifest)
    {
        // Unescaped separators would let two distinct manifests share one
        // canonical material (and thus one valid signature) — never report
        // Valid over ambiguous content, even for programmatically built
        // manifests that skipped parse_release_manifest.
        (EntryVerifyStatus::Malformed, declared_by, Some(ambiguity))
    } else if declared_sig.is_some_and(|s| s.starts_with(&format!("{SIG_SCHEME_HMAC_SHA256}:"))) {
        // Release attestation is ed25519-only: HMAC is symmetric, so anyone
        // with read access to a verifier's trust store could forge a
        // signature=valid manifest (weakest-key downgrade). Signing
        // ([`sign_release_manifest`]) can only produce ed25519 anyway.
        (
            EntryVerifyStatus::Malformed,
            declared_by,
            Some("release manifests must be ed25519-signed (hmac-sha256 rejected: symmetric keys cannot attest a release)".into()),
        )
    } else {
        let material = canonical_release_manifest_material(manifest);
        verify_material_signature(
            &material,
            manifest.signature.as_deref(),
            manifest.signed_by.as_deref(),
            keys,
        )
    };

    let mut drift = Vec::new();
    if manifest.locus_version.trim() != current.locus_version.trim() {
        drift.push(format!(
            "manifest exported from locus {} but this binary is {}",
            manifest.locus_version, current.locus_version
        ));
    }

    let by_id =
        |m: &ReleaseManifest| -> std::collections::BTreeMap<String, ReleaseManifestAdapter> {
            m.adapters
                .iter()
                .map(|a| (a.id.trim().to_ascii_lowercase(), a.clone()))
                .collect()
        };
    let want = by_id(manifest);
    let have = by_id(current);

    for (id, w) in &want {
        let Some(h) = have.get(id) else {
            drift.push(format!("adapter `{id}` in manifest but not in this binary"));
            continue;
        };
        if w.name.trim() != h.name.trim() {
            drift.push(format!(
                "adapter `{id}` name drift: manifest `{}` vs binary `{}`",
                w.name, h.name
            ));
        }
        if w.version.trim() != h.version.trim() {
            drift.push(format!(
                "adapter `{id}` version drift: manifest `{}` vs binary `{}`",
                w.version, h.version
            ));
        }
        let mut wt: Vec<String> = w.tools.iter().map(|t| t.trim().to_string()).collect();
        let mut ht: Vec<String> = h.tools.iter().map(|t| t.trim().to_string()).collect();
        wt.sort_by_key(|t| t.to_ascii_lowercase());
        ht.sort_by_key(|t| t.to_ascii_lowercase());
        if wt != ht {
            drift.push(format!(
                "adapter `{id}` tool drift: manifest [{}] vs binary [{}]",
                wt.join(", "),
                ht.join(", ")
            ));
        }
        if w.digest.trim() != h.digest.trim() {
            drift.push(format!(
                "adapter `{id}` digest drift: manifest {} vs binary {}",
                w.digest, h.digest
            ));
        }
    }
    for id in have.keys() {
        if !want.contains_key(id) {
            drift.push(format!(
                "adapter `{id}` present in binary but missing from manifest"
            ));
        }
    }

    let signature_ok = match signature {
        EntryVerifyStatus::Valid => true,
        EntryVerifyStatus::Unsigned => allow_unsigned,
        _ => false,
    };
    let ok = signature_ok && drift.is_empty();

    ReleaseManifestVerifyReport {
        signature,
        signed_by,
        signature_detail,
        manifest_version: manifest.locus_version.clone(),
        binary_version: current.locus_version.clone(),
        adapter_count: manifest.adapters.len(),
        allow_unsigned,
        drift,
        ok,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    /// Fixed mock HMAC registry key for unit tests (not a production secret).
    const MOCK_KEY_ID: &str = "locus-registry-mock";
    const MOCK_SECRET_HEX: &str =
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn mock_hmac_key() -> RegistryTrustKey {
        RegistryTrustKey::hmac_sha256(MOCK_KEY_ID, MOCK_SECRET_HEX)
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

    fn gen_ed25519() -> (SigningKey, RegistryTrustKey) {
        let signing = SigningKey::generate(&mut OsRng);
        let vk = signing.verifying_key();
        let trust =
            RegistryTrustKey::ed25519_public("locus-ed25519-test", ed25519_public_key_b64(&vk));
        (signing, trust)
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

    /// Drift guard: the built-in registry catalog (adapters/manifest.toml)
    /// and the dispatchable provider set (`adapters::known_providers()`, kept
    /// in sync with the `adapter_for` match by its own test) must be equal
    /// both ways — a future adapter cannot miss either surface.
    #[test]
    fn builtin_catalog_matches_dispatchable_providers() {
        use std::collections::BTreeSet;
        let registry: BTreeSet<String> = builtin_manifest()
            .expect("builtin manifest")
            .providers
            .iter()
            .map(|p| p.id.to_ascii_lowercase())
            .collect();
        let dispatch: BTreeSet<String> = crate::adapters::known_providers()
            .iter()
            .map(|p| p.to_ascii_lowercase())
            .collect();
        let missing_from_registry: Vec<_> = dispatch.difference(&registry).collect();
        let missing_from_dispatch: Vec<_> = registry.difference(&dispatch).collect();
        assert!(
            missing_from_registry.is_empty(),
            "dispatchable providers missing from adapters/manifest.toml registry catalog: {missing_from_registry:?}"
        );
        assert!(
            missing_from_dispatch.is_empty(),
            "registry catalog providers with no dispatchable adapter in adapters/mod.rs: {missing_from_dispatch:?}"
        );
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
    fn hmac_sign_and_verify_roundtrip() {
        let key = mock_hmac_key();
        let mut entry = sample_entry("stripe");
        let sig = sign_entry(&entry, &key).unwrap();
        assert!(sig.starts_with("hmac-sha256:"));
        entry.signature = Some(sig);
        entry.signed_by = Some(MOCK_KEY_ID.into());

        let report = verify_entry_with_keys(&entry, &[key]);
        assert_eq!(report.status, EntryVerifyStatus::Valid);
        assert!(report.status.is_trusted());
    }

    #[test]
    fn ed25519_sign_and_verify_roundtrip() {
        let (signing, trust) = gen_ed25519();
        let mut entry = sample_entry("github");
        let sig = sign_entry_ed25519(&entry, &signing);
        assert!(
            sig.starts_with("ed25519:"),
            "expected ed25519: prefix, got {sig}"
        );
        entry.signature = Some(sig);
        entry.signed_by = Some(trust.id.clone());

        let report = verify_entry_with_keys(&entry, &[trust]);
        assert_eq!(report.status, EntryVerifyStatus::Valid, "{report:?}");
        assert!(report.status.is_trusted());
    }

    #[test]
    fn ed25519_verify_detects_tamper() {
        let (signing, trust) = gen_ed25519();
        let mut entry = sample_entry("aws");
        entry.signature = Some(sign_entry_ed25519(&entry, &signing));
        entry.signed_by = Some(trust.id.clone());
        entry.tools.push("aws.evil".into());

        let report = verify_entry_with_keys(&entry, &[trust]);
        assert_eq!(report.status, EntryVerifyStatus::Invalid);
    }

    #[test]
    fn verify_detects_tamper_hmac() {
        let key = mock_hmac_key();
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
        let key = mock_hmac_key();
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
        let report = verify_entry_with_keys(&entry, &[mock_hmac_key()]);
        assert_eq!(report.status, EntryVerifyStatus::Malformed);
    }

    #[test]
    fn scheme_mismatch_is_malformed() {
        let (signing, _ed_trust) = gen_ed25519();
        let hmac = mock_hmac_key();
        let mut entry = sample_entry("vercel");
        // Sign with ed25519 but present an HMAC trust key under the same id.
        entry.signature = Some(sign_entry_ed25519(&entry, &signing));
        entry.signed_by = Some(MOCK_KEY_ID.into());
        let report = verify_entry_with_keys(&entry, &[hmac]);
        assert_eq!(report.status, EntryVerifyStatus::Malformed);
        assert!(
            report
                .detail
                .as_deref()
                .unwrap_or("")
                .contains("does not match"),
            "{report:?}"
        );
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
    fn require_signed_accepts_hmac_or_ed25519_when_trusted() {
        let hmac = mock_hmac_key();
        let (signing, ed_trust) = gen_ed25519();

        let mut e1 = sample_entry("github");
        e1.signature = Some(sign_entry(&e1, &hmac).unwrap());
        e1.signed_by = Some(MOCK_KEY_ID.into());

        let mut e2 = sample_entry("vercel");
        e2.signature = Some(sign_entry_ed25519(&e2, &signing));
        e2.signed_by = Some(ed_trust.id.clone());

        let m = AdapterManifest {
            version: 1,
            providers: vec![e1, e2],
            signature: None,
            signed_by: None,
        };
        let keys = vec![hmac, ed_trust];
        let report = verify_manifest_with_keys(&m, true, &keys);
        assert!(report.ok, "errors: {:?}", report.errors);
        assert_eq!(report.trusted, 2);
        assert_eq!(report.unsigned, 0);
        assert!(report.errors.is_empty());
    }

    #[test]
    fn require_signed_passes_when_all_valid_hmac() {
        let key = mock_hmac_key();
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
        let key = mock_hmac_key();
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
        // Built-in catalog ships unsigned; soft verify must pass.
        let report = verify_builtin(false).unwrap();
        assert!(report.ok, "builtin soft verify failed: {:?}", report.errors);
        assert_eq!(report.failed, 0);
    }

    #[test]
    fn parse_trust_keys_env_ed25519_and_hmac() {
        let (signing, _) = gen_ed25519();
        let pub_b64 = ed25519_public_key_b64(&signing.verifying_key());
        let raw = format!("root:ed25519:{pub_b64};mock:hmac-sha256:{MOCK_SECRET_HEX}");
        let keys = parse_trust_keys_env(&raw).expect("parse env keys");
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0].id, "root");
        assert_eq!(keys[0].scheme(), SIG_SCHEME_ED25519);
        assert_eq!(keys[1].id, "mock");
        assert_eq!(keys[1].scheme(), SIG_SCHEME_HMAC_SHA256);

        // Round-trip verify with parsed ed25519 key.
        let mut entry = sample_entry("stripe");
        entry.signature = Some(sign_entry_ed25519(&entry, &signing));
        entry.signed_by = Some("root".into());
        let report = verify_entry_with_keys(&entry, &keys);
        assert_eq!(report.status, EntryVerifyStatus::Valid);
    }

    #[test]
    fn parse_trust_keys_env_rejects_bad_scheme() {
        let err = parse_trust_keys_env("k:rsa:abc").unwrap_err().to_string();
        assert!(err.contains("unknown scheme"), "{err}");
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

    // ---- release manifest (signed registry export) ----

    fn small_catalog() -> AdapterManifest {
        AdapterManifest {
            version: 1,
            providers: vec![sample_entry("github"), sample_entry("vercel")],
            signature: None,
            signed_by: None,
        }
    }

    #[test]
    fn release_manifest_sign_verify_roundtrip() {
        let (signing, trust) = gen_ed25519();
        let current = build_release_manifest_from(&small_catalog(), "0.4.0");
        let mut m = current.clone();
        sign_release_manifest(&mut m, &trust.id, &signing);
        assert!(m.signature.as_deref().unwrap().starts_with("ed25519:"));
        assert_eq!(m.signed_by.as_deref(), Some(trust.id.as_str()));

        let report =
            verify_release_manifest_against(&m, &current, std::slice::from_ref(&trust), false);
        assert_eq!(report.signature, EntryVerifyStatus::Valid, "{report:?}");
        assert!(report.drift.is_empty(), "{:?}", report.drift);
        assert!(report.ok, "{report:?}");
    }

    fn manifest_with_tools(tools: Vec<String>) -> ReleaseManifest {
        ReleaseManifest {
            schema: RELEASE_MANIFEST_SCHEMA.into(),
            locus_version: "0.4.0".into(),
            adapters: vec![ReleaseManifestAdapter {
                id: "github".into(),
                name: "GitHub".into(),
                version: "0.4.0".into(),
                tools,
                digest: "sha256:abc".into(),
            }],
            signature: None,
            signed_by: None,
        }
    }

    #[test]
    fn release_manifest_comma_tool_name_cannot_reuse_split_list_signature() {
        let (signing, trust) = gen_ed25519();
        let split = manifest_with_tools(vec!["a".into(), "b".into()]);
        let mut joined = manifest_with_tools(vec!["a,b".into()]);
        // Unescaped separators make the two distinct documents share one
        // canonical material — exactly the ambiguity the verifier must refuse
        // to attest over.
        assert_eq!(
            canonical_release_manifest_material(&split),
            canonical_release_manifest_material(&joined)
        );

        let mut signed_split = split.clone();
        sign_release_manifest(&mut signed_split, &trust.id, &signing);
        joined.signature = signed_split.signature.clone();
        joined.signed_by = signed_split.signed_by.clone();

        // Current == joined so the drift comparison is silent and only the
        // signature verdict is in play (the --json report exports it standalone).
        let report =
            verify_release_manifest_against(&joined, &joined, std::slice::from_ref(&trust), false);
        assert_eq!(report.signature, EntryVerifyStatus::Malformed, "{report:?}");
        assert!(!report.ok, "{report:?}");
        assert!(
            report
                .signature_detail
                .as_deref()
                .unwrap_or("")
                .contains("separator"),
            "{report:?}"
        );

        // The honest split-tool manifest still verifies.
        let honest = verify_release_manifest_against(
            &signed_split,
            &split,
            std::slice::from_ref(&trust),
            false,
        );
        assert_eq!(honest.signature, EntryVerifyStatus::Valid, "{honest:?}");
    }

    #[test]
    fn parse_release_manifest_rejects_separator_bytes() {
        for bad in [
            manifest_with_tools(vec!["a,b".into()]),
            manifest_with_tools(vec!["a|b".into()]),
            {
                let mut m = manifest_with_tools(vec!["a".into()]);
                m.adapters[0].name = "Git|Hub".into();
                m
            },
            {
                let mut m = manifest_with_tools(vec!["a".into()]);
                m.adapters[0].version = "0.4.0\nx".into();
                m
            },
        ] {
            let json = release_manifest_json(&bad).unwrap();
            let err = parse_release_manifest(&json)
                .expect_err("separator bytes in fields must be rejected");
            assert!(err.to_string().contains("separator"), "{err}");
        }
    }

    #[test]
    fn release_manifest_hmac_signature_rejected() {
        // Signing is ed25519-only; a symmetric HMAC trust key (whose secret
        // every verifier holds) must never yield signature=valid for a
        // release manifest — that would be a weakest-key downgrade.
        let key = mock_hmac_key();
        let current = build_release_manifest_from(&small_catalog(), "0.4.0");
        let mut m = current.clone();
        let material = canonical_release_manifest_material(&m);
        m.signature = Some(sign_entry_material(&material, &key).unwrap());
        m.signed_by = Some(key.id.clone());

        let report = verify_release_manifest_against(&m, &current, &[key], false);
        assert_eq!(report.signature, EntryVerifyStatus::Malformed, "{report:?}");
        assert!(!report.ok, "{report:?}");
        assert!(
            report
                .signature_detail
                .as_deref()
                .unwrap_or("")
                .contains("ed25519"),
            "{report:?}"
        );
    }

    #[test]
    fn release_manifest_tamper_fails() {
        let (signing, trust) = gen_ed25519();
        let current = build_release_manifest_from(&small_catalog(), "0.4.0");
        let mut m = current.clone();
        sign_release_manifest(&mut m, &trust.id, &signing);
        // Tamper after signing.
        m.adapters[0].tools.push("github.evil".into());

        let report = verify_release_manifest_against(&m, &current, &[trust], false);
        assert_eq!(report.signature, EntryVerifyStatus::Invalid, "{report:?}");
        assert!(!report.ok);
    }

    #[test]
    fn release_manifest_drifted_adapter_set_fails() {
        let (signing, trust) = gen_ed25519();
        // Manifest signed over a two-adapter set…
        let signed_over = build_release_manifest_from(&small_catalog(), "0.4.0");
        let mut m = signed_over.clone();
        sign_release_manifest(&mut m, &trust.id, &signing);

        // …but the "running binary" carries an extra adapter.
        let mut bigger = small_catalog();
        bigger.providers.push(sample_entry("stripe"));
        let current = build_release_manifest_from(&bigger, "0.4.0");

        let report = verify_release_manifest_against(&m, &current, &[trust], false);
        assert_eq!(report.signature, EntryVerifyStatus::Valid, "{report:?}");
        assert!(
            report
                .drift
                .iter()
                .any(|d| d.contains("stripe") && d.contains("missing from manifest")),
            "{:?}",
            report.drift
        );
        assert!(!report.ok, "drift must fail closed: {report:?}");
    }

    #[test]
    fn release_manifest_tool_drift_fails() {
        let (signing, trust) = gen_ed25519();
        let current = build_release_manifest_from(&small_catalog(), "0.4.0");
        // Manifest claims a different tool set for github (re-signed so the
        // signature itself is valid — drift alone must fail).
        let mut m = current.clone();
        m.adapters[0].tools = vec!["github.scope".into()];
        sign_release_manifest(&mut m, &trust.id, &signing);

        let report = verify_release_manifest_against(&m, &current, &[trust], false);
        assert_eq!(report.signature, EntryVerifyStatus::Valid, "{report:?}");
        assert!(
            report.drift.iter().any(|d| d.contains("tool drift")),
            "{:?}",
            report.drift
        );
        assert!(!report.ok);
    }

    #[test]
    fn release_manifest_unsigned_fails_closed() {
        let current = build_release_manifest_from(&small_catalog(), "0.4.0");
        let report = verify_release_manifest_against(&current, &current, &[], false);
        assert_eq!(report.signature, EntryVerifyStatus::Unsigned);
        assert!(!report.ok, "unsigned must fail without --allow-unsigned");

        let relaxed = verify_release_manifest_against(&current, &current, &[], true);
        assert!(
            relaxed.ok,
            "drift-only check with allow_unsigned: {relaxed:?}"
        );
    }

    #[test]
    fn release_manifest_bad_signature_fails_even_with_allow_unsigned() {
        let (signing, _trust) = gen_ed25519();
        let current = build_release_manifest_from(&small_catalog(), "0.4.0");
        let mut m = current.clone();
        sign_release_manifest(&mut m, "unknown-root", &signing);

        // Key not in trust store: present-but-unverifiable signature must fail
        // even when unsigned manifests are permitted.
        let report = verify_release_manifest_against(&m, &current, &[], true);
        assert_eq!(report.signature, EntryVerifyStatus::UnknownKey);
        assert!(!report.ok, "{report:?}");
    }

    #[test]
    fn release_manifest_version_mismatch_is_drift() {
        let (signing, trust) = gen_ed25519();
        let mut m = build_release_manifest_from(&small_catalog(), "0.3.0");
        sign_release_manifest(&mut m, &trust.id, &signing);
        let current = build_release_manifest_from(&small_catalog(), "0.4.0");

        let report = verify_release_manifest_against(&m, &current, &[trust], false);
        assert!(
            report
                .drift
                .iter()
                .any(|d| d.contains("exported from locus")),
            "{:?}",
            report.drift
        );
        assert!(!report.ok);
    }

    #[test]
    fn canonical_release_material_stable_under_reorder() {
        let m1 = build_release_manifest_from(&small_catalog(), "0.4.0");
        let mut m2 = m1.clone();
        m2.adapters.reverse();
        m2.adapters[0].tools.reverse();
        assert_eq!(
            canonical_release_manifest_material(&m1),
            canonical_release_manifest_material(&m2)
        );
    }

    #[test]
    fn release_manifest_json_roundtrip() {
        let (signing, trust) = gen_ed25519();
        let mut m = build_release_manifest().expect("builtin release manifest");
        assert_eq!(m.schema, RELEASE_MANIFEST_SCHEMA);
        assert_eq!(m.locus_version, env!("CARGO_PKG_VERSION"));
        assert!(m.adapters.len() >= 5);
        sign_release_manifest(&mut m, &trust.id, &signing);

        let body = release_manifest_json(&m).unwrap();
        let parsed = parse_release_manifest(&body).unwrap();
        assert_eq!(parsed, m);

        let report = verify_release_manifest_with_keys(&parsed, &[trust], false).unwrap();
        assert!(report.ok, "{report:?}");
        assert_eq!(report.signature, EntryVerifyStatus::Valid);
    }

    #[test]
    fn parse_release_manifest_rejects_wrong_schema() {
        let err = parse_release_manifest(
            r#"{"schema":"other/v9","locus_version":"0.4.0","adapters":[]}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("schema"), "{err}");
    }

    #[test]
    fn parse_release_manifest_rejects_duplicate_ids() {
        let err = parse_release_manifest(
            r#"{"schema":"locus-adapter-registry/v1","locus_version":"0.4.0","adapters":[
                {"id":"github","name":"A","version":"0.4.0","tools":[],"digest":"sha256:aa"},
                {"id":"GitHub","name":"B","version":"0.4.0","tools":[],"digest":"sha256:bb"}
            ]}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("duplicate"), "{err}");
    }

    #[test]
    fn parse_ed25519_signing_key_hex_and_base64() {
        let signing = SigningKey::generate(&mut OsRng);
        let seed = signing.to_bytes();

        let from_hex = parse_ed25519_signing_key(&hex::encode(seed)).unwrap();
        assert_eq!(from_hex.to_bytes(), seed);

        let from_b64 = parse_ed25519_signing_key(&B64.encode(seed)).unwrap();
        assert_eq!(from_b64.to_bytes(), seed);

        // Errors must not echo material.
        let bad = "!!!not-a-key!!!";
        let err = parse_ed25519_signing_key(bad).unwrap_err().to_string();
        assert!(
            !err.contains(bad),
            "error must not echo key material: {err}"
        );
        assert!(parse_ed25519_signing_key("").is_err());
        assert!(parse_ed25519_signing_key(&B64.encode([1u8; 16])).is_err());
    }
}

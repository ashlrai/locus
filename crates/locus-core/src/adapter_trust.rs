//! Production adapter-registry trust store under `$LOCUS_HOME/trust/`.
//!
//! ## Layout
//!
//! ```text
//! $LOCUS_HOME/trust/adapter-keys.toml   # mode 0600
//! ```
//!
//! ## Merge order
//!
//! 1. File keys from `adapter-keys.toml` (missing file → empty)
//! 2. `LOCUS_ADAPTER_TRUST_KEYS` env overlay (same id → env replaces)
//!
//! Production ships **no** baked-in keys. Prefer ed25519 public pins;
//! HMAC-SHA256 is backcompat only (secrets never printed in listings).
//!
//! CLI: `locus adapter trust list` · `locus adapter trust add --id root --ed25519-pub <b64>`
//!
//! See [docs/adapter-sdk.md](../../../docs/adapter-sdk.md).

use crate::adapter_registry::{
    parse_trust_keys_env, RegistryKeyMaterial, RegistryTrustKey, LOCUS_ADAPTER_TRUST_KEYS_ENV,
    SIG_SCHEME_ED25519, SIG_SCHEME_HMAC_SHA256,
};
use crate::error::{LocusError, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Directory under `$LOCUS_HOME` for registry trust pins.
pub const ADAPTER_TRUST_DIR_NAME: &str = "trust";

/// File-backed trust store relative to `$LOCUS_HOME/trust/`.
pub const ADAPTER_TRUST_KEYS_FILE_NAME: &str = "adapter-keys.toml";

/// Schema version for `adapter-keys.toml`.
pub const ADAPTER_TRUST_KEYS_VERSION: u32 = 1;

/// Where a trust key was loaded from (for CLI listing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustKeyOrigin {
    /// `$LOCUS_HOME/trust/adapter-keys.toml`
    File,
    /// `LOCUS_ADAPTER_TRUST_KEYS` environment variable
    Env,
}

/// One trust key with its origin (file vs env).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustKeyListing {
    pub id: String,
    pub scheme: String,
    /// Material preview: ed25519 public base64, or a redacted HMAC label.
    pub material: String,
    pub origin: TrustKeyOrigin,
}

/// On-disk trust store document (`adapter-keys.toml`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AdapterTrustKeysFile {
    /// Schema version (currently `1`).
    #[serde(default = "default_trust_file_version")]
    pub version: u32,
    #[serde(default)]
    pub keys: Vec<AdapterTrustKeyFileEntry>,
}

fn default_trust_file_version() -> u32 {
    ADAPTER_TRUST_KEYS_VERSION
}

/// One key row in `adapter-keys.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdapterTrustKeyFileEntry {
    pub id: String,
    /// `ed25519` or `hmac-sha256`.
    pub scheme: String,
    /// ed25519 verifying key (standard base64 of 32 bytes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_key_b64: Option<String>,
    /// HMAC secret (64 hex chars). Prefer ed25519 in production.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_hex: Option<String>,
}

/// Result of adding a key to the file store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustKeyAddResult {
    pub path: PathBuf,
    pub key: RegistryTrustKey,
    /// True when an existing key with the same id was replaced.
    pub replaced: bool,
}

/// `$LOCUS_HOME/trust` directory.
pub fn adapter_trust_dir(locus_home: &Path) -> PathBuf {
    locus_home.join(ADAPTER_TRUST_DIR_NAME)
}

/// `$LOCUS_HOME/trust/adapter-keys.toml`.
pub fn adapter_trust_keys_path(locus_home: &Path) -> PathBuf {
    adapter_trust_dir(locus_home).join(ADAPTER_TRUST_KEYS_FILE_NAME)
}

/// Load merged trust keys using [`crate::store::locus_home`] + env.
///
/// On home-resolution failure returns empty. Bad env / bad file are skipped
/// for that source (fail closed for `--require-signed`).
pub fn load_merged_trust_keys_default() -> Vec<RegistryTrustKey> {
    match crate::store::locus_home() {
        Ok(home) => load_merged_trust_keys(&home),
        Err(_) => load_env_trust_keys().unwrap_or_default(),
    }
}

/// Load file keys then overlay `LOCUS_ADAPTER_TRUST_KEYS` (env wins on same id).
pub fn load_merged_trust_keys(locus_home: &Path) -> Vec<RegistryTrustKey> {
    let path = adapter_trust_keys_path(locus_home);
    let mut keys = load_trust_keys_file(&path).unwrap_or_default();
    if let Ok(env_keys) = load_env_trust_keys() {
        merge_trust_keys(&mut keys, env_keys);
    }
    keys
}

fn load_env_trust_keys() -> Result<Vec<RegistryTrustKey>> {
    match std::env::var(LOCUS_ADAPTER_TRUST_KEYS_ENV) {
        Ok(raw) if !raw.trim().is_empty() => parse_trust_keys_env(&raw),
        _ => Ok(Vec::new()),
    }
}

/// Merge `extra` into `base`, replacing any existing key with the same id
/// (case-insensitive). New ids from `extra` are appended in encounter order.
pub fn merge_trust_keys(base: &mut Vec<RegistryTrustKey>, extra: Vec<RegistryTrustKey>) {
    for k in extra {
        if let Some(existing) = base.iter_mut().find(|e| e.id.eq_ignore_ascii_case(&k.id)) {
            *existing = k;
        } else {
            base.push(k);
        }
    }
}

/// Load trust keys from a TOML file. Missing file → empty vec (not an error).
pub fn load_trust_keys_file(path: &Path) -> Result<Vec<RegistryTrustKey>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let body = fs::read_to_string(path)
        .map_err(|e| LocusError::msg(format!("read adapter trust keys {}: {e}", path.display())))?;
    let doc = parse_trust_keys_file(&body)?;
    trust_file_to_keys(&doc)
}

/// Parse `adapter-keys.toml` body.
pub fn parse_trust_keys_file(toml_src: &str) -> Result<AdapterTrustKeysFile> {
    let doc: AdapterTrustKeysFile = toml::from_str(toml_src)
        .map_err(|e| LocusError::msg(format!("adapter trust keys parse error: {e}")))?;
    if doc.version == 0 {
        return Err(LocusError::msg(
            "adapter trust keys version must be >= 1 (got 0)",
        ));
    }
    Ok(doc)
}

fn trust_file_to_keys(doc: &AdapterTrustKeysFile) -> Result<Vec<RegistryTrustKey>> {
    let mut out = Vec::with_capacity(doc.keys.len());
    let mut seen = std::collections::BTreeSet::new();
    for entry in &doc.keys {
        let id = entry.id.trim();
        if id.is_empty() {
            return Err(LocusError::msg("adapter trust key has empty id"));
        }
        if !seen.insert(id.to_ascii_lowercase()) {
            return Err(LocusError::msg(format!(
                "adapter trust keys: duplicate id `{id}`"
            )));
        }
        let scheme = entry.scheme.trim().to_ascii_lowercase();
        let key = match scheme.as_str() {
            SIG_SCHEME_ED25519 => {
                let material = entry
                    .public_key_b64
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        LocusError::msg(format!(
                            "trust key `{id}` scheme ed25519 requires public_key_b64"
                        ))
                    })?;
                let k = RegistryTrustKey::ed25519_public(id, material);
                k.ed25519_verifying_key()?;
                k
            }
            SIG_SCHEME_HMAC_SHA256 => {
                let material = entry
                    .secret_hex
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        LocusError::msg(format!(
                            "trust key `{id}` scheme hmac-sha256 requires secret_hex"
                        ))
                    })?;
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

fn keys_to_trust_file(keys: &[RegistryTrustKey]) -> AdapterTrustKeysFile {
    AdapterTrustKeysFile {
        version: ADAPTER_TRUST_KEYS_VERSION,
        keys: keys
            .iter()
            .map(|k| match &k.key {
                RegistryKeyMaterial::Ed25519Public { public_key_b64 } => AdapterTrustKeyFileEntry {
                    id: k.id.clone(),
                    scheme: SIG_SCHEME_ED25519.into(),
                    public_key_b64: Some(public_key_b64.clone()),
                    secret_hex: None,
                },
                RegistryKeyMaterial::HmacSha256 { secret_hex } => AdapterTrustKeyFileEntry {
                    id: k.id.clone(),
                    scheme: SIG_SCHEME_HMAC_SHA256.into(),
                    public_key_b64: None,
                    secret_hex: Some(secret_hex.clone()),
                },
            })
            .collect(),
    }
}

/// Serialize and write trust keys to `path` with mode `0600` (Unix).
///
/// Creates the parent directory (`trust/`) if needed (`0700` on Unix).
pub fn save_trust_keys_file(path: &Path, keys: &[RegistryTrustKey]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            LocusError::msg(format!(
                "create adapter trust dir {}: {e}",
                parent.display()
            ))
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(parent)
                .map_err(|e| LocusError::msg(format!("stat {}: {e}", parent.display())))?
                .permissions();
            perms.set_mode(0o700);
            fs::set_permissions(parent, perms).map_err(|e| {
                LocusError::msg(format!("chmod trust dir {}: {e}", parent.display()))
            })?;
        }
    }
    let doc = keys_to_trust_file(keys);
    let body = toml::to_string_pretty(&doc)
        .map_err(|e| LocusError::msg(format!("serialize adapter trust keys: {e}")))?;
    let header = "# Locus adapter registry trust keys\n\
# Public material for verifying signed adapters/manifest entries.\n\
# Mode 0600. Prefer ed25519; HMAC is backcompat only.\n\
# CLI: locus adapter trust list | trust add --id <id> --ed25519-pub <b64>\n\
# Docs: docs/adapter-sdk.md\n\n";
    let full = format!("{header}{body}");
    write_restricted_file(path, full.as_bytes())
}

fn write_restricted_file(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::write(path, bytes)
        .map_err(|e| LocusError::msg(format!("write {}: {e}", path.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)
            .map_err(|e| LocusError::msg(format!("stat {}: {e}", path.display())))?
            .permissions();
        perms.set_mode(0o600);
        fs::set_permissions(path, perms)
            .map_err(|e| LocusError::msg(format!("chmod {}: {e}", path.display())))?;
    }
    Ok(())
}

/// Add (or replace) an ed25519 public trust key in `$LOCUS_HOME/trust/adapter-keys.toml`.
pub fn add_ed25519_trust_key(
    locus_home: &Path,
    id: &str,
    public_key_b64: &str,
) -> Result<TrustKeyAddResult> {
    let id = id.trim();
    if id.is_empty() {
        return Err(LocusError::msg("trust key id must not be empty"));
    }
    if id.contains(':') || id.contains(',') || id.contains(';') {
        return Err(LocusError::msg(
            "trust key id must not contain ':', ',', or ';'",
        ));
    }
    let key = RegistryTrustKey::ed25519_public(id, public_key_b64.trim());
    key.ed25519_verifying_key()?;
    add_trust_key(locus_home, key)
}

/// Add (or replace) a trust key in the file store.
pub fn add_trust_key(locus_home: &Path, key: RegistryTrustKey) -> Result<TrustKeyAddResult> {
    let path = adapter_trust_keys_path(locus_home);
    let mut keys = load_trust_keys_file(&path)?;
    let replaced =
        if let Some(existing) = keys.iter_mut().find(|e| e.id.eq_ignore_ascii_case(&key.id)) {
            *existing = key.clone();
            true
        } else {
            keys.push(key.clone());
            false
        };
    save_trust_keys_file(&path, &keys)?;
    Ok(TrustKeyAddResult {
        path,
        key,
        replaced,
    })
}

/// List keys from the file store only (missing file → empty).
pub fn list_trust_keys_file(locus_home: &Path) -> Result<Vec<RegistryTrustKey>> {
    load_trust_keys_file(&adapter_trust_keys_path(locus_home))
}

/// List merged keys with origin metadata (file then env overlay).
pub fn list_trust_keys_with_origin(locus_home: &Path) -> Result<Vec<TrustKeyListing>> {
    let path = adapter_trust_keys_path(locus_home);
    let file_keys = load_trust_keys_file(&path)?;
    let mut listings: Vec<TrustKeyListing> = file_keys
        .iter()
        .map(|k| TrustKeyListing {
            id: k.id.clone(),
            scheme: k.scheme().into(),
            material: trust_key_material_display(k),
            origin: TrustKeyOrigin::File,
        })
        .collect();

    if let Ok(raw) = std::env::var(LOCUS_ADAPTER_TRUST_KEYS_ENV) {
        if !raw.trim().is_empty() {
            let env_keys = parse_trust_keys_env(&raw)?;
            for k in env_keys {
                if let Some(existing) = listings
                    .iter_mut()
                    .find(|e| e.id.eq_ignore_ascii_case(&k.id))
                {
                    *existing = TrustKeyListing {
                        id: k.id.clone(),
                        scheme: k.scheme().into(),
                        material: trust_key_material_display(&k),
                        origin: TrustKeyOrigin::Env,
                    };
                } else {
                    listings.push(TrustKeyListing {
                        id: k.id.clone(),
                        scheme: k.scheme().into(),
                        material: trust_key_material_display(&k),
                        origin: TrustKeyOrigin::Env,
                    });
                }
            }
        }
    }
    Ok(listings)
}

fn trust_key_material_display(k: &RegistryTrustKey) -> String {
    match &k.key {
        RegistryKeyMaterial::Ed25519Public { public_key_b64 } => public_key_b64.clone(),
        // Never print HMAC secrets in CLI/listings.
        RegistryKeyMaterial::HmacSha256 { .. } => "<redacted hmac secret>".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter_registry::{
        ed25519_public_key_b64, sign_entry_ed25519, verify_entry_with_keys, AdapterManifestEntry,
        EntryVerifyStatus,
    };
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    const MOCK_SECRET_HEX: &str =
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn gen_ed25519() -> (SigningKey, String) {
        let signing = SigningKey::generate(&mut OsRng);
        let pub_b64 = ed25519_public_key_b64(&signing.verifying_key());
        (signing, pub_b64)
    }

    fn sample_entry(id: &str) -> AdapterManifestEntry {
        AdapterManifestEntry {
            id: id.into(),
            name: id.to_ascii_uppercase(),
            status: "built-in".into(),
            synthetic: true,
            capabilities: vec!["scope".into()],
            frozen_selectors: vec![],
            tools: vec![format!("{id}.scope")],
            destructive_tools: vec![],
            description: format!("{id} test"),
            signature: None,
            signed_by: None,
        }
    }

    #[test]
    fn trust_file_roundtrip_ed25519() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let (signing, pub_b64) = gen_ed25519();

        let add = add_ed25519_trust_key(home, "root", &pub_b64).expect("add");
        assert!(!add.replaced);
        assert_eq!(add.key.id, "root");
        assert!(add.path.exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&add.path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "trust file must be 0600, got {mode:o}");
            let dir_mode = fs::metadata(adapter_trust_dir(home))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(dir_mode, 0o700, "trust dir must be 0700, got {dir_mode:o}");
        }

        let loaded = list_trust_keys_file(home).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "root");
        assert_eq!(loaded[0].scheme(), SIG_SCHEME_ED25519);

        let mut entry = sample_entry("github");
        entry.signature = Some(sign_entry_ed25519(&entry, &signing));
        entry.signed_by = Some("root".into());
        let report = verify_entry_with_keys(&entry, &loaded);
        assert_eq!(report.status, EntryVerifyStatus::Valid, "{report:?}");
    }

    #[test]
    fn trust_file_replace_same_id() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let (_, p1) = gen_ed25519();
        let (_, p2) = gen_ed25519();

        add_ed25519_trust_key(home, "root", &p1).unwrap();
        let add2 = add_ed25519_trust_key(home, "root", &p2).unwrap();
        assert!(add2.replaced);
        let loaded = list_trust_keys_file(home).unwrap();
        assert_eq!(loaded.len(), 1);
        match &loaded[0].key {
            RegistryKeyMaterial::Ed25519Public { public_key_b64 } => {
                assert_eq!(public_key_b64, &p2);
            }
            _ => panic!("expected ed25519"),
        }
    }

    #[test]
    fn merge_file_and_env_env_wins_same_id() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let (_, p_file) = gen_ed25519();
        let (_, p_env) = gen_ed25519();

        add_ed25519_trust_key(home, "root", &p_file).unwrap();

        let mut keys = load_trust_keys_file(&adapter_trust_keys_path(home)).unwrap();
        let env_key = RegistryTrustKey::ed25519_public("root", &p_env);
        merge_trust_keys(&mut keys, vec![env_key]);
        assert_eq!(keys.len(), 1);
        match &keys[0].key {
            RegistryKeyMaterial::Ed25519Public { public_key_b64 } => {
                assert_eq!(public_key_b64, &p_env);
            }
            _ => panic!("expected ed25519"),
        }

        let extra = RegistryTrustKey::hmac_sha256("mock", MOCK_SECRET_HEX);
        merge_trust_keys(&mut keys, vec![extra]);
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[1].id, "mock");
    }

    #[test]
    fn load_merged_includes_file_keys() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let (signing, pub_b64) = gen_ed25519();
        add_ed25519_trust_key(home, "root", &pub_b64).unwrap();

        let merged = load_merged_trust_keys(home);
        assert!(
            merged.iter().any(|k| k.id.eq_ignore_ascii_case("root")),
            "merged must include file key: {merged:?}"
        );

        let mut entry = sample_entry("vercel");
        entry.signature = Some(sign_entry_ed25519(&entry, &signing));
        entry.signed_by = Some("root".into());
        let report = verify_entry_with_keys(&entry, &merged);
        let root = merged
            .iter()
            .find(|k| k.id.eq_ignore_ascii_case("root"))
            .unwrap();
        match &root.key {
            RegistryKeyMaterial::Ed25519Public { public_key_b64 } if public_key_b64 == &pub_b64 => {
                assert_eq!(report.status, EntryVerifyStatus::Valid, "{report:?}");
            }
            _ => {
                // Ambient LOCUS_ADAPTER_TRUST_KEYS overrode root; file store still worked.
            }
        }
    }

    #[test]
    fn parse_trust_keys_file_rejects_bad_version() {
        let err = parse_trust_keys_file("version = 0\nkeys = []\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("version"), "{err}");
    }

    #[test]
    fn missing_trust_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let keys = load_trust_keys_file(&adapter_trust_keys_path(dir.path())).unwrap();
        assert!(keys.is_empty());
    }

    #[test]
    fn trust_file_toml_shape() {
        let (_, pub_b64) = gen_ed25519();
        let toml_src = format!(
            r#"
version = 1
[[keys]]
id = "root"
scheme = "ed25519"
public_key_b64 = "{pub_b64}"
"#
        );
        let doc = parse_trust_keys_file(&toml_src).unwrap();
        let keys = trust_file_to_keys(&doc).unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].scheme(), SIG_SCHEME_ED25519);
    }

    #[test]
    fn hmac_key_material_redacted_in_listing() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let key = RegistryTrustKey::hmac_sha256("mock", MOCK_SECRET_HEX);
        add_trust_key(home, key).unwrap();
        let listings = list_trust_keys_with_origin(home).unwrap();
        assert_eq!(listings.len(), 1);
        assert!(listings[0].material.contains("redacted"));
        assert!(!listings[0].material.contains(MOCK_SECRET_HEX));
    }

    #[test]
    fn reject_empty_id() {
        let dir = tempfile::tempdir().unwrap();
        let (_, pub_b64) = gen_ed25519();
        let err = add_ed25519_trust_key(dir.path(), "  ", &pub_b64)
            .unwrap_err()
            .to_string();
        assert!(err.contains("empty"), "{err}");
    }
}

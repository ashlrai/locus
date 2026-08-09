//! Encrypted binding-graph export/import.
//!
//! Team members share engagement *surfaces* (bindings + workspace templates +
//! policy) without sharing Phantom vault secrets. Only CredentialRefs are
//! serialized — never resolved secret values.
//!
//! # File format
//!
//! ```text
//! LOCUSGRAPH1\n
//! <base64( salt[16] || nonce[12] || ciphertext+tag )>
//! ```
//!
//! Passphrase → Argon2id → 32-byte key → ChaCha20-Poly1305 over JSON envelope.

use crate::binding::{validate_name_component, Binding, BindingBody};
use crate::error::{LocusError, Result};
use crate::workspace::WorkspaceConfig;
use argon2::{Algorithm, Argon2, Params, Version};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use chrono::Utc;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::io::{self, IsTerminal, Write};
use zeroize::Zeroizing;

/// Magic prefix for encrypted graph files (version 1).
pub const MAGIC: &[u8] = b"LOCUSGRAPH1\n";

/// Cleartext envelope schema version.
pub const GRAPH_VERSION: u32 = 1;

/// Env var for non-interactive passphrase supply.
pub const ENV_PASSPHRASE: &str = "LOCUS_GRAPH_PASSPHRASE";

const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

/// Workspace template carried inside a graph export.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceTemplate {
    pub name: String,
    pub config: WorkspaceConfig,
}

/// Optional metadata about the export source (never secrets).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locus_version: Option<String>,
}

/// Cleartext binding-graph envelope (JSON after decrypt).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphEnvelope {
    pub version: u32,
    pub exported_at: String,
    /// Bindings with CredentialRefs only — no secret values.
    pub bindings: Vec<BindingBody>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspaces: Vec<WorkspaceTemplate>,
    #[serde(default)]
    pub meta: GraphMeta,
}

/// Summary returned after a successful export.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphExportResult {
    pub path: String,
    pub binding_aliases: Vec<String>,
    pub workspace_names: Vec<String>,
    pub exported_at: String,
}

/// Summary returned after a successful import.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphImportResult {
    pub bindings_imported: Vec<String>,
    pub bindings_skipped: Vec<String>,
    pub workspaces_imported: Vec<String>,
    pub workspaces_skipped: Vec<String>,
    pub source_host: Option<String>,
    pub exported_at: Option<String>,
}

/// Local graph surface for `locus graph list` (no secrets).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphListEntry {
    pub kind: String, // "binding" | "workspace"
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub credential_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_binding: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_bindings: Vec<String>,
}

impl GraphEnvelope {
    /// Build an envelope from validated bindings + workspace templates.
    pub fn build(
        bindings: impl IntoIterator<Item = Binding>,
        workspaces: impl IntoIterator<Item = WorkspaceTemplate>,
        meta: GraphMeta,
    ) -> Result<Self> {
        let mut bodies = Vec::new();
        for b in bindings {
            b.validate()?;
            scrub_export_credential_refs(&b)?;
            bodies.push(binding_to_body(&b));
        }
        let mut wss: Vec<WorkspaceTemplate> = workspaces.into_iter().collect();
        for ws in &wss {
            validate_name_component("workspace name", &ws.name)?;
        }
        wss.sort_by(|a, b| a.name.cmp(&b.name));
        bodies.sort_by(|a, b| a.alias.cmp(&b.alias));

        Ok(Self {
            version: GRAPH_VERSION,
            exported_at: Utc::now().to_rfc3339(),
            bindings: bodies,
            workspaces: wss,
            meta,
        })
    }

    pub fn to_json_bytes(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec_pretty(self)?)
    }

    pub fn from_json_bytes(data: &[u8]) -> Result<Self> {
        let env: Self = serde_json::from_slice(data)?;
        if env.version != GRAPH_VERSION {
            return Err(LocusError::msg(format!(
                "unsupported graph version {} (want {GRAPH_VERSION})",
                env.version
            )));
        }
        for body in &env.bindings {
            let b = Binding::from_body(body.clone());
            b.validate()?;
            scrub_export_credential_refs(&b)?;
        }
        for ws in &env.workspaces {
            validate_name_component("workspace name", &ws.name)?;
        }
        Ok(env)
    }
}

fn binding_to_body(b: &Binding) -> BindingBody {
    BindingBody {
        id: b.id.clone(),
        alias: b.alias.clone(),
        tenant: b.tenant.clone(),
        principal: b.principal.clone(),
        description: b.description.clone(),
        policy: b.policy.clone(),
        providers: b.providers.clone(),
    }
}

/// Ensure every credential_ref is a ref pointer, never a raw-looking secret blob.
///
/// Allowed: `phm:…`, `env:…`, `keychain:…`, `test:…` (tests), or bare Phantom names.
fn scrub_export_credential_refs(b: &Binding) -> Result<()> {
    for p in &b.providers {
        let r = p.credential_ref.trim();
        if r.is_empty() {
            return Err(LocusError::msg(format!(
                "binding '{}': empty credential_ref",
                b.alias
            )));
        }
        // Reject obvious raw tokens (long base64-ish without a scheme prefix)
        if !r.contains(':') {
            // Bare name → treated as phm:NAME (OK)
            if r.len() > 128 {
                return Err(LocusError::msg(format!(
                    "binding '{}': credential_ref looks like raw material (too long bare name)",
                    b.alias
                )));
            }
            continue;
        }
        let scheme = r.split_once(':').map(|(s, _)| s).unwrap_or("");
        match scheme {
            "phm" | "env" | "keychain" | "test" => {
                // Value after scheme must not be enormous (raw secrets often are)
                let value = &r[scheme.len() + 1..];
                if scheme != "test" && value.len() > 256 {
                    return Err(LocusError::msg(format!(
                        "binding '{}': credential_ref value for {scheme}: is suspiciously long",
                        b.alias
                    )));
                }
            }
            _ => {
                return Err(LocusError::msg(format!(
                    "binding '{}': unsupported credential_ref scheme '{scheme}:' (use phm: / env:)",
                    b.alias
                )));
            }
        }
    }
    Ok(())
}

// ── Crypto ────────────────────────────────────────────────────────────────

/// Derive a 32-byte ChaCha20 key from passphrase + salt via Argon2id.
fn derive_key(passphrase: &str, salt: &[u8]) -> Result<Zeroizing<[u8; KEY_LEN]>> {
    // Interactive-ish Argon2id. Tests use thinner params so parallel suites stay fast.
    #[cfg(test)]
    let (m_kib, t_cost) = (8 * 1024, 1u32); // 8 MiB, t=1
    #[cfg(not(test))]
    let (m_kib, t_cost) = (19 * 1024, 2u32); // ~OWASP interactive: 19 MiB, t=2
    let params = Params::new(m_kib, t_cost, 1, Some(KEY_LEN))
        .map_err(|e| LocusError::msg(format!("argon2 params: {e}")))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    argon
        .hash_password_into(passphrase.as_bytes(), salt, &mut key[..])
        .map_err(|e| LocusError::msg(format!("argon2 derive: {e}")))?;
    Ok(key)
}

/// Encrypt cleartext JSON into a LOCUSGRAPH1 file blob.
pub fn encrypt_graph(plaintext: &[u8], passphrase: &str) -> Result<Vec<u8>> {
    if passphrase.is_empty() {
        return Err(LocusError::msg("graph passphrase must not be empty"));
    }
    let mut salt = [0u8; SALT_LEN];
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut salt);
    rand::thread_rng().fill_bytes(&mut nonce_bytes);

    let key = derive_key(passphrase, &salt)?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key[..]));
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|_| LocusError::msg("graph encryption failed"))?;

    let mut packed = Vec::with_capacity(SALT_LEN + NONCE_LEN + ciphertext.len());
    packed.extend_from_slice(&salt);
    packed.extend_from_slice(&nonce_bytes);
    packed.extend_from_slice(&ciphertext);

    let mut out = Vec::with_capacity(MAGIC.len() + packed.len() * 4 / 3 + 4);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(B64.encode(&packed).as_bytes());
    out.push(b'\n');
    Ok(out)
}

/// Decrypt a LOCUSGRAPH1 file blob to cleartext JSON bytes.
pub fn decrypt_graph(file_bytes: &[u8], passphrase: &str) -> Result<Vec<u8>> {
    if passphrase.is_empty() {
        return Err(LocusError::msg("graph passphrase must not be empty"));
    }
    if !file_bytes.starts_with(MAGIC) {
        return Err(LocusError::msg(
            "not a Locus graph file (missing LOCUSGRAPH1 magic)",
        ));
    }
    let b64 = std::str::from_utf8(&file_bytes[MAGIC.len()..])
        .map_err(|_| LocusError::msg("graph file is not valid UTF-8 after magic"))?
        .trim();
    if b64.is_empty() {
        return Err(LocusError::msg("graph file has empty payload"));
    }
    let packed = B64
        .decode(b64)
        .map_err(|e| LocusError::msg(format!("graph base64 decode: {e}")))?;
    if packed.len() < SALT_LEN + NONCE_LEN + 16 {
        return Err(LocusError::msg("graph ciphertext too short"));
    }
    let salt = &packed[..SALT_LEN];
    let nonce_bytes = &packed[SALT_LEN..SALT_LEN + NONCE_LEN];
    let ciphertext = &packed[SALT_LEN + NONCE_LEN..];

    let key = derive_key(passphrase, salt)?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key[..]));
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| LocusError::msg("graph decrypt failed (wrong passphrase or corrupt file)"))
}

/// Resolve passphrase from `LOCUS_GRAPH_PASSPHRASE` or interactive TTY prompt.
///
/// Non-TTY without env → error (fail closed for scripts).
pub fn resolve_passphrase() -> Result<Zeroizing<String>> {
    if let Ok(p) = std::env::var(ENV_PASSPHRASE) {
        if !p.is_empty() {
            return Ok(Zeroizing::new(p));
        }
    }
    if io::stdin().is_terminal() {
        eprint!("Graph passphrase: ");
        let _ = io::stderr().flush();
        let mut line = String::new();
        io::stdin()
            .read_line(&mut line)
            .map_err(|e| LocusError::msg(format!("read passphrase: {e}")))?;
        let p = line.trim_end_matches(['\r', '\n']).to_string();
        if p.is_empty() {
            return Err(LocusError::msg("graph passphrase must not be empty"));
        }
        return Ok(Zeroizing::new(p));
    }
    Err(LocusError::msg(format!(
        "{ENV_PASSPHRASE} is required for non-interactive graph export/import"
    )))
}

/// Default export filename with timestamp.
pub fn default_export_filename() -> String {
    let ts = Utc::now().format("%Y%m%dT%H%M%SZ");
    format!("locus-graph-{ts}.locusgraph")
}

/// Hostname for meta (best-effort; never fails).
pub fn source_host() -> Option<String> {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| hostname_from_uname().filter(|s| !s.is_empty()))
}

fn hostname_from_uname() -> Option<String> {
    let out = std::process::Command::new("hostname").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::{Policy, ProviderBinding, Scope};

    fn sample_binding(alias: &str, tenant: &str) -> Binding {
        Binding::from_body(BindingBody {
            id: format!("bnd_{alias}"),
            alias: alias.into(),
            tenant: tenant.into(),
            principal: None,
            description: Some(format!("{tenant} engagement")),
            policy: Policy {
                require_approval: vec!["*.delete*".into()],
                ..Policy::default()
            },
            providers: vec![ProviderBinding {
                provider: "github".into(),
                account: tenant.into(),
                credential_ref: format!("phm:GH_TOKEN_{}", alias.to_uppercase().replace('-', "_")),
                scope: Scope {
                    orgs: vec![tenant.into()],
                    ..Scope::default()
                },
                upstream: None,
            }],
        })
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let plain = br#"{"version":1,"hello":"world"}"#;
        let pass = "test-passphrase-xyz";
        let enc = encrypt_graph(plain, pass).unwrap();
        assert!(enc.starts_with(MAGIC));
        let dec = decrypt_graph(&enc, pass).unwrap();
        assert_eq!(dec, plain);
    }

    #[test]
    fn wrong_passphrase_fails() {
        let enc = encrypt_graph(b"secret-json", "correct").unwrap();
        let err = decrypt_graph(&enc, "wrong").unwrap_err().to_string();
        assert!(
            err.contains("decrypt") || err.contains("passphrase"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn missing_magic_fails() {
        let err = decrypt_graph(b"not-a-graph", "x").unwrap_err().to_string();
        assert!(
            err.contains("magic") || err.contains("LOCUSGRAPH1"),
            "{err}"
        );
    }

    #[test]
    fn envelope_roundtrip_json() {
        let b = sample_binding("acme", "acme-corp");
        let ws = WorkspaceTemplate {
            name: "acme".into(),
            config: WorkspaceConfig {
                version: 1,
                default_binding: Some("acme".into()),
                allowed_bindings: vec!["acme".into(), "acme-ro".into()],
                require_pin: true,
            },
        };
        let env = GraphEnvelope::build(
            vec![b],
            vec![ws],
            GraphMeta {
                source_host: Some("test-host".into()),
                locus_version: Some(crate::VERSION.into()),
            },
        )
        .unwrap();
        let bytes = env.to_json_bytes().unwrap();
        // Cleartext must not contain raw secret material — only refs
        let s = String::from_utf8(bytes.clone()).unwrap();
        assert!(s.contains("phm:GH_TOKEN_ACME"));
        assert!(!s.contains("ghp_"));
        assert!(!s.contains("sk_"));
        let env2 = GraphEnvelope::from_json_bytes(&bytes).unwrap();
        assert_eq!(env.bindings, env2.bindings);
        assert_eq!(env.workspaces, env2.workspaces);
    }

    #[test]
    fn full_file_roundtrip() {
        let b = sample_binding("client-a", "client-a-corp");
        let env = GraphEnvelope::build(vec![b], vec![], GraphMeta::default()).unwrap();
        let plain = env.to_json_bytes().unwrap();
        let file = encrypt_graph(&plain, "test").unwrap();
        let plain2 = decrypt_graph(&file, "test").unwrap();
        let env2 = GraphEnvelope::from_json_bytes(&plain2).unwrap();
        assert_eq!(env.bindings[0].alias, env2.bindings[0].alias);
        assert_eq!(
            env.bindings[0].providers[0].credential_ref,
            env2.bindings[0].providers[0].credential_ref
        );
    }

    #[test]
    fn rejects_unsupported_credential_scheme() {
        let mut b = sample_binding("x", "t");
        b.providers[0].credential_ref = "raw:supersecrettokenvalue".into();
        let err = GraphEnvelope::build(vec![b], vec![], GraphMeta::default())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("unsupported") || err.contains("scheme"),
            "{err}"
        );
    }

    #[test]
    fn empty_passphrase_rejected() {
        assert!(encrypt_graph(b"x", "").is_err());
        assert!(decrypt_graph(MAGIC, "").is_err());
    }
}

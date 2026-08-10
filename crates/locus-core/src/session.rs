//! Live session pin — exclusive or namespaced bindings for the active agent/CLI context.

pub use crate::authority_anchor::SessionAuthorityAnchor;
use crate::authority_anchor::AUTHORITY_ANCHOR_VERSION;
use crate::binding::Binding;
use crate::error::{LocusError, Result};
use crate::seal::{seal_material, SealKey};
use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Component, Path};

/// How the pin was established.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PinSource {
    Explicit,
    Dir {
        path: String,
    },
    Default,
    /// One-shot `locus run` temporary pin (not necessarily active.json).
    Run,
    /// CI / ephemeral mint (`sessions/ci-*.json`, not active.json).
    Ci,
    /// Matched via opt-in `[[autopin.remotes]]` git remote rule.
    Autopin {
        match_pattern: String,
    },
}

/// Mode of isolation. Default is exclusive (one binding).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    #[default]
    Exclusive,
    /// Opt-in: multiple bindings with tool names prefixed `alias__`.
    Namespaced,
}

/// Authority carried by a sealed session.
///
/// `LocalControl` is minted only by direct local control operations. CI and
/// MCP-auto sessions are `Delegated`; a human-created run session is also
/// confined while selected through `LOCUS_SESSION_ID`. No caller-provided
/// environment or client label can upgrade authority. Missing authority on
/// legacy files is deliberately untrusted.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionAuthority {
    LocalControl,
    Delegated,
    #[default]
    LegacyUntrusted,
}

pub const CURRENT_SEAL_VERSION: u32 = 3;
pub const SESSION_BACKING_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionBackingType {
    #[default]
    Active,
    Run,
    Ci,
}

impl SessionBackingType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Run => "run",
            Self::Ci => "ci",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionBackingIdentity {
    pub version: u32,
    pub backing_type: SessionBackingType,
    pub canonical_path: String,
    pub file_name: String,
}

const fn legacy_seal_version() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Session {
    /// Version of the canonical session material covered by `seal`.
    #[serde(default = "legacy_seal_version")]
    pub seal_version: u32,
    pub session_id: String,
    pub binding_id: String,
    pub binding_alias: String,
    pub tenant: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    pub source: PinSource,
    /// Sealed authority class. Caller-controlled labels never alter this.
    #[serde(default)]
    pub authority: SessionAuthority,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    pub pinned_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub mode: SessionMode,
    /// HMAC seal — verified on every privileged op.
    pub seal: String,
    /// Absolute path to isolated worker home for this session.
    pub worker_home: String,
    /// When true, tools/call must fail closed until human re-pins.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub frozen: bool,
    /// Machine-readable reason the session was frozen (e.g. `providers_drift`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frozen_reason: Option<String>,
    /// Fingerprint of primary binding material at pin time (id/tenant/providers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_fp: Option<String>,
    /// Additional binding aliases when [`SessionMode::Namespaced`].
    /// Primary is always `binding_alias`; this list excludes the primary.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub namespaces: Vec<String>,
    /// Fingerprints for namespaced aliases, parallel to `namespaces`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub namespace_fps: Vec<String>,
    /// Exact file authorized to carry this record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backing: Option<SessionBackingIdentity>,
    /// Monotonic generation held by the live authority broker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority_anchor: Option<SessionAuthorityAnchor>,
}

impl Session {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        binding_id: &str,
        binding_alias: &str,
        tenant: &str,
        principal: Option<String>,
        source: PinSource,
        client: Option<String>,
        ttl: Duration,
        worker_home: String,
        key: &SealKey,
    ) -> Self {
        Self::new_with_authority(
            binding_id,
            binding_alias,
            tenant,
            principal,
            source,
            SessionAuthority::LocalControl,
            client,
            ttl,
            worker_home,
            key,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_authority(
        binding_id: &str,
        binding_alias: &str,
        tenant: &str,
        principal: Option<String>,
        source: PinSource,
        authority: SessionAuthority,
        client: Option<String>,
        ttl: Duration,
        worker_home: String,
        key: &SealKey,
    ) -> Self {
        let session_id = mint_session_id();
        let pinned_at = Utc::now();
        let expires_at = pinned_at + ttl;
        let mut session = Self {
            seal_version: CURRENT_SEAL_VERSION,
            session_id,
            binding_id: binding_id.into(),
            binding_alias: binding_alias.into(),
            tenant: tenant.into(),
            principal,
            source,
            authority,
            client,
            pinned_at,
            expires_at,
            mode: SessionMode::Exclusive,
            seal: String::new(),
            worker_home,
            frozen: false,
            frozen_reason: None,
            binding_fp: None,
            namespaces: Vec::new(),
            namespace_fps: Vec::new(),
            backing: None,
            authority_anchor: None,
        };
        session.reseal(key);
        session
    }

    pub fn with_mode(mut self, mode: SessionMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn with_binding_fp(mut self, fp: impl Into<String>) -> Self {
        self.binding_fp = Some(fp.into());
        self
    }

    pub fn with_namespaces(mut self, namespaces: Vec<String>, fps: Vec<String>) -> Self {
        self.namespaces = namespaces;
        self.namespace_fps = fps;
        self
    }

    /// All binding aliases in this session (primary first).
    pub fn all_aliases(&self) -> Vec<String> {
        let mut out = vec![self.binding_alias.clone()];
        for a in &self.namespaces {
            if !out.iter().any(|x| x == a) {
                out.push(a.clone());
            }
        }
        out
    }

    /// True when this session is multi-binding namespaced.
    pub fn is_namespaced(&self) -> bool {
        self.mode == SessionMode::Namespaced && !self.namespaces.is_empty()
    }

    pub fn material(&self) -> String {
        if self.seal_version < CURRENT_SEAL_VERSION {
            return seal_material(
                &self.session_id,
                &self.binding_id,
                &self.pinned_at.to_rfc3339(),
                &self.expires_at.to_rfc3339(),
            );
        }

        // serde_json's map representation has deterministic key ordering, and
        // every authorization-relevant mutable field is covered by the HMAC.
        serde_json::to_string(&serde_json::json!({
            "authority": self.authority,
            "authority_anchor": self.authority_anchor,
            "backing": self.backing,
            "binding_alias": self.binding_alias,
            "binding_fp": self.binding_fp,
            "binding_id": self.binding_id,
            "client": self.client,
            "expires_at": self.expires_at.to_rfc3339(),
            "frozen": self.frozen,
            "frozen_reason": self.frozen_reason,
            "mode": self.mode,
            "namespace_fps": self.namespace_fps,
            "namespaces": self.namespaces,
            "pinned_at": self.pinned_at.to_rfc3339(),
            "principal": self.principal,
            "seal_version": self.seal_version,
            "session_id": self.session_id,
            "source": self.source,
            "tenant": self.tenant,
            "worker_home": self.worker_home,
        }))
        .expect("session seal tuple is serializable")
    }

    /// Stable broker subject for every authorization-relevant session field.
    /// The live anchor itself is excluded because its generation is assigned
    /// by the broker after this subject has been constructed.
    pub(crate) fn authority_subject_digest(&self) -> String {
        let subject = serde_json::to_vec(&serde_json::json!({
            "authority": self.authority,
            "backing": self.backing,
            "binding_alias": self.binding_alias,
            "binding_fp": self.binding_fp,
            "binding_id": self.binding_id,
            "client": self.client,
            "expires_at": self.expires_at.to_rfc3339(),
            "mode": self.mode,
            "namespace_fps": self.namespace_fps,
            "namespaces": self.namespaces,
            "pinned_at": self.pinned_at.to_rfc3339(),
            "principal": self.principal,
            "seal_version": self.seal_version,
            "session_id": self.session_id,
            "source": self.source,
            "tenant": self.tenant,
            "worker_home": self.worker_home,
        }))
        .expect("session authority subject is serializable");
        hex::encode(Sha256::digest(subject))
    }

    /// Refresh the HMAC after store-owned session state changes.
    pub fn reseal(&mut self, key: &SealKey) {
        self.seal_version = CURRENT_SEAL_VERSION;
        self.seal = key.seal(&self.material());
    }

    pub fn verify(&self, key: &SealKey) -> Result<()> {
        if self.seal_version < CURRENT_SEAL_VERSION {
            return Err(LocusError::LegacySessionSeal);
        }
        if self.seal_version != CURRENT_SEAL_VERSION
            || self.authority == SessionAuthority::LegacyUntrusted
        {
            return Err(LocusError::InvalidSeal);
        }
        if !key.verify(&self.material(), &self.seal) {
            return Err(LocusError::InvalidSeal);
        }
        if Utc::now() > self.expires_at {
            return Err(LocusError::SessionExpired(self.expires_at.to_rfc3339()));
        }
        if self.frozen {
            return Err(LocusError::SessionFrozen(
                self.frozen_reason
                    .clone()
                    .unwrap_or_else(|| "re-pin".into()),
            ));
        }
        Ok(())
    }

    /// Seal + expiry only — does not fail on frozen (for drift checks / doctor).
    pub fn verify_seal(&self, key: &SealKey) -> Result<()> {
        if self.seal_version < CURRENT_SEAL_VERSION {
            return Err(LocusError::LegacySessionSeal);
        }
        if self.seal_version != CURRENT_SEAL_VERSION
            || self.authority == SessionAuthority::LegacyUntrusted
        {
            return Err(LocusError::InvalidSeal);
        }
        if !key.verify(&self.material(), &self.seal) {
            return Err(LocusError::InvalidSeal);
        }
        if Utc::now() > self.expires_at {
            return Err(LocusError::SessionExpired(self.expires_at.to_rfc3339()));
        }
        Ok(())
    }

    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at
    }

    pub fn is_frozen(&self) -> bool {
        self.frozen
    }

    pub fn is_delegated(&self) -> bool {
        self.authority != SessionAuthority::LocalControl
    }

    pub fn set_backing(
        &mut self,
        backing_type: SessionBackingType,
        canonical_path: &Path,
    ) -> Result<()> {
        if !canonical_path.is_absolute()
            || canonical_path
                .components()
                .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(LocusError::InvalidSeal);
        }
        let file_name = canonical_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(LocusError::InvalidSeal)?
            .to_string();
        self.backing = Some(SessionBackingIdentity {
            version: SESSION_BACKING_VERSION,
            backing_type,
            canonical_path: canonical_path.display().to_string(),
            file_name,
        });
        Ok(())
    }

    pub fn verify_backing(
        &self,
        backing_type: SessionBackingType,
        canonical_path: &Path,
    ) -> Result<()> {
        let backing = self.backing.as_ref().ok_or(LocusError::InvalidSeal)?;
        if backing.version != SESSION_BACKING_VERSION
            || backing.backing_type != backing_type
            || Path::new(&backing.canonical_path) != canonical_path
            || canonical_path.file_name().and_then(|name| name.to_str())
                != Some(backing.file_name.as_str())
        {
            return Err(LocusError::InvalidSeal);
        }
        Ok(())
    }

    pub fn verify_authority_shape(&self) -> Result<()> {
        let anchor = self
            .authority_anchor
            .as_ref()
            .ok_or(LocusError::LegacySessionSeal)?;
        if anchor.version != AUTHORITY_ANCHOR_VERSION
            || anchor.epoch.is_empty()
            || anchor.generation == 0
        {
            return Err(LocusError::InvalidSeal);
        }
        Ok(())
    }

    /// Mark session frozen (persist separately via store).
    pub fn freeze(&mut self, reason: impl Into<String>) {
        self.frozen = true;
        self.frozen_reason = Some(reason.into());
    }
}

/// Stable fingerprint of binding identity material (never secrets values).
///
/// Covers id, tenant, principal, and each provider's account / credential_ref /
/// key scope selectors so drift is detected when the binding file mutates.
pub fn binding_fingerprint(binding: &Binding) -> String {
    let mut parts: Vec<String> = vec![
        binding.id.clone(),
        binding.alias.clone(),
        binding.tenant.clone(),
        binding.principal.clone().unwrap_or_default(),
    ];
    for p in &binding.providers {
        parts.push(format!(
            "{}|{}|{}|{}|{}|{}|{}|{}",
            p.provider,
            p.account,
            p.credential_ref,
            p.scope.project_ref.as_deref().unwrap_or(""),
            p.scope.team_id.as_deref().unwrap_or(""),
            p.scope.account_id.as_deref().unwrap_or(""),
            p.scope.orgs.join(","),
            p.scope.repos.join(","),
        ));
    }
    let material = parts.join("\n");
    let digest = Sha256::digest(material.as_bytes());
    hex::encode(digest)
}

fn mint_session_id() -> String {
    let mut buf = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut buf);
    format!("ses_{}", hex::encode(buf))
}

/// Parse human TTL like "8h", "90m", "1d", "3600s".
pub fn parse_ttl(s: &str) -> Result<Duration> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(Duration::hours(8));
    }
    let (num, unit) = s.split_at(s.len().saturating_sub(1));
    let n: i64 = num
        .parse()
        .map_err(|_| LocusError::msg(format!("invalid ttl: {s}")))?;
    match unit {
        "s" => Ok(Duration::seconds(n)),
        "m" => Ok(Duration::minutes(n)),
        "h" => Ok(Duration::hours(n)),
        "d" => Ok(Duration::days(n)),
        _ => {
            // bare number = hours
            let n: i64 = s
                .parse()
                .map_err(|_| LocusError::msg(format!("invalid ttl: {s}")))?;
            Ok(Duration::hours(n))
        }
    }
}

/// Prefix a tool name with a binding alias for namespaced catalogs: `alias__tool`.
pub fn namespace_tool(alias: &str, tool: &str) -> String {
    format!("{alias}__{tool}")
}

/// Split `alias__tool` into (alias, tool). Returns None if no `__` separator.
pub fn split_namespaced_tool(name: &str) -> Option<(&str, &str)> {
    let (alias, rest) = name.split_once("__")?;
    if alias.is_empty() || rest.is_empty() {
        return None;
    }
    // Control tools are never namespaced this way
    if alias.starts_with("locus") {
        return None;
    }
    Some((alias, rest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::{BindingBody, Policy, ProviderBinding, Scope};
    use crate::seal::SealKey;

    fn sample_binding() -> Binding {
        Binding::from_body(BindingBody {
            id: "bnd_a".into(),
            alias: "acme".into(),
            tenant: "acme-corp".into(),
            principal: None,
            description: None,
            policy: Policy::default(),
            providers: vec![ProviderBinding {
                provider: "supabase".into(),
                account: "acme".into(),
                credential_ref: "phm:X".into(),
                scope: Scope {
                    project_ref: Some("proj".into()),
                    ..Scope::default()
                },
                upstream: None,
            }],
        })
    }

    #[test]
    fn session_seal_valid() {
        let key = SealKey::generate();
        let s = Session::new(
            "bnd_a",
            "acme",
            "acme-corp",
            None,
            PinSource::Explicit,
            Some("cli".into()),
            Duration::hours(1),
            "/tmp/locus-worker".into(),
            &key,
        );
        s.verify(&key).unwrap();
    }

    #[test]
    fn authority_and_binding_metadata_are_sealed() {
        let key = SealKey::generate();
        let s = Session::new_with_authority(
            "bnd_a",
            "acme",
            "acme-corp",
            None,
            PinSource::Ci,
            SessionAuthority::Delegated,
            Some("ci".into()),
            Duration::hours(1),
            "/tmp/locus-worker".into(),
            &key,
        );
        s.verify(&key).unwrap();

        let mut alias_forged = s.clone();
        alias_forged.binding_alias = "personal".into();
        assert!(matches!(
            alias_forged.verify(&key),
            Err(LocusError::InvalidSeal)
        ));

        let mut authority_forged = s.clone();
        authority_forged.authority = SessionAuthority::LocalControl;
        assert!(matches!(
            authority_forged.verify(&key),
            Err(LocusError::InvalidSeal)
        ));

        let mut worker_home_forged = s;
        worker_home_forged.worker_home = "/tmp/other".into();
        assert!(matches!(
            worker_home_forged.verify(&key),
            Err(LocusError::InvalidSeal)
        ));
    }

    #[test]
    fn parse_ttl_variants() {
        assert_eq!(parse_ttl("8h").unwrap(), Duration::hours(8));
        assert_eq!(parse_ttl("90m").unwrap(), Duration::minutes(90));
        assert_eq!(parse_ttl("30s").unwrap(), Duration::seconds(30));
    }

    #[test]
    fn fingerprint_stable_and_sensitive() {
        let b = sample_binding();
        let fp1 = binding_fingerprint(&b);
        let fp2 = binding_fingerprint(&b);
        assert_eq!(fp1, fp2);
        assert_eq!(fp1.len(), 64);

        let mut b2 = sample_binding();
        b2.tenant = "other".into();
        assert_ne!(binding_fingerprint(&b2), fp1);

        let mut b3 = sample_binding();
        b3.providers[0].scope.project_ref = Some("mutated".into());
        assert_ne!(binding_fingerprint(&b3), fp1);
    }

    #[test]
    fn frozen_session_fails_verify() {
        let key = SealKey::generate();
        let mut s = Session::new(
            "bnd_a",
            "acme",
            "acme-corp",
            None,
            PinSource::Explicit,
            None,
            Duration::hours(1),
            "/tmp/w".into(),
            &key,
        );
        s.verify(&key).unwrap();
        s.freeze("providers_drift");
        s.reseal(&key);
        assert!(matches!(s.verify(&key), Err(LocusError::SessionFrozen(_))));
        // seal-only path still ok
        s.verify_seal(&key).unwrap();
    }

    #[test]
    fn namespace_tool_helpers() {
        assert_eq!(namespace_tool("acme", "github.scope"), "acme__github.scope");
        assert_eq!(
            split_namespaced_tool("acme__github.scope"),
            Some(("acme", "github.scope"))
        );
        assert!(split_namespaced_tool("github.scope").is_none());
        assert!(split_namespaced_tool("locus_whoami").is_none());
        assert!(split_namespaced_tool("__foo").is_none());
    }

    #[test]
    fn all_aliases_primary_first() {
        let key = SealKey::generate();
        let s = Session::new(
            "bnd_a",
            "acme",
            "acme-corp",
            None,
            PinSource::Explicit,
            None,
            Duration::hours(1),
            "/tmp/w".into(),
            &key,
        )
        .with_mode(SessionMode::Namespaced)
        .with_namespaces(vec!["personal".into()], vec!["fp".into()]);
        assert_eq!(s.all_aliases(), vec!["acme", "personal"]);
        assert!(s.is_namespaced());
    }
}

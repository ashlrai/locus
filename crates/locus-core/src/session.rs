//! Live session pin — exclusive or namespaced bindings for the active agent/CLI context.

use crate::binding::Binding;
use crate::error::{LocusError, Result};
use crate::seal::{seal_material, SealKey};
use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Session {
    pub session_id: String,
    pub binding_id: String,
    pub binding_alias: String,
    pub tenant: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    pub source: PinSource,
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
        let session_id = mint_session_id();
        let pinned_at = Utc::now();
        let expires_at = pinned_at + ttl;
        let material = seal_material(
            &session_id,
            binding_id,
            &pinned_at.to_rfc3339(),
            &expires_at.to_rfc3339(),
        );
        let seal = key.seal(&material);
        Self {
            session_id,
            binding_id: binding_id.into(),
            binding_alias: binding_alias.into(),
            tenant: tenant.into(),
            principal,
            source,
            client,
            pinned_at,
            expires_at,
            mode: SessionMode::Exclusive,
            seal,
            worker_home,
            frozen: false,
            frozen_reason: None,
            binding_fp: None,
            namespaces: Vec::new(),
            namespace_fps: Vec::new(),
        }
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
        seal_material(
            &self.session_id,
            &self.binding_id,
            &self.pinned_at.to_rfc3339(),
            &self.expires_at.to_rfc3339(),
        )
    }

    pub fn verify(&self, key: &SealKey) -> Result<()> {
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

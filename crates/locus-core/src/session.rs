//! Live session pin — exclusive binding for the active agent/CLI context.

use crate::error::{LocusError, Result};
use crate::seal::{seal_material, SealKey};
use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};

/// How the pin was established.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PinSource {
    Explicit,
    Dir { path: String },
    Default,
}

/// Mode of isolation. MVP is exclusive only.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    #[default]
    Exclusive,
    /// Future: multiple bindings namespaced in one session.
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
        }
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
        Ok(())
    }

    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seal::SealKey;

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
}

//! Opt-in, signed, value-free log of credential injections.
//!
//! When `[credential_audit] enabled = true` is set in `$LOCUS_HOME/config.toml`,
//! every time Locus places a resolved credential into a child environment it
//! appends one line to `$LOCUS_HOME/audit/credential-injections.jsonl`:
//!
//! - what: provider, the credential **reference** (for example `env:GH_TOKEN`
//!   or `phm:NAME`), and the environment variable **names** filled
//! - where: the target (`child-env` for a launched child or worker, `ci-export`
//!   for the CI export map), binding alias and session id
//! - when: an RFC 3339 timestamp
//!
//! It never records a credential value, a hash of a value, or a length.
//! Each line carries `prev` (the previous line's MAC) and `mac`, an
//! HMAC-SHA256 under a local key (`$LOCUS_HOME/credential-audit.key`, 32
//! random bytes as hex, mode 0600, created on first use). `verify` re-checks
//! every MAC and the chain, so an edited, inserted, removed or reordered line
//! is detected by anyone holding the key. It is not proof against someone who
//! can read that key on this machine.
//!
//! Off by default: with the flag unset nothing is read beyond `config.toml`
//! and nothing is written. When enabled and a record cannot be written, the
//! credential is withheld (fail closed) and reported as a resolution issue.

use crate::credential::CredentialRef;
use crate::error::{LocusError, Result};
use crate::seal::SealKey;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Chain start marker for the first record.
pub const GENESIS: &str = "genesis";

/// One value-free injection record (field order is the signed order).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InjectionRecord {
    pub v: u32,
    pub ts: String,
    pub session_id: String,
    pub binding: String,
    pub provider: String,
    pub target: String,
    /// Reference only (scheme and name), never a value.
    pub credential_ref: String,
    /// Environment variable names filled (sorted).
    pub env_keys: Vec<String>,
    pub prev: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mac: String,
}

/// What a caller wants recorded for one provider.
#[derive(Debug, Clone)]
pub struct Injection<'a> {
    pub session_id: &'a str,
    pub binding: &'a str,
    pub provider: &'a str,
    pub target: &'a str,
    pub credential_ref: &'a str,
    pub env_keys: Vec<String>,
}

pub fn log_path(home: &Path) -> PathBuf {
    home.join("audit").join("credential-injections.jsonl")
}

pub fn key_path(home: &Path) -> PathBuf {
    home.join("credential-audit.key")
}

/// Whether the feature is enabled for `home` (reads `config.toml` only).
pub fn enabled(home: &Path) -> bool {
    crate::config::load_config(home).credential_audit.enabled
}

/// A reference that is safe to log. Anything that does not parse as a known
/// reference form is replaced, so a raw value mistakenly configured as a
/// reference can never land in the log.
pub fn loggable_ref(raw: &str) -> String {
    match CredentialRef::validate(raw) {
        Ok(_) if raw.contains(':') && !raw.chars().any(char::is_whitespace) => raw.to_string(),
        _ => "<unrecognized reference>".to_string(),
    }
}

fn load_or_create_key(home: &Path) -> Result<SealKey> {
    let path = key_path(home);
    match fs::read_to_string(&path) {
        Ok(s) => SealKey::from_hex(&s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let key = SealKey::generate();
            let mut opts = fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            match opts.open(&path) {
                Ok(mut f) => {
                    f.write_all(key.to_hex().as_bytes())?;
                    f.sync_all()?;
                    Ok(key)
                }
                // Another process created it first: use theirs.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    SealKey::from_hex(&fs::read_to_string(&path)?)
                }
                Err(e) => Err(e.into()),
            }
        }
        Err(e) => Err(e.into()),
    }
}

fn material(rec: &InjectionRecord) -> Result<String> {
    let mut unsigned = rec.clone();
    unsigned.mac.clear();
    serde_json::to_string(&unsigned).map_err(|e| LocusError::msg(e.to_string()))
}

fn last_mac(raw: &str) -> Result<String> {
    match raw.lines().rev().find(|l| !l.trim().is_empty()) {
        None => Ok(GENESIS.to_string()),
        Some(line) => {
            let rec: InjectionRecord = serde_json::from_str(line)
                .map_err(|e| LocusError::msg(format!("credential audit log is unreadable: {e}")))?;
            Ok(rec.mac)
        }
    }
}

/// Append one signed record. Callers check [`enabled`] first.
pub fn record(home: &Path, inj: &Injection<'_>) -> Result<InjectionRecord> {
    fs::create_dir_all(home.join("audit"))?;
    let key = load_or_create_key(home)?;
    let mut f = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(log_path(home))?;
    f.lock()?;
    let mut raw = String::new();
    f.seek(SeekFrom::Start(0))?;
    f.read_to_string(&mut raw)?;
    let mut env_keys = inj.env_keys.clone();
    env_keys.sort();
    env_keys.dedup();
    let mut rec = InjectionRecord {
        v: 1,
        ts: chrono::Utc::now().to_rfc3339(),
        session_id: inj.session_id.to_string(),
        binding: inj.binding.to_string(),
        provider: inj.provider.to_string(),
        target: inj.target.to_string(),
        credential_ref: loggable_ref(inj.credential_ref),
        env_keys,
        prev: last_mac(&raw)?,
        mac: String::new(),
    };
    rec.mac = key.seal(&material(&rec)?);
    let line = serde_json::to_string(&rec).map_err(|e| LocusError::msg(e.to_string()))?;
    if !raw.is_empty() && !raw.ends_with('\n') {
        f.write_all(b"\n")?;
    }
    writeln!(f, "{line}")?;
    f.sync_all()?;
    Ok(rec)
}

/// Record every provider whose credential was just placed into `vars`, when
/// the feature is enabled for the Locus home. On a write failure the
/// provider's injected variables are removed again (fail closed) and an issue
/// is returned. A no-op returning no issues when disabled.
pub(crate) fn audit_injected(
    session: &crate::session::Session,
    binding: &crate::binding::Binding,
    vars: &mut std::collections::BTreeMap<String, String>,
    injected: &std::collections::BTreeSet<String>,
    target: &str,
) -> Vec<crate::credential::CredentialResolutionIssue> {
    if injected.is_empty() {
        return Vec::new();
    }
    let Ok(home) = crate::store::locus_home() else {
        return Vec::new();
    };
    if !enabled(&home) {
        return Vec::new();
    }
    let mut issues = Vec::new();
    for p in &binding.providers {
        let keys: Vec<String> = crate::credential::inject_keys_for_binding_provider(p)
            .unwrap_or_default()
            .iter()
            .map(|k| k.to_string())
            .filter(|k| injected.contains(k))
            .collect();
        if keys.is_empty() {
            continue;
        }
        let res = record(
            &home,
            &Injection {
                session_id: &session.session_id,
                binding: &binding.alias,
                provider: &p.provider,
                target,
                credential_ref: &p.credential_ref,
                env_keys: keys.clone(),
            },
        );
        if res.is_err() {
            for k in &keys {
                vars.remove(k);
            }
            vars.insert(
                format!("LOCUS_{}_CREDENTIAL_RESOLVED", p.provider.to_uppercase()),
                "0".into(),
            );
            issues.push(crate::credential::CredentialResolutionIssue {
                provider: p.provider.clone(),
                source: "credential_audit".into(),
                code: "audit_write_failed".into(),
            });
        }
    }
    issues
}

/// Result of [`verify`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    pub records: usize,
    /// 1-based line number of the first record that fails, if any.
    pub first_bad_line: Option<usize>,
    pub reason: Option<String>,
}

impl VerifyReport {
    pub fn ok(&self) -> bool {
        self.first_bad_line.is_none()
    }
}

/// Re-check every MAC and the `prev` chain. A missing log verifies as empty.
pub fn verify(home: &Path) -> Result<VerifyReport> {
    let raw = match fs::read_to_string(log_path(home)) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    let key_hex = match fs::read_to_string(key_path(home)) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && raw.trim().is_empty() => {
            return Ok(VerifyReport {
                records: 0,
                first_bad_line: None,
                reason: None,
            })
        }
        Err(e) => return Err(e.into()),
    };
    let key = SealKey::from_hex(&key_hex)?;
    let mut prev = GENESIS.to_string();
    let mut n = 0usize;
    for (i, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let bad = |reason: &str| VerifyReport {
            records: n,
            first_bad_line: Some(i + 1),
            reason: Some(reason.to_string()),
        };
        let Ok(rec) = serde_json::from_str::<InjectionRecord>(line) else {
            return Ok(bad("not a valid record"));
        };
        if rec.prev != prev {
            return Ok(bad("chain broken (line removed, inserted or reordered)"));
        }
        if !key.verify(&material(&rec)?, &rec.mac) {
            return Ok(bad("signature does not match (line edited or wrong key)"));
        }
        prev = rec.mac.clone();
        n += 1;
    }
    Ok(VerifyReport {
        records: n,
        first_bad_line: None,
        reason: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn inj<'a>(provider: &'a str, cref: &'a str) -> Injection<'a> {
        Injection {
            session_id: "ses_test",
            binding: "example",
            provider,
            target: "child-env",
            credential_ref: cref,
            env_keys: vec!["GH_TOKEN".into(), "GITHUB_TOKEN".into()],
        }
    }

    #[test]
    fn records_chain_and_verify() {
        let d = tempdir().unwrap();
        let a = record(d.path(), &inj("github", "env:EXAMPLE_GH")).unwrap();
        let b = record(d.path(), &inj("vercel", "keychain:locus/example-vc")).unwrap();
        assert_eq!(a.prev, GENESIS);
        assert_eq!(b.prev, a.mac);
        let r = verify(d.path()).unwrap();
        assert!(r.ok());
        assert_eq!(r.records, 2);
    }

    #[test]
    fn never_contains_values_only_names_and_refs() {
        let d = tempdir().unwrap();
        record(d.path(), &inj("github", "env:EXAMPLE_GH")).unwrap();
        let raw = fs::read_to_string(log_path(d.path())).unwrap();
        let rec: InjectionRecord = serde_json::from_str(raw.trim()).unwrap();
        assert_eq!(rec.credential_ref, "env:EXAMPLE_GH");
        assert_eq!(rec.env_keys, vec!["GH_TOKEN", "GITHUB_TOKEN"]);
        // Exactly the documented fields; nothing value-shaped.
        let v: serde_json::Value = serde_json::from_str(raw.trim()).unwrap();
        let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "binding",
                "credential_ref",
                "env_keys",
                "mac",
                "prev",
                "provider",
                "session_id",
                "target",
                "ts",
                "v"
            ]
        );
    }

    #[test]
    fn a_raw_value_given_as_a_reference_is_not_logged() {
        let d = tempdir().unwrap();
        let rec = record(d.path(), &inj("github", "ghp_notarealtokenbutlookslikeone")).unwrap();
        assert_eq!(rec.credential_ref, "<unrecognized reference>");
        let raw = fs::read_to_string(log_path(d.path())).unwrap();
        assert!(!raw.contains("ghp_notarealtoken"));
    }

    #[test]
    fn edited_line_fails_verification() {
        let d = tempdir().unwrap();
        record(d.path(), &inj("github", "env:EXAMPLE_GH")).unwrap();
        record(d.path(), &inj("vercel", "env:EXAMPLE_VC")).unwrap();
        let p = log_path(d.path());
        let raw = fs::read_to_string(&p)
            .unwrap()
            .replace("\"vercel\"", "\"supabase\"");
        fs::write(&p, raw).unwrap();
        let r = verify(d.path()).unwrap();
        assert_eq!(r.first_bad_line, Some(2));
        assert!(r.reason.unwrap().contains("signature"));
    }

    #[test]
    fn removed_line_breaks_the_chain() {
        let d = tempdir().unwrap();
        for p in ["a", "b", "c"] {
            record(d.path(), &inj(p, "env:X")).unwrap();
        }
        let path = log_path(d.path());
        let raw = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = raw.lines().collect();
        fs::write(&path, format!("{}\n{}\n", lines[0], lines[2])).unwrap();
        let r = verify(d.path()).unwrap();
        assert_eq!(r.first_bad_line, Some(2));
        assert!(r.reason.unwrap().contains("chain"));
    }

    #[test]
    fn wrong_key_fails_verification() {
        let d = tempdir().unwrap();
        record(d.path(), &inj("github", "env:X")).unwrap();
        fs::write(key_path(d.path()), SealKey::generate().to_hex()).unwrap();
        assert!(!verify(d.path()).unwrap().ok());
    }

    #[cfg(unix)]
    #[test]
    fn key_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempdir().unwrap();
        record(d.path(), &inj("github", "env:X")).unwrap();
        let mode = fs::metadata(key_path(d.path()))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn disabled_by_default() {
        let d = tempdir().unwrap();
        assert!(!enabled(d.path()));
        assert!(verify(d.path()).unwrap().ok());
        assert!(!log_path(d.path()).exists());
        assert!(!key_path(d.path()).exists());
    }
}

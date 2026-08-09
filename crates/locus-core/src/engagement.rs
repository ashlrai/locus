//! Engagement lifecycle — fast client onboarding / offboard unit.
//!
//! Metadata lives in `$LOCUS_HOME/engagements/<alias>.json` (not in the binding
//! TOML) so Phantom vault secrets and binding files stay independent.
//! Audit archives land in `$LOCUS_HOME/archives/<alias>-<date>.jsonl`.

use crate::binding::{Binding, BindingBody, Policy, ProviderBinding, Scope};
use crate::error::{LocusError, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Sidecar metadata for one client engagement.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EngagementMeta {
    pub alias: String,
    pub tenant: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<String>,
    /// `open` | `closed`
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl EngagementMeta {
    pub fn open(alias: impl Into<String>, tenant: impl Into<String>) -> Self {
        Self {
            alias: alias.into(),
            tenant: tenant.into(),
            created_at: Utc::now().to_rfc3339(),
            closed_at: None,
            status: "open".into(),
            archive_path: None,
            description: None,
        }
    }

    pub fn is_closed(&self) -> bool {
        self.status == "closed" || self.closed_at.is_some()
    }

    pub fn mark_closed(&mut self, archive_path: Option<String>) {
        self.closed_at = Some(Utc::now().to_rfc3339());
        self.status = "closed".into();
        if archive_path.is_some() {
            self.archive_path = archive_path;
        }
    }
}

/// Result of closing an engagement (for CLI print).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngagementCloseResult {
    pub alias: String,
    pub tenant: String,
    pub closed_at: String,
    pub left_session: bool,
    pub archive_path: Option<String>,
    pub checklist: Vec<String>,
}

/// Credential suffix for phm: names — `acme-ro` → `ACME_RO`.
pub fn cred_suffix(alias: &str) -> String {
    alias
        .chars()
        .map(|c| {
            if c == '-' {
                '_'
            } else {
                c.to_ascii_uppercase()
            }
        })
        .collect()
}

/// Client engagement binding template (supabase + github + vercel stubs).
///
/// All credentials are `phm:` refs namespaced by alias. Placeholders must be
/// edited before real work; never raw secrets.
pub fn client_binding_template(alias: &str, tenant: &str) -> Binding {
    let suf = cred_suffix(alias);
    let tenant_s = tenant.to_string();
    Binding::from_body(BindingBody {
        id: format!("bnd_{alias}"),
        alias: alias.to_string(),
        tenant: tenant_s.clone(),
        principal: None,
        description: Some(format!("{tenant} client engagement")),
        policy: Policy {
            require_approval: vec![
                "*.delete*".into(),
                "*.drop*".into(),
                "vercel.deploy.prod".into(),
            ],
            dual_control: vec!["*.delete*".into(), "vercel.deploy.prod".into()],
            max_ttl: Some("8h".into()),
            ..Policy::default()
        },
        providers: vec![
            ProviderBinding {
                provider: "supabase".into(),
                account: format!("{alias}-prod"),
                credential_ref: format!("phm:SUPABASE_{suf}"),
                scope: Scope {
                    project_ref: Some(format!("{alias}_ref_replace_me")),
                    read_only: Some(false),
                    ..Scope::default()
                },
                upstream: None,
            },
            ProviderBinding {
                provider: "github".into(),
                account: tenant_s.clone(),
                credential_ref: format!("phm:GH_TOKEN_{suf}"),
                scope: Scope {
                    orgs: vec![tenant_s.clone()],
                    repos: vec![format!("{tenant_s}/*")],
                    ..Scope::default()
                },
                upstream: None,
            },
            ProviderBinding {
                provider: "vercel".into(),
                account: format!("{alias}-team"),
                credential_ref: format!("phm:VERCEL_TOKEN_{suf}"),
                scope: Scope {
                    team_id: Some(format!("team_{alias}_replace_me")),
                    projects: vec![format!("{alias}-web")],
                    env: vec!["preview".into()],
                    ..Scope::default()
                },
                upstream: None,
            },
        ],
    })
}

/// Short engagement README for the client repo (no secrets).
pub fn engagement_readme(alias: &str, tenant: &str) -> String {
    format!(
        r#"# Engagement: {tenant}

Locus binding alias: **`{alias}`**

## Daily loop

```bash
locus enter {alias}          # or: locus enter  (uses .locus.toml)
locus whoami
# … agent / CLI work hard-scoped to this pin …
locus leave
```

## Credentials (Phantom)

Binding stores **refs only** — create these in Phantom (never commit values):

- `phm:SUPABASE_{suf}`
- `phm:GH_TOKEN_{suf}`
- `phm:VERCEL_TOKEN_{suf}`

Edit `~/.locus/bindings/{alias}.toml` to set real `project_ref` / `team_id` / orgs.

## Offboard

```bash
locus engagement close {alias} --archive
# then revoke provider access + rotate Phantom secrets
```

See [firm mode](https://github.com/ashlrai/locus/blob/main/docs/firm-mode.md).
"#,
        tenant = tenant,
        alias = alias,
        suf = cred_suffix(alias),
    )
}

/// Write engagement meta JSON under `engagements/`.
pub fn write_meta(dir: &Path, meta: &EngagementMeta) -> Result<PathBuf> {
    crate::binding::validate_name_component("alias", &meta.alias)?;
    fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.json", meta.alias));
    ensure_under(dir, &path)?;
    fs::write(&path, serde_json::to_string_pretty(meta)?)?;
    Ok(path)
}

/// Load engagement meta if present.
pub fn read_meta(dir: &Path, alias: &str) -> Result<Option<EngagementMeta>> {
    crate::binding::validate_name_component("alias", alias)?;
    let path = dir.join(format!("{alias}.json"));
    if !path.exists() {
        return Ok(None);
    }
    ensure_under(dir, &path)?;
    let raw = fs::read_to_string(&path)?;
    Ok(Some(serde_json::from_str(&raw)?))
}

/// Default offboard checklist (manual provider steps — Locus does not delete vault secrets).
pub fn close_checklist(alias: &str) -> Vec<String> {
    let suf = cred_suffix(alias);
    vec![
        format!("Binding pin cleared if it was active (`locus leave`)"),
        format!("Engagement marked closed (metadata only — binding file kept)"),
        format!("Archive audit slice if --archive was used"),
        format!(
            "Rotate/revoke Phantom secrets: SUPABASE_{suf}, GH_TOKEN_{suf}, VERCEL_TOKEN_{suf}"
        ),
        "Revoke provider access (GH org, Vercel team, Supabase project invites)".into(),
        "Strip or rewrite client repo `.locus.toml` if custody ends".into(),
        "Do NOT expect Locus to delete vault secrets — Phantom owns those".into(),
    ]
}

fn ensure_under(base: &Path, path: &Path) -> Result<()> {
    // Lightweight path guard (alias already validated).
    if let (Ok(b), Ok(p)) = (
        base.canonicalize(),
        path.parent().unwrap_or(path).canonicalize(),
    ) {
        if !p.starts_with(&b) && p != b {
            return Err(LocusError::msg(format!(
                "refusing path outside engagements: {}",
                path.display()
            )));
        }
    }
    let _ = base;
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn template_has_phm_refs_only() {
        let b = client_binding_template("acme", "acme-corp");
        assert_eq!(b.alias, "acme");
        assert_eq!(b.tenant, "acme-corp");
        assert_eq!(b.providers.len(), 3);
        for p in &b.providers {
            assert!(
                p.credential_ref.starts_with("phm:"),
                "raw secret?: {}",
                p.credential_ref
            );
            assert!(!p.credential_ref.contains("sk_"));
        }
        assert!(b
            .provider("supabase")
            .unwrap()
            .credential_ref
            .contains("SUPABASE_ACME"));
        b.validate().unwrap();
    }

    #[test]
    fn cred_suffix_normalizes_hyphen() {
        assert_eq!(cred_suffix("acme-ro"), "ACME_RO");
    }

    #[test]
    fn meta_roundtrip_close() {
        let dir = tempdir().unwrap();
        let mut meta = EngagementMeta::open("acme", "acme-corp");
        write_meta(dir.path(), &meta).unwrap();
        let loaded = read_meta(dir.path(), "acme").unwrap().unwrap();
        assert_eq!(loaded.status, "open");
        meta.mark_closed(Some("/tmp/a.jsonl".into()));
        assert!(meta.is_closed());
        write_meta(dir.path(), &meta).unwrap();
        let closed = read_meta(dir.path(), "acme").unwrap().unwrap();
        assert_eq!(closed.status, "closed");
        assert!(closed.archive_path.is_some());
    }

    #[test]
    fn readme_contains_alias_no_secrets() {
        let md = engagement_readme("acme", "acme-corp");
        assert!(md.contains("locus enter acme"));
        assert!(md.contains("phm:SUPABASE_ACME"));
        assert!(!md.contains("service_role"));
    }
}

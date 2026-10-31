//! Client offboarding — remove every local record a client alias owns.
//!
//! `locus offboard <alias>` lists the client's binding file, approval
//! records, pin-history entries, MCP tenant grants, sealed session files and
//! engagement sidecar as a `locus.plan.v1` plan; `--apply` removes them under
//! the operator control capability.
//!
//! The inventory carries provider names, record ids and paths only — never a
//! credential reference or a resolved secret. Audit events and audit
//! archives are kept on purpose: offboarding removes operable state, not
//! evidence, and the offboard itself appends `client.offboard` to the log.

use crate::binding::validate_name_component;
use crate::error::{LocusError, Result};
use crate::store::Store;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

/// The client's binding file (provider names only — never credential refs).
#[derive(Debug, Clone, Serialize)]
pub struct OffboardBinding {
    pub path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    pub providers: Vec<String>,
}

/// An approval record filed under the client's alias.
#[derive(Debug, Clone, Serialize)]
pub struct OffboardApproval {
    pub id: String,
    pub tool: String,
    pub status: String,
    pub path: PathBuf,
}

/// A pin-history entry naming the client's alias (pinned or expected).
#[derive(Debug, Clone, Serialize)]
pub struct OffboardHistoryEntry {
    pub ts: String,
    pub event: String,
}

/// A sealed session file pinned to the client (`run-*`, `ci-*`; the active
/// pin is reported as [`OffboardInventory::active_pin`] instead).
#[derive(Debug, Clone, Serialize)]
pub struct OffboardSession {
    pub session_id: String,
    pub path: PathBuf,
    /// Worker home recorded in the session file; cleaned on apply only when
    /// it still canonicalizes under this store's `workers/` root.
    #[serde(skip_serializing)]
    pub worker_home: String,
}

/// An MCP tenant grant minted for the client.
#[derive(Debug, Clone, Serialize)]
pub struct OffboardGrant {
    pub grant_id: String,
    pub session_id: String,
    pub path: PathBuf,
}

/// Everything `locus offboard <alias>` found for the client.
#[derive(Debug, Clone, Serialize)]
pub struct OffboardInventory {
    pub alias: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding: Option<OffboardBinding>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub approvals: Vec<OffboardApproval>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pin_history: Vec<OffboardHistoryEntry>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<OffboardSession>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mcp_grants: Vec<OffboardGrant>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engagement_meta: Option<PathBuf>,
    /// `sessions/active.json` names the client (removed via `leave`, never
    /// by file edit).
    pub active_pin: bool,
    /// Workspace templates still naming the alias — listed, never edited.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub kept_workspace_templates: Vec<String>,
    /// Audit archives for the alias — kept as evidence, never edited.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub kept_archives: Vec<PathBuf>,
}

impl OffboardInventory {
    /// Nothing removable was found for the alias.
    pub fn is_empty(&self) -> bool {
        self.binding.is_none()
            && self.engagement_meta.is_none()
            && self.approvals.is_empty()
            && self.pin_history.is_empty()
            && self.sessions.is_empty()
            && self.mcp_grants.is_empty()
            && !self.active_pin
    }
}

/// What `--apply` actually changed.
#[derive(Debug, Clone, Serialize)]
pub struct OffboardOutcome {
    pub alias: String,
    pub left_active_pin: bool,
    pub removed_binding: bool,
    pub removed_engagement_meta: bool,
    pub removed_approvals: usize,
    pub dropped_pin_history_entries: usize,
    pub removed_sessions: usize,
    pub revoked_mcp_grants: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub kept_workspace_templates: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub kept_archives: Vec<PathBuf>,
}

/// Read-only inventory of every record the client's alias owns. Opens no
/// locks and writes nothing, so a `--dry-run` plan stays byte-exact.
pub fn inventory(store: &Store, alias: &str) -> Result<OffboardInventory> {
    validate_name_component("alias", alias)?;
    let home = store.home();

    // Binding file; provider names only (never a credential reference). A
    // file that no longer parses is still listed — removal is the point.
    let binding_path = store.bindings_dir().join(format!("{alias}.toml"));
    let binding = if binding_path.exists() {
        let loaded = store.load_binding(alias).ok();
        Some(OffboardBinding {
            path: binding_path,
            tenant: loaded.as_ref().map(|b| b.tenant.clone()),
            providers: loaded
                .map(|b| b.providers.iter().map(|p| p.provider.clone()).collect())
                .unwrap_or_default(),
        })
    } else {
        None
    };

    let engagement_meta = {
        let p = store.engagements_dir().join(format!("{alias}.json"));
        p.exists().then_some(p)
    };

    // Same scan as `Store::list_approvals`, keeping each record's file so a
    // record whose id does not match its filename is still removed.
    let mut approvals = Vec::new();
    let approvals_dir = store.approvals_dir();
    if approvals_dir.is_dir() {
        let mut entries: Vec<_> = fs::read_dir(&approvals_dir)?
            .filter_map(|e| e.ok())
            .collect();
        entries.sort_by_key(|e| e.file_name());
        for ent in entries {
            let path = ent.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let Ok(raw) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(rec) = serde_json::from_str::<crate::approval::ApprovalRecord>(&raw) else {
                continue;
            };
            if rec.binding == alias {
                approvals.push(OffboardApproval {
                    id: rec.id,
                    tool: rec.tool,
                    status: rec.status.as_str().to_string(),
                    path,
                });
            }
        }
    }

    let pin_history = crate::pin_history::read(home)
        .into_iter()
        .filter(|e| e.alias.as_deref() == Some(alias) || e.expected.as_deref() == Some(alias))
        .map(|e| OffboardHistoryEntry {
            ts: e.ts,
            event: e.event,
        })
        .collect();

    // Sealed session files pinned to the client. Parsed best-effort: a
    // corrupt record still shows below as an unknown entry only when its
    // file names the alias — unreadable files never block the inventory.
    let mut sessions = Vec::new();
    let mut active_pin = false;
    let sessions_dir = home.join("sessions");
    if sessions_dir.is_dir() {
        let mut entries: Vec<_> = fs::read_dir(&sessions_dir)?
            .filter_map(|e| e.ok())
            .collect();
        entries.sort_by_key(|e| e.file_name());
        for ent in entries {
            let path = ent.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let Ok(raw) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(session) = serde_json::from_str::<crate::session::Session>(&raw) else {
                continue;
            };
            // Sessions are sealed to exactly one binding: `binding_alias`
            // ownership only. A namespace alias inside another client's
            // namespaced session must not drag that session into this
            // offboard.
            if session.binding_alias != alias {
                continue;
            }
            if path.file_name().and_then(|n| n.to_str()) == Some("active.json") {
                active_pin = true;
            } else {
                sessions.push(OffboardSession {
                    session_id: session.session_id,
                    worker_home: session.worker_home,
                    path,
                });
            }
        }
    }

    let mcp_grants = store
        .list_mcp_grants()?
        .into_iter()
        .filter(|g| g.binding_alias == alias)
        .map(|g| OffboardGrant {
            path: store.mcp_grants_dir().join(format!("{}.json", g.grant_id)),
            grant_id: g.grant_id,
            session_id: g.session_id,
        })
        .collect();

    // References the client is gone but these survive: workspace templates
    // are operator-owned and audit archives are evidence.
    let kept_workspace_templates = store
        .list_workspace_templates()?
        .into_iter()
        .filter(|t| {
            t.config.default_binding.as_deref() == Some(alias)
                || t.config.allowed_bindings.iter().any(|a| a == alias)
        })
        .map(|t| t.name)
        .collect();

    let mut kept_archives = Vec::new();
    let archives_dir = store.archives_dir();
    if archives_dir.is_dir() {
        let mut entries: Vec<_> = fs::read_dir(&archives_dir)?
            .filter_map(|e| e.ok())
            .collect();
        entries.sort_by_key(|e| e.file_name());
        let prefix = format!("{alias}-");
        for ent in entries {
            let path = ent.path();
            let matches = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&prefix) && n.ends_with(".jsonl"));
            if matches {
                kept_archives.push(path);
            }
        }
    }

    Ok(OffboardInventory {
        alias: alias.to_string(),
        binding,
        approvals,
        pin_history,
        sessions,
        mcp_grants,
        engagement_meta,
        active_pin,
        kept_workspace_templates,
        kept_archives,
    })
}

/// Perform the offboard: drop the pin if the client owns it, revoke its
/// grants, remove its binding/approvals/sessions/engagement meta, and scrub
/// its pin-history entries. Everything else — audit events, archives,
/// workspace templates, other clients' records — is left untouched.
///
/// Requires the operator control capability, like every control mutation.
/// The inventory is re-read here rather than trusted from a caller's plan:
/// apply works on what is on disk now.
pub fn apply(store: &Store, alias: &str) -> Result<OffboardOutcome> {
    store.require_local_control("offboard client")?;
    let inv = inventory(store, alias)?;
    if inv.is_empty() {
        return Err(LocusError::BindingNotFound(alias.into()));
    }

    // Drop the live pin first — before the binding file goes away. Re-check
    // the active session's owner best-effort: the inventory raced ahead of
    // any re-pin, `leave` removes whatever active.json holds *now*, and a
    // strict read would make a corrupt pin unoffboardable. `leave` is the
    // audited teardown for a healthy pin; when the pin is wedged (dead
    // broker, unreadable file) the `leave --force` verifier path still
    // requires a real capability, never a bypass.
    let active_is_ours = fs::read_to_string(store.active_session_path())
        .ok()
        .and_then(|raw| serde_json::from_str::<crate::session::Session>(&raw).ok())
        .is_some_and(|s| s.binding_alias == alias);
    let left_active_pin = if inv.active_pin && active_is_ours {
        match store.leave() {
            Ok(session) => session.is_some(),
            Err(strict) => match store.force_leave("client offboard", false) {
                Ok(outcome) => outcome.cleared,
                Err(force) => {
                    return Err(LocusError::msg(format!(
                        "cannot drop the active pin for {alias}: {force} (strict leave: {strict})"
                    )))
                }
            },
        }
    } else {
        false
    };

    // Grants carry their own revocation + backing-session teardown.
    let mut revoked_mcp_grants = 0;
    for grant in &inv.mcp_grants {
        if store.revoke_mcp_grant(&grant.grant_id)?.is_some() {
            revoked_mcp_grants += 1;
        }
    }

    // The binding file (audited `binding.remove`). A file that vanished
    // since the inventory is a concurrent delete, not an error.
    let mut removed_binding = false;
    if let Some(binding) = &inv.binding {
        if binding.path.exists() {
            store.remove_binding(alias)?;
            removed_binding = true;
        }
    }

    let mut removed_approvals = 0;
    for approval in &inv.approvals {
        removed_approvals += remove_file_if_present(&approval.path)?;
    }

    let mut removed_engagement_meta = false;
    if let Some(meta) = &inv.engagement_meta {
        removed_engagement_meta = remove_file_if_present(meta)? == 1;
    }

    // Leftover sealed session files for the alias (`run-*`, `ci-*`). The
    // file is the lease's root: removing it makes the session unpresentable
    // and its broker generation dies with the broker.
    let mut removed_sessions = 0;
    for session in &inv.sessions {
        if remove_file_if_present(&session.path)? == 1 {
            removed_sessions += 1;
            store.remove_worker_home_if_safe(&session.worker_home);
        }
    }

    // Pin history last: the steps above may append their own entries for
    // the alias (`leave`, `binding.remove`); scrub once at the end.
    let dropped_pin_history_entries = crate::pin_history::remove_alias(store.home(), alias)
        .map_err(|e| LocusError::msg(format!("cannot rewrite pin history: {e}")))?;

    store.audit(
        "client.offboard",
        alias,
        Some(serde_json::json!({
            "left_active_pin": left_active_pin,
            "removed_binding": removed_binding,
            "removed_engagement_meta": removed_engagement_meta,
            "removed_approvals": removed_approvals,
            "dropped_pin_history_entries": dropped_pin_history_entries,
            "removed_sessions": removed_sessions,
            "revoked_mcp_grants": revoked_mcp_grants,
        })),
    )?;

    Ok(OffboardOutcome {
        alias: alias.to_string(),
        left_active_pin,
        removed_binding,
        removed_engagement_meta,
        removed_approvals,
        dropped_pin_history_entries,
        removed_sessions,
        revoked_mcp_grants,
        kept_workspace_templates: inv.kept_workspace_templates,
        kept_archives: inv.kept_archives,
    })
}

/// Delete a file that may have vanished since the inventory; `Ok(1)` when
/// this call removed it.
fn remove_file_if_present(path: &Path) -> Result<usize> {
    match fs::remove_file(path) {
        Ok(()) => Ok(1),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::{mint_approval_id, ApprovalRecord, ApprovalStatus};
    use chrono::Utc;

    fn write_approval(store: &Store, binding: &str, tool: &str) -> ApprovalRecord {
        let rec = ApprovalRecord {
            id: mint_approval_id(),
            tool: tool.into(),
            binding: binding.into(),
            args_digest: "sha256:test".into(),
            created_at: Utc::now(),
            status: ApprovalStatus::Pending,
            session_id: "ses_test".into(),
            requester: String::new(),
            grants: Vec::new(),
            expires_at: None,
            granted_at: None,
        };
        fs::create_dir_all(store.approvals_dir()).unwrap();
        fs::write(
            store.approvals_dir().join(format!("{}.json", rec.id)),
            serde_json::to_string_pretty(&rec).unwrap(),
        )
        .unwrap();
        rec
    }

    fn fixture() -> (tempfile::TempDir, Store) {
        let home = tempfile::tempdir().unwrap();
        let store = Store::open(home.path()).unwrap();
        for alias in ["acme", "beta"] {
            let binding = crate::engagement::client_binding_template(alias, &format!("{alias}-co"));
            fs::write(
                store.bindings_dir().join(format!("{alias}.toml")),
                binding.to_toml().unwrap(),
            )
            .unwrap();
        }
        (home, store)
    }

    fn write_session(store: &Store, alias: &str, file: &str, worker_dir: &str) -> PathBuf {
        let key = store.seal_key().unwrap();
        let worker = store.home().join("workers").join(worker_dir);
        fs::create_dir_all(&worker).unwrap();
        let session = crate::session::Session::new(
            &format!("bnd_{alias}"),
            alias,
            &format!("{alias}-co"),
            None,
            crate::session::PinSource::Run,
            None,
            chrono::Duration::hours(1),
            worker.display().to_string(),
            &key,
        );
        let path = store.home().join("sessions").join(file);
        fs::write(&path, serde_json::to_string(&session).unwrap()).unwrap();
        path
    }

    fn write_grant(store: &Store, grant_id: &str, alias: &str) -> PathBuf {
        let grant = serde_json::json!({
            "v": 1,
            "grant_id": grant_id,
            "token_seal": "deadbeef",
            "session_id": "ses_gone",
            "binding_alias": alias,
            "tenant": format!("{alias}-co"),
            "created_at": Utc::now().to_rfc3339(),
            "expires_at": Utc::now() + chrono::Duration::hours(1),
            "revoked": false,
        });
        fs::create_dir_all(store.mcp_grants_dir()).unwrap();
        let path = store.mcp_grants_dir().join(format!("{grant_id}.json"));
        fs::write(&path, serde_json::to_string_pretty(&grant).unwrap()).unwrap();
        path
    }

    #[test]
    fn inventory_lists_only_the_clients_records() {
        let (_home, store) = fixture();
        write_approval(&store, "acme", "supabase.delete_row");
        write_approval(&store, "beta", "vercel.deploy.prod");
        let key = crate::pin_history::history_key(store.home()).unwrap();
        for (event, alias, expected) in [
            ("pin", Some("acme"), None),
            ("pin", Some("beta"), None),
            ("pin_mismatch", Some("beta"), Some("acme")),
        ] {
            let e = crate::pin_history::entry_for(&key, event, alias, expected, None);
            crate::pin_history::append(store.home(), &e, 100).unwrap();
        }

        let inv = inventory(&store, "acme").unwrap();
        assert!(!inv.is_empty());
        assert_eq!(inv.binding.as_ref().unwrap().providers.len(), 3);
        assert_eq!(inv.approvals.len(), 1);
        assert_eq!(inv.approvals[0].tool, "supabase.delete_row");
        // The pin and the pin_mismatch that *expected* acme are both named.
        assert_eq!(inv.pin_history.len(), 2);
        assert!(inv.kept_archives.is_empty());
        // Nothing in the inventory may look credential-like.
        let json = serde_json::to_string(&inv).unwrap();
        for leak in [
            "SUPABASE_ACME",
            "GH_TOKEN_ACME",
            "VERCEL_TOKEN_ACME",
            "phm:",
        ] {
            assert!(!json.contains(leak), "{leak} leaked: {json}");
        }
    }

    #[test]
    fn apply_removes_client_and_keeps_the_other() {
        let (_home, store) = fixture();
        let acme_appr = write_approval(&store, "acme", "supabase.delete_row");
        let beta_appr = write_approval(&store, "beta", "vercel.deploy.prod");
        let key = crate::pin_history::history_key(store.home()).unwrap();
        for (event, alias) in [("pin", "acme"), ("leave", "acme"), ("pin", "beta")] {
            let e = crate::pin_history::entry_for(&key, event, Some(alias), None, None);
            crate::pin_history::append(store.home(), &e, 100).unwrap();
        }
        fs::write(
            store.engagements_dir().join("acme.json"),
            serde_json::to_string(&crate::engagement::EngagementMeta::open("acme", "acme-co"))
                .unwrap(),
        )
        .unwrap();
        let acme_session = write_session(&store, "acme", "run-acme.json", "w-acme");
        let beta_session = write_session(&store, "beta", "run-beta.json", "w-beta");
        let acme_grant = write_grant(&store, "0123456789abcdef", "acme");
        let beta_binding_bytes = fs::read(store.bindings_dir().join("beta.toml")).unwrap();
        let beta_session_bytes = fs::read(&beta_session).unwrap();

        let out = apply(&store, "acme").unwrap();
        assert!(out.removed_binding);
        assert!(out.removed_engagement_meta);
        assert_eq!(out.removed_approvals, 1);
        assert_eq!(out.dropped_pin_history_entries, 2);
        assert_eq!(out.removed_sessions, 1);
        assert_eq!(out.revoked_mcp_grants, 1);

        assert!(!store.bindings_dir().join("acme.toml").exists());
        assert!(!store
            .approvals_dir()
            .join(format!("{}.json", acme_appr.id))
            .exists());
        assert!(!store.engagements_dir().join("acme.json").exists());
        assert!(!acme_session.exists());
        assert!(!acme_grant.exists());
        assert!(!store.home().join("workers/w-acme").exists());
        assert_eq!(
            fs::read(store.bindings_dir().join("beta.toml")).unwrap(),
            beta_binding_bytes
        );
        assert!(store
            .approvals_dir()
            .join(format!("{}.json", beta_appr.id))
            .exists());
        assert_eq!(fs::read(&beta_session).unwrap(), beta_session_bytes);
        assert!(store.home().join("workers/w-beta").exists());
        let left = crate::pin_history::read(store.home());
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].alias.as_deref(), Some("beta"));
        // The offboard itself is audited.
        assert!(store
            .read_audit_events()
            .unwrap()
            .iter()
            .any(|e| e.op == "client.offboard" && e.binding == "acme"));
    }

    #[test]
    fn apply_without_any_trace_is_binding_not_found() {
        let (_home, store) = fixture();
        assert!(matches!(
            apply(&store, "ghost"),
            Err(LocusError::BindingNotFound(_))
        ));
    }

    #[test]
    fn inventory_rejects_traversal() {
        let (_home, store) = fixture();
        for bad in ["../x", "a/b", "a\\b", ".."] {
            assert!(inventory(&store, bad).is_err(), "{bad}");
        }
    }
}

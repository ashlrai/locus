//! Local control-plane store under `~/.locus/` (or `LOCUS_HOME`).

use crate::approval::{
    args_digest, default_grant_ttl, mint_approval_id, ApprovalRecord, ApprovalStatus,
};
use crate::binding::{Binding, BindingSummary};
use crate::error::{LocusError, Result};
use crate::seal::SealKey;
use crate::session::{parse_ttl, PinSource, Session};
use crate::workspace::{find_workspace, WorkspaceConfig};
use chrono::{Duration, Utc};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

/// Layout:
/// ```text
/// $LOCUS_HOME/
///   config.toml
///   daemon.key          # seal key hex (0600)
///   bindings/*.toml
///   sessions/active.json
///   workers/<session_id>/
///   audit/events.jsonl
///   approvals/{id}.json
/// ```
#[derive(Debug, Clone)]
pub struct Store {
    home: PathBuf,
}

impl Store {
    pub fn open_default() -> Result<Self> {
        let home = locus_home()?;
        Self::open(home)
    }

    pub fn open(home: impl Into<PathBuf>) -> Result<Self> {
        let home = home.into();
        fs::create_dir_all(home.join("bindings"))?;
        fs::create_dir_all(home.join("sessions"))?;
        fs::create_dir_all(home.join("workers"))?;
        fs::create_dir_all(home.join("audit"))?;
        fs::create_dir_all(home.join("approvals"))?;
        let s = Self { home };
        // Ensure seal key exists
        let _ = s.seal_key()?;
        Ok(s)
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn bindings_dir(&self) -> PathBuf {
        self.home.join("bindings")
    }

    pub fn approvals_dir(&self) -> PathBuf {
        self.home.join("approvals")
    }

    pub fn seal_key_path(&self) -> PathBuf {
        self.home.join("daemon.key")
    }

    pub fn active_session_path(&self) -> PathBuf {
        self.home.join("sessions").join("active.json")
    }

    pub fn audit_path(&self) -> PathBuf {
        self.home.join("audit").join("events.jsonl")
    }

    pub fn seal_key(&self) -> Result<SealKey> {
        let path = self.seal_key_path();
        if path.exists() {
            let hex = fs::read_to_string(&path)?;
            SealKey::from_hex(hex.trim())
        } else {
            let key = SealKey::generate();
            write_secret_file(&path, key.to_hex().as_bytes())?;
            Ok(key)
        }
    }

    // ── Bindings ──────────────────────────────────────────────────────────

    pub fn save_binding(&self, binding: &Binding) -> Result<PathBuf> {
        binding.validate()?;
        let path = self.bindings_dir().join(format!("{}.toml", binding.alias));
        fs::write(&path, binding.to_toml()?)?;
        self.audit("binding.save", &binding.alias, None)?;
        Ok(path)
    }

    pub fn load_binding(&self, alias_or_id: &str) -> Result<Binding> {
        // Prefer alias filename
        let by_alias = self.bindings_dir().join(format!("{alias_or_id}.toml"));
        if by_alias.exists() {
            let raw = fs::read_to_string(&by_alias)?;
            return Binding::parse_toml(&raw);
        }
        // Scan for id match
        for b in self.list_bindings()? {
            if b.id == alias_or_id || b.alias == alias_or_id {
                return self.load_binding(&b.alias);
            }
        }
        Err(LocusError::BindingNotFound(alias_or_id.into()))
    }

    pub fn list_bindings(&self) -> Result<Vec<BindingSummary>> {
        let mut out = Vec::new();
        let dir = self.bindings_dir();
        if !dir.exists() {
            return Ok(out);
        }
        let mut entries: Vec<_> = fs::read_dir(&dir)?.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for ent in entries {
            let path = ent.path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            let raw = fs::read_to_string(&path)?;
            match Binding::parse_toml(&raw) {
                Ok(b) => out.push(BindingSummary::from(&b)),
                Err(e) => {
                    // Skip corrupt files but don't abort listing
                    let _ = e;
                }
            }
        }
        Ok(out)
    }

    pub fn remove_binding(&self, alias: &str) -> Result<()> {
        let path = self.bindings_dir().join(format!("{alias}.toml"));
        if !path.exists() {
            return Err(LocusError::BindingNotFound(alias.into()));
        }
        fs::remove_file(path)?;
        self.audit("binding.remove", alias, None)?;
        Ok(())
    }

    // ── Sessions ──────────────────────────────────────────────────────────

    pub fn pin(
        &self,
        alias_or_id: &str,
        cwd: &Path,
        client: Option<String>,
        force: bool,
    ) -> Result<Session> {
        let binding = self.load_binding(alias_or_id)?;
        let ws = find_workspace(cwd);

        if let Some((_, ref cfg)) = ws {
            if !cfg.allows(&binding.alias) && !cfg.allows(&binding.id) && !force {
                return Err(LocusError::BindingNotAllowed(binding.alias.clone()));
            }
        }

        let source = if let Some((ref path, ref cfg)) = ws {
            if cfg.default_binding.as_deref() == Some(binding.alias.as_str())
                || cfg.default_binding.as_deref() == Some(binding.id.as_str())
            {
                PinSource::Dir {
                    path: path.display().to_string(),
                }
            } else {
                PinSource::Explicit
            }
        } else {
            PinSource::Explicit
        };

        let ttl = binding
            .policy
            .max_ttl
            .as_deref()
            .map(parse_ttl)
            .transpose()?
            .unwrap_or_else(|| Duration::hours(8));

        let key = self.seal_key()?;
        let worker_home = self
            .home
            .join("workers")
            .join(format!("pending-{}", binding.alias));
        // session id not known yet — create after
        let mut session = Session::new(
            &binding.id,
            &binding.alias,
            &binding.tenant,
            binding.principal.clone(),
            source,
            client,
            ttl,
            worker_home.display().to_string(),
            &key,
        );
        // Fix worker home to real session id
        let worker_home = self.home.join("workers").join(&session.session_id);
        fs::create_dir_all(&worker_home)?;
        // Private CLI config dirs (never touch global ~/.config/gh etc.)
        fs::create_dir_all(worker_home.join("gh"))?;
        fs::create_dir_all(worker_home.join("aws"))?;
        session.worker_home = worker_home.display().to_string();

        // Re-seal is not needed — worker_home not in seal material. Good.

        let path = self.active_session_path();
        fs::write(&path, serde_json::to_string_pretty(&session)?)?;
        self.audit(
            "session.pin",
            &binding.alias,
            Some(serde_json::json!({
                "session_id": session.session_id,
                "tenant": session.tenant,
                "cwd": cwd.display().to_string(),
            })),
        )?;
        Ok(session)
    }

    /// Pin using workspace default if no alias given.
    pub fn pin_auto(&self, cwd: &Path, client: Option<String>, force: bool) -> Result<Session> {
        let alias = find_workspace(cwd)
            .and_then(|(_, c)| c.default_binding)
            .ok_or_else(|| {
                LocusError::msg(
                    "no binding specified and no default_binding in .locus.toml — try `locus pin <alias>`",
                )
            })?;
        self.pin(&alias, cwd, client, force)
    }

    pub fn active_session(&self) -> Result<Option<Session>> {
        let path = self.active_session_path();
        if !path.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(&path)?;
        let session: Session = serde_json::from_str(&raw)?;
        Ok(Some(session))
    }

    pub fn require_active(&self) -> Result<Session> {
        let key = self.seal_key()?;
        match self.active_session()? {
            None => Err(LocusError::NotPinned),
            Some(s) => {
                s.verify(&key)?;
                Ok(s)
            }
        }
    }

    pub fn leave(&self) -> Result<Option<Session>> {
        let path = self.active_session_path();
        if !path.exists() {
            return Ok(None);
        }
        let session = self.active_session()?;
        if let Some(ref s) = session {
            // Best-effort cleanup of worker home
            let wh = PathBuf::from(&s.worker_home);
            if wh.exists() && wh.starts_with(self.home.join("workers")) {
                let _ = fs::remove_dir_all(&wh);
            }
            self.audit(
                "session.leave",
                &s.binding_alias,
                Some(serde_json::json!({ "session_id": s.session_id })),
            )?;
        }
        fs::remove_file(path)?;
        Ok(session)
    }

    // ── Whoami / isolation surface ────────────────────────────────────────

    /// Public identity snapshot for the active pin — never secrets.
    pub fn whoami(&self) -> Result<Whoami> {
        let session = self.require_active()?;
        let binding = self.load_binding(&session.binding_alias)?;
        Ok(Whoami {
            session_id: session.session_id,
            binding_alias: session.binding_alias,
            binding_id: session.binding_id,
            tenant: session.tenant,
            principal: session.principal,
            providers: binding
                .providers
                .iter()
                .map(|p| ProviderView {
                    provider: p.provider.clone(),
                    account: p.account.clone(),
                    credential_ref: p.credential_ref.clone(),
                    project_ref: p.scope.project_ref.clone(),
                    team_id: p.scope.team_id.clone(),
                    account_id: p.scope.account_id.clone(),
                    read_only: p.scope.read_only,
                    orgs: p.scope.orgs.clone(),
                })
                .collect(),
            expires_at: session.expires_at.to_rfc3339(),
            worker_home: session.worker_home,
            seal_ok: true,
        })
    }

    /// Continuous identity check: re-load active session + binding and report drift.
    ///
    /// Intended for future whoami heartbeats (agents / prompt hooks). Never returns secrets.
    /// Returns `Ok` with a populated [`RuntimeDrift`] even when unpinned (drift flags set).
    pub fn verify_runtime(&self) -> Result<RuntimeDrift> {
        let key = self.seal_key()?;
        let mut drift = RuntimeDrift {
            pinned: false,
            seal_ok: false,
            binding_present: false,
            binding_id_match: false,
            tenant_match: false,
            expired: false,
            session_id: None,
            binding_alias: None,
            binding_id_session: None,
            binding_id_file: None,
            tenant_session: None,
            tenant_file: None,
            providers: Vec::new(),
            issues: Vec::new(),
            ok: false,
        };

        let Some(session) = self.active_session()? else {
            drift.issues.push("not_pinned".into());
            return Ok(drift);
        };

        drift.pinned = true;
        drift.session_id = Some(session.session_id.clone());
        drift.binding_alias = Some(session.binding_alias.clone());
        drift.binding_id_session = Some(session.binding_id.clone());
        drift.tenant_session = Some(session.tenant.clone());
        drift.expired = session.is_expired();

        match session.verify(&key) {
            Ok(()) => drift.seal_ok = true,
            Err(LocusError::InvalidSeal) => {
                drift.seal_ok = false;
                drift.issues.push("invalid_seal".into());
            }
            Err(LocusError::SessionExpired(_)) => {
                // verify() checks seal then expiry; if we got here seal was ok
                drift.seal_ok = true;
                drift.issues.push("session_expired".into());
            }
            Err(e) => {
                drift.issues.push(format!("session_verify: {e}"));
            }
        }
        if drift.expired && !drift.issues.iter().any(|i| i == "session_expired") {
            drift.issues.push("session_expired".into());
        }

        match self.load_binding(&session.binding_alias) {
            Ok(binding) => {
                drift.binding_present = true;
                drift.binding_id_file = Some(binding.id.clone());
                drift.tenant_file = Some(binding.tenant.clone());
                drift.binding_id_match = binding.id == session.binding_id;
                drift.tenant_match = binding.tenant == session.tenant;
                if !drift.binding_id_match {
                    drift.issues.push("binding_id_drift".into());
                }
                if !drift.tenant_match {
                    drift.issues.push("tenant_drift".into());
                }
                drift.providers = binding
                    .providers
                    .iter()
                    .map(|p| ProviderView {
                        provider: p.provider.clone(),
                        account: p.account.clone(),
                        credential_ref: p.credential_ref.clone(),
                        project_ref: p.scope.project_ref.clone(),
                        team_id: p.scope.team_id.clone(),
                        account_id: p.scope.account_id.clone(),
                        read_only: p.scope.read_only,
                        orgs: p.scope.orgs.clone(),
                    })
                    .collect();
            }
            Err(_) => {
                drift.issues.push("binding_missing".into());
            }
        }

        drift.ok = drift.pinned
            && drift.seal_ok
            && drift.binding_present
            && drift.binding_id_match
            && drift.tenant_match
            && !drift.expired
            && drift.issues.is_empty();
        Ok(drift)
    }

    // ── Audit ─────────────────────────────────────────────────────────────

    pub fn audit(&self, op: &str, binding: &str, detail: Option<serde_json::Value>) -> Result<()> {
        use std::io::Write;
        let event = serde_json::json!({
            "ts": chrono::Utc::now().to_rfc3339(),
            "op": op,
            "binding": binding,
            "detail": detail,
        });
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.audit_path())?;
        writeln!(f, "{event}")?;
        Ok(())
    }

    /// Read all audit events (jsonl). Corrupt lines are skipped.
    pub fn read_audit_events(&self) -> Result<Vec<AuditEvent>> {
        let path = self.audit_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let raw = fs::read_to_string(path)?;
        let mut out = Vec::new();
        for line in raw.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(ev) = serde_json::from_str::<AuditEvent>(line) {
                out.push(ev);
            }
        }
        Ok(out)
    }

    // ── Approvals ─────────────────────────────────────────────────────────

    fn approval_path(&self, id: &str) -> PathBuf {
        self.approvals_dir().join(format!("{id}.json"))
    }

    fn write_approval(&self, rec: &ApprovalRecord) -> Result<()> {
        fs::create_dir_all(self.approvals_dir())?;
        let path = self.approval_path(&rec.id);
        fs::write(&path, serde_json::to_string_pretty(rec)?)?;
        Ok(())
    }

    /// Load a single approval by id (`appr_…`).
    pub fn load_approval(&self, id: &str) -> Result<ApprovalRecord> {
        let path = self.approval_path(id);
        if !path.exists() {
            return Err(LocusError::msg(format!("approval not found: {id}")));
        }
        let raw = fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    /// All approval records on disk (any status). Corrupt files are skipped.
    pub fn list_approvals(&self) -> Result<Vec<ApprovalRecord>> {
        let dir = self.approvals_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let mut entries: Vec<_> = fs::read_dir(&dir)?.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for ent in entries {
            let path = ent.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let raw = match fs::read_to_string(&path) {
                Ok(r) => r,
                Err(_) => continue,
            };
            if let Ok(rec) = serde_json::from_str::<ApprovalRecord>(&raw) {
                out.push(rec);
            }
        }
        Ok(out)
    }

    /// Pending tool calls blocked by `require_approval` (for `locus approve list`).
    ///
    /// Reads `$LOCUS_HOME/approvals/*.json` with `status=pending`, newest first.
    pub fn pending_approvals(&self) -> Result<Vec<ApprovalRecord>> {
        let mut pending: Vec<_> = self
            .list_approvals()?
            .into_iter()
            .filter(|r| r.status == ApprovalStatus::Pending)
            .collect();
        pending.sort_by_key(|b| std::cmp::Reverse(b.created_at));
        Ok(pending)
    }

    /// Create or reuse a pending approval for this tool call fingerprint.
    ///
    /// If a pending record already exists for the same tool + binding +
    /// args_digest, returns it (stable id across retries).
    pub fn create_pending_approval(
        &self,
        tool: &str,
        binding: &str,
        args: &Value,
        session_id: &str,
    ) -> Result<ApprovalRecord> {
        let digest = args_digest(args);
        for rec in self.list_approvals()? {
            if rec.status == ApprovalStatus::Pending
                && rec.matches_call(tool, binding, &digest)
            {
                return Ok(rec);
            }
        }
        let rec = ApprovalRecord {
            id: mint_approval_id(),
            tool: tool.into(),
            binding: binding.into(),
            args_digest: digest,
            created_at: Utc::now(),
            status: ApprovalStatus::Pending,
            session_id: session_id.into(),
            expires_at: None,
            granted_at: None,
        };
        self.write_approval(&rec)?;
        self.audit(
            "approval.pending",
            binding,
            Some(serde_json::json!({
                "id": rec.id,
                "tool": rec.tool,
                "args_digest": rec.args_digest,
                "session_id": rec.session_id,
                "status": "pending",
            })),
        )?;
        Ok(rec)
    }

    /// Mark approval granted until `now + ttl` (default 15m).
    pub fn grant_approval(&self, id: &str, ttl: Option<Duration>) -> Result<ApprovalRecord> {
        let mut rec = self.load_approval(id)?;
        if rec.status == ApprovalStatus::Denied {
            return Err(LocusError::msg(format!(
                "approval {id} was denied — request a new one"
            )));
        }
        let ttl = ttl.unwrap_or_else(default_grant_ttl);
        let now = Utc::now();
        rec.status = ApprovalStatus::Approved;
        rec.granted_at = Some(now);
        rec.expires_at = Some(now + ttl);
        self.write_approval(&rec)?;
        self.audit(
            "approval.grant",
            &rec.binding,
            Some(serde_json::json!({
                "id": rec.id,
                "tool": rec.tool,
                "args_digest": rec.args_digest,
                "expires_at": rec.expires_at.map(|t| t.to_rfc3339()),
                "status": "approved",
            })),
        )?;
        Ok(rec)
    }

    /// Mark approval denied (terminal).
    pub fn deny_approval(&self, id: &str) -> Result<ApprovalRecord> {
        let mut rec = self.load_approval(id)?;
        rec.status = ApprovalStatus::Denied;
        rec.expires_at = None;
        rec.granted_at = None;
        self.write_approval(&rec)?;
        self.audit(
            "approval.deny",
            &rec.binding,
            Some(serde_json::json!({
                "id": rec.id,
                "tool": rec.tool,
                "status": "denied",
            })),
        )?;
        Ok(rec)
    }

    /// Find a still-valid approved grant matching tool + binding + args_digest.
    pub fn find_valid_grant(
        &self,
        tool: &str,
        binding: &str,
        args: &Value,
    ) -> Result<Option<ApprovalRecord>> {
        let digest = args_digest(args);
        for rec in self.list_approvals()? {
            if rec.is_valid_grant() && rec.matches_call(tool, binding, &digest) {
                return Ok(Some(rec));
            }
        }
        Ok(None)
    }

    /// Validate an explicit `approval_id` for a gated call.
    ///
    /// Requires status=approved, unexpired, and tool+binding match.
    /// Args digest must also match (grant is for that fingerprint).
    pub fn check_approval_id(
        &self,
        id: &str,
        tool: &str,
        binding: &str,
        args: &Value,
    ) -> Result<ApprovalRecord> {
        let rec = self.load_approval(id)?;
        if !rec.is_valid_grant() {
            return Err(LocusError::msg(format!(
                "approval {id} is not a valid grant (status={}, expired={})",
                rec.status.as_str(),
                rec.expires_at
                    .map(|e| (Utc::now() > e).to_string())
                    .unwrap_or_else(|| "n/a".into())
            )));
        }
        let digest = args_digest(args);
        if !rec.matches_call(tool, binding, &digest) {
            return Err(LocusError::msg(format!(
                "approval {id} does not match this call (tool/binding/args_digest)"
            )));
        }
        Ok(rec)
    }

    pub fn workspace_for(&self, cwd: &Path) -> Option<(PathBuf, WorkspaceConfig)> {
        find_workspace(cwd)
    }
}

/// One line from `$LOCUS_HOME/audit/events.jsonl`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AuditEvent {
    pub ts: String,
    pub op: String,
    pub binding: String,
    #[serde(default)]
    pub detail: Option<serde_json::Value>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Whoami {
    pub session_id: String,
    pub binding_alias: String,
    pub binding_id: String,
    pub tenant: String,
    pub principal: Option<String>,
    pub providers: Vec<ProviderView>,
    pub expires_at: String,
    pub worker_home: String,
    pub seal_ok: bool,
}

/// Result of [`Store::verify_runtime`] — continuous identity / drift check.
///
/// Never contains secret values. Safe for agent-facing heartbeats.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RuntimeDrift {
    pub pinned: bool,
    pub seal_ok: bool,
    pub binding_present: bool,
    pub binding_id_match: bool,
    pub tenant_match: bool,
    pub expired: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding_alias: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding_id_session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding_id_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant_session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant_file: Option<String>,
    pub providers: Vec<ProviderView>,
    /// Machine-readable issue tags (e.g. `invalid_seal`, `tenant_drift`).
    pub issues: Vec<String>,
    /// True only when pin is present, sealed, unexpired, and binding matches.
    pub ok: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProviderView {
    pub provider: String,
    pub account: String,
    pub credential_ref: String,
    pub project_ref: Option<String>,
    pub team_id: Option<String>,
    pub account_id: Option<String>,
    pub read_only: Option<bool>,
    pub orgs: Vec<String>,
}

pub fn locus_home() -> Result<PathBuf> {
    if let Ok(h) = std::env::var("LOCUS_HOME") {
        return Ok(PathBuf::from(h));
    }
    let home = dirs::home_dir().ok_or_else(|| LocusError::msg("cannot resolve home directory"))?;
    Ok(home.join(".locus"))
}

fn write_secret_file(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::write(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(path, perms)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::{BindingBody, Policy, ProviderBinding, Scope};
    use tempfile::tempdir;

    fn sample_binding(alias: &str, tenant: &str, project: &str) -> Binding {
        Binding::from_body(BindingBody {
            id: format!("bnd_{alias}"),
            alias: alias.into(),
            tenant: tenant.into(),
            principal: Some("mason".into()),
            description: Some(format!("{tenant} work")),
            policy: Policy {
                max_ttl: Some("1h".into()),
                ..Policy::default()
            },
            providers: vec![
                ProviderBinding {
                    provider: "supabase".into(),
                    account: format!("{alias}-db"),
                    credential_ref: format!("phm:SUPABASE_{}", alias.to_uppercase()),
                    scope: Scope {
                        project_ref: Some(project.into()),
                        read_only: Some(false),
                        ..Scope::default()
                    },
                },
                ProviderBinding {
                    provider: "github".into(),
                    account: format!("{alias}-gh"),
                    credential_ref: format!("phm:GH_{}", alias.to_uppercase()),
                    scope: Scope {
                        orgs: vec![tenant.into()],
                        ..Scope::default()
                    },
                },
            ],
        })
    }

    #[test]
    fn isolation_pin_switches_providers() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "proj_acme"))
            .unwrap();
        store
            .save_binding(&sample_binding("personal", "personal", "proj_me"))
            .unwrap();

        let s1 = store
            .pin("acme", dir.path(), Some("test".into()), false)
            .unwrap();
        assert_eq!(s1.tenant, "acme-corp");
        let w1 = store.whoami().unwrap();
        assert_eq!(w1.binding_alias, "acme");
        assert_eq!(
            w1.providers
                .iter()
                .find(|p| p.provider == "supabase")
                .unwrap()
                .project_ref
                .as_deref(),
            Some("proj_acme")
        );
        // Only ACME credential refs
        for p in &w1.providers {
            assert!(
                p.credential_ref.to_uppercase().contains("ACME"),
                "leaked ref: {}",
                p.credential_ref
            );
        }

        let s2 = store
            .pin("personal", dir.path(), Some("test".into()), false)
            .unwrap();
        assert_eq!(s2.tenant, "personal");
        let w2 = store.whoami().unwrap();
        assert_eq!(w2.binding_alias, "personal");
        for p in &w2.providers {
            assert!(
                p.credential_ref.to_uppercase().contains("PERSONAL"),
                "cross-binding leak: {}",
                p.credential_ref
            );
        }
        // Acme project must not be visible
        assert!(w2
            .providers
            .iter()
            .all(|p| p.project_ref.as_deref() != Some("proj_acme")));
    }

    #[test]
    fn workspace_allowlist_blocks() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path().join("locus-home")).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        store
            .save_binding(&sample_binding("personal", "personal", "p2"))
            .unwrap();

        let project = dir.path().join("client-acme");
        fs::create_dir_all(&project).unwrap();
        fs::write(
            project.join(".locus.toml"),
            r#"
version = 1
default_binding = "acme"
allowed_bindings = ["acme"]
"#,
        )
        .unwrap();

        store.pin("acme", &project, None, false).unwrap();
        let err = store.pin("personal", &project, None, false).unwrap_err();
        assert!(matches!(err, LocusError::BindingNotAllowed(_)));
        // force overrides
        store.pin("personal", &project, None, true).unwrap();
    }

    #[test]
    fn leave_clears_pin() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        store.pin("acme", dir.path(), None, false).unwrap();
        assert!(store.require_active().is_ok());
        store.leave().unwrap();
        assert!(matches!(
            store.require_active().unwrap_err(),
            LocusError::NotPinned
        ));
    }

    #[test]
    fn seal_rejects_tamper() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        store.pin("acme", dir.path(), None, false).unwrap();
        // Tamper active session
        let path = store.active_session_path();
        let mut s: Session = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        s.binding_id = "bnd_evil".into();
        fs::write(&path, serde_json::to_string(&s).unwrap()).unwrap();
        assert!(matches!(
            store.require_active().unwrap_err(),
            LocusError::InvalidSeal
        ));
    }

    #[test]
    fn verify_runtime_ok_when_pinned() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        store.pin("acme", dir.path(), None, false).unwrap();
        let d = store.verify_runtime().unwrap();
        assert!(d.ok);
        assert!(d.pinned);
        assert!(d.seal_ok);
        assert!(d.binding_id_match);
        assert!(d.tenant_match);
        assert!(!d.expired);
        assert!(d.issues.is_empty());
        assert_eq!(d.binding_alias.as_deref(), Some("acme"));
    }

    #[test]
    fn verify_runtime_detects_unpinned() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let d = store.verify_runtime().unwrap();
        assert!(!d.ok);
        assert!(!d.pinned);
        assert!(d.issues.iter().any(|i| i == "not_pinned"));
    }

    #[test]
    fn verify_runtime_detects_binding_id_drift() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let mut b = sample_binding("acme", "acme-corp", "p1");
        store.save_binding(&b).unwrap();
        store.pin("acme", dir.path(), None, false).unwrap();
        // Rewrite binding file with a different id (alias same)
        b.id = "bnd_mutated".into();
        store.save_binding(&b).unwrap();
        let d = store.verify_runtime().unwrap();
        assert!(!d.ok);
        assert!(d.pinned);
        assert!(d.seal_ok);
        assert!(!d.binding_id_match);
        assert!(d.issues.iter().any(|i| i == "binding_id_drift"));
    }

    #[test]
    fn approval_grant_flow() {
        use crate::adapters::{call_tool_gated, ApprovalGate};
        use crate::approval::ApprovalStatus;
        use serde_json::json;

        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let mut b = sample_binding("acme", "acme-corp", "p1");
        b.policy.require_approval = vec!["*.delete*".into()];
        store.save_binding(&b).unwrap();
        let session = store.pin("acme", dir.path(), None, false).unwrap();
        let binding = store.load_binding("acme").unwrap();
        let args = json!({ "table": "users" });
        let gate = ApprovalGate {
            store: &store,
            session_id: &session.session_id,
        };

        // 1) First call creates pending, blocks
        let r1 = call_tool_gated(&binding, "supabase.table.delete", &args, Some(gate)).unwrap();
        assert!(!r1.ok);
        assert_eq!(
            r1.content.get("error").and_then(|v| v.as_str()),
            Some("requires_approval")
        );
        let approval_id = r1
            .content
            .get("approval_id")
            .and_then(|v| v.as_str())
            .expect("approval_id in response")
            .to_string();
        assert!(approval_id.starts_with("appr_"));

        let pending = store.pending_approvals().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, approval_id);
        assert_eq!(pending[0].status, ApprovalStatus::Pending);
        assert_eq!(pending[0].tool, "supabase.table.delete");
        assert_eq!(pending[0].binding, "acme");
        assert!(!pending[0].args_digest.is_empty());

        // 2) Retry without grant reuses same id
        let r2 = call_tool_gated(&binding, "supabase.table.delete", &args, Some(gate)).unwrap();
        assert!(!r2.ok);
        assert_eq!(
            r2.content.get("approval_id").and_then(|v| v.as_str()),
            Some(approval_id.as_str())
        );
        assert_eq!(store.pending_approvals().unwrap().len(), 1);

        // 3) confirm=true without grant still blocks
        let r3 = call_tool_gated(
            &binding,
            "supabase.table.delete",
            &json!({ "table": "users", "confirm": true }),
            Some(gate),
        )
        .unwrap();
        assert!(!r3.ok);

        // 4) Grant
        let granted = store.grant_approval(&approval_id, None).unwrap();
        assert_eq!(granted.status, ApprovalStatus::Approved);
        assert!(granted.expires_at.is_some());
        assert!(granted.is_valid_grant());
        assert!(store.pending_approvals().unwrap().is_empty());

        // 5) Same args within TTL — allowed (no confirm needed)
        let r5 = call_tool_gated(&binding, "supabase.table.delete", &args, Some(gate)).unwrap();
        assert!(r5.ok, "expected allow after grant: {:?}", r5.content);

        // 6) Different args still need approval
        let r6 = call_tool_gated(
            &binding,
            "supabase.table.delete",
            &json!({ "table": "orders" }),
            Some(gate),
        )
        .unwrap();
        assert!(!r6.ok);
        let other_id = r6
            .content
            .get("approval_id")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        assert_ne!(other_id, approval_id);

        // 7) deny
        store.deny_approval(&other_id).unwrap();
        let denied = store.load_approval(&other_id).unwrap();
        assert_eq!(denied.status, ApprovalStatus::Denied);
        assert!(store.grant_approval(&other_id, None).is_err());

        // 8) confirm=true + approval_id path (new grant)
        let r8 = call_tool_gated(
            &binding,
            "supabase.table.delete",
            &json!({ "table": "payments" }),
            Some(gate),
        )
        .unwrap();
        let id8 = r8
            .content
            .get("approval_id")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        store.grant_approval(&id8, None).unwrap();
        let r8b = call_tool_gated(
            &binding,
            "supabase.table.delete",
            &json!({
                "table": "payments",
                "confirm": true,
                "approval_id": id8,
            }),
            Some(gate),
        )
        .unwrap();
        assert!(r8b.ok, "approval_id path: {:?}", r8b.content);
    }

    #[test]
    fn approval_expired_grant_blocks() {
        use crate::adapters::{call_tool_gated, ApprovalGate};
        use serde_json::json;

        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let mut b = sample_binding("acme", "acme-corp", "p1");
        b.policy.require_approval = vec!["*.delete*".into()];
        store.save_binding(&b).unwrap();
        let session = store.pin("acme", dir.path(), None, false).unwrap();
        let binding = store.load_binding("acme").unwrap();
        let args = json!({ "table": "users" });
        let gate = ApprovalGate {
            store: &store,
            session_id: &session.session_id,
        };

        let r = call_tool_gated(&binding, "supabase.table.delete", &args, Some(gate)).unwrap();
        let id = r
            .content
            .get("approval_id")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        // Grant then force expires_at into the past
        let mut rec = store
            .grant_approval(&id, Some(Duration::minutes(15)))
            .unwrap();
        rec.expires_at = Some(Utc::now() - Duration::seconds(5));
        let path = store.approvals_dir().join(format!("{id}.json"));
        fs::write(&path, serde_json::to_string_pretty(&rec).unwrap()).unwrap();

        let r2 = call_tool_gated(&binding, "supabase.table.delete", &args, Some(gate)).unwrap();
        assert!(!r2.ok);
        assert_eq!(
            r2.content.get("error").and_then(|v| v.as_str()),
            Some("requires_approval")
        );
    }
}

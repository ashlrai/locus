//! Local control-plane store under `~/.locus/` (or `LOCUS_HOME`).

use crate::approval::{
    args_digest, mint_approval_id, validate_approval_id, ApprovalAuthority, ApprovalRecord,
    ApprovalStatus,
};
use crate::authority_anchor::{self, ValidationMode};
use crate::autopin::{self, AutoPinTarget};
use crate::binding::{validate_name_component, Binding, BindingSummary};
use crate::config::{self, LocusConfig};
use crate::credential::{credential_metadata, CredentialMetadata};
use crate::engagement::{
    self, client_binding_template, close_checklist, engagement_readme, EngagementCloseResult,
    EngagementMeta,
};
use crate::error::{LocusError, Result};
use crate::graph::{
    decrypt_graph, encrypt_graph, source_host, GraphEnvelope, GraphExportResult, GraphImportResult,
    GraphListEntry, GraphMeta, WorkspaceTemplate,
};
use crate::seal::SealKey;
use crate::session::{
    binding_fingerprint, parse_ttl, PinSource, Session, SessionAuthority, SessionBackingType,
    SessionMode,
};
use crate::ticket::{self, CapabilityTicket};
use crate::workspace::{find_workspace, WorkspaceConfig};
use chrono::{Duration, Utc};
use rand::RngCore;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

/// Layout:
/// ```text
/// $LOCUS_HOME/
///   config.toml
///   daemon.key          # seal key hex (0600)
///   bindings/*.toml
///   workspaces/*.toml   # workspace templates for graph share
///   sessions/active.json
///   workers/<session_id>/
///   audit/events.jsonl
///   approvals/{id}.json
///   engagements/<alias>.json
///   archives/<alias>-<date>.jsonl
///   trust/adapter-keys.toml  # adapter registry trust pins (0600; lazy)
/// ```
#[derive(Debug, Clone)]
pub struct Store {
    home: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ResolvedSession {
    pub session: Session,
    pub path: PathBuf,
    pub backing_type: SessionBackingType,
}

impl Store {
    pub fn open_default() -> Result<Self> {
        let home = locus_home()?;
        Self::open(home)
    }

    pub fn open(home: impl Into<PathBuf>) -> Result<Self> {
        let home = home.into();
        fs::create_dir_all(&home)?;
        let home = fs::canonicalize(home)?;
        fs::create_dir_all(home.join("bindings"))?;
        fs::create_dir_all(home.join("workspaces"))?;
        fs::create_dir_all(home.join("sessions"))?;
        fs::create_dir_all(home.join("workers"))?;
        fs::create_dir_all(home.join("audit"))?;
        fs::create_dir_all(home.join("approvals"))?;
        fs::create_dir_all(home.join("engagements"))?;
        fs::create_dir_all(home.join("archives"))?;
        fs::create_dir_all(home.join("runtime"))?;
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

    pub fn workspaces_dir(&self) -> PathBuf {
        self.home.join("workspaces")
    }

    pub fn approvals_dir(&self) -> PathBuf {
        self.home.join("approvals")
    }

    pub fn engagements_dir(&self) -> PathBuf {
        self.home.join("engagements")
    }

    pub fn archives_dir(&self) -> PathBuf {
        self.home.join("archives")
    }

    pub fn config_path(&self) -> PathBuf {
        self.home.join("config.toml")
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

    /// Load `$LOCUS_HOME/config.toml` (defaults if missing).
    pub fn load_config(&self) -> LocusConfig {
        config::load_config(&self.home)
    }

    /// Persist config.toml.
    pub fn save_config(&self, cfg: &LocusConfig) -> Result<PathBuf> {
        self.require_local_control("save config")?;
        config::save_config(&self.home, cfg)
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

    fn issue_session_authority(
        &self,
        session: &Session,
    ) -> Result<crate::session::SessionAuthorityAnchor> {
        let backing_type = session
            .backing
            .as_ref()
            .map(|backing| backing.backing_type)
            .ok_or(LocusError::InvalidSeal)?;
        authority_anchor::issue(
            &self.home,
            &session.session_id,
            backing_type.as_str(),
            &session.authority_subject_digest(),
        )
    }

    fn validate_session_authority(&self, session: &Session) -> Result<ValidationMode> {
        session.verify_authority_shape()?;
        let lease = session
            .authority_anchor
            .as_ref()
            .ok_or(LocusError::LegacySessionSeal)?;
        let backing_type = session
            .backing
            .as_ref()
            .map(|backing| backing.backing_type)
            .ok_or(LocusError::InvalidSeal)?;
        authority_anchor::validate(
            &self.home,
            lease,
            &session.session_id,
            backing_type.as_str(),
            &session.authority_subject_digest(),
        )
    }

    fn revoke_session_authority(&self, session: &Session) -> Result<()> {
        let Some(lease) = session.authority_anchor.as_ref() else {
            return Ok(());
        };
        let Some(backing_type) = session.backing.as_ref().map(|backing| backing.backing_type)
        else {
            return Ok(());
        };
        authority_anchor::revoke(
            &self.home,
            lease,
            &session.session_id,
            backing_type.as_str(),
        )
    }

    pub fn grant_executor_capability(&self, session: &Session) -> Result<String> {
        session.verify_authority_shape()?;
        let lease = session
            .authority_anchor
            .as_ref()
            .ok_or(LocusError::LegacySessionSeal)?;
        let backing_type = session
            .backing
            .as_ref()
            .map(|backing| backing.backing_type)
            .ok_or(LocusError::InvalidSeal)?;
        authority_anchor::grant_executor(
            &self.home,
            lease,
            &session.session_id,
            backing_type.as_str(),
            &session.authority_subject_digest(),
        )
    }

    /// Authenticate the operator control capability and, when a session is
    /// selected, verify its exact sealed backing and live broker generation.
    pub fn require_local_control(&self, operation: &str) -> Result<()> {
        authority_anchor::authorize_control(&self.home)?;
        let Some(resolved) = self.resolve_active_session()? else {
            return Ok(());
        };
        resolved.session.verify_seal(&self.seal_key()?)?;
        self.validate_session_authority(&resolved.session)?;
        if resolved.session.authority != SessionAuthority::LocalControl {
            return Err(LocusError::msg(format!(
                "{operation} requires an authenticated local-control session"
            )));
        }
        Ok(())
    }

    // ── Bindings ──────────────────────────────────────────────────────────

    pub fn save_binding(&self, binding: &Binding) -> Result<PathBuf> {
        self.require_local_control("save binding")?;
        binding.validate()?;
        // Double-check alias cannot escape bindings/ (also enforced in validate)
        validate_name_component("alias", &binding.alias)?;
        // Reserved prefix: the MCP gate never routes `locus*__tool` names
        // (session::split_namespaced_tool), so a `locus*` alias would be
        // silently unreachable from agents. Fail closed at create/import;
        // legacy hand-written files are flagged by doctor (`reserved_alias`).
        if binding.alias.starts_with("locus") {
            return Err(LocusError::msg(format!(
                "alias '{}' is reserved: aliases starting with 'locus' collide with the control-tool \
                 namespace and cannot be routed through the MCP gate — choose a different alias",
                binding.alias
            )));
        }
        let _lock = crate::credential_migration::lock_bindings(self)?;
        let path = self.bindings_dir().join(format!("{}.toml", binding.alias));
        ensure_under_dir(&self.bindings_dir(), &path)?;
        fs::write(&path, binding.to_toml()?)?;
        self.audit("binding.save", &binding.alias, None)?;
        Ok(path)
    }

    pub fn load_binding(&self, alias_or_id: &str) -> Result<Binding> {
        // Reject path traversal before any filesystem join
        if alias_or_id.contains('/')
            || alias_or_id.contains('\\')
            || alias_or_id.contains("..")
            || alias_or_id.contains('\0')
        {
            return Err(LocusError::msg(format!(
                "invalid binding name '{alias_or_id}': path separators and '..' are not allowed"
            )));
        }
        // Prefer alias filename (alias charset validated on save; still constrain join)
        let bindings_lock = crate::credential_migration::lock_bindings(self)?;
        let by_alias = self.bindings_dir().join(format!("{alias_or_id}.toml"));
        if by_alias.exists() {
            ensure_under_dir(&self.bindings_dir(), &by_alias)?;
            let raw = fs::read_to_string(&by_alias)
                .map_err(|_| LocusError::msg(format!("binding '{alias_or_id}' is unreadable")))?;
            let binding = parse_binding_safely(&raw, alias_or_id)?;
            validate_loaded_binding(&binding, alias_or_id)?;
            return Ok(binding);
        }
        drop(bindings_lock);
        // Scan for id match
        for b in self.list_bindings()? {
            if b.id == alias_or_id || b.alias == alias_or_id {
                return self.load_binding(&b.alias);
            }
        }
        Err(LocusError::BindingNotFound(alias_or_id.into()))
    }

    pub fn list_bindings(&self) -> Result<Vec<BindingSummary>> {
        let _lock = crate::credential_migration::lock_bindings(self)?;
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
            let label = path
                .file_stem()
                .and_then(|s| s.to_str())
                .filter(|s| validate_name_component("alias", s).is_ok())
                .unwrap_or("unknown");
            let raw = fs::read_to_string(&path)
                .map_err(|_| LocusError::msg(format!("binding '{label}' is unreadable")))?;
            let binding = parse_binding_safely(&raw, label)?;
            validate_loaded_binding(&binding, label)?;
            out.push(BindingSummary::from(&binding));
        }
        Ok(out)
    }

    /// Explicitly convert conservative legacy bare Phantom names to `phm:NAME`.
    /// Dry-run is the default; unsafe values require manual editing and are never echoed.
    pub fn migrate_legacy_credential_refs(
        &self,
        alias: &str,
        write: bool,
    ) -> Result<CredentialRefMigration> {
        if write {
            self.require_local_control("migrate binding credential references")?;
        }
        crate::credential_migration::migrate(self, alias, write)
    }

    pub fn remove_binding(&self, alias: &str) -> Result<()> {
        self.require_local_control("remove binding")?;
        validate_name_component("alias", alias)?;
        let _lock = crate::credential_migration::lock_bindings(self)?;
        let path = self.bindings_dir().join(format!("{alias}.toml"));
        ensure_under_dir(&self.bindings_dir(), &path)?;
        if !path.exists() {
            return Err(LocusError::BindingNotFound(alias.into()));
        }
        fs::remove_file(path)?;
        self.audit("binding.remove", alias, None)?;
        Ok(())
    }

    // ── Workspace templates (graph share surface) ─────────────────────────

    /// Persist a named workspace template under `$LOCUS_HOME/workspaces/`.
    pub fn save_workspace_template(&self, name: &str, cfg: &WorkspaceConfig) -> Result<PathBuf> {
        self.require_local_control("save workspace template")?;
        validate_name_component("workspace name", name)?;
        fs::create_dir_all(self.workspaces_dir())?;
        let path = self.workspaces_dir().join(format!("{name}.toml"));
        ensure_under_dir(&self.workspaces_dir(), &path)?;
        fs::write(&path, cfg.to_toml()?)?;
        Ok(path)
    }

    /// Load one workspace template by name.
    pub fn load_workspace_template(&self, name: &str) -> Result<WorkspaceConfig> {
        validate_name_component("workspace name", name)?;
        let path = self.workspaces_dir().join(format!("{name}.toml"));
        ensure_under_dir(&self.workspaces_dir(), &path)?;
        if !path.exists() {
            return Err(LocusError::msg(format!(
                "workspace template not found: {name}"
            )));
        }
        let raw = fs::read_to_string(&path)?;
        WorkspaceConfig::parse(&raw)
    }

    /// List workspace templates as `(name, config)`.
    pub fn list_workspace_templates(&self) -> Result<Vec<WorkspaceTemplate>> {
        let mut out = Vec::new();
        let dir = self.workspaces_dir();
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
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if name.is_empty() {
                continue;
            }
            let raw = fs::read_to_string(&path)?;
            match WorkspaceConfig::parse(&raw) {
                Ok(config) => out.push(WorkspaceTemplate { name, config }),
                Err(_) => continue,
            }
        }
        Ok(out)
    }

    // ── Binding graph export / import ─────────────────────────────────────

    /// List the shareable local graph surface (bindings + workspace templates).
    pub fn graph_list(&self) -> Result<Vec<GraphListEntry>> {
        let mut out = Vec::new();
        for summary in self.list_bindings()? {
            let binding = self.load_binding(&summary.alias)?;
            out.push(GraphListEntry {
                kind: "binding".into(),
                name: binding.alias.clone(),
                tenant: Some(binding.tenant.clone()),
                description: binding.description.clone(),
                providers: binding
                    .providers
                    .iter()
                    .map(|p| p.provider.clone())
                    .collect(),
                credentials: binding
                    .providers
                    .iter()
                    .map(|p| crate::credential::credential_metadata(&p.credential_ref))
                    .collect(),
                default_binding: None,
                allowed_bindings: Vec::new(),
            });
        }
        for ws in self.list_workspace_templates()? {
            out.push(GraphListEntry {
                kind: "workspace".into(),
                name: ws.name,
                tenant: None,
                description: None,
                providers: Vec::new(),
                credentials: Vec::new(),
                default_binding: ws.config.default_binding,
                allowed_bindings: ws.config.allowed_bindings,
            });
        }
        Ok(out)
    }

    /// Export selected (or all) bindings + workspace templates to an encrypted `.locusgraph` file.
    ///
    /// Only CredentialRefs are written — never resolved secret values.
    pub fn graph_export(
        &self,
        aliases: Option<&[String]>,
        out: &Path,
        passphrase: &str,
    ) -> Result<GraphExportResult> {
        self.require_local_control("export binding graph")?;
        let summaries = self.list_bindings()?;
        let wanted: Vec<String> = match aliases {
            None | Some([]) => summaries.iter().map(|s| s.alias.clone()).collect(),
            Some(list) => list.to_vec(),
        };

        let mut bindings = Vec::new();
        for alias in &wanted {
            let b = self.load_binding(alias)?;
            bindings.push(b);
        }
        if bindings.is_empty() {
            return Err(LocusError::msg(
                "no bindings to export — add bindings first or pass --bindings",
            ));
        }

        // Include workspace templates that reference any exported alias (or all if exporting everything)
        let export_all = aliases.map(|a| a.is_empty()).unwrap_or(true) || aliases.is_none();
        let exported_aliases: std::collections::BTreeSet<_> =
            bindings.iter().map(|b| b.alias.as_str()).collect();
        let mut workspaces = Vec::new();
        for ws in self.list_workspace_templates()? {
            let include = export_all
                || ws
                    .config
                    .default_binding
                    .as_deref()
                    .is_some_and(|d| exported_aliases.contains(d))
                || ws
                    .config
                    .allowed_bindings
                    .iter()
                    .any(|a| exported_aliases.contains(a.as_str()))
                || exported_aliases.contains(ws.name.as_str());
            if include {
                workspaces.push(ws);
            }
        }

        let meta = GraphMeta {
            source_host: source_host(),
            locus_version: Some(crate::VERSION.into()),
        };
        let envelope = GraphEnvelope::build(bindings, workspaces, meta)?;
        let plain = envelope.to_json_bytes()?;
        let encrypted = encrypt_graph(&plain, passphrase)?;

        if let Some(parent) = out.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        fs::write(out, &encrypted)?;

        let binding_aliases: Vec<String> =
            envelope.bindings.iter().map(|b| b.alias.clone()).collect();
        let workspace_names: Vec<String> =
            envelope.workspaces.iter().map(|w| w.name.clone()).collect();

        self.audit(
            "graph.export",
            binding_aliases.first().map(|s| s.as_str()).unwrap_or("-"),
            Some(serde_json::json!({
                "path": out.display().to_string(),
                "bindings": binding_aliases,
                "workspaces": workspace_names,
                "exported_at": envelope.exported_at,
            })),
        )?;

        Ok(GraphExportResult {
            path: out.display().to_string(),
            binding_aliases,
            workspace_names,
            exported_at: envelope.exported_at,
        })
    }

    /// Import an encrypted `.locusgraph` file: validate + `save_binding` each entry.
    ///
    /// Without `force`, existing bindings / workspace templates are skipped (reported).
    /// With `force`, they are overwritten.
    pub fn graph_import(
        &self,
        path: &Path,
        passphrase: &str,
        force: bool,
    ) -> Result<GraphImportResult> {
        self.require_local_control("import binding graph")?;
        let file_bytes = fs::read(path)
            .map_err(|e| LocusError::msg(format!("read graph file {}: {e}", path.display())))?;
        let plain = decrypt_graph(&file_bytes, passphrase)?;
        let envelope = GraphEnvelope::from_json_bytes(&plain)?;

        let mut bindings_imported = Vec::new();
        let mut bindings_skipped = Vec::new();
        for body in &envelope.bindings {
            let binding = Binding::from_body(body.clone());
            binding.validate()?;
            let exists = self
                .bindings_dir()
                .join(format!("{}.toml", binding.alias))
                .exists();
            if exists && !force {
                bindings_skipped.push(binding.alias.clone());
                continue;
            }
            // save_binding audits binding.save — also track graph.import at end
            self.save_binding(&binding)?;
            bindings_imported.push(binding.alias.clone());
        }

        let mut workspaces_imported = Vec::new();
        let mut workspaces_skipped = Vec::new();
        for ws in &envelope.workspaces {
            validate_name_component("workspace name", &ws.name)?;
            let exists = self
                .workspaces_dir()
                .join(format!("{}.toml", ws.name))
                .exists();
            if exists && !force {
                workspaces_skipped.push(ws.name.clone());
                continue;
            }
            self.save_workspace_template(&ws.name, &ws.config)?;
            workspaces_imported.push(ws.name.clone());
        }

        self.audit(
            "graph.import",
            bindings_imported
                .first()
                .or(bindings_skipped.first())
                .map(|s| s.as_str())
                .unwrap_or("-"),
            Some(serde_json::json!({
                "path": path.display().to_string(),
                "bindings_imported": bindings_imported,
                "bindings_skipped": bindings_skipped,
                "workspaces_imported": workspaces_imported,
                "workspaces_skipped": workspaces_skipped,
                "force": force,
                "source_exported_at": envelope.exported_at,
            })),
        )?;

        Ok(GraphImportResult {
            bindings_imported,
            bindings_skipped,
            workspaces_imported,
            workspaces_skipped,
            source_host: envelope.meta.source_host,
            exported_at: Some(envelope.exported_at),
        })
    }

    // ── Sessions ──────────────────────────────────────────────────────────

    pub fn pin(
        &self,
        alias_or_id: &str,
        cwd: &Path,
        client: Option<String>,
        force: bool,
    ) -> Result<Session> {
        self.pin_with_ttl(alias_or_id, cwd, client, force, None)
    }

    /// [`Store::pin`] with an explicit auto-leave TTL request (`locus enter
    /// --ttl`). The request is capped by the binding's `policy.max_ttl` —
    /// never extended (fail closed).
    pub fn pin_with_ttl(
        &self,
        alias_or_id: &str,
        cwd: &Path,
        client: Option<String>,
        force: bool,
        ttl: Option<Duration>,
    ) -> Result<Session> {
        self.pin_with_opts(alias_or_id, cwd, client, force, None, true, ttl)
    }

    /// Experimental namespaced multi-binding pin.
    ///
    /// `aliases` must contain at least two distinct binding aliases. Tools are
    /// exposed as `alias__toolname` in locus-mcp. Primary (first) alias owns
    /// whoami tenant display and seal binding_id.
    pub fn pin_namespaced(
        &self,
        aliases: &[String],
        cwd: &Path,
        client: Option<String>,
        force: bool,
    ) -> Result<Session> {
        self.pin_namespaced_with_ttl(aliases, cwd, client, force, None)
    }

    /// [`Store::pin_namespaced`] with an explicit auto-leave TTL request,
    /// capped by the primary binding's `policy.max_ttl`.
    pub fn pin_namespaced_with_ttl(
        &self,
        aliases: &[String],
        cwd: &Path,
        client: Option<String>,
        force: bool,
        ttl: Option<Duration>,
    ) -> Result<Session> {
        if aliases.len() < 2 {
            return Err(LocusError::msg(
                "namespaced pin requires at least two bindings (e.g. `locus pin --ns a,b`)",
            ));
        }
        let mut seen = Vec::new();
        for a in aliases {
            let t = a.trim();
            if t.is_empty() {
                continue;
            }
            if !seen.iter().any(|x: &String| x == t) {
                seen.push(t.to_string());
            }
        }
        if seen.len() < 2 {
            return Err(LocusError::msg(
                "namespaced pin requires at least two distinct bindings",
            ));
        }
        let primary = seen[0].clone();
        let rest = seen[1..].to_vec();
        self.pin_with_opts(&primary, cwd, client, force, Some(rest), true, ttl)
    }

    /// Create a session for `locus run` — sealed temporary pin that does **not**
    /// overwrite `active.json` unless `share_pin` is true.
    ///
    /// Session file is written to `sessions/run-<suffix>.json` (caller supplies
    /// a unique suffix, typically process pid).
    pub fn create_run_session(
        &self,
        alias_or_id: &str,
        cwd: &Path,
        client: Option<String>,
        force: bool,
        share_pin: bool,
        run_suffix: &str,
    ) -> Result<(Session, PathBuf)> {
        let run_path = self.run_session_path(run_suffix);
        let backing_type = if share_pin {
            SessionBackingType::Active
        } else {
            SessionBackingType::Run
        };
        let backing_path = if share_pin {
            self.active_session_path()
        } else {
            run_path.clone()
        };
        let session = self.pin_with_opts_source(
            alias_or_id,
            cwd,
            client.or_else(|| Some("run".into())),
            force,
            None,
            share_pin,
            backing_type,
            Some(backing_path.clone()),
            Some(PinSource::Run),
            None,
            SessionAuthority::LocalControl,
        )?;
        // When share_pin is false, pin_with_opts still built the session but did
        // not write active.json — write run session file.
        if !share_pin {
            self.write_session_file(&run_path, &session)?;
        }
        self.audit(
            "session.run",
            &session.binding_alias,
            Some(serde_json::json!({
                "session_id": session.session_id,
                "share_pin": share_pin,
                "run_path": run_path.display().to_string(),
            })),
        )?;
        Ok((session, backing_path))
    }

    pub fn run_session_path(&self, suffix: &str) -> PathBuf {
        self.home
            .join("sessions")
            .join(format!("run-{}.json", sanitize_session_suffix(suffix)))
    }

    /// Remove a temporary run session file (best-effort worker cleanup).
    pub fn cleanup_run_session(&self, path: &Path, session: &Session) -> Result<()> {
        self.cleanup_named_session(path, session, "run-")
    }

    /// Create a CI / ephemeral sealed session under `sessions/ci-<id>.json`.
    ///
    /// Does **not** touch `active.json`. Workspace allowlist is enforced unless
    /// `force`. TTL is capped by the binding's `policy.max_ttl` when set.
    ///
    /// Audit op: `ci.mint`.
    pub fn create_ci_session(
        &self,
        alias_or_id: &str,
        cwd: &Path,
        force: bool,
        ttl: Option<Duration>,
    ) -> Result<(Session, PathBuf)> {
        let session = self.pin_with_opts_source(
            alias_or_id,
            cwd,
            Some("ci".into()),
            force,
            None,
            false, // never write active.json
            SessionBackingType::Ci,
            None,
            Some(PinSource::Ci),
            ttl,
            SessionAuthority::Delegated,
        )?;
        // Prefer session_id tail as file suffix so LOCUS_SESSION_ID can find it.
        let suffix = session
            .session_id
            .strip_prefix("ses_")
            .unwrap_or(&session.session_id);
        let ci_path = self.ci_session_path(suffix);
        self.write_session_file(&ci_path, &session)?;
        self.audit(
            "ci.mint",
            &session.binding_alias,
            Some(serde_json::json!({
                "session_id": session.session_id,
                "expires_at": session.expires_at.to_rfc3339(),
                "path": ci_path.display().to_string(),
            })),
        )?;
        Ok((session, ci_path))
    }

    pub fn ci_session_path(&self, suffix: &str) -> PathBuf {
        self.home
            .join("sessions")
            .join(format!("ci-{}.json", sanitize_session_suffix(suffix)))
    }

    /// Remove a temporary CI session file (best-effort worker cleanup).
    pub fn cleanup_ci_session(&self, path: &Path, session: &Session) -> Result<()> {
        self.cleanup_named_session(path, session, "ci-")
    }

    /// Best-effort worker-home removal, hardened against traversal and
    /// symlink escape: the recorded path must *canonicalize* to a location
    /// strictly under this store's `workers/` root or it is left untouched
    /// (fail closed — a forged `worker_home` with `..` components or a
    /// symlink pointing outside the store must never be followed into a
    /// recursive delete). Removal failures are swallowed: cleanup is
    /// best-effort and must never block a teardown.
    fn remove_worker_home_if_safe(&self, worker_home: &str) {
        let wh = PathBuf::from(worker_home);
        if !wh.exists() {
            return;
        }
        let Ok(workers_root) = self.home.join("workers").canonicalize() else {
            return;
        };
        let Ok(canonical) = wh.canonicalize() else {
            return;
        };
        // Strictly under the root: never the root itself, never a sibling
        // that merely shares a textual prefix (component-wise comparison).
        if canonical != workers_root && canonical.starts_with(&workers_root) {
            let _ = fs::remove_dir_all(&canonical);
        }
    }

    fn cleanup_named_session(&self, path: &Path, session: &Session, prefix: &str) -> Result<()> {
        self.revoke_session_authority(session)?;
        self.remove_worker_home_if_safe(&session.worker_home);
        if path.exists()
            && path.starts_with(self.home.join("sessions"))
            && path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(prefix))
        {
            let _ = fs::remove_file(path);
        }
        Ok(())
    }

    // ── MCP multi-tenant grants ──────────────────────────────────────────

    /// Directory holding MCP multi-tenant grant records
    /// (`mcp-grants/<grant_id>.json`, 0600 — token HMAC only, never secrets).
    pub fn mcp_grants_dir(&self) -> PathBuf {
        self.home.join("mcp-grants")
    }

    fn mcp_grant_path(&self, grant_id: &str) -> Option<PathBuf> {
        if !is_safe_mcp_grant_id(grant_id) {
            return None;
        }
        Some(self.mcp_grants_dir().join(format!("{grant_id}.json")))
    }

    fn mcp_grant_token_material(grant_id: &str, secret: &str) -> String {
        format!("mcp-grant:{grant_id}:{secret}")
    }

    /// Mint a multi-tenant MCP grant: a sealed delegated CI-backed session
    /// (never touches `active.json`) plus an opaque bearer token
    /// `lmt_<grant_id>.<secret>`. Only the seal-key HMAC of the token is
    /// stored at rest; the returned token is printed exactly once by the CLI.
    ///
    /// Audit op: `mcp.grant_mint` (grant_id / session_id / alias / expiry only).
    pub fn create_mcp_grant(
        &self,
        alias_or_id: &str,
        cwd: &Path,
        ttl: Option<Duration>,
        label: Option<String>,
        force: bool,
    ) -> Result<(Session, McpGrant, String)> {
        let (session, _ci_path) = self.create_ci_session(alias_or_id, cwd, force, ttl)?;
        let mut id_bytes = [0u8; 8];
        rand::thread_rng().fill_bytes(&mut id_bytes);
        let grant_id = hex::encode(id_bytes);
        let mut secret_bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut secret_bytes);
        let secret = hex::encode(secret_bytes);
        let token = format!("lmt_{grant_id}.{secret}");
        let token_seal = self
            .seal_key()?
            .seal(&Self::mcp_grant_token_material(&grant_id, &secret));
        let grant = McpGrant {
            v: 1,
            grant_id: grant_id.clone(),
            token_seal,
            session_id: session.session_id.clone(),
            binding_alias: session.binding_alias.clone(),
            tenant: session.tenant.clone(),
            created_at: Utc::now().to_rfc3339(),
            expires_at: session.expires_at,
            label,
            revoked: false,
        };
        fs::create_dir_all(self.mcp_grants_dir())?;
        let path = self
            .mcp_grant_path(&grant_id)
            .ok_or_else(|| LocusError::msg("invalid grant id"))?;
        // Fail closed on grant_id collision: never overwrite an existing
        // grant record (create_new / O_EXCL semantics).
        write_secret_file_new(&path, serde_json::to_string_pretty(&grant)?.as_bytes()).map_err(
            |e| {
                LocusError::msg(format!(
                    "refusing to mint MCP grant `{grant_id}`: {e} (existing grant file is never overwritten)"
                ))
            },
        )?;
        self.audit(
            "mcp.grant_mint",
            &grant.binding_alias,
            Some(serde_json::json!({
                "grant_id": grant.grant_id,
                "session_id": grant.session_id,
                "tenant": grant.tenant,
                "expires_at": grant.expires_at.to_rfc3339(),
                "label": grant.label,
            })),
        )?;
        Ok((session, grant, token))
    }

    /// Load a grant record by public grant id. Corrupt / mismatched files
    /// read as unknown (fail closed).
    pub fn load_mcp_grant(&self, grant_id: &str) -> Result<Option<McpGrant>> {
        let Some(path) = self.mcp_grant_path(grant_id) else {
            return Ok(None);
        };
        if !path.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(&path)?;
        let grant: McpGrant = match serde_json::from_str(&raw) {
            Ok(g) => g,
            Err(_) => return Ok(None),
        };
        if grant.v != 1 || grant.grant_id != grant_id {
            return Ok(None);
        }
        Ok(Some(grant))
    }

    /// All grant records (operator surface: `locus mcp list`, doctor).
    pub fn list_mcp_grants(&self) -> Result<Vec<McpGrant>> {
        let dir = self.mcp_grants_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if !is_safe_mcp_grant_id(stem) {
                continue;
            }
            if let Some(grant) = self.load_mcp_grant(stem)? {
                out.push(grant);
            }
        }
        out.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(out)
    }

    /// Verify an opaque tenant bearer token `lmt_<grant_id>.<secret>`.
    ///
    /// Uniform `Invalid` on parse failure, unknown grant, MAC mismatch, or
    /// revocation — never reveals which check failed. `Expired` is only
    /// returned AFTER the HMAC proved possession of the secret (safe to hint
    /// a re-mint to the legitimate holder).
    pub fn verify_mcp_grant_token(
        &self,
        token: &str,
    ) -> std::result::Result<McpGrant, McpGrantAuthError> {
        let Some((grant_id, secret)) = parse_mcp_grant_token(token) else {
            return Err(McpGrantAuthError::Invalid {
                grant_id: None,
                grant_dead: false,
            });
        };
        let invalid = |grant_dead: bool| McpGrantAuthError::Invalid {
            grant_id: Some(grant_id.to_string()),
            grant_dead,
        };
        let grant = match self.load_mcp_grant(grant_id) {
            Ok(Some(g)) => g,
            // Missing / corrupt record: definitively dead (revoke deletes the
            // file) — safe for the server to sweep this grant's sessions.
            Ok(None) => return Err(invalid(true)),
            // Transient IO error: fail closed but do NOT declare death.
            Err(_) => return Err(invalid(false)),
        };
        let Ok(key) = self.seal_key() else {
            return Err(invalid(false));
        };
        // Constant-time MAC compare (SealKey::verify).
        if !key.verify(
            &Self::mcp_grant_token_material(grant_id, secret),
            &grant.token_seal,
        ) {
            return Err(invalid(false));
        }
        if grant.revoked {
            // MAC proved possession; the grant is definitively revoked.
            return Err(invalid(true));
        }
        if grant.is_expired() {
            return Err(McpGrantAuthError::Expired {
                grant: Box::new(grant),
            });
        }
        Ok(grant)
    }

    /// Revoke a grant: mark + delete the grant file, then tear down the
    /// backing sealed session (authority revoke + session file + hardened
    /// worker-home removal via [`Store::cleanup_ci_session`]).
    ///
    /// Audit op: `mcp.grant_revoke`.
    pub fn revoke_mcp_grant(&self, grant_id: &str) -> Result<Option<McpGrant>> {
        let Some(grant) = self.load_mcp_grant(grant_id)? else {
            return Ok(None);
        };
        let mut revoked = grant.clone();
        revoked.revoked = true;
        if let Some(path) = self.mcp_grant_path(grant_id) {
            // Mark revoked first so a crash between steps still fails closed,
            // then delete the record.
            let _ = write_secret_file(&path, serde_json::to_string_pretty(&revoked)?.as_bytes());
            let _ = fs::remove_file(&path);
        }
        if let Ok(Some(resolved)) = self.load_session_by_id_resolved(&grant.session_id) {
            let _ = self.cleanup_ci_session(&resolved.path, &resolved.session);
        }
        self.audit(
            "mcp.grant_revoke",
            &grant.binding_alias,
            Some(serde_json::json!({
                "grant_id": grant.grant_id,
                "session_id": grant.session_id,
            })),
        )?;
        Ok(Some(revoked))
    }

    /// Load a session by `session_id` from `sessions/*.json` (active, run-*, ci-*).
    pub fn load_session_by_id(&self, session_id: &str) -> Result<Option<Session>> {
        Ok(self
            .load_session_by_id_resolved(session_id)?
            .map(|resolved| resolved.session))
    }

    pub fn load_session_by_id_resolved(&self, session_id: &str) -> Result<Option<ResolvedSession>> {
        let Some(path) = self.session_path_by_id(session_id)? else {
            return Ok(None);
        };
        let backing_type = self.backing_type_for_path(&path)?;
        let session = self.read_session_file(&path, backing_type)?;
        Ok(
            (session.session_id == session_id).then_some(ResolvedSession {
                session,
                path: self.canonical_session_path(&path)?,
                backing_type,
            }),
        )
    }

    fn session_path_by_id(&self, session_id: &str) -> Result<Option<PathBuf>> {
        if session_id.is_empty()
            || session_id.contains('/')
            || session_id.contains('\\')
            || session_id.contains("..")
            || session_id.contains('\0')
        {
            return Ok(None);
        }
        let dir = self.home.join("sessions");
        if !dir.exists() {
            return Ok(None);
        }
        // Fast path: ci-<id without ses_>
        if let Some(tail) = session_id.strip_prefix("ses_") {
            let ci = self.ci_session_path(tail);
            if ci.exists() {
                let raw = fs::read_to_string(&ci)?;
                let s: Session = serde_json::from_str(&raw)?;
                if s.session_id == session_id {
                    return Ok(Some(ci));
                }
            }
        }
        // active.json
        if let Some(s) = self.read_active_session_file()? {
            if s.session_id == session_id {
                return Ok(Some(self.active_session_path()));
            }
        }
        // Scan remaining session files
        for ent in fs::read_dir(&dir)?.filter_map(|e| e.ok()) {
            let path = ent.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let raw = match fs::read_to_string(&path) {
                Ok(r) => r,
                Err(_) => continue,
            };
            if let Ok(s) = serde_json::from_str::<Session>(&raw) {
                if s.session_id == session_id {
                    return Ok(Some(path));
                }
            }
        }
        Ok(None)
    }

    /// Internal pin builder. When `write_active` is false, builds a sealed
    /// session + worker home but leaves `active.json` untouched.
    #[allow(clippy::too_many_arguments)]
    fn pin_with_opts(
        &self,
        alias_or_id: &str,
        cwd: &Path,
        client: Option<String>,
        force: bool,
        extra_namespaces: Option<Vec<String>>,
        write_active: bool,
        ttl_override: Option<Duration>,
    ) -> Result<Session> {
        self.pin_with_opts_source(
            alias_or_id,
            cwd,
            client,
            force,
            extra_namespaces,
            write_active,
            SessionBackingType::Active,
            Some(self.active_session_path()),
            None,
            ttl_override,
            SessionAuthority::LocalControl,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn pin_with_opts_source(
        &self,
        alias_or_id: &str,
        cwd: &Path,
        client: Option<String>,
        force: bool,
        extra_namespaces: Option<Vec<String>>,
        write_active: bool,
        backing_type: SessionBackingType,
        backing_path: Option<PathBuf>,
        source_override: Option<PinSource>,
        ttl_override: Option<Duration>,
        authority: SessionAuthority,
    ) -> Result<Session> {
        self.require_local_control("mint or replace session")?;
        let binding = self.load_binding(alias_or_id)?;
        let ws = find_workspace(cwd)?;

        if let Some((_, ref cfg)) = ws {
            if !cfg.allows(&binding.alias) && !cfg.allows(&binding.id) && !force {
                return Err(LocusError::BindingNotAllowed(binding.alias.clone()));
            }
        }

        // Validate + fingerprint extra namespaces (namespaced mode).
        let mut ns_aliases = Vec::new();
        let mut ns_fps = Vec::new();
        if let Some(extra) = extra_namespaces {
            for alias in extra {
                let b = self.load_binding(&alias)?;
                if let Some((_, ref cfg)) = ws {
                    if !cfg.allows(&b.alias) && !cfg.allows(&b.id) && !force {
                        return Err(LocusError::BindingNotAllowed(b.alias.clone()));
                    }
                }
                if b.alias == binding.alias {
                    continue;
                }
                ns_aliases.push(b.alias.clone());
                ns_fps.push(binding_fingerprint(&b));
            }
        }

        let source = if let Some(src) = source_override {
            src
        } else if client.as_deref() == Some("run") {
            PinSource::Run
        } else if client.as_deref() == Some("ci") {
            PinSource::Ci
        } else if let Some((ref path, ref cfg)) = ws {
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

        let max_ttl = binding
            .policy
            .max_ttl
            .as_deref()
            .map(parse_ttl)
            .transpose()?
            .unwrap_or_else(|| Duration::hours(8));
        let default_ttl = binding
            .policy
            .default_ttl
            .as_deref()
            .map(parse_ttl)
            .transpose()?;
        // Precedence: explicit request (--ttl / CI mint) > binding
        // policy.default_ttl > policy.max_ttl. The winner is always capped by
        // max_ttl (fail closed on over-long pins).
        let (requested, ttl_source) = match (ttl_override, default_ttl) {
            (Some(req), _) => (Some(req), "flag"),
            (None, Some(d)) => (Some(d), "binding_default"),
            (None, None) => (None, "max_ttl"),
        };
        let ttl = match requested {
            Some(req) if req <= max_ttl => req,
            _ => max_ttl,
        };

        let key = self.seal_key()?;
        let worker_home = self
            .home
            .join("workers")
            .join(format!("pending-{}", binding.alias));
        let mut session = Session::new_with_authority(
            &binding.id,
            &binding.alias,
            &binding.tenant,
            binding.principal.clone(),
            source,
            authority,
            client,
            ttl,
            worker_home.display().to_string(),
            &key,
        );
        let worker_home = self.home.join("workers").join(&session.session_id);
        fs::create_dir_all(&worker_home)?;
        fs::create_dir_all(worker_home.join("gh"))?;
        fs::create_dir_all(worker_home.join("aws"))?;
        session.worker_home = worker_home.display().to_string();
        session.binding_fp = Some(binding_fingerprint(&binding));

        if !ns_aliases.is_empty() {
            session.mode = SessionMode::Namespaced;
            session.namespaces = ns_aliases;
            session.namespace_fps = ns_fps;
        }
        let backing_path = match backing_path {
            Some(path) => path,
            None if backing_type == SessionBackingType::Ci => {
                let suffix = session
                    .session_id
                    .strip_prefix("ses_")
                    .unwrap_or(&session.session_id);
                self.ci_session_path(suffix)
            }
            None => return Err(LocusError::InvalidSeal),
        };
        let canonical_backing = self.canonical_session_path(&backing_path)?;
        session.set_backing(backing_type, &canonical_backing)?;
        session.authority_anchor = Some(self.issue_session_authority(&session)?);
        session.reseal(&key);

        if write_active {
            let path = self.active_session_path();
            self.write_session_file(&path, &session)?;
            self.audit(
                "session.pin",
                &binding.alias,
                Some(serde_json::json!({
                    "session_id": session.session_id,
                    "tenant": session.tenant,
                    "cwd": cwd.display().to_string(),
                    "mode": match session.mode {
                        SessionMode::Exclusive => "exclusive",
                        SessionMode::Namespaced => "namespaced",
                    },
                    "namespaces": session.namespaces,
                    "ttl_secs": ttl.num_seconds(),
                    "ttl_source": ttl_source,
                })),
            )?;
        }
        Ok(session)
    }

    fn write_session_file(&self, path: &Path, session: &Session) -> Result<()> {
        let backing_type = session
            .backing
            .as_ref()
            .map(|backing| backing.backing_type)
            .ok_or(LocusError::InvalidSeal)?;
        session.verify_backing(backing_type, &self.canonical_session_path(path)?)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_string_pretty(session)?)?;
        Ok(())
    }

    fn canonical_session_path(&self, path: &Path) -> Result<PathBuf> {
        let sessions = fs::canonicalize(self.home.join("sessions"))?;
        let parent = path
            .parent()
            .ok_or_else(|| LocusError::msg("session path has no parent"))?;
        if fs::canonicalize(parent)? != sessions {
            return Err(LocusError::InvalidSeal);
        }
        let file_name = path.file_name().ok_or(LocusError::InvalidSeal)?;
        Ok(sessions.join(file_name))
    }

    fn backing_type_for_path(&self, path: &Path) -> Result<SessionBackingType> {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(LocusError::InvalidSeal)?;
        if name == "active.json" {
            Ok(SessionBackingType::Active)
        } else if name.starts_with("run-") && name.ends_with(".json") {
            Ok(SessionBackingType::Run)
        } else if name.starts_with("ci-") && name.ends_with(".json") {
            Ok(SessionBackingType::Ci)
        } else {
            Err(LocusError::InvalidSeal)
        }
    }

    fn read_session_file(&self, path: &Path, backing_type: SessionBackingType) -> Result<Session> {
        let canonical = self.canonical_session_path(path)?;
        let raw = fs::read_to_string(&canonical)?;
        let session: Session = serde_json::from_str(&raw)?;
        session.verify_backing(backing_type, &canonical)?;
        Ok(session)
    }

    fn save_session_at(&self, path: &Path, session: &Session) -> Result<()> {
        let mut sealed = session.clone();
        sealed.reseal(&self.seal_key()?);
        self.write_session_file(path, &sealed)
    }

    /// Resolve bare pin target: workspace `default_binding`, then opt-in git remote autopin.
    pub fn resolve_auto_pin(&self, cwd: &Path) -> Result<AutoPinTarget> {
        autopin::resolve_auto_pin(cwd, &self.home)
    }

    /// Pin using workspace default or (if enabled) git remote autopin.
    ///
    /// Autopin never uses `force` — allowlist blocks are skipped at resolve time.
    pub fn pin_auto(&self, cwd: &Path, client: Option<String>, force: bool) -> Result<Session> {
        self.pin_auto_with_ttl(cwd, client, force, None)
    }

    /// [`Store::pin_auto`] with an explicit auto-leave TTL request, capped by
    /// the resolved binding's `policy.max_ttl`.
    pub fn pin_auto_with_ttl(
        &self,
        cwd: &Path,
        client: Option<String>,
        force: bool,
        ttl: Option<Duration>,
    ) -> Result<Session> {
        let target = self.resolve_auto_pin(cwd)?;
        let use_force = match &target.source {
            PinSource::Autopin { .. } => false,
            _ => force,
        };
        self.pin_with_opts_source(
            &target.alias,
            cwd,
            client,
            use_force,
            None,
            true,
            SessionBackingType::Active,
            Some(self.active_session_path()),
            Some(target.source),
            ttl,
            SessionAuthority::LocalControl,
        )
    }

    /// Auto-pin requested by an agent/MCP runtime — always refused.
    ///
    /// Workspace `.locus.toml` defaults and the `LOCUS_AUTO_PIN` /
    /// `LOCUS_MCP_AUTO_PIN` knobs are advisory hints only: the workspace file
    /// is repo-local (agent-writable), and executor authority is bound to an
    /// operator-supervised launch of a specific session generation, so an
    /// agent-facing process can neither prove operator intent nor validate a
    /// session it minted for itself. Until an explicit operator-delegation
    /// design exists, this fails closed with an honest error instead of a
    /// transient-looking "unavailable".
    pub fn pin_auto_delegated(
        &self,
        _cwd: &Path,
        _client: Option<String>,
        _force: bool,
    ) -> Result<Session> {
        Err(LocusError::msg(
            "auto-pin requires operator delegation, which is not available: an agent/MCP \
             process cannot self-issue session authority (workspace `.locus.toml` defaults \
             are advisory hints only) — a human must run `locus enter <alias>` or \
             `locus pin <alias>`",
        ))
    }

    /// Read `sessions/active.json` only (ignores `LOCUS_SESSION_ID`).
    fn read_active_session_file(&self) -> Result<Option<Session>> {
        let path = self.active_session_path();
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(
            self.read_session_file(&path, SessionBackingType::Active)?,
        ))
    }

    /// Resolve the current pin.
    ///
    /// When `LOCUS_SESSION_ID` is set (CI mint / child of `ci run`), that
    /// session is used **exclusively** — fail closed if missing (no fallthrough
    /// to `active.json`). Otherwise load `sessions/active.json`.
    pub fn resolve_active_session(&self) -> Result<Option<ResolvedSession>> {
        // In tests, serialize with cases that mutate LOCUS_SESSION_ID
        // (re-entrant for the same thread so holders can call require_active).
        #[cfg(test)]
        let _env_guard = tests::SessionEnvLock::acquire();

        if let Ok(id) = std::env::var("LOCUS_SESSION_ID") {
            let id = id.trim();
            if !id.is_empty() {
                return self.load_session_by_id_resolved(id);
            }
        }
        let path = self.active_session_path();
        if !path.exists() {
            return Ok(None);
        }
        let session = self.read_session_file(&path, SessionBackingType::Active)?;
        Ok(Some(ResolvedSession {
            session,
            path: self.canonical_session_path(&path)?,
            backing_type: SessionBackingType::Active,
        }))
    }

    pub fn active_session(&self) -> Result<Option<Session>> {
        Ok(self
            .resolve_active_session()?
            .map(|resolved| resolved.session))
    }

    pub fn require_active(&self) -> Result<Session> {
        let key = self.seal_key()?;
        match self.resolve_active_session()? {
            None => Err(LocusError::NotPinned),
            Some(resolved) => {
                resolved.session.verify(&key)?;
                self.validate_session_authority(&resolved.session)?;
                Ok(resolved.session)
            }
        }
    }

    /// Like [`Store::require_active`] but for an explicit sealed session id
    /// (CI mints / MCP tenant grants). Fails closed on unknown id, invalid
    /// seal, expiry, or missing session authority. Frozen sessions still
    /// return so callers can gate on drift.
    pub fn require_session_by_id(&self, session_id: &str) -> Result<Session> {
        let key = self.seal_key()?;
        match self.load_session_by_id_resolved(session_id)? {
            None => Err(LocusError::NotPinned),
            Some(resolved) => {
                resolved.session.verify(&key)?;
                self.validate_session_authority(&resolved.session)?;
                Ok(resolved.session)
            }
        }
    }

    /// Like [`require_active`] but ignores freeze (seal + expiry only).
    /// Used by doctor / whoami reporting so frozen pins remain inspectable.
    pub fn require_active_allow_frozen(&self) -> Result<Session> {
        let key = self.seal_key()?;
        match self.resolve_active_session()? {
            None => Err(LocusError::NotPinned),
            Some(resolved) => {
                resolved.session.verify_seal(&key)?;
                self.validate_session_authority(&resolved.session)?;
                Ok(resolved.session)
            }
        }
    }

    pub fn leave(&self) -> Result<Option<Session>> {
        authority_anchor::authorize_control(&self.home)?;
        // Always operates on active.json — not LOCUS_SESSION_ID overrides.
        let path = self.active_session_path();
        if !path.exists() {
            return Ok(None);
        }
        let session = self.read_active_session_file()?;
        if let Some(ref s) = session {
            let key = self.seal_key()?;
            let authentic = s.seal_version == crate::session::CURRENT_SEAL_VERSION
                && s.authority != SessionAuthority::LegacyUntrusted
                && key.verify(&s.material(), &s.seal);
            if authentic {
                self.validate_session_authority(s)?;
                if s.authority != SessionAuthority::LocalControl {
                    return Err(LocusError::msg(
                        "leave active session requires authenticated local-control authority",
                    ));
                }
            }
            self.revoke_session_authority(s)?;
            // Best-effort cleanup of worker home (canonicalized containment check)
            self.remove_worker_home_if_safe(&s.worker_home);
            self.audit(
                "session.leave",
                &s.binding_alias,
                Some(serde_json::json!({ "session_id": s.session_id })),
            )?;
        }
        fs::remove_file(path)?;
        Ok(session)
    }

    /// Forcibly clear the active session even when its seal is invalid, the
    /// authority anchor is unavailable, or the supervising broker is gone
    /// (`locus leave --force`).
    ///
    /// Recovery hatch for the supervisor-anchor wedge: a pin minted under a
    /// now-dead supervisor leaves `leave` / `status` / `doctor` failing closed
    /// with no built-in teardown. This path requires the operator control
    /// capability (like normal [`leave`](Self::leave)) but never requires the
    /// wedged session to validate. The capability is authenticated against the
    /// best available verifier — the live broker when anything answers its
    /// endpoint, else the persisted operator capability; when neither exists
    /// (the `--no-persist-capability` strict posture) teardown additionally
    /// requires the explicit `allow_unverified` acknowledgement
    /// (`locus leave --force --no-verifier`). It only deletes session state —
    /// `sessions/active.json` plus the session's worker home — and audits
    /// `session.force_leave`; it never mints or refreshes any authority.
    pub fn force_leave(&self, reason: &str, allow_unverified: bool) -> Result<ForceLeaveOutcome> {
        authority_anchor::authorize_control_teardown(&self.home, allow_unverified)?;
        // Always operates on active.json — never LOCUS_SESSION_ID overrides.
        let path = self.active_session_path();
        if !path.exists() {
            return Ok(ForceLeaveOutcome {
                cleared: false,
                session_id: None,
                binding_alias: None,
                diagnosis: Vec::new(),
            });
        }
        // Best-effort parse: a corrupt file must still be removable.
        let parsed: Option<Session> = fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok());
        let mut diagnosis: Vec<String> = Vec::new();
        match parsed {
            None => diagnosis.push("unreadable_session".into()),
            Some(ref session) => {
                match self.seal_key() {
                    Err(_) => diagnosis.push("seal_key_unavailable".into()),
                    Ok(key) => match session.verify_seal(&key) {
                        Ok(()) => {}
                        Err(LocusError::SessionExpired(_)) => {
                            diagnosis.push("session_expired".into())
                        }
                        Err(LocusError::LegacySessionSeal) => diagnosis.push("legacy_seal".into()),
                        Err(_) => diagnosis.push("invalid_seal".into()),
                    },
                }
                match self.validate_session_authority(session) {
                    Ok(_) => {}
                    Err(LocusError::AuthorityAnchorMismatch) => {
                        diagnosis.push("anchor_mismatch".into())
                    }
                    Err(_) => diagnosis.push("anchor_unavailable".into()),
                }
                // Best-effort revocation + worker-home cleanup: failures must
                // not block teardown (the broker may be gone entirely). The
                // worker home is only removed when it canonicalizes to a
                // path under this store's workers/ root (fail closed).
                let _ = self.revoke_session_authority(session);
                self.remove_worker_home_if_safe(&session.worker_home);
            }
        }
        // The removal of active.json is what defines a successful force-leave
        // — perform it first, then audit what actually happened. A failed
        // removal is audited as a failure (best-effort) and surfaced.
        if let Err(remove_err) = fs::remove_file(&path) {
            let _ = self.audit(
                "session.force_leave_failed",
                parsed
                    .as_ref()
                    .map(|s| s.binding_alias.as_str())
                    .unwrap_or("-"),
                Some(serde_json::json!({
                    "session_id": parsed.as_ref().map(|s| s.session_id.clone()),
                    "reason": reason,
                    "diagnosis": diagnosis,
                    "error": remove_err.to_string(),
                })),
            );
            return Err(remove_err.into());
        }
        self.audit(
            "session.force_leave",
            parsed
                .as_ref()
                .map(|s| s.binding_alias.as_str())
                .unwrap_or("-"),
            Some(serde_json::json!({
                "session_id": parsed.as_ref().map(|s| s.session_id.clone()),
                "reason": reason,
                "diagnosis": diagnosis,
            })),
        )?;
        Ok(ForceLeaveOutcome {
            cleared: true,
            session_id: parsed.as_ref().map(|s| s.session_id.clone()),
            binding_alias: parsed.map(|s| s.binding_alias),
            diagnosis,
        })
    }

    // ── Engagements ───────────────────────────────────────────────────────

    /// Create a client engagement: binding template + sidecar meta + optional workspace/README.
    ///
    /// Does not resolve or store secrets — `phm:` stubs only. Fast path for firm onboarding.
    pub fn engagement_init(
        &self,
        alias: &str,
        tenant: &str,
        cwd: &Path,
        write_workspace: bool,
        write_readme: bool,
        force: bool,
    ) -> Result<EngagementInitResult> {
        self.require_local_control("initialize engagement")?;
        validate_name_component("alias", alias)?;
        if tenant.trim().is_empty() {
            return Err(LocusError::msg("tenant must be non-empty"));
        }

        let binding_path = self.bindings_dir().join(format!("{alias}.toml"));
        if binding_path.exists() && !force {
            return Err(LocusError::msg(format!(
                "binding '{alias}' already exists — use --force to overwrite"
            )));
        }

        let binding = client_binding_template(alias, tenant);
        let path = self.save_binding(&binding)?;

        let mut meta = EngagementMeta::open(alias, tenant);
        meta.description = Some(format!("{tenant} client engagement"));
        engagement::write_meta(&self.engagements_dir(), &meta)?;

        let mut workspace_path = None;
        if write_workspace {
            let wp = cwd.join(".locus.toml");
            if wp.exists() && !force {
                return Err(LocusError::msg(
                    ".locus.toml already exists — use --force or skip --workspace",
                ));
            }
            let cfg = WorkspaceConfig {
                version: 1,
                default_binding: Some(alias.to_string()),
                allowed_bindings: vec![alias.to_string()],
                require_pin: true,
            };
            fs::write(&wp, cfg.to_toml()?)?;
            workspace_path = Some(wp);
        }

        let mut readme_path = None;
        if write_readme {
            let locus_dir = cwd.join(".locus");
            fs::create_dir_all(&locus_dir)?;
            let rp = locus_dir.join("README.md");
            if rp.exists() && !force {
                // Non-fatal: binding already created
            } else {
                fs::write(&rp, engagement_readme(alias, tenant))?;
                readme_path = Some(rp);
            }
        }

        self.audit(
            "engagement.init",
            alias,
            Some(serde_json::json!({
                "tenant": tenant,
                "binding_path": path.display().to_string(),
                "workspace": workspace_path.as_ref().map(|p| p.display().to_string()),
            })),
        )?;

        Ok(EngagementInitResult {
            alias: alias.to_string(),
            tenant: tenant.to_string(),
            binding_path: path,
            workspace_path,
            readme_path,
            credentials: binding
                .providers
                .iter()
                .map(|p| crate::credential::credential_metadata(&p.credential_ref))
                .collect(),
        })
    }

    /// Close an engagement: leave if active, mark closed_at, optional audit archive.
    ///
    /// Does **not** delete Phantom vault secrets or the binding file.
    pub fn engagement_close(&self, alias: &str, archive: bool) -> Result<EngagementCloseResult> {
        self.require_local_control("close engagement")?;
        validate_name_component("alias", alias)?;
        // Ensure binding exists (or meta-only close of known alias)
        let binding = self.load_binding(alias).ok();
        let tenant = binding
            .as_ref()
            .map(|b| b.tenant.clone())
            .or_else(|| {
                engagement::read_meta(&self.engagements_dir(), alias)
                    .ok()
                    .flatten()
                    .map(|m| m.tenant)
            })
            .unwrap_or_else(|| alias.to_string());

        if binding.is_none() && engagement::read_meta(&self.engagements_dir(), alias)?.is_none() {
            return Err(LocusError::BindingNotFound(alias.into()));
        }

        let mut left_session = false;
        if let Some(session) = self.active_session()? {
            if session.binding_alias == alias {
                let _ = self.leave()?;
                left_session = true;
            }
        }

        let mut archive_path = None;
        if archive {
            let ap = self.archive_audit_for_binding(alias)?;
            archive_path = Some(ap.display().to_string());
        }

        let mut meta = engagement::read_meta(&self.engagements_dir(), alias)?
            .unwrap_or_else(|| EngagementMeta::open(alias, &tenant));
        meta.mark_closed(archive_path.clone());
        engagement::write_meta(&self.engagements_dir(), &meta)?;

        self.audit(
            "engagement.close",
            alias,
            Some(serde_json::json!({
                "tenant": tenant,
                "archive": archive_path,
                "left_session": left_session,
            })),
        )?;

        Ok(EngagementCloseResult {
            alias: alias.to_string(),
            tenant,
            closed_at: meta.closed_at.clone().unwrap_or_default(),
            left_session,
            archive_path,
            checklist: close_checklist(alias),
        })
    }

    /// Filter audit events for a binding into `$LOCUS_HOME/archives/<alias>-<date>.jsonl`.
    pub fn archive_audit_for_binding(&self, alias: &str) -> Result<PathBuf> {
        self.require_local_control("archive engagement audit")?;
        validate_name_component("alias", alias)?;
        let events = self.read_audit_events()?;
        let matched: Vec<_> = events.into_iter().filter(|e| e.binding == alias).collect();
        fs::create_dir_all(self.archives_dir())?;
        let date = Utc::now().format("%Y%m%d");
        let path = self.archives_dir().join(format!("{alias}-{date}.jsonl"));
        ensure_under_dir(&self.archives_dir(), &path)?;
        let mut lines = String::new();
        for e in &matched {
            lines.push_str(&serde_json::to_string(e)?);
            lines.push('\n');
        }
        fs::write(&path, lines)?;
        self.audit(
            "engagement.archive",
            alias,
            Some(serde_json::json!({
                "path": path.display().to_string(),
                "events": matched.len(),
            })),
        )?;
        Ok(path)
    }

    pub fn load_engagement_meta(&self, alias: &str) -> Result<Option<EngagementMeta>> {
        engagement::read_meta(&self.engagements_dir(), alias)
    }

    // ── Whoami / isolation surface ────────────────────────────────────────

    /// Public identity snapshot for the active pin — never secrets.
    ///
    /// Frozen sessions are still reported (with `frozen=true`) so operators can
    /// diagnose drift without a hard error.
    pub fn whoami(&self) -> Result<Whoami> {
        let session = self.require_active_allow_frozen()?;
        self.whoami_from_session(session)
    }

    /// [`Store::whoami`] for an explicit resolved session (MCP tenant grants).
    /// Same checks as the active path: seal verify + session authority
    /// validation; frozen sessions still report (`frozen=true`).
    pub fn whoami_for(&self, resolved: &ResolvedSession) -> Result<Whoami> {
        let key = self.seal_key()?;
        resolved.session.verify_seal(&key)?;
        self.validate_session_authority(&resolved.session)?;
        self.whoami_from_session(resolved.session.clone())
    }

    fn whoami_from_session(&self, session: Session) -> Result<Whoami> {
        let binding = self.load_binding(&session.binding_alias)?;
        let backing = session.backing.as_ref().ok_or(LocusError::InvalidSeal)?;
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
                    credential: credential_metadata(&p.credential_ref),
                    project_ref: p.scope.project_ref.clone(),
                    team_id: p.scope.team_id.clone(),
                    account_id: p.scope.account_id.clone(),
                    read_only: p.scope.read_only,
                    orgs: p.scope.orgs.clone(),
                    repos: p.scope.repos.clone(),
                })
                .collect(),
            expires_at: session.expires_at.to_rfc3339(),
            expires_in_secs: (session.expires_at - Utc::now()).num_seconds().max(0),
            worker_home: session.worker_home,
            seal_ok: true,
            seal: session.seal,
            authority: match session.authority {
                SessionAuthority::LocalControl => "local_control",
                SessionAuthority::Delegated => "delegated",
                SessionAuthority::LegacyUntrusted => "legacy_untrusted",
            }
            .into(),
            authority_anchor_ok: true,
            backing_type: backing.backing_type,
            backing_path: backing.canonical_path.clone(),
            frozen: session.frozen,
            frozen_reason: session.frozen_reason,
            mode: match session.mode {
                SessionMode::Exclusive => "exclusive".into(),
                SessionMode::Namespaced => "namespaced".into(),
            },
            namespaces: session.namespaces,
        })
    }

    /// Continuous identity check: re-load active session + binding and report drift.
    ///
    /// Never returns secrets. Returns `Ok` with a populated [`RuntimeDrift`] even
    /// when unpinned (drift flags set). Does **not** mutate the session (see
    /// [`check_drift_and_freeze`]).
    pub fn verify_runtime(&self) -> Result<RuntimeDrift> {
        match self.resolve_active_session()? {
            Some(resolved) => self.verify_runtime_for(&resolved),
            None => {
                let mut drift = empty_runtime_drift();
                drift.issues.push("not_pinned".into());
                Ok(drift)
            }
        }
    }

    /// [`Store::verify_runtime`] against an explicit resolved session (MCP
    /// tenant grants) — identical checks, no `active.json` involvement.
    pub fn verify_runtime_for(&self, resolved: &ResolvedSession) -> Result<RuntimeDrift> {
        let key = self.seal_key()?;
        let mut drift = empty_runtime_drift();
        let session = resolved.session.clone();

        drift.pinned = true;
        drift.backing_ok = true;
        drift.backing_type = Some(resolved.backing_type);
        drift.backing_path = Some(resolved.path.display().to_string());
        drift.authority = Some(
            match session.authority {
                SessionAuthority::LocalControl => "local_control",
                SessionAuthority::Delegated => "delegated",
                SessionAuthority::LegacyUntrusted => "legacy_untrusted",
            }
            .into(),
        );
        drift.session_id = Some(session.session_id.clone());
        drift.binding_alias = Some(session.binding_alias.clone());
        drift.binding_id_session = Some(session.binding_id.clone());
        drift.tenant_session = Some(session.tenant.clone());
        drift.expired = session.is_expired();
        drift.frozen = session.frozen;

        // Use seal-only verify so we still detect material drift on frozen pins.
        match session.verify_seal(&key) {
            Ok(()) => drift.seal_ok = true,
            Err(LocusError::InvalidSeal | LocusError::LegacySessionSeal) => {
                drift.seal_ok = false;
                drift.issues.push("invalid_seal".into());
            }
            Err(LocusError::SessionExpired(_)) => {
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
        if drift.frozen {
            drift.issues.push("session_frozen".into());
        }

        match self.validate_session_authority(&session) {
            Ok(_) => drift.authority_anchor_ok = true,
            Err(LocusError::AuthorityAnchorMismatch) => {
                drift.issues.push("authority_anchor_mismatch".into())
            }
            Err(LocusError::ExecutorAuthorityUnavailable(_)) => {
                drift.issues.push("executor_authority_unavailable".into())
            }
            Err(_) => drift.issues.push("authority_anchor_unavailable".into()),
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

                // Provider / full material fingerprint
                let fp_now = binding_fingerprint(&binding);
                if let Some(ref fp_pin) = session.binding_fp {
                    if fp_now != *fp_pin {
                        drift.providers_match = false;
                        if !drift.issues.iter().any(|i| i == "providers_drift") {
                            drift.issues.push("providers_drift".into());
                        }
                    }
                } else if !drift.binding_id_match || !drift.tenant_match {
                    // Legacy sessions without fp: id/tenant already covered
                    drift.providers_match = drift.binding_id_match && drift.tenant_match;
                }

                drift.providers = binding
                    .providers
                    .iter()
                    .map(|p| ProviderView {
                        provider: p.provider.clone(),
                        account: p.account.clone(),
                        credential: credential_metadata(&p.credential_ref),
                        project_ref: p.scope.project_ref.clone(),
                        team_id: p.scope.team_id.clone(),
                        account_id: p.scope.account_id.clone(),
                        read_only: p.scope.read_only,
                        orgs: p.scope.orgs.clone(),
                        repos: p.scope.repos.clone(),
                    })
                    .collect();
            }
            Err(_) => {
                drift.issues.push("binding_missing".into());
            }
        }

        // Namespaced secondary bindings
        for (i, alias) in session.namespaces.iter().enumerate() {
            match self.load_binding(alias) {
                Ok(b) => {
                    let fp_now = binding_fingerprint(&b);
                    let fp_pin = session.namespace_fps.get(i);
                    if fp_pin.is_some_and(|fp| fp != &fp_now) {
                        drift.providers_match = false;
                        if !drift.issues.iter().any(|i| i == "providers_drift") {
                            drift.issues.push("providers_drift".into());
                        }
                        if !drift.issues.iter().any(|i| i == "namespace_drift") {
                            drift.issues.push("namespace_drift".into());
                        }
                    }
                }
                Err(_) => {
                    drift
                        .issues
                        .push(format!("namespace_binding_missing:{alias}"));
                    drift.providers_match = false;
                }
            }
        }

        // ok only when healthy and not frozen and no drift issues
        let blocking: Vec<&str> = drift
            .issues
            .iter()
            .filter(|i| {
                *i != "session_frozen" // frozen alone means not ok, but listed separately
            })
            .map(|s| s.as_str())
            .collect();
        drift.ok = drift.pinned
            && drift.seal_ok
            && drift.authority_anchor_ok
            && drift.backing_ok
            && drift.binding_present
            && drift.binding_id_match
            && drift.tenant_match
            && drift.providers_match
            && !drift.expired
            && !drift.frozen
            && blocking.is_empty();
        Ok(drift)
    }

    /// Verify runtime identity and, if binding material drifted under the
    /// active pin, mark the session **frozen** and persist it.
    ///
    /// Frozen sessions cause privileged ops (exec, tools/call) to fail with
    /// `session_frozen: re-pin` until a human re-pins.
    pub fn check_drift_and_freeze(&self) -> Result<RuntimeDrift> {
        match self.resolve_active_session()? {
            Some(resolved) => self.check_drift_and_freeze_for(&resolved),
            None => self.verify_runtime(),
        }
    }

    /// [`Store::check_drift_and_freeze`] against an explicit resolved session
    /// (MCP tenant grants). A freeze writes ONLY the resolved session's file
    /// (`save_session_at`) — `active.json` and other grants stay untouched.
    pub fn check_drift_and_freeze_for(&self, resolved: &ResolvedSession) -> Result<RuntimeDrift> {
        let mut drift = self.verify_runtime_for(resolved)?;
        if !drift.pinned {
            return Ok(drift);
        }

        let should_freeze = drift.issues.iter().any(|i| {
            matches!(
                i.as_str(),
                "binding_id_drift"
                    | "tenant_drift"
                    | "providers_drift"
                    | "namespace_drift"
                    | "binding_missing"
            ) || i.starts_with("namespace_binding_missing:")
        });

        if should_freeze && drift.seal_ok && drift.backing_ok && drift.authority_anchor_ok {
            {
                let mut session = resolved.session.clone();
                if !session.frozen {
                    let reason = drift
                        .issues
                        .iter()
                        .find(|i| {
                            matches!(
                                i.as_str(),
                                "binding_id_drift"
                                    | "tenant_drift"
                                    | "providers_drift"
                                    | "namespace_drift"
                                    | "binding_missing"
                            ) || i.starts_with("namespace_binding_missing:")
                        })
                        .cloned()
                        .unwrap_or_else(|| "binding_drift".into());
                    session.freeze(reason.clone());
                    self.save_session_at(&resolved.path, &session)?;
                    self.audit(
                        "session.freeze",
                        &session.binding_alias,
                        Some(serde_json::json!({
                            "session_id": session.session_id,
                            "reason": reason,
                            "issues": drift.issues,
                        })),
                    )?;
                    drift.frozen = true;
                    if !drift.issues.iter().any(|i| i == "session_frozen") {
                        drift.issues.push("session_frozen".into());
                    }
                    drift.ok = false;
                }
            }
        }
        Ok(drift)
    }

    // ── Capability tickets ────────────────────────────────────────────────

    /// Mint a short-lived capability ticket (HMAC over session|binding|tool|exp).
    ///
    /// The returned `ticket_id` is safe for audit logs — it is not raw credential
    /// material. Default TTL is 30s ([`crate::ticket::DEFAULT_TICKET_TTL_SECS`]).
    pub fn mint_capability_ticket(
        &self,
        session_id: &str,
        binding_id: &str,
        tool: &str,
    ) -> Result<CapabilityTicket> {
        let key = self.seal_key()?;
        ticket::mint_ticket(&key, session_id, binding_id, tool, None)
    }

    /// Mint with an explicit TTL (tests / longer CI windows).
    pub fn mint_capability_ticket_ttl(
        &self,
        session_id: &str,
        binding_id: &str,
        tool: &str,
        ttl: Duration,
    ) -> Result<CapabilityTicket> {
        let key = self.seal_key()?;
        ticket::mint_ticket(&key, session_id, binding_id, tool, Some(ttl))
    }

    /// Verify a capability ticket against the daemon seal key (HMAC + TTL).
    pub fn verify_capability_ticket(&self, ticket: &CapabilityTicket) -> Result<()> {
        let key = self.seal_key()?;
        ticket::verify_ticket(&key, ticket)
    }

    /// Verify from discrete fields (e.g. reconstructed from audit).
    pub fn verify_capability_ticket_parts(
        &self,
        ticket_id: &str,
        session_id: &str,
        binding_id: &str,
        tool: &str,
        exp: i64,
    ) -> Result<()> {
        let key = self.seal_key()?;
        ticket::verify_ticket_parts(&key, ticket_id, session_id, binding_id, tool, exp)
    }

    // ── Audit ─────────────────────────────────────────────────────────────

    pub fn audit(&self, op: &str, binding: &str, detail: Option<serde_json::Value>) -> Result<()> {
        use std::io::{Read, Seek, SeekFrom, Write};
        let event = serde_json::json!({
            "ts": chrono::Utc::now().to_rfc3339(),
            "op": op,
            "binding": binding,
            "detail": detail.map(sanitize_audit_value),
        });
        let mut f = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(self.audit_path())?;
        f.lock()?;
        let len = f.metadata()?.len();
        if len > 0 {
            f.seek(SeekFrom::Start(len - 1))?;
            let mut tail = [0u8; 1];
            f.read_exact(&mut tail)?;
            if tail[0] != b'\n' {
                f.write_all(b"\n")?;
            }
        }
        writeln!(f, "{event}")?;
        f.sync_all()?;
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

    fn approval_path(&self, id: &str) -> Result<PathBuf> {
        validate_approval_id(id)?;
        let path = self.approvals_dir().join(format!("{id}.json"));
        ensure_under_dir(&self.approvals_dir(), &path)?;
        Ok(path)
    }

    fn write_approval(&self, rec: &ApprovalRecord) -> Result<()> {
        fs::create_dir_all(self.approvals_dir())?;
        let path = self.approval_path(&rec.id)?;
        fs::write(&path, serde_json::to_string_pretty(rec)?)?;
        Ok(())
    }

    /// Load a single approval by id (`appr_…`).
    ///
    /// Rejects ids with path separators / `..` so callers cannot escape
    /// `$LOCUS_HOME/approvals/`.
    pub fn load_approval(&self, id: &str) -> Result<ApprovalRecord> {
        let path = self.approval_path(id)?;
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
    ///
    /// On **new** pending records, optional desktop notification may fire when
    /// the user has opted in (`LOCUS_NOTIFY=1` or `[notify] enabled = true`).
    /// Default is silent — agents create many pending approvals.
    pub fn create_pending_approval(
        &self,
        tool: &str,
        binding: &str,
        args: &Value,
        session_id: &str,
        requester: &str,
    ) -> Result<ApprovalRecord> {
        let digest = args_digest(args);
        for rec in self.list_approvals()? {
            if rec.status == ApprovalStatus::Pending && rec.matches_call(tool, binding, &digest) {
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
            requester: requester.into(),
            grants: Vec::new(),
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
                "requester": rec.requester,
                "status": "pending",
            })),
        )?;
        // Best-effort UX only — never surface notify errors to the agent path.
        crate::approval::try_notify_pending_approval(&rec);
        Ok(rec)
    }

    /// Whether the binding policy requires dual-control for this tool.
    pub fn tool_requires_dual_control(&self, binding_alias: &str, tool: &str) -> bool {
        self.load_binding(binding_alias)
            .map(|b| b.policy.requires_dual_control(tool))
            .unwrap_or(false)
    }

    /// Record a caller-controlled local advisory label.
    ///
    /// CLI, environment, Touch ID, and dashboard callers cannot establish
    /// human-principal authority. This method therefore never transitions a
    /// record to approved, regardless of the number of distinct strings.
    pub fn grant_approval(
        &self,
        id: &str,
        _ttl: Option<Duration>,
        principal: &str,
    ) -> Result<ApprovalRecord> {
        self.require_local_control("record local approval assertion")?;
        validate_approval_id(id)?;
        let principal = principal.trim();
        if principal.is_empty() {
            return Err(LocusError::msg(
                "principal is required (pass --as <name>, LOCUS_PRINCIPAL, or $USER)",
            ));
        }
        // Principals become path/audit labels — constrain charset (no injection)
        validate_name_component("principal", principal)?;

        let mut rec = self.load_approval(id)?;
        if rec.status == ApprovalStatus::Denied {
            return Err(LocusError::msg(format!(
                "approval {id} was denied — request a new one"
            )));
        }
        if rec.status == ApprovalStatus::Approved {
            return Err(LocusError::msg(format!(
                "approval {id} is marked approved but no peer-authenticated external authority verifier is available"
            )));
        }

        if rec.has_grant_from(principal) {
            return Err(LocusError::msg(format!(
                "local advisory label '{principal}' is already recorded for approval {id}"
            )));
        }

        let dual = self.tool_requires_dual_control(&rec.binding, &rec.tool);
        let required = crate::approval::required_grant_count(dual);
        let now = Utc::now();

        rec.grants.push(crate::approval::ApprovalGrant {
            principal: principal.into(),
            granted_at: now,
            authority: ApprovalAuthority::LocalAdvisory,
            envelope_id: None,
        });
        rec.status = ApprovalStatus::Pending;
        rec.granted_at = None;
        rec.expires_at = None;
        self.write_approval(&rec)?;
        self.audit(
            "approval.advisory",
            &rec.binding,
            Some(serde_json::json!({
                "id": rec.id,
                "tool": rec.tool,
                "args_digest": rec.args_digest,
                "principal_label": principal,
                "authority": ApprovalAuthority::LocalAdvisory.as_str(),
                "advisory_assertions": rec.grants.len(),
                "required_authoritative_grants": required,
                "dual_control": dual,
                "authoritative_path_enabled": false,
                "authority_blocker": crate::approval::EXTERNAL_APPROVAL_AUTHORITY_BLOCKER,
                "peer_authenticated_os_broker_required": true,
                "non_agent_issue_capability_required": true,
                "status": "pending",
            })),
        )?;
        if dual && rec.grants.len() == 1 {
            // Best-effort UX only: opt-in, rate-limited, async on macOS, and
            // explicitly non-authoritative. Delivery cannot affect the record.
            crate::approval::try_notify_partial_grant(&rec);
        }
        Ok(rec)
    }

    /// Mark approval denied (terminal).
    pub fn deny_approval(&self, id: &str) -> Result<ApprovalRecord> {
        self.require_local_control("deny approval")?;
        validate_approval_id(id)?;
        let mut rec = self.load_approval(id)?;
        rec.status = ApprovalStatus::Denied;
        rec.expires_at = None;
        rec.granted_at = None;
        // Keep grants history for audit forensics
        self.write_approval(&rec)?;
        self.audit(
            "approval.deny",
            &rec.binding,
            Some(serde_json::json!({
                "id": rec.id,
                "tool": rec.tool,
                "status": "denied",
                "grants": rec.grants.len(),
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
    /// Requires independently verified external authority plus an unexpired,
    /// exact tool, binding, and argument fingerprint. This release ships no
    /// external verifier, so even an edited `status=approved` record is denied.
    pub fn check_approval_id(
        &self,
        id: &str,
        tool: &str,
        binding: &str,
        args: &Value,
    ) -> Result<ApprovalRecord> {
        validate_approval_id(id)?;
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

    /// Health snapshot for the approvals directory (doctor / ops).
    ///
    /// Never includes raw tool args — counts and status only.
    pub fn approvals_health(&self) -> Result<ApprovalsHealth> {
        let dir = self.approvals_dir();
        let exists = dir.exists();
        let mut pending = 0usize;
        let mut approved = 0usize;
        let mut untrusted_approved = 0usize;
        let mut denied = 0usize;
        let mut expired_grants = 0usize;
        let mut corrupt = 0usize;
        let mut total = 0usize;
        let mut writable = false;

        if exists {
            // Probe writability without leaving debris when possible
            let probe = dir.join(".locus_write_probe");
            writable = fs::write(&probe, b"ok").is_ok();
            let _ = fs::remove_file(&probe);

            if let Ok(entries) = fs::read_dir(&dir) {
                for ent in entries.flatten() {
                    let path = ent.path();
                    if path.extension().and_then(|s| s.to_str()) != Some("json") {
                        continue;
                    }
                    total += 1;
                    let raw = match fs::read_to_string(&path) {
                        Ok(r) => r,
                        Err(_) => {
                            corrupt += 1;
                            continue;
                        }
                    };
                    match serde_json::from_str::<ApprovalRecord>(&raw) {
                        Ok(rec) => match rec.status {
                            ApprovalStatus::Pending => pending += 1,
                            ApprovalStatus::Denied => denied += 1,
                            ApprovalStatus::Approved => {
                                if rec.is_valid_grant() {
                                    approved += 1;
                                } else if rec.is_expired_authenticated_grant() {
                                    expired_grants += 1;
                                } else {
                                    untrusted_approved += 1;
                                }
                            }
                        },
                        Err(_) => corrupt += 1,
                    }
                }
            }
        }

        Ok(ApprovalsHealth {
            dir: dir.display().to_string(),
            exists,
            writable: if exists { writable } else { false },
            total,
            pending,
            approved_valid: approved,
            untrusted_approved,
            expired_grants,
            denied,
            corrupt,
            approval_authority: "local_advisory".into(),
            authoritative_path_enabled: crate::approval::external_approval_authority_enabled(),
            ok: exists && writable && corrupt == 0 && untrusted_approved == 0,
        })
    }

    pub fn workspace_for(&self, cwd: &Path) -> Result<Option<(PathBuf, WorkspaceConfig)>> {
        find_workspace(cwd)
    }
}

fn sanitize_audit_value(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(object) => serde_json::Value::Object(
            object
                .into_iter()
                .map(|(key, value)| {
                    let lower = key.to_ascii_lowercase();
                    if lower.contains("credential_ref") {
                        (key, serde_json::Value::String("<redacted>".into()))
                    } else {
                        (key, sanitize_audit_value(value))
                    }
                })
                .collect(),
        ),
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(sanitize_audit_value).collect())
        }
        serde_json::Value::String(value)
            if ["phm:", "env:", "test:"]
                .iter()
                .any(|prefix| value.contains(prefix)) =>
        {
            let source = if value.starts_with("phm:") {
                "phantom"
            } else if value.starts_with("env:") {
                "environment"
            } else {
                "unsupported"
            };
            serde_json::Value::String(format!("<redacted:{source}>"))
        }
        other => other,
    }
}

/// Sanitize a session file suffix to a single path component.
fn sanitize_session_suffix(suffix: &str) -> String {
    suffix
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Ensure `path` resolves under `base` (no path traversal escapes).
fn ensure_under_dir(base: &Path, path: &Path) -> Result<()> {
    // Lexical check: no parent components after join
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(LocusError::msg(format!(
            "refusing path outside store: {}",
            path.display()
        )));
    }
    // When both exist, also compare canonical prefixes
    if base.exists() && path.exists() {
        let base_c = fs::canonicalize(base)?;
        let path_c = fs::canonicalize(path)?;
        if !path_c.starts_with(&base_c) {
            return Err(LocusError::msg(format!(
                "refusing path outside store: {}",
                path.display()
            )));
        }
    } else if base.exists() {
        let base_c = fs::canonicalize(base)?;
        // Resolve parent + file name against base
        if let Some(parent) = path.parent() {
            if parent.exists() {
                let parent_c = fs::canonicalize(parent)?;
                if !parent_c.starts_with(&base_c) {
                    return Err(LocusError::msg(format!(
                        "refusing path outside store: {}",
                        path.display()
                    )));
                }
            }
        }
    }
    Ok(())
}

/// Doctor / ops view of `$LOCUS_HOME/approvals`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ApprovalsHealth {
    pub dir: String,
    pub exists: bool,
    pub writable: bool,
    pub total: usize,
    pub pending: usize,
    pub approved_valid: usize,
    /// Approved-looking files that lack independently verified authority.
    pub untrusted_approved: usize,
    pub expired_grants: usize,
    pub denied: usize,
    pub corrupt: usize,
    pub approval_authority: String,
    pub authoritative_path_enabled: bool,
    /// True when dir exists, is writable, and has no corrupt records.
    pub ok: bool,
}

/// One line from `$LOCUS_HOME/audit/events.jsonl`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AuditEvent {
    pub ts: String,
    pub op: String,
    pub binding: String,
    #[serde(default)]
    pub detail: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CredentialRefMigration {
    pub alias: String,
    pub migrated: usize,
    pub written: bool,
    #[serde(default)]
    pub audit_pending: bool,
    #[serde(default)]
    pub recovery_pending: bool,
    #[serde(default)]
    pub recovered: bool,
}

fn parse_binding_safely(raw: &str, label: &str) -> Result<Binding> {
    Binding::parse_toml(raw).map_err(|_| LocusError::msg(format!("binding '{label}' is malformed")))
}

fn validate_loaded_binding(binding: &Binding, label: &str) -> Result<()> {
    if binding
        .providers
        .iter()
        .any(|p| crate::credential::migrate_legacy_phantom_ref(&p.credential_ref).is_some())
    {
        return Err(LocusError::msg(format!(
            "binding '{label}' uses legacy bare Phantom names; run `locus binding migrate-credential-refs {label} --write`"
        )));
    }
    binding.validate().map_err(|_| {
        LocusError::msg(format!(
            "binding '{label}' has invalid credential configuration; use explicit phm:NAME or env:VAR"
        ))
    })
}

/// Result of [`Store::engagement_init`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EngagementInitResult {
    pub alias: String,
    pub tenant: String,
    pub binding_path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readme_path: Option<PathBuf>,
    pub credentials: Vec<CredentialMetadata>,
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
    /// Seconds until expiry (0 when already expired). Additive field.
    #[serde(default)]
    pub expires_in_secs: i64,
    pub worker_home: String,
    pub seal_ok: bool,
    pub seal: String,
    pub authority: String,
    pub authority_anchor_ok: bool,
    pub backing_type: SessionBackingType,
    pub backing_path: String,
    #[serde(default)]
    pub frozen: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frozen_reason: Option<String>,
    #[serde(default = "default_mode_exclusive")]
    pub mode: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub namespaces: Vec<String>,
}

fn default_mode_exclusive() -> String {
    "exclusive".into()
}

/// Result of [`Store::verify_runtime`] — continuous identity / drift check.
///
/// Never contains secret values. Safe for agent-facing heartbeats.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RuntimeDrift {
    pub pinned: bool,
    pub seal_ok: bool,
    #[serde(default)]
    pub authority_anchor_ok: bool,
    #[serde(default)]
    pub backing_ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backing_type: Option<SessionBackingType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backing_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority: Option<String>,
    pub binding_present: bool,
    pub binding_id_match: bool,
    pub tenant_match: bool,
    /// True when binding fingerprint (providers/scopes) still matches pin time.
    #[serde(default = "default_true")]
    pub providers_match: bool,
    /// Session has been frozen after detected drift.
    #[serde(default)]
    pub frozen: bool,
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
    /// Machine-readable issue tags (e.g. `invalid_seal`, `tenant_drift`, `providers_drift`).
    pub issues: Vec<String>,
    /// True only when pin is present, sealed, unexpired, unfrozen, and binding matches.
    pub ok: bool,
}

impl RuntimeDrift {
    /// Degraded drift facts for a **wedged** active session — one whose file
    /// exists but cannot even be read/resolved (corrupt JSON, invalid backing,
    /// unavailable seal key). Doctor uses this to keep reporting (with a
    /// `stale_session` finding naming `locus leave --force`) instead of dying
    /// on the read error. Everything stays fail-closed: `ok` is false and no
    /// field claims a verified state.
    pub fn wedged(detail: &str) -> Self {
        RuntimeDrift {
            pinned: true,
            seal_ok: false,
            authority_anchor_ok: false,
            backing_ok: false,
            backing_type: None,
            backing_path: None,
            authority: None,
            binding_present: false,
            binding_id_match: false,
            tenant_match: false,
            providers_match: false,
            frozen: false,
            expired: false,
            session_id: None,
            binding_alias: None,
            binding_id_session: None,
            binding_id_file: None,
            tenant_session: None,
            tenant_file: None,
            providers: Vec::new(),
            issues: vec![
                "stale_session".into(),
                format!("session_unreadable: {detail}"),
            ],
            ok: false,
        }
    }
}

/// MCP multi-tenant grant record at rest (`mcp-grants/<grant_id>.json`).
///
/// Carries the seal-key HMAC of the bearer token (`token_seal`) — never the
/// secret itself, never credentials, never an executor capability.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct McpGrant {
    /// Record schema version (currently 1).
    pub v: u32,
    /// Public grant id (16 lowercase hex chars) — safe for audits and lists.
    pub grant_id: String,
    /// Seal-key HMAC over `mcp-grant:<grant_id>:<secret>`.
    pub token_seal: String,
    /// Backing sealed session id (ci-backed, delegated, TTL-capped).
    pub session_id: String,
    pub binding_alias: String,
    pub tenant: String,
    pub created_at: String,
    pub expires_at: chrono::DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default)]
    pub revoked: bool,
}

impl McpGrant {
    pub fn is_expired(&self) -> bool {
        self.expires_at <= Utc::now()
    }
}

/// Failure of [`Store::verify_mcp_grant_token`]. `Invalid` is deliberately
/// uniform; `Expired` is only produced after the token HMAC verified.
#[derive(Debug, Clone)]
pub enum McpGrantAuthError {
    Invalid {
        /// Present only when the token parsed (safe to audit — public id).
        grant_id: Option<String>,
        /// True only when the token parsed AND the grant is definitively dead:
        /// the record is missing/unreadable-as-a-grant (post-revoke deletion)
        /// or the MAC verified against a record marked `revoked`. NEVER set on
        /// a MAC mismatch — a forged token naming a live grant must not be
        /// able to trigger session/worker sweeps for that grant. The external
        /// error body stays uniform regardless.
        grant_dead: bool,
    },
    Expired {
        grant: Box<McpGrant>,
    },
}

/// Grant ids are 16 lowercase hex chars — path-safe by construction.
pub fn is_safe_mcp_grant_id(id: &str) -> bool {
    id.len() == 16
        && id
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// Parse `lmt_<grant_id>.<secret>` without any store access. Charset-strict
/// so a hostile token can never influence a filesystem path.
pub fn parse_mcp_grant_token(token: &str) -> Option<(&str, &str)> {
    let rest = token.trim().strip_prefix("lmt_")?;
    let (grant_id, secret) = rest.split_once('.')?;
    if !is_safe_mcp_grant_id(grant_id) {
        return None;
    }
    if secret.len() != 64
        || !secret
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return None;
    }
    Some((grant_id, secret))
}

/// Outcome of a forced session teardown ([`Store::force_leave`]).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ForceLeaveOutcome {
    /// True when an `active.json` existed and was removed.
    pub cleared: bool,
    pub session_id: Option<String>,
    pub binding_alias: Option<String>,
    /// Best-effort classification of why the session was wedged
    /// (e.g. `invalid_seal`, `anchor_unavailable`, `unreadable_session`);
    /// empty when the session was healthy.
    pub diagnosis: Vec<String>,
}

fn default_true() -> bool {
    true
}

/// All-false baseline drift record (shared by active + per-session paths).
fn empty_runtime_drift() -> RuntimeDrift {
    RuntimeDrift {
        pinned: false,
        seal_ok: false,
        authority_anchor_ok: false,
        backing_ok: false,
        backing_type: None,
        backing_path: None,
        authority: None,
        binding_present: false,
        binding_id_match: false,
        tenant_match: false,
        providers_match: true,
        frozen: false,
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
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProviderView {
    pub provider: String,
    pub account: String,
    pub credential: CredentialMetadata,
    pub project_ref: Option<String>,
    pub team_id: Option<String>,
    pub account_id: Option<String>,
    pub read_only: Option<bool>,
    pub orgs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repos: Vec<String>,
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

/// Like [`write_secret_file`] but with `create_new` (O_EXCL) semantics: errors
/// if `path` already exists instead of overwriting — fail closed on id
/// collision. Mode 0600 on Unix at creation (no chmod window).
fn write_secret_file_new(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write as _;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::{BindingBody, Policy, ProviderBinding, Scope};
    use std::sync::{Mutex, MutexGuard};
    use tempfile::tempdir;

    /// Process-global env (`LOCUS_SESSION_ID`) must not race across parallel tests.
    static LOCUS_SESSION_ENV_LOCK: Mutex<()> = Mutex::new(());

    thread_local! {
        static HOLDING_SESSION_ENV_LOCK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    /// Re-entrant (per-thread) guard for LOCUS_SESSION_ID mutations + reads.
    pub(super) struct SessionEnvLock {
        _guard: Option<MutexGuard<'static, ()>>,
    }

    impl SessionEnvLock {
        pub(super) fn acquire() -> Self {
            if HOLDING_SESSION_ENV_LOCK.with(|h| h.get()) {
                return Self { _guard: None };
            }
            let guard = LOCUS_SESSION_ENV_LOCK
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            HOLDING_SESSION_ENV_LOCK.with(|h| h.set(true));
            Self {
                _guard: Some(guard),
            }
        }
    }

    impl Drop for SessionEnvLock {
        fn drop(&mut self) {
            if self._guard.is_some() {
                HOLDING_SESSION_ENV_LOCK.with(|h| h.set(false));
            }
        }
    }

    pub(super) fn lock_session_env() -> SessionEnvLock {
        SessionEnvLock::acquire()
    }

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
                    upstream: None,
                },
                ProviderBinding {
                    provider: "github".into(),
                    account: format!("{alias}-gh"),
                    credential_ref: format!("phm:GH_{}", alias.to_uppercase()),
                    scope: Scope {
                        orgs: vec![tenant.into()],
                        ..Scope::default()
                    },
                    upstream: None,
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
        // Agent-facing view reports source metadata, never credential refs.
        for p in &w1.providers {
            assert!(p.credential.present);
            assert_eq!(p.credential.source, "phantom");
        }
        assert!(!serde_json::to_string(&w1)
            .unwrap()
            .contains("SUPABASE_ACME"));

        let s2 = store
            .pin("personal", dir.path(), Some("test".into()), false)
            .unwrap();
        assert_eq!(s2.tenant, "personal");
        let w2 = store.whoami().unwrap();
        assert_eq!(w2.binding_alias, "personal");
        for p in &w2.providers {
            assert!(p.credential.present);
            assert_eq!(p.credential.source, "phantom");
        }
        let w2_json = serde_json::to_string(&w2).unwrap();
        assert!(!w2_json.contains("GH_TOKEN_PERSONAL"));
        assert!(!w2_json.contains("credential_ref"));
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
    fn malformed_workspace_blocks_explicit_and_forced_pin() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path().join("locus-home")).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        let project = dir.path().join("client-acme");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join(".locus.toml"), "allowed_bindings = [").unwrap();

        for force in [false, true] {
            let err = store.pin("acme", &project, None, force).unwrap_err();
            assert!(err.to_string().contains("workspace policy malformed"));
            assert!(store.active_session().unwrap().is_none());
        }
    }

    #[test]
    fn load_binding_rejects_raw_credential_ref_from_disk() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let path = store.bindings_dir().join("raw.toml");
        fs::write(
            path,
            r#"
[binding]
id = "bnd_raw"
alias = "raw"
tenant = "raw-tenant"

[[binding.providers]]
provider = "github"
account = "raw-account"
credential_ref = "ghp_RAW_TOKEN_CANARY"
"#,
        )
        .unwrap();

        let err = store.load_binding("raw").unwrap_err().to_string();
        assert!(err.contains("invalid credential configuration"));
        assert!(!err.contains("ghp_RAW_TOKEN_CANARY"));
    }

    #[test]
    fn legacy_bare_refs_are_actionable_and_migrate_only_explicitly() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let path = store.bindings_dir().join("legacy.toml");
        fs::write(
            &path,
            r#"
[binding]
id = "bnd_legacy"
alias = "legacy"
tenant = "legacy-tenant"

[[binding.providers]]
provider = "github"
account = "legacy-account"
credential_ref = "LEGACY_PHANTOM_CANARY"
"#,
        )
        .unwrap();

        let list_error = store.list_bindings().unwrap_err().to_string();
        assert!(list_error.contains("migrate-credential-refs legacy --write"));
        assert!(!list_error.contains("LEGACY_PHANTOM_CANARY"));

        let dry_run = store
            .migrate_legacy_credential_refs("legacy", false)
            .unwrap();
        assert_eq!(dry_run.migrated, 1);
        assert!(!dry_run.written);
        assert!(fs::read_to_string(&path)
            .unwrap()
            .contains("credential_ref = \"LEGACY_PHANTOM_CANARY\""));

        let written = store
            .migrate_legacy_credential_refs("legacy", true)
            .unwrap();
        assert_eq!(written.migrated, 1);
        assert!(written.written);
        let migrated = store.load_binding("legacy").unwrap();
        assert_eq!(
            migrated.providers[0].credential_ref,
            "phm:LEGACY_PHANTOM_CANARY"
        );

        let audit = fs::read_to_string(store.audit_path()).unwrap();
        assert!(!audit.contains("LEGACY_PHANTOM_CANARY"));
        assert!(audit.contains("binding.credential_refs_migrated"));
    }

    #[test]
    fn unsafe_legacy_ref_never_echoes_and_cannot_auto_migrate() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        fs::write(
            store.bindings_dir().join("unsafe.toml"),
            r#"
[binding]
id = "bnd_unsafe"
alias = "unsafe"
tenant = "unsafe-tenant"

[[binding.providers]]
provider = "github"
account = "unsafe-account"
credential_ref = "ghp_UNSAFE/CANARY"
"#,
        )
        .unwrap();

        let error = store
            .migrate_legacy_credential_refs("unsafe", true)
            .unwrap_err()
            .to_string();
        assert!(error.contains("edit it manually"));
        assert!(!error.contains("ghp_UNSAFE/CANARY"));
    }

    #[test]
    fn audit_redacts_locator_keys_and_values_recursively() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .audit(
                "probe",
                "safe",
                Some(serde_json::json!({
                    "credential_ref": "phm:AUDIT_LOCATOR_CANARY",
                    "nested": ["--ref=env:AUDIT_ENV_CANARY", {"value": "prefix test:AUDIT_TEST_CANARY"}],
                })),
            )
            .unwrap();
        let audit = fs::read_to_string(store.audit_path()).unwrap();
        for canary in [
            "AUDIT_LOCATOR_CANARY",
            "AUDIT_ENV_CANARY",
            "AUDIT_TEST_CANARY",
        ] {
            assert!(!audit.contains(canary));
        }
        assert!(audit.contains("<redacted>"));
    }

    #[cfg(unix)]
    #[test]
    fn broken_workspace_link_blocks_pin_force_and_run() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let store = Store::open(dir.path().join("locus-home")).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        let project = dir.path().join("project");
        fs::create_dir_all(&project).unwrap();
        symlink("missing-policy.toml", project.join(".locus.toml")).unwrap();

        for force in [false, true] {
            let pin_error = store
                .pin("acme", &project, None, force)
                .unwrap_err()
                .to_string();
            assert!(pin_error.contains("broken or unreadable"));
            let run_error = store
                .create_run_session("acme", &project, None, force, false, "broken")
                .unwrap_err()
                .to_string();
            assert!(run_error.contains("broken or unreadable"));
        }
        assert!(store.active_session().unwrap().is_none());
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
    fn force_leave_clears_invalid_seal_session() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        store.pin("acme", dir.path(), None, false).unwrap();

        // Corrupt the seal deliberately — the production wedge.
        let path = store.active_session_path();
        let mut s: Session = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        s.seal = "deadbeef".into();
        fs::write(&path, serde_json::to_string(&s).unwrap()).unwrap();

        // The control boundary used by normal `locus leave` fails closed.
        assert!(matches!(
            store.require_local_control("locus leave").unwrap_err(),
            LocusError::InvalidSeal
        ));

        // Recovery: force-leave tears the session state down anyway.
        let outcome = store.force_leave("test wedge", false).unwrap();
        assert!(outcome.cleared);
        assert_eq!(outcome.binding_alias.as_deref(), Some("acme"));
        assert!(outcome.diagnosis.iter().any(|d| d == "invalid_seal"));
        assert!(!store.active_session_path().exists());
        assert!(matches!(
            store.require_active().unwrap_err(),
            LocusError::NotPinned
        ));

        // Audited as session.force_leave with the reason; nothing minted.
        let audit = fs::read_to_string(store.audit_path()).unwrap();
        assert!(audit.contains("session.force_leave"));
        assert!(audit.contains("test wedge"));
    }

    #[test]
    fn force_leave_clears_anchor_unavailable_session() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        store.pin("acme", dir.path(), None, false).unwrap();

        // Supervisor-anchor wedge: the broker runtime is gone; any broker that
        // comes back re-anchors under a fresh epoch, so the pinned session's
        // authority generation can never validate again.
        fs::remove_dir_all(dir.path().join("runtime")).unwrap();

        // The broker endpoint is gone and no operator capability was ever
        // persisted for this home: no verifier remains, so teardown fails
        // closed until the operator explicitly acknowledges the degraded gate.
        let err = store.force_leave("supervisor gone", false).unwrap_err();
        assert!(err.to_string().contains("no verifier"), "err={err}");
        assert!(store.active_session_path().exists());

        // Normal leave fails closed on the unavailable/stale anchor (its
        // control path restarts a broker under a FRESH epoch, so the pinned
        // session's authority generation can never validate again).
        assert!(store.leave().is_err());
        assert!(store.active_session_path().exists());

        // That restarted live broker now authenticates the control capability
        // directly, so teardown proceeds without the acknowledgement flag.
        let outcome = store.force_leave("supervisor gone", false).unwrap();
        assert!(outcome.cleared);
        assert!(outcome
            .diagnosis
            .iter()
            .any(|d| d == "anchor_unavailable" || d == "anchor_mismatch"));
        assert!(!store.active_session_path().exists());
    }

    #[test]
    fn force_leave_without_matching_capability_fails() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        // No broker was ever started for this home, so teardown authentication
        // falls back to the persisted operator capability — which deliberately
        // differs from this process's control capability.
        crate::authority_anchor::mint_persisted_control_capability(dir.path()).unwrap();
        fs::create_dir_all(dir.path().join("sessions")).unwrap();
        fs::write(store.active_session_path(), "{ wedged").unwrap();

        let err = store.force_leave("attempt", false).unwrap_err().to_string();
        assert!(err.contains("control capability"), "err={err}");
        // The no-verifier acknowledgement never overrides a verifier that is
        // present and disagrees.
        let err = store.force_leave("attempt", true).unwrap_err().to_string();
        assert!(err.contains("control capability"), "err={err}");
        // Fail closed: nothing was deleted.
        assert!(store.active_session_path().exists());
    }

    #[test]
    fn force_leave_without_session_is_noop() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        // Fresh home: no broker and no persisted capability, so the noop is
        // only reachable through the acknowledged-unverified posture.
        assert!(store.force_leave("nothing", false).is_err());
        let outcome = store.force_leave("nothing", true).unwrap();
        assert!(!outcome.cleared);
        assert!(outcome.session_id.is_none());
        assert!(outcome.diagnosis.is_empty());
    }

    #[test]
    fn force_leave_clears_unreadable_session_file() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        store.pin("acme", dir.path(), None, false).unwrap();
        fs::write(store.active_session_path(), "{ not json").unwrap();

        // Even resolve fails on the corrupt file — the second production wedge.
        assert!(store.active_session().is_err());

        let outcome = store.force_leave("corrupt file", false).unwrap();
        assert!(outcome.cleared);
        assert!(outcome.diagnosis.iter().any(|d| d == "unreadable_session"));
        assert!(!store.active_session_path().exists());
    }

    #[test]
    fn normal_leave_still_works_after_force_leave_exists() {
        // Guard: the recovery hatch must not change the healthy-path contract.
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        store.pin("acme", dir.path(), None, false).unwrap();
        let left = store.leave().unwrap();
        assert_eq!(left.map(|s| s.binding_alias).as_deref(), Some("acme"));
        assert!(!store.active_session_path().exists());
        let audit = fs::read_to_string(store.audit_path()).unwrap();
        assert!(audit.contains("session.leave"));
        assert!(!audit.contains("session.force_leave"));
    }

    /// The force-leave audit must record what actually happened: when the
    /// active.json removal fails, no success record may exist — only a
    /// `session.force_leave_failed` record, and the error is surfaced.
    #[cfg(unix)]
    #[test]
    fn force_leave_failed_removal_audits_failure_not_success() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        store.pin("acme", dir.path(), None, false).unwrap();

        // Make the sessions dir read-only so remove_file(active.json) fails.
        let sessions_dir = store.active_session_path().parent().unwrap().to_path_buf();
        let orig = fs::metadata(&sessions_dir).unwrap().permissions();
        fs::set_permissions(&sessions_dir, fs::Permissions::from_mode(0o555)).unwrap();
        let result = store.force_leave("wedge under readonly dir", false);
        fs::set_permissions(&sessions_dir, orig).unwrap();

        assert!(result.is_err(), "removal failure must surface as an error");
        assert!(store.active_session_path().exists());
        let audit = fs::read_to_string(store.audit_path()).unwrap();
        assert!(audit.contains("session.force_leave_failed"));
        // No success record: a successful audit serializes the op with an
        // immediately closing quote; only the `_failed` variant may appear.
        assert!(!audit.contains("session.force_leave\""));

        // The wedge is still recoverable once the dir is writable again.
        let outcome = store.force_leave("retry", false).unwrap();
        assert!(outcome.cleared);
        let audit = fs::read_to_string(store.audit_path()).unwrap();
        assert!(audit.contains("session.force_leave\""));
    }

    /// A forged `worker_home` using `..` traversal passes a naive
    /// component-prefix check (`<home>/workers/../victim` starts with
    /// `<home>/workers` component-wise) — the canonicalized containment
    /// check must refuse to delete it.
    #[test]
    fn force_leave_never_removes_worker_home_outside_workers_root() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        store.pin("acme", dir.path(), None, false).unwrap();

        let victim = dir.path().join("victim");
        fs::create_dir_all(&victim).unwrap();
        fs::write(victim.join("keep.txt"), "keep").unwrap();

        let path = store.active_session_path();
        let mut s: Session = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        s.worker_home = dir
            .path()
            .join("workers")
            .join("..")
            .join("victim")
            .display()
            .to_string();
        fs::write(&path, serde_json::to_string(&s).unwrap()).unwrap();

        let outcome = store
            .force_leave("forged worker_home traversal", false)
            .unwrap();
        assert!(outcome.cleared);
        assert!(
            victim.join("keep.txt").exists(),
            "directory outside workers/ must survive force_leave"
        );
    }

    /// A symlink planted under `workers/` whose target lies outside the
    /// store must not be followed into a recursive delete.
    #[cfg(unix)]
    #[test]
    fn force_leave_never_follows_worker_home_symlink_escape() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        store.pin("acme", dir.path(), None, false).unwrap();

        let victim = dir.path().join("victim-symlink");
        fs::create_dir_all(&victim).unwrap();
        fs::write(victim.join("keep.txt"), "keep").unwrap();

        let workers = dir.path().join("workers");
        fs::create_dir_all(&workers).unwrap();
        let link = workers.join("escape");
        std::os::unix::fs::symlink(&victim, &link).unwrap();

        let path = store.active_session_path();
        let mut s: Session = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        s.worker_home = link.display().to_string();
        fs::write(&path, serde_json::to_string(&s).unwrap()).unwrap();

        let outcome = store.force_leave("symlinked worker_home", false).unwrap();
        assert!(outcome.cleared);
        assert!(
            victim.join("keep.txt").exists(),
            "symlink target outside workers/ must survive force_leave"
        );
    }

    /// Guard: the hardening must not break legitimate cleanup — a real
    /// worker home under workers/ is still removed on force_leave.
    #[test]
    fn force_leave_still_removes_legit_worker_home() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        let session = store.pin("acme", dir.path(), None, false).unwrap();
        let wh = PathBuf::from(&session.worker_home);
        assert!(wh.exists(), "pin must create the worker home");

        let outcome = store.force_leave("normal teardown", false).unwrap();
        assert!(outcome.cleared);
        assert!(!wh.exists(), "worker home under workers/ must be removed");
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
            principal: Some("agent"),
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
        assert_eq!(pending[0].requester, "agent");
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

        // 4) A caller-controlled principal label remains advisory.
        let granted = store.grant_approval(&approval_id, None, "alice").unwrap();
        assert_eq!(granted.status, ApprovalStatus::Pending);
        assert!(granted.expires_at.is_none());
        assert!(!granted.is_valid_grant());
        assert_eq!(granted.grants.len(), 1);
        assert_eq!(granted.grants[0].principal, "alice");
        assert_eq!(
            granted.grants[0].authority,
            ApprovalAuthority::LocalAdvisory
        );
        assert_eq!(store.pending_approvals().unwrap().len(), 1);

        // 5) Same args remain blocked.
        let r5 = call_tool_gated(&binding, "supabase.table.delete", &args, Some(gate)).unwrap();
        assert!(!r5.ok, "local advisory must not allow: {:?}", r5.content);
        assert_eq!(r5.content["authoritative_grants"], 0);

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
        assert!(store.grant_approval(&other_id, None, "alice").is_err());

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
        store.grant_approval(&id8, None, "alice").unwrap();
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
        assert!(
            !r8b.ok,
            "approval_id path must remain blocked: {:?}",
            r8b.content
        );

        // Path traversal / unsafe approval ids must never touch the filesystem
        assert!(store.load_approval("../evil").is_err());
        assert!(store
            .grant_approval("appr/../../../x", None, "mason")
            .is_err());
        assert!(store.load_approval("appr_foo.json").is_err());
    }

    #[test]
    fn approvals_health_reports_dir() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let h = store.approvals_health().unwrap();
        assert!(h.exists);
        assert!(h.writable);
        assert!(h.ok);
        assert_eq!(h.total, 0);
    }

    #[test]
    fn dual_control_one_grant_insufficient() {
        use crate::adapters::{call_tool_gated, ApprovalGate};
        use crate::approval::ApprovalStatus;
        use serde_json::json;

        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let mut b = sample_binding("acme", "acme-corp", "p1");
        b.policy.require_approval = vec!["*.delete*".into()];
        b.policy.dual_control = vec!["*.delete*".into()];
        store.save_binding(&b).unwrap();
        let session = store.pin("acme", dir.path(), None, false).unwrap();
        let binding = store.load_binding("acme").unwrap();
        let args = json!({ "table": "users" });
        let gate = ApprovalGate {
            store: &store,
            session_id: &session.session_id,
            principal: Some("agent"),
        };

        let r = call_tool_gated(&binding, "supabase.table.delete", &args, Some(gate)).unwrap();
        assert!(!r.ok);
        assert_eq!(
            r.content.get("dual_control").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            r.content.get("required_grants").and_then(|v| v.as_u64()),
            Some(2)
        );
        let id = r
            .content
            .get("approval_id")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let hint = r.content.get("hint").and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            hint.contains("records local advisory evidence only")
                && hint.contains("requires 2 externally authenticated approvers"),
            "agent hint must distinguish advisory labels from authority: {hint}"
        );
        assert!(hint.contains(&id), "hint must include approval id: {hint}");

        // One grant → still pending (partial notify is opt-in; LOCUS_NOTIFY off stays silent)
        let prev_notify = std::env::var_os("LOCUS_NOTIFY");
        std::env::set_var("LOCUS_NOTIFY", "0");
        let partial = store.grant_approval(&id, None, "alice").unwrap();
        match prev_notify {
            Some(v) => std::env::set_var("LOCUS_NOTIFY", v),
            None => std::env::remove_var("LOCUS_NOTIFY"),
        }
        assert_eq!(partial.status, ApprovalStatus::Pending);
        assert_eq!(partial.grants.len(), 1);
        assert!(!partial.is_valid_grant());
        assert!(store
            .pending_approvals()
            .unwrap()
            .iter()
            .any(|p| p.id == id));

        // Tool still blocked
        let r2 = call_tool_gated(&binding, "supabase.table.delete", &args, Some(gate)).unwrap();
        assert!(!r2.ok);
        assert_eq!(
            r2.content.get("error").and_then(|v| v.as_str()),
            Some("requires_approval")
        );
        assert_eq!(r2.content.get("grants").and_then(|v| v.as_u64()), Some(1));
        let hint2 = r2
            .content
            .get("hint")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert!(
            hint2.contains("records local advisory evidence only")
                && hint2.contains("provider execution remains blocked"),
            "advisory dual-control hint: {hint2}"
        );
    }

    #[test]
    fn dual_control_two_local_labels_never_authorize() {
        use crate::adapters::{call_tool_gated, ApprovalGate};
        use crate::approval::ApprovalStatus;
        use serde_json::json;

        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let mut b = sample_binding("acme", "acme-corp", "p1");
        b.policy.require_approval = vec!["*.delete*".into()];
        b.policy.dual_control = vec!["*.delete*".into()];
        store.save_binding(&b).unwrap();
        let session = store.pin("acme", dir.path(), None, false).unwrap();
        let binding = store.load_binding("acme").unwrap();
        let args = json!({ "table": "users" });
        let gate = ApprovalGate {
            store: &store,
            session_id: &session.session_id,
            principal: Some("agent"),
        };

        let r = call_tool_gated(&binding, "supabase.table.delete", &args, Some(gate)).unwrap();
        let id = r
            .content
            .get("approval_id")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();

        store.grant_approval(&id, None, "alice").unwrap();
        let full = store.grant_approval(&id, None, "bob").unwrap();
        assert_eq!(full.status, ApprovalStatus::Pending);
        assert_eq!(full.grants.len(), 2);
        assert!(!full.is_valid_grant());
        assert!(full.has_grant_from("alice"));
        assert!(full.has_grant_from("bob"));
        assert_eq!(store.pending_approvals().unwrap().len(), 1);

        let blocked =
            call_tool_gated(&binding, "supabase.table.delete", &args, Some(gate)).unwrap();
        assert!(
            !blocked.ok,
            "two labels must not allow: {:?}",
            blocked.content
        );
        assert_eq!(blocked.content["authoritative_grants"], 0);
        assert_eq!(blocked.content["required_authoritative_grants"], 2);
    }

    #[test]
    fn dual_control_same_principal_cannot_grant_twice() {
        use crate::adapters::{call_tool_gated, ApprovalGate};
        use crate::approval::ApprovalStatus;
        use serde_json::json;

        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let mut b = sample_binding("acme", "acme-corp", "p1");
        b.policy.require_approval = vec!["*.delete*".into()];
        b.policy.dual_control_all_approvals = true;
        store.save_binding(&b).unwrap();
        let session = store.pin("acme", dir.path(), None, false).unwrap();
        let binding = store.load_binding("acme").unwrap();
        let args = json!({ "table": "users" });
        let gate = ApprovalGate {
            store: &store,
            session_id: &session.session_id,
            principal: None,
        };

        let r = call_tool_gated(&binding, "supabase.table.delete", &args, Some(gate)).unwrap();
        let id = r
            .content
            .get("approval_id")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();

        let first = store.grant_approval(&id, None, "alice").unwrap();
        assert_eq!(first.status, ApprovalStatus::Pending);
        assert_eq!(first.grants.len(), 1);

        let err = store.grant_approval(&id, None, "alice").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("already recorded"), "unexpected: {msg}");

        // Still only one grant
        let rec = store.load_approval(&id).unwrap();
        assert_eq!(rec.grants.len(), 1);
        assert_eq!(rec.status, ApprovalStatus::Pending);

        // Tool still blocked
        let r2 = call_tool_gated(&binding, "supabase.table.delete", &args, Some(gate)).unwrap();
        assert!(!r2.ok);
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
            principal: Some("agent"),
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
            .grant_approval(&id, Some(Duration::minutes(15)), "alice")
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

    #[test]
    fn forged_same_user_approval_json_and_spoofed_principals_fail_closed() {
        use crate::adapters::{call_tool_gated, ApprovalGate};
        use serde_json::json;

        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let mut binding = sample_binding("acme", "acme-corp", "p1");
        binding.policy.require_approval = vec!["*.delete*".into()];
        binding.policy.dual_control = vec!["*.delete*".into()];
        store.save_binding(&binding).unwrap();
        let session = store.pin("acme", dir.path(), None, false).unwrap();
        let binding = store.load_binding("acme").unwrap();
        let args = json!({"table": "users", "project_ref": "p1"});
        let gate = ApprovalGate {
            store: &store,
            session_id: &session.session_id,
            principal: Some("agent"),
        };

        let first = call_tool_gated(&binding, "supabase.table.delete", &args, Some(gate)).unwrap();
        let id = first.content["approval_id"].as_str().unwrap().to_string();
        for label in ["agent", "company_ceo"] {
            let record = store.grant_approval(&id, None, label).unwrap();
            assert_eq!(record.status, ApprovalStatus::Pending);
            assert!(record.grants.iter().all(|grant| {
                grant.authority == ApprovalAuthority::LocalAdvisory && grant.envelope_id.is_none()
            }));
        }

        let mut forged = store.load_approval(&id).unwrap();
        forged.status = ApprovalStatus::Approved;
        forged.granted_at = Some(Utc::now());
        forged.expires_at = Some(Utc::now() + Duration::minutes(15));
        for (index, grant) in forged.grants.iter_mut().enumerate() {
            grant.authority = ApprovalAuthority::ExternalAuthenticated;
            grant.envelope_id = Some(format!("unsigned-same-user-{index}"));
        }
        fs::write(
            store.approvals_dir().join(format!("{id}.json")),
            serde_json::to_string_pretty(&forged).unwrap(),
        )
        .unwrap();

        assert!(!crate::approval::external_approval_authority_enabled());
        assert!(!store.load_approval(&id).unwrap().is_valid_grant());
        for _ in 0..2 {
            assert!(store
                .find_valid_grant("supabase.table.delete", "acme", &args)
                .unwrap()
                .is_none());
        }
        let blocked =
            call_tool_gated(&binding, "supabase.table.delete", &args, Some(gate)).unwrap();
        assert!(!blocked.ok);
        assert_eq!(blocked.content["authoritative_grants"], 0);

        let health = store.approvals_health().unwrap();
        assert_eq!(health.approved_valid, 0);
        assert_eq!(health.untrusted_approved, 1);
        assert_eq!(health.expired_grants, 0);
        assert_eq!(health.approval_authority, "local_advisory");
        assert!(!health.authoritative_path_enabled);
        assert!(!health.ok);

        assert!(!forged.matches_call("supabase.table.delete", "other", &args_digest(&args)));
        assert!(!forged.matches_call(
            "supabase.table.delete",
            "acme",
            &args_digest(&json!({"table": "payments", "project_ref": "p1"}))
        ));
    }

    /// Sequential stress: many pin/leave cycles must not leave a sticky pin
    /// or corrupt the seal key / active session path.
    #[test]
    fn pin_leave_stress_many_iterations() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        store
            .save_binding(&sample_binding("personal", "personal", "p2"))
            .unwrap();

        const N: usize = 64;
        for i in 0..N {
            let alias = if i % 2 == 0 { "acme" } else { "personal" };
            let s = store
                .pin(alias, dir.path(), Some(format!("stress-{i}")), false)
                .unwrap();
            assert_eq!(s.binding_alias, alias);
            let w = store.whoami().unwrap();
            assert_eq!(w.binding_alias, alias);
            assert!(w.seal_ok);
            store.require_active().unwrap();
            let left = store.leave().unwrap();
            assert!(left.is_some());
            assert!(matches!(
                store.require_active().unwrap_err(),
                LocusError::NotPinned
            ));
        }

        // Final pin still seals cleanly after stress
        store.pin("acme", dir.path(), None, false).unwrap();
        let d = store.verify_runtime().unwrap();
        assert!(d.ok);
        assert!(d.seal_ok);
    }

    /// After leave → re-pin, tampering the new active session still fails closed.
    #[test]
    fn invalid_seal_after_leave_repin_cycle() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        store
            .save_binding(&sample_binding("personal", "personal", "p2"))
            .unwrap();

        let s1 = store.pin("acme", dir.path(), None, false).unwrap();
        let s1_id = s1.session_id.clone();
        store.leave().unwrap();
        assert!(store.active_session().unwrap().is_none());

        let s2 = store.pin("personal", dir.path(), None, false).unwrap();
        assert_ne!(s2.session_id, s1_id, "re-pin must mint a new session id");
        store.require_active().unwrap();

        // Tamper seal on the post-repin active session
        let path = store.active_session_path();
        let mut s: Session = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        s.seal = "deadbeef".into();
        fs::write(&path, serde_json::to_string(&s).unwrap()).unwrap();
        assert!(matches!(
            store.require_active().unwrap_err(),
            LocusError::InvalidSeal
        ));

        // verify_runtime surfaces invalid_seal without panicking
        let d = store.verify_runtime().unwrap();
        assert!(!d.ok);
        assert!(d.pinned);
        assert!(!d.seal_ok);
        assert!(d.issues.iter().any(|i| i == "invalid_seal"));

        // Leave + re-pin recovers to a healthy pin
        store.leave().unwrap();
        store.pin("acme", dir.path(), None, false).unwrap();
        assert!(store.verify_runtime().unwrap().ok);
    }

    #[test]
    fn readable_hmac_key_cannot_rebind_live_broker_authority() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        store.pin("acme", dir.path(), None, false).unwrap();

        let path = store.active_session_path();
        let mut forged: Session =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        forged.tenant = "other-tenant".into();
        forged.worker_home = dir.path().join("aliased-worker").display().to_string();
        forged.reseal(&store.seal_key().unwrap());
        fs::write(&path, serde_json::to_vec(&forged).unwrap()).unwrap();

        assert!(matches!(
            store.require_active().unwrap_err(),
            LocusError::AuthorityAnchorMismatch
        ));
        let runtime = store.verify_runtime().unwrap();
        assert!(!runtime.ok);
        assert!(runtime
            .issues
            .iter()
            .any(|issue| issue == "authority_anchor_mismatch"));
    }

    #[test]
    fn drift_check_does_not_mutate_broker_rejected_hmac_record() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        store.pin("acme", dir.path(), None, false).unwrap();

        let path = store.active_session_path();
        let mut forged: Session =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        forged.binding_alias = "missing-binding".into();
        forged.reseal(&store.seal_key().unwrap());
        let forged_bytes = serde_json::to_vec(&forged).unwrap();
        fs::write(&path, &forged_bytes).unwrap();

        let runtime = store.check_drift_and_freeze().unwrap();
        assert!(!runtime.authority_anchor_ok);
        assert!(runtime
            .issues
            .iter()
            .any(|issue| issue == "binding_missing"));
        assert!(!runtime.frozen);
        assert_eq!(fs::read(&path).unwrap(), forged_bytes);
    }

    #[test]
    fn leave_then_restored_record_cannot_resurrect_revoked_generation() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        store.pin("acme", dir.path(), None, false).unwrap();
        let path = store.active_session_path();
        let replay = fs::read(&path).unwrap();

        store.leave().unwrap();
        fs::write(&path, replay).unwrap();

        assert!(matches!(
            store.require_active().unwrap_err(),
            LocusError::AuthorityAnchorMismatch
        ));
        assert!(!store.verify_runtime().unwrap().ok);
    }

    #[test]
    fn legacy_seal_downgrade_is_invalid_in_runtime_and_doctor_inputs() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        store.pin("acme", dir.path(), None, false).unwrap();
        let path = store.active_session_path();
        let mut downgraded: Session =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        downgraded.seal_version = 2;
        downgraded.seal = store.seal_key().unwrap().seal(&downgraded.material());
        fs::write(&path, serde_json::to_vec(&downgraded).unwrap()).unwrap();

        assert!(matches!(
            store.require_active().unwrap_err(),
            LocusError::LegacySessionSeal
        ));
        let runtime = store.verify_runtime().unwrap();
        assert!(!runtime.seal_ok);
        assert!(runtime.issues.iter().any(|issue| issue == "invalid_seal"));
    }

    #[test]
    fn save_binding_rejects_empty_providers_and_bad_alias() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();

        let empty = Binding::from_body(BindingBody {
            id: "bnd_e".into(),
            alias: "empty".into(),
            tenant: "t".into(),
            principal: None,
            description: None,
            policy: Policy::default(),
            providers: vec![],
        });
        assert!(store.save_binding(&empty).is_err());

        let bad_alias = Binding::from_body(BindingBody {
            id: "bnd_b".into(),
            alias: "not valid!".into(),
            tenant: "t".into(),
            principal: None,
            description: None,
            policy: Policy::default(),
            providers: vec![ProviderBinding {
                provider: "github".into(),
                account: "a".into(),
                credential_ref: "phm:X".into(),
                scope: Scope::default(),
                upstream: None,
            }],
        });
        assert!(store.save_binding(&bad_alias).is_err());
    }

    #[test]
    fn save_binding_rejects_reserved_locus_prefix() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        for alias in ["locus", "locusx", "locus-client"] {
            let b = Binding::from_body(BindingBody {
                id: format!("bnd_{alias}"),
                alias: (*alias).into(),
                tenant: "t".into(),
                principal: None,
                description: None,
                policy: Policy::default(),
                providers: vec![ProviderBinding {
                    provider: "github".into(),
                    account: "a".into(),
                    credential_ref: "phm:X".into(),
                    scope: Scope::default(),
                    upstream: None,
                }],
            });
            let err = store.save_binding(&b).unwrap_err();
            assert!(
                err.to_string().contains("reserved"),
                "alias {alias} must be rejected as reserved: {err}"
            );
            assert!(!store.bindings_dir().join(format!("{alias}.toml")).exists());
        }
        // Non-prefixed alias containing "locus" elsewhere is fine.
        let ok = Binding::from_body(BindingBody {
            id: "bnd_my-locus".into(),
            alias: "my-locus".into(),
            tenant: "t".into(),
            principal: None,
            description: None,
            policy: Policy::default(),
            providers: vec![ProviderBinding {
                provider: "github".into(),
                account: "a".into(),
                credential_ref: "phm:X".into(),
                scope: Scope::default(),
                upstream: None,
            }],
        });
        store.save_binding(&ok).unwrap();
    }

    #[test]
    fn engagement_init_and_close_archives_audit() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("locus-home");
        let project = dir.path().join("client");
        fs::create_dir_all(&project).unwrap();
        let store = Store::open(&home).unwrap();

        let init = store
            .engagement_init("acme", "acme-corp", &project, true, true, false)
            .unwrap();
        assert_eq!(init.alias, "acme");
        assert!(init.binding_path.exists());
        assert!(init.workspace_path.as_ref().unwrap().exists());
        assert!(init.readme_path.as_ref().unwrap().exists());
        assert!(init.credentials.iter().all(|credential| credential.present));
        assert!(init
            .credentials
            .iter()
            .all(|credential| credential.source == "phantom"));
        let init_json = serde_json::to_string(&init).unwrap();
        assert!(!init_json.contains("SUPABASE_ACME"));

        // Pin and generate audit, then close with archive
        store.pin("acme", &project, None, false).unwrap();
        assert!(store.require_active().is_ok());
        let closed = store.engagement_close("acme", true).unwrap();
        assert!(closed.left_session);
        assert!(closed.archive_path.is_some());
        let ap = PathBuf::from(closed.archive_path.as_ref().unwrap());
        assert!(ap.exists());
        let meta = store.load_engagement_meta("acme").unwrap().unwrap();
        assert!(meta.is_closed());
        // Binding file kept; vault secrets untouched
        assert!(store.load_binding("acme").is_ok());
        assert!(matches!(
            store.require_active().unwrap_err(),
            LocusError::NotPinned
        ));
    }

    #[test]
    fn pin_auto_uses_workspace_default() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path().join("home")).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        let project = dir.path().join("proj");
        fs::create_dir_all(&project).unwrap();
        fs::write(
            project.join(".locus.toml"),
            r#"
version = 1
default_binding = "acme"
"#,
        )
        .unwrap();
        let s = store.pin_auto(&project, None, false).unwrap();
        assert_eq!(s.binding_alias, "acme");
        assert!(matches!(s.source, PinSource::Dir { .. }));
    }

    #[test]
    fn pin_auto_delegated_refuses_with_honest_operator_delegation_error() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path().join("home")).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        let project = dir.path().join("proj");
        fs::create_dir_all(&project).unwrap();
        // Even a fully operator-shaped workspace (default + allowlist) cannot
        // let an agent/MCP runtime self-pin.
        fs::write(
            project.join(".locus.toml"),
            r#"
version = 1
default_binding = "acme"
allowed_bindings = ["acme"]
"#,
        )
        .unwrap();
        let err = store
            .pin_auto_delegated(&project, Some("mcp-auto".into()), false)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("auto-pin requires operator delegation"),
            "unexpected: {err}"
        );
        assert!(err.contains("locus enter"), "unexpected: {err}");
        // Fail closed: nothing was pinned.
        assert!(store.active_session().unwrap().is_none());
    }

    #[test]
    fn create_run_session_does_not_overwrite_active_pin() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        store
            .save_binding(&sample_binding("personal", "personal", "p2"))
            .unwrap();

        let active = store.pin("acme", dir.path(), None, false).unwrap();
        let active_id = active.session_id.clone();

        let (run_sess, run_path) = store
            .create_run_session("personal", dir.path(), None, false, false, "12345")
            .unwrap();
        assert_eq!(run_sess.binding_alias, "personal");
        assert!(run_path.exists());
        assert!(run_path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("run-"));

        // Global pin unchanged
        let still = store.active_session().unwrap().unwrap();
        assert_eq!(still.session_id, active_id);
        assert_eq!(still.binding_alias, "acme");

        // Run session has LOCUS-ready identity
        assert!(!run_sess.seal.is_empty());
        assert!(run_sess.binding_fp.is_some());
        assert!(matches!(run_sess.source, PinSource::Run));

        store.cleanup_run_session(&run_path, &run_sess).unwrap();
        assert!(!run_path.exists());
        // Active still fine
        assert_eq!(
            store.active_session().unwrap().unwrap().session_id,
            active_id
        );
    }

    #[test]
    fn create_run_session_works_without_global_pin() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        assert!(store.active_session().unwrap().is_none());

        let (run_sess, run_path) = store
            .create_run_session("acme", dir.path(), None, false, false, "pid99")
            .unwrap();
        assert_eq!(run_sess.binding_alias, "acme");
        assert!(store.active_session().unwrap().is_none());
        assert!(run_path.exists());
        store.cleanup_run_session(&run_path, &run_sess).unwrap();
    }

    #[test]
    fn create_run_session_share_pin_updates_active() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        store
            .save_binding(&sample_binding("personal", "personal", "p2"))
            .unwrap();
        store.pin("acme", dir.path(), None, false).unwrap();

        let (run_sess, _) = store
            .create_run_session("personal", dir.path(), None, false, true, "share1")
            .unwrap();
        let active = store.active_session().unwrap().unwrap();
        assert_eq!(active.session_id, run_sess.session_id);
        assert_eq!(active.binding_alias, "personal");
    }

    #[test]
    fn create_ci_session_does_not_touch_active() {
        let _env_guard = lock_session_env();
        std::env::remove_var("LOCUS_SESSION_ID");

        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        store
            .save_binding(&sample_binding("personal", "personal", "p2"))
            .unwrap();

        let active = store.pin("acme", dir.path(), None, false).unwrap();
        let active_id = active.session_id.clone();

        let (ci, path) = store
            .create_ci_session("personal", dir.path(), false, Some(Duration::minutes(15)))
            .unwrap();
        assert_eq!(ci.binding_alias, "personal");
        assert!(matches!(ci.source, PinSource::Ci));
        assert!(path.exists());
        assert!(path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("ci-"));
        // TTL should be ~15m (binding max is 1h)
        let span = ci.expires_at - ci.pinned_at;
        assert!(span <= Duration::minutes(16));
        assert!(span >= Duration::minutes(14));

        // Global pin unchanged
        assert_eq!(
            store.active_session().unwrap().unwrap().session_id,
            active_id
        );

        // LOCUS_SESSION_ID resolves the CI session
        std::env::set_var("LOCUS_SESSION_ID", &ci.session_id);
        let resolved = store.require_active().unwrap();
        assert_eq!(resolved.session_id, ci.session_id);
        assert_eq!(resolved.binding_alias, "personal");
        std::env::remove_var("LOCUS_SESSION_ID");

        // After clearing env, active is still acme
        assert_eq!(
            store.active_session().unwrap().unwrap().binding_alias,
            "acme"
        );

        store.cleanup_ci_session(&path, &ci).unwrap();
        assert!(!path.exists());
    }

    /// Every file under LOCUS_HOME (recursively) must be free of `needle`.
    fn assert_no_file_contains(root: &Path, needle: &str) {
        fn walk(dir: &Path, needle: &str) {
            let Ok(entries) = fs::read_dir(dir) else {
                return;
            };
            for ent in entries.flatten() {
                let path = ent.path();
                if path.is_dir() {
                    walk(&path, needle);
                } else if let Ok(raw) = fs::read_to_string(&path) {
                    assert!(
                        !raw.contains(needle),
                        "secret material leaked into {}",
                        path.display()
                    );
                }
            }
        }
        walk(root, needle);
    }

    #[test]
    fn mcp_grant_mint_verify_roundtrip_and_no_secret_at_rest() {
        let _env_guard = lock_session_env();
        std::env::remove_var("LOCUS_SESSION_ID");
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();

        let (session, grant, token) = store
            .create_mcp_grant(
                "acme",
                dir.path(),
                Some(Duration::minutes(30)),
                Some("job-1".into()),
                false,
            )
            .unwrap();
        // Token shape: lmt_<16 hex>.<64 hex>, parseable, id matches grant.
        let (gid, secret) = parse_mcp_grant_token(&token).expect("token parses");
        assert_eq!(gid, grant.grant_id);
        assert!(is_safe_mcp_grant_id(gid));
        assert_eq!(secret.len(), 64);
        assert_eq!(grant.session_id, session.session_id);
        assert_eq!(grant.binding_alias, "acme");
        assert!(!grant.revoked);

        // active.json untouched; grant session resolvable + delegated.
        assert!(store.active_session().unwrap().is_none());
        let resolved = store
            .load_session_by_id_resolved(&grant.session_id)
            .unwrap()
            .expect("grant session resolvable");
        assert_eq!(resolved.session.authority, SessionAuthority::Delegated);

        // Verify roundtrip.
        let verified = store
            .verify_mcp_grant_token(&token)
            .expect("token verifies");
        assert_eq!(verified.grant_id, grant.grant_id);

        // Secret never at rest: not in the grant file, audit log, or ANY file.
        let grant_path = store
            .mcp_grants_dir()
            .join(format!("{}.json", grant.grant_id));
        assert!(grant_path.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&grant_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "grant file must be 0600");
        }
        assert_no_file_contains(dir.path(), secret);
        assert_no_file_contains(dir.path(), &token);
        let audit_raw = fs::read_to_string(store.audit_path()).unwrap();
        assert!(audit_raw.contains("mcp.grant_mint"));
        assert!(!audit_raw.contains("lmt_"), "token leaked into audit");

        // list surfaces it.
        let listed = store.list_mcp_grants().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].grant_id, grant.grant_id);
    }

    #[test]
    fn secret_file_create_new_refuses_overwrite() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("grant.json");
        write_secret_file_new(&path, b"first").unwrap();
        // Second write to the same path must fail closed, leaving the
        // original bytes untouched.
        assert!(write_secret_file_new(&path, b"second").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"first");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn mcp_grant_verify_fail_closed_matrix() {
        let _env_guard = lock_session_env();
        std::env::remove_var("LOCUS_SESSION_ID");
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        let (_s, grant, token) = store
            .create_mcp_grant("acme", dir.path(), Some(Duration::minutes(30)), None, false)
            .unwrap();

        let is_invalid = |t: &str| {
            matches!(
                store.verify_mcp_grant_token(t),
                Err(McpGrantAuthError::Invalid { .. })
            )
        };
        // Malformed / unknown / wrong secret → uniform Invalid.
        assert!(is_invalid(""));
        assert!(is_invalid("not-a-token"));
        assert!(is_invalid(&format!(
            "lmt_{}.{}",
            "0".repeat(16),
            "0".repeat(64)
        )));
        let (gid, _) = parse_mcp_grant_token(&token).unwrap();
        assert!(is_invalid(&format!("lmt_{gid}.{}", "0".repeat(64))));

        // Tampered token_seal → Invalid (MAC mismatch).
        let path = store.mcp_grants_dir().join(format!("{gid}.json"));
        let mut g: McpGrant = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let original = fs::read_to_string(&path).unwrap();
        g.token_seal = "hmac-sha256:deadbeef".into();
        fs::write(&path, serde_json::to_string_pretty(&g).unwrap()).unwrap();
        assert!(is_invalid(&token));
        fs::write(&path, &original).unwrap();

        // Corrupt file → Invalid.
        fs::write(&path, "{not json").unwrap();
        assert!(is_invalid(&token));
        fs::write(&path, &original).unwrap();

        // Expired (MAC still valid) → Expired, not Invalid.
        let mut g: McpGrant = serde_json::from_str(&original).unwrap();
        g.expires_at = Utc::now() - Duration::minutes(1);
        fs::write(&path, serde_json::to_string_pretty(&g).unwrap()).unwrap();
        assert!(matches!(
            store.verify_mcp_grant_token(&token),
            Err(McpGrantAuthError::Expired { .. })
        ));
        fs::write(&path, &original).unwrap();

        // Revoked flag → Invalid (uniform; revocation is not advertised).
        let mut g: McpGrant = serde_json::from_str(&original).unwrap();
        g.revoked = true;
        fs::write(&path, serde_json::to_string_pretty(&g).unwrap()).unwrap();
        assert!(is_invalid(&token));
        fs::write(&path, &original).unwrap();

        // Deleted grant file → Invalid.
        fs::remove_file(&path).unwrap();
        assert!(is_invalid(&token));
        let _ = grant;
    }

    #[test]
    fn mcp_grant_dead_flag_only_for_revoked_or_deleted() {
        let _env_guard = lock_session_env();
        std::env::remove_var("LOCUS_SESSION_ID");
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        let (_s, grant, token) = store
            .create_mcp_grant("acme", dir.path(), Some(Duration::minutes(30)), None, false)
            .unwrap();
        let (gid, _) = parse_mcp_grant_token(&token).unwrap();
        let dead_of = |t: &str| match store.verify_mcp_grant_token(t) {
            Err(McpGrantAuthError::Invalid { grant_dead, .. }) => grant_dead,
            other => panic!("expected Invalid, got {other:?}"),
        };

        // Parse failure: no grant id, never dead.
        assert!(!dead_of("not-a-token"));
        // MAC mismatch on a LIVE grant: NOT dead — a forged token naming a
        // real grant must never be able to trigger a session/worker sweep.
        assert!(!dead_of(&format!("lmt_{gid}.{}", "0".repeat(64))));

        // Revoked-marked record (crash between mark + delete): dead.
        let path = store.mcp_grants_dir().join(format!("{gid}.json"));
        let original = fs::read_to_string(&path).unwrap();
        let mut g: McpGrant = serde_json::from_str(&original).unwrap();
        g.revoked = true;
        fs::write(&path, serde_json::to_string_pretty(&g).unwrap()).unwrap();
        assert!(dead_of(&token), "MAC-verified revoked grant is dead");
        fs::write(&path, &original).unwrap();

        // Full revoke (deletes the record): dead — the running MT server uses
        // this to sweep the grant's sessions and tear down its workers.
        store.revoke_mcp_grant(&grant.grant_id).unwrap().unwrap();
        assert!(dead_of(&token), "deleted grant record is dead");
    }

    #[test]
    fn mcp_grant_revoke_removes_grant_session_and_worker_home() {
        let _env_guard = lock_session_env();
        std::env::remove_var("LOCUS_SESSION_ID");
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        let (session, grant, token) = store
            .create_mcp_grant("acme", dir.path(), Some(Duration::minutes(30)), None, false)
            .unwrap();
        let worker_home = PathBuf::from(&session.worker_home);
        fs::create_dir_all(&worker_home).unwrap();
        let session_path = store
            .load_session_by_id_resolved(&grant.session_id)
            .unwrap()
            .unwrap()
            .path;

        let revoked = store.revoke_mcp_grant(&grant.grant_id).unwrap().unwrap();
        assert!(revoked.revoked);
        assert!(!store
            .mcp_grants_dir()
            .join(format!("{}.json", grant.grant_id))
            .exists());
        assert!(!session_path.exists(), "grant session file must be removed");
        assert!(!worker_home.exists(), "worker home must be removed");
        assert!(matches!(
            store.verify_mcp_grant_token(&token),
            Err(McpGrantAuthError::Invalid { .. })
        ));
        // Idempotent: second revoke is a no-op.
        assert!(store.revoke_mcp_grant(&grant.grant_id).unwrap().is_none());
        let audit_raw = fs::read_to_string(store.audit_path()).unwrap();
        assert!(audit_raw.contains("mcp.grant_revoke"));
    }

    #[test]
    fn check_drift_and_freeze_for_freezes_only_that_grant() {
        let _env_guard = lock_session_env();
        std::env::remove_var("LOCUS_SESSION_ID");
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        store
            .save_binding(&sample_binding("beta", "beta-corp", "p2"))
            .unwrap();
        store
            .save_binding(&sample_binding("gamma", "gamma-corp", "p3"))
            .unwrap();

        // Global pin + two grants.
        let active = store.pin("gamma", dir.path(), None, false).unwrap();
        let (_sa, grant_a, _ta) = store
            .create_mcp_grant("acme", dir.path(), Some(Duration::minutes(30)), None, false)
            .unwrap();
        let (_sb, grant_b, _tb) = store
            .create_mcp_grant("beta", dir.path(), Some(Duration::minutes(30)), None, false)
            .unwrap();

        // Mutate acme's binding material → drift for grant A only.
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1-changed"))
            .unwrap();

        let resolved_a = store
            .load_session_by_id_resolved(&grant_a.session_id)
            .unwrap()
            .unwrap();
        let drift_a = store.check_drift_and_freeze_for(&resolved_a).unwrap();
        assert!(!drift_a.ok);
        assert!(drift_a.frozen, "grant A must freeze on providers drift");

        // Grant A's session FILE is frozen; B and active are untouched.
        let re_a = store
            .load_session_by_id_resolved(&grant_a.session_id)
            .unwrap()
            .unwrap();
        assert!(re_a.session.frozen);
        let re_b = store
            .load_session_by_id_resolved(&grant_b.session_id)
            .unwrap()
            .unwrap();
        assert!(!re_b.session.frozen, "grant B must not be frozen");
        let drift_b = store.check_drift_and_freeze_for(&re_b).unwrap();
        assert!(drift_b.ok, "grant B stays healthy: {:?}", drift_b.issues);
        let active_now = store.active_session().unwrap().unwrap();
        assert_eq!(active_now.session_id, active.session_id);
        assert!(!active_now.frozen, "active.json must be untouched");
    }

    #[test]
    fn whoami_for_reports_grant_binding() {
        let _env_guard = lock_session_env();
        std::env::remove_var("LOCUS_SESSION_ID");
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        store
            .save_binding(&sample_binding("beta", "beta-corp", "p2"))
            .unwrap();
        store.pin("beta", dir.path(), None, false).unwrap();

        let (_s, grant, _t) = store
            .create_mcp_grant("acme", dir.path(), Some(Duration::minutes(30)), None, false)
            .unwrap();
        let resolved = store
            .load_session_by_id_resolved(&grant.session_id)
            .unwrap()
            .unwrap();
        let who = store.whoami_for(&resolved).unwrap();
        assert_eq!(who.binding_alias, "acme");
        assert_eq!(who.tenant, "acme-corp");
        assert_eq!(who.session_id, grant.session_id);
        // Global whoami still answers as the operator pin.
        assert_eq!(store.whoami().unwrap().binding_alias, "beta");
    }

    #[test]
    fn create_ci_session_ttl_capped_by_max_ttl() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        // sample_binding has max_ttl = 1h
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        let (ci, path) = store
            .create_ci_session("acme", dir.path(), false, Some(Duration::hours(48)))
            .unwrap();
        let span = ci.expires_at - ci.pinned_at;
        assert!(span <= Duration::hours(1) + Duration::seconds(5));
        store.cleanup_ci_session(&path, &ci).unwrap();
    }

    #[test]
    fn pin_with_ttl_shortens_expiry() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        let s = store
            .pin_with_ttl("acme", dir.path(), None, false, Some(Duration::minutes(2)))
            .unwrap();
        let span = s.expires_at - s.pinned_at;
        assert!(span <= Duration::minutes(2) + Duration::seconds(5));
        assert!(span >= Duration::minutes(1) + Duration::seconds(55));
    }

    #[test]
    fn pin_with_ttl_capped_by_max_ttl() {
        // sample_binding has max_ttl = 1h; a 48h request must be clamped.
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        let s = store
            .pin_with_ttl("acme", dir.path(), None, false, Some(Duration::hours(48)))
            .unwrap();
        let span = s.expires_at - s.pinned_at;
        assert!(span <= Duration::hours(1) + Duration::seconds(5));
    }

    #[test]
    fn default_ttl_used_when_no_flag_and_flag_beats_default() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let mut b = sample_binding("acme", "acme-corp", "p1");
        b.policy.default_ttl = Some("30m".into());
        store.save_binding(&b).unwrap();

        // No explicit request → policy.default_ttl wins over max_ttl.
        let s = store.pin("acme", dir.path(), None, false).unwrap();
        let span = s.expires_at - s.pinned_at;
        assert!(span <= Duration::minutes(30) + Duration::seconds(5));
        assert!(span >= Duration::minutes(29));

        // Explicit request beats default_ttl.
        let s = store
            .pin_with_ttl("acme", dir.path(), None, false, Some(Duration::minutes(5)))
            .unwrap();
        let span = s.expires_at - s.pinned_at;
        assert!(span <= Duration::minutes(5) + Duration::seconds(5));

        // Audit trail records ttl_secs + ttl_source (values-free).
        let events = store.read_audit_events().unwrap();
        let sources: Vec<String> = events
            .iter()
            .filter(|e| e.op == "session.pin")
            .filter_map(|e| e.detail.as_ref())
            .filter_map(|d| d.get("ttl_source"))
            .map(|v| v.as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(sources, vec!["binding_default", "flag"]);
    }

    #[test]
    fn default_ttl_capped_by_max_ttl() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let mut b = sample_binding("acme", "acme-corp", "p1");
        b.policy.default_ttl = Some("4h".into()); // max_ttl = 1h
        store.save_binding(&b).unwrap();
        let s = store.pin("acme", dir.path(), None, false).unwrap();
        let span = s.expires_at - s.pinned_at;
        assert!(span <= Duration::hours(1) + Duration::seconds(5));
    }

    #[test]
    fn expired_ttl_session_fails_verify_without_timer() {
        // Auto-leave needs no timer: expiry is enforced passively by verify()
        // on the next privileged op.
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        let s = store
            .pin_with_ttl("acme", dir.path(), None, false, Some(Duration::seconds(1)))
            .unwrap();
        std::thread::sleep(std::time::Duration::from_secs(2));
        let key = store.seal_key().unwrap();
        assert!(matches!(s.verify(&key), Err(LocusError::SessionExpired(_))));
        // Fail closed on the store surface too.
        assert!(store.require_active().is_err());
    }

    #[test]
    fn create_ci_session_respects_workspace_allowlist() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        store
            .save_binding(&sample_binding("personal", "personal", "p2"))
            .unwrap();
        let project = dir.path().join("proj");
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

        let err = store
            .create_ci_session("personal", &project, false, Some(Duration::minutes(5)))
            .unwrap_err();
        assert!(matches!(err, LocusError::BindingNotAllowed(_)));

        // --force bypasses
        let (ci, path) = store
            .create_ci_session("personal", &project, true, Some(Duration::minutes(5)))
            .unwrap();
        assert_eq!(ci.binding_alias, "personal");
        store.cleanup_ci_session(&path, &ci).unwrap();
    }

    #[test]
    fn create_ci_session_audits_mint() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        let (ci, path) = store
            .create_ci_session("acme", dir.path(), false, Some(Duration::minutes(10)))
            .unwrap();
        let events = store.read_audit_events().unwrap();
        assert!(events.iter().any(|e| e.op == "ci.mint"));
        store.cleanup_ci_session(&path, &ci).unwrap();
    }

    #[test]
    fn exact_ci_drift_freezes_ci_file_without_replacing_global_pin() {
        let _env_guard = lock_session_env();
        std::env::remove_var("LOCUS_SESSION_ID");

        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let mut acme = sample_binding("acme", "acme-corp", "p1");
        store.save_binding(&acme).unwrap();
        store
            .save_binding(&sample_binding("personal", "personal", "p2"))
            .unwrap();
        let global = store.pin("personal", dir.path(), None, false).unwrap();
        let (ci, ci_path) = store
            .create_ci_session("acme", dir.path(), false, Some(Duration::minutes(10)))
            .unwrap();

        acme.providers[0].scope.project_ref = Some("drifted".into());
        store.save_binding(&acme).unwrap();
        std::env::set_var("LOCUS_SESSION_ID", &ci.session_id);
        let drift = store.check_drift_and_freeze().unwrap();
        assert!(drift.frozen);
        std::env::remove_var("LOCUS_SESSION_ID");

        let still_global = store.active_session().unwrap().unwrap();
        assert_eq!(still_global.session_id, global.session_id);
        assert_eq!(still_global.binding_alias, "personal");
        assert!(!still_global.frozen);

        let frozen_ci: Session =
            serde_json::from_str(&fs::read_to_string(&ci_path).unwrap()).unwrap();
        assert_eq!(frozen_ci.session_id, ci.session_id);
        assert!(frozen_ci.frozen);
        frozen_ci.verify_seal(&store.seal_key().unwrap()).unwrap();
        store.cleanup_ci_session(&ci_path, &frozen_ci).unwrap();
    }

    #[test]
    fn locus_session_id_missing_does_not_fallthrough() {
        let _env_guard = lock_session_env();
        std::env::remove_var("LOCUS_SESSION_ID");

        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        store.pin("acme", dir.path(), None, false).unwrap();

        std::env::set_var("LOCUS_SESSION_ID", "ses_doesnotexist0000");
        // Fail closed: env points at missing session → NotPinned, not active.json
        assert!(store.require_active().is_err());
        std::env::remove_var("LOCUS_SESSION_ID");
        assert!(store.require_active().is_ok());
    }

    #[test]
    fn check_drift_and_freeze_on_providers_change() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let mut b = sample_binding("acme", "acme-corp", "p1");
        store.save_binding(&b).unwrap();
        store.pin("acme", dir.path(), None, false).unwrap();
        assert!(store.verify_runtime().unwrap().ok);

        // Mutate provider scope under the pin
        b.providers[0].scope.project_ref = Some("mutated_ref".into());
        store.save_binding(&b).unwrap();

        let d = store.check_drift_and_freeze().unwrap();
        assert!(!d.ok);
        assert!(d.frozen);
        assert!(d.issues.iter().any(|i| i == "providers_drift"));
        assert!(d.issues.iter().any(|i| i == "session_frozen"));

        let sess = store.active_session().unwrap().unwrap();
        assert!(sess.frozen);
        assert!(matches!(
            store.require_active().unwrap_err(),
            LocusError::SessionFrozen(_)
        ));

        // Re-pin clears freeze
        store.leave().unwrap();
        store.pin("acme", dir.path(), None, false).unwrap();
        assert!(store.verify_runtime().unwrap().ok);
        assert!(!store.active_session().unwrap().unwrap().frozen);
    }

    #[test]
    fn check_drift_and_freeze_on_tenant_change() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let mut b = sample_binding("acme", "acme-corp", "p1");
        store.save_binding(&b).unwrap();
        store.pin("acme", dir.path(), None, false).unwrap();

        b.tenant = "evil-corp".into();
        store.save_binding(&b).unwrap();
        let d = store.check_drift_and_freeze().unwrap();
        assert!(d.frozen);
        assert!(d.issues.iter().any(|i| i == "tenant_drift"));
    }

    #[test]
    fn pin_namespaced_two_bindings() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        store
            .save_binding(&sample_binding("personal", "personal", "p2"))
            .unwrap();

        let s = store
            .pin_namespaced(
                &["acme".into(), "personal".into()],
                dir.path(),
                Some("cli".into()),
                false,
            )
            .unwrap();
        assert!(s.is_namespaced());
        assert_eq!(s.binding_alias, "acme");
        assert_eq!(s.namespaces, vec!["personal".to_string()]);
        assert_eq!(s.all_aliases(), vec!["acme", "personal"]);
        assert_eq!(s.namespace_fps.len(), 1);
        assert!(s.binding_fp.is_some());

        let d = store.verify_runtime().unwrap();
        assert!(d.ok);

        // Drift on secondary freezes
        let mut personal = store.load_binding("personal").unwrap();
        personal.providers[0].account = "mutated".into();
        store.save_binding(&personal).unwrap();
        let d2 = store.check_drift_and_freeze().unwrap();
        assert!(d2.frozen);
        assert!(d2
            .issues
            .iter()
            .any(|i| i == "namespace_drift" || i == "providers_drift"));
    }

    #[test]
    fn pin_namespaced_requires_two() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        assert!(store
            .pin_namespaced(&["acme".into()], dir.path(), None, false)
            .is_err());
    }

    #[test]
    fn graph_export_import_roundtrip() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path().join("src")).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "proj_acme"))
            .unwrap();
        store
            .save_binding(&sample_binding("personal", "personal", "proj_me"))
            .unwrap();
        store
            .save_workspace_template(
                "acme",
                &WorkspaceConfig {
                    version: 1,
                    default_binding: Some("acme".into()),
                    allowed_bindings: vec!["acme".into()],
                    require_pin: true,
                },
            )
            .unwrap();

        let out = dir.path().join("team.locusgraph");
        let exp = store.graph_export(None, &out, "test").expect("export");
        assert_eq!(exp.binding_aliases.len(), 2);
        assert!(exp.workspace_names.contains(&"acme".to_string()));
        assert!(out.exists());

        // Ciphertext must not contain plaintext credential refs or alias secrets
        let raw = fs::read(&out).unwrap();
        assert!(raw.starts_with(crate::graph::MAGIC));
        let as_str = String::from_utf8_lossy(&raw);
        assert!(
            !as_str.contains("phm:SUPABASE_ACME"),
            "encrypted file must not leak cleartext credential_refs"
        );
        assert!(!as_str.contains("proj_acme"));

        // Import into a fresh home
        let dest = Store::open(dir.path().join("dst")).unwrap();
        let imp = dest.graph_import(&out, "test", false).expect("import");
        assert_eq!(imp.bindings_imported.len(), 2);
        assert!(imp.bindings_skipped.is_empty());
        assert_eq!(imp.workspaces_imported, vec!["acme".to_string()]);

        let acme = dest.load_binding("acme").unwrap();
        assert_eq!(acme.tenant, "acme-corp");
        assert_eq!(
            acme.provider("supabase").unwrap().credential_ref,
            "phm:SUPABASE_ACME"
        );
        assert_eq!(
            acme.provider("supabase")
                .unwrap()
                .scope
                .project_ref
                .as_deref(),
            Some("proj_acme")
        );
        let ws = dest.load_workspace_template("acme").unwrap();
        assert_eq!(ws.default_binding.as_deref(), Some("acme"));
        assert!(ws.require_pin);

        // Re-import without force skips existing
        let imp2 = dest.graph_import(&out, "test", false).unwrap();
        assert!(imp2.bindings_imported.is_empty());
        assert_eq!(imp2.bindings_skipped.len(), 2);

        // force overwrites
        let imp3 = dest.graph_import(&out, "test", true).unwrap();
        assert_eq!(imp3.bindings_imported.len(), 2);

        // Audit events recorded
        let events = store.read_audit_events().unwrap();
        assert!(events.iter().any(|e| e.op == "graph.export"));
        let events_d = dest.read_audit_events().unwrap();
        assert!(events_d.iter().any(|e| e.op == "graph.import"));
    }

    #[test]
    fn graph_export_filter_bindings() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        store
            .save_binding(&sample_binding("personal", "personal", "p2"))
            .unwrap();
        let out = dir.path().join("partial.locusgraph");
        let exp = store
            .graph_export(Some(&["acme".into()]), &out, "test")
            .unwrap();
        assert_eq!(exp.binding_aliases, vec!["acme".to_string()]);

        let dest = Store::open(dir.path().join("dst")).unwrap();
        let imp = dest.graph_import(&out, "test", false).unwrap();
        assert_eq!(imp.bindings_imported, vec!["acme".to_string()]);
        assert!(dest.load_binding("personal").is_err());
    }

    #[test]
    fn graph_list_surface() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        store
            .save_workspace_template(
                "acme",
                &WorkspaceConfig {
                    version: 1,
                    default_binding: Some("acme".into()),
                    allowed_bindings: vec!["acme".into()],
                    require_pin: true,
                },
            )
            .unwrap();
        let list = store.graph_list().unwrap();
        assert!(list.iter().any(|e| e.kind == "binding" && e.name == "acme"));
        assert!(list
            .iter()
            .any(|e| e.kind == "workspace" && e.name == "acme"));
        // List output carries sources only, never locator names.
        let acme = list
            .iter()
            .find(|e| e.name == "acme" && e.kind == "binding");
        let credentials = &acme.unwrap().credentials;
        assert!(credentials.iter().all(|credential| credential.present));
        assert!(credentials
            .iter()
            .all(|credential| credential.source == "phantom"));
        assert!(!serde_json::to_string(&list)
            .unwrap()
            .contains("SUPABASE_ACME"));
    }

    #[test]
    fn graph_wrong_passphrase_fails() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp", "p1"))
            .unwrap();
        let out = dir.path().join("g.locusgraph");
        store.graph_export(None, &out, "correct").unwrap();
        let dest = Store::open(dir.path().join("dst")).unwrap();
        let err = dest.graph_import(&out, "wrong", false).unwrap_err();
        assert!(
            err.to_string().contains("decrypt") || err.to_string().contains("passphrase"),
            "{err}"
        );
    }

    #[test]
    fn capability_ticket_mint_and_verify() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let t = store
            .mint_capability_ticket("ses_1", "bnd_acme", "github.scope")
            .unwrap();
        assert!(t.ticket_id.starts_with("cap_"));
        store.verify_capability_ticket(&t).unwrap();
        store
            .verify_capability_ticket_parts(
                &t.ticket_id,
                &t.session_id,
                &t.binding_id,
                &t.tool,
                t.exp,
            )
            .unwrap();

        // Wrong tool fails
        let err = store
            .verify_capability_ticket_parts(
                &t.ticket_id,
                &t.session_id,
                &t.binding_id,
                "other.tool",
                t.exp,
            )
            .unwrap_err();
        assert!(err.to_string().contains("HMAC") || err.to_string().contains("mismatch"));

        // Cross-store with different daemon key fails
        let other = Store::open(dir.path().join("other-home")).unwrap();
        assert!(other.verify_capability_ticket(&t).is_err());
    }

    #[test]
    fn capability_ticket_ttl_override() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let t = store
            .mint_capability_ticket_ttl("ses", "bnd", "tool.x", Duration::seconds(120))
            .unwrap();
        let now = Utc::now().timestamp();
        assert!(t.exp >= now + 100);
        store.verify_capability_ticket(&t).unwrap();
    }
}

//! `doctor --fix` logic: plan repairs, apply them only on an explicit call.
//!
//! Wired to the CLI as `locus doctor --fix [--apply]`. Dry run is the default: [`plan_fixes`] reads and
//! returns a [`FixPlan`] and writes nothing. [`apply_fixes`] is the explicit
//! step; it backs up every file before changing it.
//!
//! Scope:
//! - Stale workspace allowlist entries and a stale `default_binding` in
//!   `.locus.toml` (naming no binding in the store) are removed or cleared.
//!   Removing the last allowlist entry is refused: an empty `allowed_bindings`
//!   means "allow every binding", which would widen access.
//! - Legacy bare Phantom names are migrated to `phm:NAME` through the
//!   existing crash-consistent `Store::migrate_legacy_credential_refs`.
//! - Malformed references and phm references Phantom does not list are
//!   reported only, never rewritten.
//!
//! Never resolves or reveals a credential, never runs `phantom reveal`, never
//! reads a value. Credential reference strings and locator names never appear
//! in a plan or an outcome; only binding aliases, provider labels and codes do.
//! Phantom names, when known, are passed in by the caller (from
//! `phantom list`), so this module never starts Phantom itself.

use crate::binding::Binding;
use crate::credential::{migrate_legacy_phantom_ref, CredentialRef};
use crate::error::{LocusError, Result};
use crate::store::Store;
use crate::workspace::find_workspace;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// A repair `apply_fixes` may perform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum PlannedFix {
    /// Remove `entry` from `allowed_bindings` in `file`.
    StaleAllowlistEntry { file: PathBuf, entry: String },
    /// Clear `default_binding` (`entry`) in `file`. A replacement is never picked.
    StaleDefaultBinding { file: PathBuf, entry: String },
    /// Rewrite legacy bare Phantom names in binding `alias` to `phm:NAME`.
    LegacyPhantomRef {
        alias: String,
        providers: Vec<String>,
    },
}

impl PlannedFix {
    /// One value-free line describing the change.
    pub fn describe(&self) -> String {
        match self {
            Self::StaleAllowlistEntry { file, entry } => format!(
                "{}: remove allowed_bindings entry '{entry}' (no such binding)",
                file.display()
            ),
            Self::StaleDefaultBinding { file, entry } => format!(
                "{}: clear default_binding '{entry}' (no such binding)",
                file.display()
            ),
            Self::LegacyPhantomRef { alias, providers } => format!(
                "binding '{alias}': rewrite legacy bare Phantom names to phm: form for {}",
                providers.join(", ")
            ),
        }
    }
}

/// A finding that is reported and never changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportOnly {
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
    pub message: String,
}

/// What `doctor --fix` would do. Building it writes nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixPlan {
    pub fixes: Vec<PlannedFix>,
    pub report_only: Vec<ReportOnly>,
}

impl FixPlan {
    pub fn is_empty(&self) -> bool {
        self.fixes.is_empty() && self.report_only.is_empty()
    }
}

pub const LAST_ENTRY_MESSAGE: &str = "refusing to remove the last allowed_bindings entry: an empty list allows every binding. Edit the file by hand to name the bindings this directory may use";

/// Bindings found by reading `bindings/*.toml` directly (the strict loaders
/// refuse a legacy or malformed binding, which is exactly what we report).
struct ScannedBinding {
    alias: String,
    binding: Option<Binding>,
}

fn safe_label(s: &str) -> String {
    if !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        s.to_string()
    } else {
        "unknown".to_string()
    }
}

fn scan_bindings(store: &Store) -> Result<Vec<ScannedBinding>> {
    let dir = store.bindings_dir();
    let mut out = Vec::new();
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
        let Some(alias) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let binding = fs::read_to_string(&path)
            .ok()
            .and_then(|raw| Binding::parse_toml(&raw).ok());
        out.push(ScannedBinding {
            alias: alias.to_string(),
            binding,
        });
    }
    Ok(out)
}

/// Plan repairs for `store` and the workspace that governs `cwd`.
///
/// `phantom_names` is the name list from `phantom list` when the caller has
/// one; `None` (Phantom off PATH or the listing failed) reports every phm
/// reference as unavailable, never fewer findings (fail closed).
pub fn plan_fixes(
    store: &Store,
    cwd: &Path,
    phantom_names: Option<&BTreeSet<String>>,
) -> Result<FixPlan> {
    let scanned = scan_bindings(store)?;
    let mut plan = FixPlan::default();

    // References.
    for sb in &scanned {
        let alias = safe_label(&sb.alias);
        let Some(b) = &sb.binding else {
            plan.report_only.push(ReportOnly {
                code: "binding_unparseable".into(),
                alias: Some(alias),
                provider: None,
                file: None,
                message: "binding file does not parse; fix it by hand".into(),
            });
            continue;
        };
        let mut legacy = Vec::new();
        for p in &b.providers {
            let provider = safe_label(&p.provider);
            let raw = p.credential_ref.as_str();
            if migrate_legacy_phantom_ref(raw).is_some() {
                legacy.push(provider);
                continue;
            }
            match CredentialRef::validate(raw) {
                Err(_) => plan.report_only.push(ReportOnly {
                    code: "malformed_ref".into(),
                    alias: Some(alias.clone()),
                    provider: Some(provider),
                    file: None,
                    message: "credential_ref is not a valid phm:NAME or env:VAR reference; edit it by hand".into(),
                }),
                Ok(CredentialRef::Phantom { name }) => {
                    let missing = phantom_names.map(|names| !names.contains(&name));
                    match missing {
                        None => plan.report_only.push(ReportOnly {
                            code: "phm_unavailable".into(),
                            alias: Some(alias.clone()),
                            provider: Some(provider),
                            file: None,
                            message: "Phantom names could not be listed, so this phm reference is unverified".into(),
                        }),
                        Some(true) => plan.report_only.push(ReportOnly {
                            code: "dangling_phm_ref".into(),
                            alias: Some(alias.clone()),
                            provider: Some(provider),
                            file: None,
                            message: "Phantom does not list this phm reference; add it in Phantom or edit the binding".into(),
                        }),
                        Some(false) => {}
                    }
                }
                Ok(_) => {}
            }
        }
        if !legacy.is_empty() {
            plan.fixes.push(PlannedFix::LegacyPhantomRef {
                alias: sb.alias.clone(),
                providers: legacy,
            });
        }
    }

    // Workspace allowlist and default binding.
    let ws = match find_workspace(cwd) {
        Ok(ws) => ws,
        Err(_) => {
            plan.report_only.push(ReportOnly {
                code: "workspace_unreadable".into(),
                alias: None,
                provider: None,
                file: None,
                message: "the workspace .locus.toml could not be read or parsed; fix it by hand"
                    .into(),
            });
            None
        }
    };
    if let Some((file, cfg)) = ws {
        if scanned.iter().any(|s| s.binding.is_none()) {
            plan.report_only.push(ReportOnly {
                code: "allowlist_inconclusive".into(),
                alias: None,
                provider: None,
                file: Some(file.clone()),
                message: "a binding file does not parse, so allowlist entries are not checked"
                    .into(),
            });
        } else {
            let known = known_names(&scanned);
            let stale: Vec<&String> = cfg
                .allowed_bindings
                .iter()
                .filter(|e| !known.contains(e.as_str()))
                .collect();
            let keeps_one = stale.len() < cfg.allowed_bindings.len();
            for (i, entry) in stale.iter().enumerate() {
                if !keeps_one && i == stale.len() - 1 {
                    plan.report_only.push(ReportOnly {
                        code: "allowlist_last_entry".into(),
                        alias: None,
                        provider: None,
                        file: Some(file.clone()),
                        message: format!("'{entry}' names no binding, but {LAST_ENTRY_MESSAGE}"),
                    });
                    continue;
                }
                plan.fixes.push(PlannedFix::StaleAllowlistEntry {
                    file: file.clone(),
                    entry: (*entry).clone(),
                });
            }
            if let Some(d) = &cfg.default_binding {
                if !known.contains(d.as_str()) {
                    plan.fixes.push(PlannedFix::StaleDefaultBinding {
                        file: file.clone(),
                        entry: d.clone(),
                    });
                }
            }
        }
    }
    Ok(plan)
}

fn known_names(scanned: &[ScannedBinding]) -> BTreeSet<String> {
    let mut known = BTreeSet::new();
    for s in scanned {
        known.insert(s.alias.clone());
        if let Some(b) = &s.binding {
            known.insert(b.alias.clone());
            known.insert(b.id.clone());
        }
    }
    known
}

/// What happened to one planned fix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedFix {
    pub fix: PlannedFix,
    pub applied: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixOutcome {
    pub results: Vec<AppliedFix>,
}

impl FixOutcome {
    pub fn all_applied(&self) -> bool {
        self.results.iter().all(|r| r.applied)
    }
}

/// Copy `src` to a new `dest` (never overwriting), mode 0600 on Unix.
fn backup_copy(src: &Path, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = fs::read(src)?;
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(dest)?;
    f.write_all(&bytes)?;
    f.sync_all()?;
    Ok(())
}

fn unique_path(base: PathBuf) -> PathBuf {
    if !base.exists() {
        return base;
    }
    for n in 1..1000 {
        let p = PathBuf::from(format!("{}-{n}", base.display()));
        if !p.exists() {
            return p;
        }
    }
    base
}

fn stamp() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}

/// Apply `plan`. The explicit step: requires local control, re-checks each
/// fix against the current files, backs up each file before its first
/// change, and refuses to empty an allowlist. A file whose backup fails is
/// not changed.
pub fn apply_fixes(store: &Store, plan: &FixPlan) -> Result<FixOutcome> {
    store.require_local_control("doctor fix")?;
    let ts = stamp();
    let mut out = FixOutcome::default();

    // Workspace files: group all edits per file so it is backed up and
    // rewritten once.
    let mut files: Vec<PathBuf> = Vec::new();
    for f in &plan.fixes {
        if let PlannedFix::StaleAllowlistEntry { file, .. }
        | PlannedFix::StaleDefaultBinding { file, .. } = f
        {
            if !files.contains(file) {
                files.push(file.clone());
            }
        }
    }
    let known = known_names(&scan_bindings(store)?);
    for file in files {
        let fixes: Vec<&PlannedFix> = plan
            .fixes
            .iter()
            .filter(|f| matches!(f, PlannedFix::StaleAllowlistEntry { file: p, .. } | PlannedFix::StaleDefaultBinding { file: p, .. } if *p == file))
            .collect();
        out.results
            .extend(apply_workspace_file(&file, &fixes, &known, &ts));
    }

    for f in &plan.fixes {
        if let PlannedFix::LegacyPhantomRef { alias, .. } = f {
            out.results.push(apply_legacy(store, f, alias, &ts));
        }
    }
    Ok(out)
}

fn failed(fix: &PlannedFix, message: impl Into<String>) -> AppliedFix {
    AppliedFix {
        fix: fix.clone(),
        applied: false,
        backup: None,
        message: Some(message.into()),
    }
}

fn apply_workspace_file(
    file: &Path,
    fixes: &[&PlannedFix],
    known: &BTreeSet<String>,
    ts: &str,
) -> Vec<AppliedFix> {
    let all_failed = |msg: &str| fixes.iter().map(|f| failed(f, msg)).collect::<Vec<_>>();
    match fs::symlink_metadata(file) {
        Ok(m) if m.file_type().is_symlink() => {
            return all_failed("workspace file is a symlink; edit it by hand")
        }
        Ok(_) => {}
        Err(_) => return all_failed("workspace file is gone"),
    }
    let Ok(raw) = fs::read_to_string(file) else {
        return all_failed("workspace file is unreadable");
    };
    // A generic table keeps keys this version does not know about.
    let Ok(mut table) = raw.parse::<toml::Table>() else {
        return all_failed("workspace file does not parse");
    };
    let mut allowed: Vec<String> = match table.get("allowed_bindings") {
        None => Vec::new(),
        Some(toml::Value::Array(a)) => {
            let v: Option<Vec<String>> = a.iter().map(|x| x.as_str().map(str::to_string)).collect();
            match v {
                Some(v) => v,
                None => return all_failed("allowed_bindings is not a list of names"),
            }
        }
        Some(_) => return all_failed("allowed_bindings is not a list of names"),
    };
    let mut results = Vec::new();
    let mut changed = false;
    for fix in fixes {
        match fix {
            PlannedFix::StaleAllowlistEntry { entry, .. } => {
                if known.contains(entry) {
                    results.push(failed(fix, "the binding exists now; entry kept"));
                } else if !allowed.contains(entry) {
                    results.push(failed(fix, "entry is no longer in the file"));
                } else if allowed.len() == 1 {
                    results.push(failed(fix, LAST_ENTRY_MESSAGE));
                } else {
                    allowed.retain(|e| e != entry);
                    changed = true;
                    results.push(AppliedFix {
                        fix: (*fix).clone(),
                        applied: true,
                        backup: None,
                        message: None,
                    });
                }
            }
            PlannedFix::StaleDefaultBinding { entry, .. } => {
                let current = table.get("default_binding").and_then(|v| v.as_str());
                if known.contains(entry) {
                    results.push(failed(fix, "the binding exists now; default kept"));
                } else if current != Some(entry.as_str()) {
                    results.push(failed(fix, "default_binding changed since the plan"));
                } else {
                    table.remove("default_binding");
                    changed = true;
                    results.push(AppliedFix {
                        fix: (*fix).clone(),
                        applied: true,
                        backup: None,
                        message: None,
                    });
                }
            }
            PlannedFix::LegacyPhantomRef { .. } => {}
        }
    }
    if !changed {
        return results;
    }
    if table.contains_key("allowed_bindings") {
        table.insert(
            "allowed_bindings".into(),
            toml::Value::Array(allowed.into_iter().map(toml::Value::String).collect()),
        );
    }
    let backup = unique_path(PathBuf::from(format!("{}.bak-{ts}", file.display())));
    let fail_all = |results: Vec<AppliedFix>, msg: &str| {
        results
            .into_iter()
            .map(|r| if r.applied { failed(&r.fix, msg) } else { r })
            .collect::<Vec<_>>()
    };
    if backup_copy(file, &backup).is_err() {
        return fail_all(results, "backup could not be written; file not changed");
    }
    let Ok(text) = toml::to_string_pretty(&table) else {
        return fail_all(
            results,
            "could not serialize the workspace file; not changed",
        );
    };
    let tmp = PathBuf::from(format!("{}.doctor-fix.tmp", file.display()));
    let written = fs::write(&tmp, text).and_then(|_| fs::rename(&tmp, file));
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
        return fail_all(results, "write failed; file not changed (backup kept)");
    }
    results
        .into_iter()
        .map(|mut r| {
            if r.applied {
                r.backup = Some(backup.clone());
            }
            r
        })
        .collect()
}

fn apply_legacy(store: &Store, fix: &PlannedFix, alias: &str, ts: &str) -> AppliedFix {
    if crate::binding::validate_name_component("alias", alias).is_err() {
        return failed(fix, "invalid binding alias");
    }
    let src = store.bindings_dir().join(format!("{alias}.toml"));
    match fs::symlink_metadata(&src) {
        Ok(m) if m.file_type().is_symlink() => {
            return failed(fix, "binding file is a symlink; edit it by hand")
        }
        Ok(_) => {}
        Err(_) => return failed(fix, "binding file is gone"),
    }
    let backup = unique_path(
        store
            .home()
            .join("backups")
            .join(format!("doctor-fix-{ts}"))
            .join(format!("{alias}.toml")),
    );
    if backup_copy(&src, &backup).is_err() {
        return failed(fix, "backup could not be written; binding not changed");
    }
    match store.migrate_legacy_credential_refs(alias, true) {
        Ok(m) if m.written || m.migrated > 0 => AppliedFix {
            fix: fix.clone(),
            applied: true,
            backup: Some(backup),
            message: None,
        },
        Ok(_) => AppliedFix {
            fix: fix.clone(),
            applied: false,
            backup: Some(backup),
            message: Some("nothing to migrate any more".into()),
        },
        Err(e) => AppliedFix {
            fix: fix.clone(),
            applied: false,
            backup: Some(backup),
            message: Some(redact_err(&e)),
        },
    }
}

/// Error text that cannot carry a reference: the migration's own errors
/// never echo values, but keep only the first line and cap it.
fn redact_err(e: &LocusError) -> String {
    let s = e.to_string();
    let first = s.lines().next().unwrap_or("migration failed");
    first.chars().take(200).collect()
}

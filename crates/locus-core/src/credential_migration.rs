//! Crash-consistent migration of legacy bare Phantom credential references.

use crate::binding::{validate_name_component, Binding};
use crate::credential::migrate_legacy_phantom_ref;
use crate::error::{LocusError, Result};
use crate::store::{CredentialRefMigration, Store};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::ffi::CString;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

use rand::{rngs::OsRng, RngCore};

const JOURNAL_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum JournalPhase {
    Intent,
    Committed,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

#[derive(Debug)]
struct AuthenticatedFile {
    file: File,
    identity: FileIdentity,
    hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MigrationJournal {
    schema_version: u32,
    alias: String,
    transaction_id: String,
    original_hash: String,
    target_hash: String,
    migrated: usize,
    phase: JournalPhase,
    original_identity: FileIdentity,
    installed_identity: FileIdentity,
    staged_name: String,
    backup_name: String,
}

#[derive(Debug)]
struct MigrationPaths {
    binding: PathBuf,
    lock: PathBuf,
    journal: PathBuf,
    staged: PathBuf,
    backup: PathBuf,
    parent: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MigrationStage {
    AfterIntent,
    BeforeBackupRename,
    AfterBackupRename,
    BeforeInstall,
    AfterInstall,
    BeforeParentSync,
    BeforeCommitted,
    BeforeAudit,
    BeforeCompleted,
}

pub(crate) trait MigrationHooks {
    fn hit(&mut self, _stage: MigrationStage, _binding: &Path) -> std::io::Result<()> {
        Ok(())
    }
}

struct NoopHooks;
impl MigrationHooks for NoopHooks {}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct MigrationReadiness {
    pub pending: usize,
    pub invalid: usize,
    pub scan_failed: bool,
}

impl MigrationReadiness {
    pub fn ready(self) -> bool {
        self.pending == 0 && self.invalid == 0 && !self.scan_failed
    }
}

pub(crate) fn migration_readiness(store: &Store) -> MigrationReadiness {
    let mut readiness = MigrationReadiness::default();
    let _lock = match lock_bindings(store) {
        Ok(lock) => lock,
        Err(_) => {
            readiness.scan_failed = true;
            return readiness;
        }
    };
    let entries = match fs::read_dir(store.bindings_dir()) {
        Ok(entries) => entries,
        Err(_) => {
            readiness.scan_failed = true;
            return readiness;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                readiness.invalid += 1;
                continue;
            }
        };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(alias) = name
            .strip_prefix('.')
            .and_then(|name| name.strip_suffix(".credential-migration.json"))
        else {
            continue;
        };
        if validate_name_component("alias", alias).is_err() {
            readiness.invalid += 1;
            continue;
        }
        let journal = match read_journal(&entry.path(), alias) {
            Ok(journal) => journal,
            Err(_) => {
                readiness.invalid += 1;
                continue;
            }
        };
        let binding = store.bindings_dir().join(format!("{alias}.toml"));
        let lock = store.bindings_dir().join(".credential-migrations.lock");
        let paths = match paths_for(&store.bindings_dir(), &binding, &lock, &journal) {
            Ok(paths) => paths,
            Err(_) => {
                readiness.invalid += 1;
                continue;
            }
        };
        if validate_journal(&journal, &paths).is_err() {
            readiness.invalid += 1;
        } else if journal.phase != JournalPhase::Completed {
            readiness.pending += 1;
        } else if authenticate_exact(
            &paths.binding,
            &journal.target_hash,
            journal.installed_identity,
            alias,
        )
        .is_err()
        {
            readiness.invalid += 1;
        }
    }
    readiness
}

pub(crate) fn migrate(store: &Store, alias: &str, write: bool) -> Result<CredentialRefMigration> {
    migrate_with_hooks(store, alias, write, &mut NoopHooks)
}

pub(crate) fn migrate_with_hooks(
    store: &Store,
    alias: &str,
    write: bool,
    hooks: &mut dyn MigrationHooks,
) -> Result<CredentialRefMigration> {
    validate_name_component("alias", alias)?;
    let binding_path = store.bindings_dir().join(format!("{alias}.toml"));
    ensure_direct_child(&store.bindings_dir(), &binding_path)?;

    if !write {
        let raw = read_authenticated(&binding_path, alias)?.1;
        let (_, migrated) = migration_target(&raw, alias)?;
        return Ok(CredentialRefMigration {
            alias: alias.into(),
            migrated,
            written: false,
            audit_pending: false,
            recovery_pending: false,
            recovered: false,
        });
    }

    let parent = store.bindings_dir();
    let lock_path = parent.join(".credential-migrations.lock");
    let journal_path = parent.join(format!(".{alias}.credential-migration.json"));

    // A prior crash may have moved the binding aside. Reconcile its durable
    // intent under the lock before attempting to read the public path.
    if journal_path.exists() {
        let _lock = lock_path_exclusive(&lock_path)?;
        if journal_path.exists() {
            let journal = read_journal(&journal_path, alias)?;
            let paths = paths_for(&parent, &binding_path, &lock_path, &journal)?;
            if journal.phase != JournalPhase::Completed
                || authenticate_exact(
                    &binding_path,
                    &journal.target_hash,
                    journal.installed_identity,
                    alias,
                )
                .is_ok()
            {
                return reconcile(store, journal, &paths, hooks, true);
            }
        }
    }

    let (original, raw) = read_authenticated(&binding_path, alias)?;
    let original_hash = original.hash.clone();
    let (target, migrated) = migration_target(&raw, alias)?;
    if migrated == 0 {
        let _lock = lock_path_exclusive(&lock_path)?;
        authenticate_exact(&binding_path, &original_hash, original.identity, alias)?;
        return Ok(CredentialRefMigration {
            alias: alias.into(),
            migrated: 0,
            written: false,
            audit_pending: false,
            recovery_pending: false,
            recovered: false,
        });
    }

    let target_hash = content_hash(target.as_bytes());
    let transaction_id = transaction_id(alias, &original_hash, &target_hash);
    let mut journal = MigrationJournal {
        schema_version: JOURNAL_SCHEMA_VERSION,
        alias: alias.into(),
        transaction_id,
        original_hash,
        target_hash,
        migrated,
        phase: JournalPhase::Intent,
        original_identity: original.identity,
        installed_identity: FileIdentity {
            device: 0,
            inode: 0,
        },
        staged_name: String::new(),
        backup_name: String::new(),
    };
    journal.staged_name = randomized_artifact_name(&journal, "new");
    journal.backup_name = randomized_artifact_name(&journal, "old");
    let paths = paths_for(&parent, &binding_path, &lock_path, &journal)?;

    // Stage and validate the exact target before obtaining authority to mutate.
    let staged_auth = stage_target(
        &paths.staged,
        target.as_bytes(),
        &journal.target_hash,
        alias,
    )?;
    journal.installed_identity = staged_auth.identity;
    let staged_raw = read_file_text(&staged_auth.file, alias, "staged binding is unreadable")?;
    let staged = Binding::parse_toml(&staged_raw)
        .map_err(|_| safe_error(alias, "staged binding is invalid"))?;
    staged
        .validate()
        .map_err(|_| safe_error(alias, "staged binding is invalid"))?;

    let _lock = lock_path_exclusive(&paths.lock)?;
    if paths.journal.exists() {
        let existing = read_journal(&paths.journal, alias)?;
        if existing.phase != JournalPhase::Completed {
            let existing_paths = paths_for(&parent, &binding_path, &lock_path, &existing)?;
            return reconcile(store, existing, &existing_paths, hooks, true);
        }
        if authenticate_exact(
            &binding_path,
            &existing.target_hash,
            existing.installed_identity,
            alias,
        )
        .is_ok()
        {
            let existing_paths = paths_for(&parent, &binding_path, &lock_path, &existing)?;
            return reconcile(store, existing, &existing_paths, hooks, true);
        }
    }

    authenticate_exact(
        &binding_path,
        &journal.original_hash,
        journal.original_identity,
        alias,
    )?;
    write_journal(&paths.journal, &journal, paths.journal.exists())?;
    hooks
        .hit(MigrationStage::AfterIntent, &paths.binding)
        .map_err(|_| safe_error(alias, "migration interrupted; retry safely"))?;

    reconcile(store, journal, &paths, hooks, false)
}

fn reconcile(
    store: &Store,
    mut journal: MigrationJournal,
    paths: &MigrationPaths,
    hooks: &mut dyn MigrationHooks,
    recovered: bool,
) -> Result<CredentialRefMigration> {
    validate_journal(&journal, paths)?;
    let alias = journal.alias.as_str();

    if journal.phase == JournalPhase::Completed {
        authenticate_exact(
            &paths.binding,
            &journal.target_hash,
            journal.installed_identity,
            alias,
        )?;
        let _ = fs::remove_file(&paths.staged);
        let _ = fs::remove_file(&paths.backup);
        let _ = sync_parent(&paths.parent);
        return Ok(result(&journal, true, recovered));
    }

    let already_installed = authenticate_exact(
        &paths.binding,
        &journal.target_hash,
        journal.installed_identity,
        alias,
    )
    .is_ok()
        && authenticate_exact(
            &paths.backup,
            &journal.original_hash,
            journal.original_identity,
            alias,
        )
        .is_ok();

    if !already_installed {
        let binding_original = authenticate_exact(
            &paths.binding,
            &journal.original_hash,
            journal.original_identity,
            alias,
        )
        .is_ok();
        let backup_original = authenticate_exact(
            &paths.backup,
            &journal.original_hash,
            journal.original_identity,
            alias,
        )
        .is_ok();
        match (
            binding_original,
            path_missing(&paths.backup)?,
            path_missing(&paths.binding)?,
            backup_original,
        ) {
            (true, true, false, false) => {
                hooks
                    .hit(MigrationStage::BeforeBackupRename, &paths.binding)
                    .map_err(|_| safe_error(alias, "migration rename failed; retry safely"))?;
                authenticate_exact(
                    &paths.binding,
                    &journal.original_hash,
                    journal.original_identity,
                    alias,
                )?;
                atomic_rename_noreplace(&paths.binding, &paths.backup)
                    .map_err(|_| safe_error(alias, "migration rename failed; retry safely"))?;
                authenticate_exact(
                    &paths.backup,
                    &journal.original_hash,
                    journal.original_identity,
                    alias,
                )?;
                hooks
                    .hit(MigrationStage::AfterBackupRename, &paths.binding)
                    .map_err(|_| safe_error(alias, "migration interrupted; retry safely"))?;
            }
            (false, false, true, true) => {}
            _ => return Err(concurrent_error(alias)),
        }

        authenticate_exact(
            &paths.staged,
            &journal.target_hash,
            journal.installed_identity,
            alias,
        )
        .map_err(|_| safe_error(alias, "migration staging is incomplete; retry safely"))?;
        hooks
            .hit(MigrationStage::BeforeInstall, &paths.binding)
            .map_err(|_| safe_error(alias, "migration install failed; retry safely"))?;
        authenticate_exact(
            &paths.staged,
            &journal.target_hash,
            journal.installed_identity,
            alias,
        )?;
        atomic_rename_noreplace(&paths.staged, &paths.binding)
            .map_err(|_| concurrent_error(alias))?;
        authenticate_exact(
            &paths.binding,
            &journal.target_hash,
            journal.installed_identity,
            alias,
        )?;
        hooks
            .hit(MigrationStage::AfterInstall, &paths.binding)
            .map_err(|_| safe_error(alias, "migration interrupted; retry safely"))?;
    }

    authenticate_transaction(&journal, paths)?;
    hooks
        .hit(MigrationStage::BeforeParentSync, &paths.binding)
        .map_err(|_| safe_error(alias, "migration durability pending; retry safely"))?;
    let installed = authenticate_transaction(&journal, paths)?;
    installed.file.sync_all()?;
    sync_parent(&paths.parent)
        .map_err(|_| safe_error(alias, "migration durability pending; retry safely"))?;

    if journal.phase == JournalPhase::Intent {
        hooks
            .hit(MigrationStage::BeforeCommitted, &paths.binding)
            .map_err(|_| safe_error(alias, "migration commit pending; retry safely"))?;
        authenticate_transaction(&journal, paths)?;
        journal.phase = JournalPhase::Committed;
        write_journal(&paths.journal, &journal, true)?;
    }

    let audit_pending = hooks
        .hit(MigrationStage::BeforeAudit, &paths.binding)
        .is_err()
        || authenticate_transaction(&journal, paths).is_err()
        || append_audit_once(store, &journal).is_err();
    if audit_pending {
        return Ok(CredentialRefMigration {
            audit_pending: true,
            recovery_pending: true,
            recovered,
            ..result(&journal, true, recovered)
        });
    }

    hooks
        .hit(MigrationStage::BeforeCompleted, &paths.binding)
        .map_err(|_| safe_error(alias, "migration completion pending; retry safely"))?;
    authenticate_transaction(&journal, paths)?;
    journal.phase = JournalPhase::Completed;
    if write_journal(&paths.journal, &journal, true).is_err() {
        return Ok(CredentialRefMigration {
            audit_pending: false,
            recovery_pending: true,
            recovered,
            ..result(&journal, true, recovered)
        });
    }

    if authenticate_transaction(&journal, paths).is_err() {
        return Ok(CredentialRefMigration {
            audit_pending: false,
            recovery_pending: true,
            recovered,
            ..result(&journal, true, recovered)
        });
    }

    let _ = fs::remove_file(&paths.staged);
    let _ = fs::remove_file(&paths.backup);
    let _ = sync_parent(&paths.parent);
    Ok(result(&journal, true, recovered))
}

fn result(journal: &MigrationJournal, written: bool, recovered: bool) -> CredentialRefMigration {
    CredentialRefMigration {
        alias: journal.alias.clone(),
        migrated: journal.migrated,
        written,
        audit_pending: false,
        recovery_pending: false,
        recovered,
    }
}

fn migration_target(raw: &str, alias: &str) -> Result<(String, usize)> {
    let mut binding =
        Binding::parse_toml(raw).map_err(|_| safe_error(alias, "binding is malformed"))?;
    if binding.alias != alias {
        return Err(safe_error(
            alias,
            "binding alias does not match its filename",
        ));
    }
    let mut migrated = 0usize;
    for provider in &mut binding.providers {
        if crate::credential::CredentialRef::validate(&provider.credential_ref).is_ok() {
            continue;
        }
        provider.credential_ref =
            migrate_legacy_phantom_ref(&provider.credential_ref).ok_or_else(|| {
                safe_error(
                    alias,
                    "has an unsafe credential reference; edit it manually to use phm:NAME or env:VAR",
                )
            })?;
        migrated += 1;
    }
    binding
        .validate()
        .map_err(|_| safe_error(alias, "binding remains invalid after migration"))?;
    let target = binding.to_toml()?;
    let reparsed =
        Binding::parse_toml(&target).map_err(|_| safe_error(alias, "staged binding is invalid"))?;
    reparsed
        .validate()
        .map_err(|_| safe_error(alias, "staged binding is invalid"))?;
    Ok((target, migrated))
}

fn paths_for(
    parent: &Path,
    binding: &Path,
    lock: &Path,
    journal: &MigrationJournal,
) -> Result<MigrationPaths> {
    validate_artifact_name(journal, &journal.staged_name, "new")?;
    validate_artifact_name(journal, &journal.backup_name, "old")?;
    Ok(MigrationPaths {
        binding: binding.to_path_buf(),
        lock: lock.to_path_buf(),
        journal: parent.join(format!(".{}.credential-migration.json", journal.alias)),
        staged: parent.join(&journal.staged_name),
        backup: parent.join(&journal.backup_name),
        parent: parent.to_path_buf(),
    })
}

fn validate_journal(journal: &MigrationJournal, paths: &MigrationPaths) -> Result<()> {
    if journal.schema_version != JOURNAL_SCHEMA_VERSION
        || validate_name_component("alias", &journal.alias).is_err()
        || journal.transaction_id
            != transaction_id(&journal.alias, &journal.original_hash, &journal.target_hash)
        || journal.original_hash.len() != 64
        || journal.target_hash.len() != 64
        || journal.original_identity.inode == 0
        || journal.installed_identity.inode == 0
    {
        return Err(safe_error(&journal.alias, "migration journal is invalid"));
    }
    ensure_direct_child(&paths.parent, &paths.binding)?;
    ensure_direct_child(&paths.parent, &paths.journal)?;
    ensure_direct_child(&paths.parent, &paths.staged)?;
    ensure_direct_child(&paths.parent, &paths.backup)?;
    Ok(())
}

fn randomized_artifact_name(journal: &MigrationJournal, suffix: &str) -> String {
    format!(
        ".{}.credential-migration.{}.{:016x}.{}",
        journal.alias,
        journal.transaction_id,
        OsRng.next_u64(),
        suffix
    )
}

fn validate_artifact_name(journal: &MigrationJournal, name: &str, suffix: &str) -> Result<()> {
    let prefix = format!(
        ".{}.credential-migration.{}.",
        journal.alias, journal.transaction_id
    );
    let expected_suffix = format!(".{suffix}");
    let direct = Path::new(name).file_name().is_some_and(|leaf| leaf == name);
    if !direct || !name.starts_with(&prefix) || !name.ends_with(&expected_suffix) {
        return Err(safe_error(&journal.alias, "migration journal is invalid"));
    }
    Ok(())
}

pub(crate) fn lock_bindings(store: &Store) -> Result<File> {
    lock_path_exclusive(&store.bindings_dir().join(".credential-migrations.lock"))
}

fn lock_path_exclusive(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    let file = options.open(path)?;
    require_regular_single_link(&file)
        .map_err(|_| LocusError::msg("credential migration lock is unsafe"))?;
    file.lock()?;
    Ok(file)
}

fn stage_target(
    path: &Path,
    bytes: &[u8],
    expected_hash: &str,
    alias: &str,
) -> Result<AuthenticatedFile> {
    let mut options = OpenOptions::new();
    options.create_new(true).read(true).write(true);
    #[cfg(unix)]
    options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    let authenticated = authenticate_handle(file)?;
    if authenticated.hash != expected_hash
        || authenticate_exact(path, expected_hash, authenticated.identity, alias).is_err()
    {
        return Err(safe_error(alias, "migration staging verification failed"));
    }
    Ok(authenticated)
}

fn write_journal(path: &Path, journal: &MigrationJournal, replace: bool) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| LocusError::msg("migration journal has no parent"))?;
    let temp = parent.join(format!(
        ".{}.journal-write.{:016x}.tmp",
        journal.transaction_id,
        OsRng.next_u64()
    ));
    let mut bytes = serde_json::to_vec(journal)?;
    bytes.push(b'\n');
    let expected_hash = content_hash(&bytes);
    let staged = stage_target(&temp, &bytes, &expected_hash, &journal.alias)?;
    if replace {
        fs::rename(&temp, path)?;
    } else {
        atomic_rename_noreplace(&temp, path)?;
    }
    authenticate_exact(path, &expected_hash, staged.identity, &journal.alias)?;
    sync_parent(parent)?;
    Ok(())
}

fn read_journal(path: &Path, alias: &str) -> Result<MigrationJournal> {
    let authenticated = open_authenticated(path)
        .map_err(|_| safe_error(alias, "migration journal is unreadable"))?;
    let raw = read_file_text(
        &authenticated.file,
        alias,
        "migration journal is unreadable",
    )?;
    let journal: MigrationJournal = serde_json::from_str(&raw)
        .map_err(|_| safe_error(alias, "migration journal is malformed"))?;
    if journal.alias != alias {
        return Err(safe_error(alias, "migration journal alias mismatch"));
    }
    Ok(journal)
}

fn append_audit_once(store: &Store, journal: &MigrationJournal) -> std::io::Result<()> {
    let path = store.audit_path();
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(path)?;
    file.lock()?;
    file.seek(SeekFrom::Start(0))?;
    let mut existing = String::new();
    file.read_to_string(&mut existing)?;
    let already_recorded = existing.lines().any(|line| {
        serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .and_then(|value| value.get("detail").cloned())
            .and_then(|detail| detail.get("migration_id").cloned())
            .and_then(|value| value.as_str().map(str::to_owned))
            .as_deref()
            == Some(journal.transaction_id.as_str())
    });
    if !already_recorded {
        if !existing.is_empty() && !existing.ends_with('\n') {
            file.write_all(b"\n")?;
            file.sync_all()?;
        }
        let event = serde_json::json!({
            "ts": chrono::Utc::now().to_rfc3339(),
            "op": "binding.credential_refs_migrated",
            "binding": journal.alias,
            "detail": {
                "migration_id": journal.transaction_id,
                "migrated": journal.migrated,
            }
        });
        let mut line = serde_json::to_vec(&event).map_err(std::io::Error::other)?;
        line.push(b'\n');
        file.write_all(&line)?;
        file.sync_all()?;
    }
    Ok(())
}

fn authenticate_transaction(
    journal: &MigrationJournal,
    paths: &MigrationPaths,
) -> Result<AuthenticatedFile> {
    let installed = authenticate_exact(
        &paths.binding,
        &journal.target_hash,
        journal.installed_identity,
        &journal.alias,
    )?;
    authenticate_exact(
        &paths.backup,
        &journal.original_hash,
        journal.original_identity,
        &journal.alias,
    )?;
    Ok(installed)
}

fn read_authenticated(path: &Path, alias: &str) -> Result<(AuthenticatedFile, String)> {
    let authenticated =
        open_authenticated(path).map_err(|_| safe_error(alias, "binding is unreadable"))?;
    let raw = read_file_text(&authenticated.file, alias, "binding is unreadable")?;
    Ok((authenticated, raw))
}

fn read_file_text(file: &File, alias: &str, message: &str) -> Result<String> {
    let mut reader = file.try_clone().map_err(|_| safe_error(alias, message))?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|_| safe_error(alias, message))?;
    let mut raw = String::new();
    reader
        .read_to_string(&mut raw)
        .map_err(|_| safe_error(alias, message))?;
    Ok(raw)
}

fn open_authenticated(path: &Path) -> Result<AuthenticatedFile> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(LocusError::msg("credential migration file is unsafe"));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    authenticate_handle(options.open(path)?)
}

fn authenticate_handle(mut file: File) -> Result<AuthenticatedFile> {
    let identity = require_regular_single_link(&file)?;
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(AuthenticatedFile {
        file,
        identity,
        hash: content_hash(&bytes),
    })
}

fn authenticate_exact(
    path: &Path,
    expected_hash: &str,
    expected_identity: FileIdentity,
    alias: &str,
) -> Result<AuthenticatedFile> {
    let authenticated = open_authenticated(path).map_err(|_| concurrent_error(alias))?;
    if authenticated.hash != expected_hash || authenticated.identity != expected_identity {
        return Err(concurrent_error(alias));
    }
    Ok(authenticated)
}

#[cfg(unix)]
fn require_regular_single_link(file: &File) -> Result<FileIdentity> {
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() || metadata.nlink() != 1 {
        return Err(LocusError::msg("credential migration file is unsafe"));
    }
    Ok(FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(not(unix))]
fn require_regular_single_link(_file: &File) -> Result<FileIdentity> {
    Err(LocusError::msg(
        "secure credential migration is unsupported on this platform",
    ))
}

fn path_missing(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error.into()),
    }
}

fn content_hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn transaction_id(alias: &str, original_hash: &str, target_hash: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(alias.as_bytes());
    hasher.update(b"\0");
    hasher.update(original_hash.as_bytes());
    hasher.update(b"\0");
    hasher.update(target_hash.as_bytes());
    hex::encode(hasher.finalize())[..24].to_string()
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn atomic_rename_noreplace(source: &Path, target: &Path) -> std::io::Result<()> {
    let source = CString::new(source.as_os_str().as_bytes())?;
    let target = CString::new(target.as_os_str().as_bytes())?;
    // SAFETY: both C strings are owned for the duration of the syscall.
    let result = unsafe { libc::renamex_np(source.as_ptr(), target.as_ptr(), libc::RENAME_EXCL) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn atomic_rename_noreplace(source: &Path, target: &Path) -> std::io::Result<()> {
    let source = CString::new(source.as_os_str().as_bytes())?;
    let target = CString::new(target.as_os_str().as_bytes())?;
    // SAFETY: both C strings are owned for the duration of the syscall.
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            target.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "linux",
    target_os = "android"
)))]
fn atomic_rename_noreplace(_source: &Path, _target: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "atomic no-replace rename is unavailable",
    ))
}

fn sync_parent(parent: &Path) -> std::io::Result<()> {
    File::open(parent)?.sync_all()
}

fn ensure_direct_child(parent: &Path, path: &Path) -> Result<()> {
    if path.parent() != Some(parent) {
        return Err(LocusError::msg("migration path escaped bindings directory"));
    }
    Ok(())
}

fn safe_error(alias: &str, message: &str) -> LocusError {
    LocusError::msg(format!("binding '{alias}' {message}"))
}

fn concurrent_error(alias: &str) -> LocusError {
    safe_error(
        alias,
        "changed during credential migration; concurrent replacement was not overwritten",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const LOCATOR: &str = "LEGACY_MIGRATION_CANARY";
    const CONCURRENT_LOCATOR: &str = "CONCURRENT_ENV_CANARY";

    fn legacy_binding(alias: &str) -> String {
        format!(
            r#"[binding]
id = "bnd_{alias}"
alias = "{alias}"
tenant = "tenant-{alias}"

[[binding.providers]]
provider = "github"
account = "account-{alias}"
credential_ref = "{LOCATOR}"
"#
        )
    }

    fn concurrent_binding(alias: &str) -> String {
        format!(
            r#"[binding]
id = "bnd_{alias}"
alias = "{alias}"
tenant = "tenant-concurrent"

[[binding.providers]]
provider = "github"
account = "account-concurrent"
credential_ref = "env:{CONCURRENT_LOCATOR}"
"#
        )
    }

    fn setup(alias: &str) -> (tempfile::TempDir, Store, PathBuf) {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path().join("home")).unwrap();
        let path = store.bindings_dir().join(format!("{alias}.toml"));
        fs::write(&path, legacy_binding(alias)).unwrap();
        (dir, store, path)
    }

    fn journal_text(store: &Store, alias: &str) -> String {
        fs::read_to_string(
            store
                .bindings_dir()
                .join(format!(".{alias}.credential-migration.json")),
        )
        .unwrap()
    }

    fn migration_audit_count(store: &Store) -> usize {
        store
            .read_audit_events()
            .unwrap()
            .iter()
            .filter(|event| event.op == "binding.credential_refs_migrated")
            .count()
    }

    struct FailOnce {
        stage: MigrationStage,
        fired: bool,
    }

    impl MigrationHooks for FailOnce {
        fn hit(&mut self, stage: MigrationStage, _binding: &Path) -> std::io::Result<()> {
            if !self.fired && stage == self.stage {
                self.fired = true;
                return Err(std::io::Error::other("injected failure"));
            }
            Ok(())
        }
    }

    struct ReplaceOnce {
        replacement: String,
        fired: bool,
    }

    struct ReplaceAfterInstall {
        fired: bool,
        stage: MigrationStage,
    }

    impl MigrationHooks for ReplaceAfterInstall {
        fn hit(&mut self, stage: MigrationStage, binding: &Path) -> std::io::Result<()> {
            if !self.fired && stage == self.stage {
                self.fired = true;
                let same_bytes = fs::read(binding)?;
                fs::remove_file(binding)?;
                fs::write(binding, same_bytes)?;
            }
            Ok(())
        }
    }

    #[derive(Default)]
    struct MutateOldDescriptor {
        original: Option<File>,
        fired: bool,
    }

    impl MigrationHooks for MutateOldDescriptor {
        fn hit(&mut self, stage: MigrationStage, binding: &Path) -> std::io::Result<()> {
            if stage == MigrationStage::BeforeBackupRename && self.original.is_none() {
                self.original = Some(OpenOptions::new().read(true).write(true).open(binding)?);
            }
            if stage == MigrationStage::AfterInstall && !self.fired {
                self.fired = true;
                let original = self.original.as_mut().unwrap();
                original.seek(SeekFrom::End(0))?;
                original.write_all(b"\n# concurrent old writer")?;
                original.sync_all()?;
            }
            Ok(())
        }
    }

    impl MigrationHooks for ReplaceOnce {
        fn hit(&mut self, stage: MigrationStage, binding: &Path) -> std::io::Result<()> {
            if !self.fired && stage == MigrationStage::BeforeBackupRename {
                self.fired = true;
                fs::write(binding, &self.replacement)?;
            }
            Ok(())
        }
    }

    #[test]
    fn interrupted_migration_reconciles_without_truncation_or_duplicate_audit() {
        for stage in [
            MigrationStage::AfterIntent,
            MigrationStage::BeforeBackupRename,
            MigrationStage::AfterBackupRename,
            MigrationStage::BeforeInstall,
            MigrationStage::AfterInstall,
            MigrationStage::BeforeParentSync,
            MigrationStage::BeforeCommitted,
            MigrationStage::BeforeCompleted,
        ] {
            let alias = format!("retry-{}", stage as u8);
            let (_dir, store, path) = setup(&alias);
            let mut hook = FailOnce {
                stage,
                fired: false,
            };
            let error = migrate_with_hooks(&store, &alias, true, &mut hook)
                .unwrap_err()
                .to_string();
            assert!(!error.contains(LOCATOR));

            let retry = migrate(&store, &alias, true).unwrap();
            assert!(retry.written);
            assert!(retry.recovered);
            assert!(!retry.audit_pending);
            assert!(!retry.recovery_pending);
            let committed = fs::read_to_string(&path).unwrap();
            assert!(committed.contains(&format!("phm:{LOCATOR}")));
            assert_eq!(migration_audit_count(&store), 1);
            assert!(!journal_text(&store, &alias).contains(LOCATOR));
        }
    }

    #[test]
    fn audit_unavailable_reports_committed_and_retry_completes_once() {
        let alias = "audit-retry";
        let (_dir, store, path) = setup(alias);
        let mut hook = FailOnce {
            stage: MigrationStage::BeforeAudit,
            fired: false,
        };
        let committed = migrate_with_hooks(&store, alias, true, &mut hook).unwrap();
        assert!(committed.written);
        assert!(committed.audit_pending);
        assert!(committed.recovery_pending);
        assert!(fs::read_to_string(&path)
            .unwrap()
            .contains(&format!("phm:{LOCATOR}")));
        assert_eq!(migration_audit_count(&store), 0);
        let readiness = migration_readiness(&store);
        assert_eq!(readiness.pending, 1);
        assert!(!readiness.ready());
        let doctor =
            crate::doctor::build_doctor_report(&store, crate::doctor::DoctorExternal::default())
                .unwrap();
        assert_eq!(doctor.verdict, crate::doctor::DoctorVerdict::Unsafe);
        assert!(doctor
            .findings
            .iter()
            .any(|finding| finding.code == "credential_migration_incomplete"));
        let agent = crate::agent_report::agent_report_from_doctor(
            doctor,
            crate::agent_report::AgentReportOptions {
                home_ready: true,
                ..Default::default()
            },
        );
        assert_eq!(agent.status, crate::agent_report::AgentStatus::Unsafe);
        assert!(!agent.ready);

        let retry = migrate(&store, alias, true).unwrap();
        assert!(retry.written);
        assert!(retry.recovered);
        assert!(!retry.audit_pending);
        assert!(!retry.recovery_pending);
        assert_eq!(migration_audit_count(&store), 1);
        assert!(migration_readiness(&store).ready());

        let idempotent = migrate(&store, alias, true).unwrap();
        assert!(idempotent.recovered);
        assert_eq!(migration_audit_count(&store), 1);
        assert!(!journal_text(&store, alias).contains(LOCATOR));
    }

    #[test]
    fn concurrent_replacement_is_preserved_rejected_and_never_disclosed() {
        let alias = "concurrent";
        let (_dir, store, path) = setup(alias);
        let replacement = concurrent_binding(alias);
        let mut hook = ReplaceOnce {
            replacement: replacement.clone(),
            fired: false,
        };

        let error = migrate_with_hooks(&store, alias, true, &mut hook)
            .unwrap_err()
            .to_string();
        assert!(error.contains("concurrent replacement was not overwritten"));
        assert!(!error.contains(LOCATOR));
        assert!(!error.contains(CONCURRENT_LOCATOR));
        assert_eq!(fs::read_to_string(&path).unwrap(), replacement);

        let retry_error = migrate(&store, alias, true).unwrap_err().to_string();
        assert!(retry_error.contains("concurrent replacement was not overwritten"));
        assert!(!retry_error.contains(LOCATOR));
        assert!(!retry_error.contains(CONCURRENT_LOCATOR));
        assert_eq!(fs::read_to_string(&path).unwrap(), replacement);
        assert_eq!(migration_audit_count(&store), 0);
        let journal = journal_text(&store, alias);
        assert!(!journal.contains(LOCATOR));
        assert!(!journal.contains(CONCURRENT_LOCATOR));
    }

    #[test]
    fn partial_audit_tail_is_preserved_and_retry_appends_one_valid_event() {
        let alias = "audit-tail";
        let (_dir, store, _path) = setup(alias);
        fs::write(store.audit_path(), b"{\"partial\":true").unwrap();

        let migrated = migrate(&store, alias, true).unwrap();
        assert!(migrated.written);
        let raw = fs::read_to_string(store.audit_path()).unwrap();
        assert!(raw.starts_with("{\"partial\":true\n"));
        assert_eq!(migration_audit_count(&store), 1);

        let retried = migrate(&store, alias, true).unwrap();
        assert!(retried.written);
        assert!(retried.recovered);
        assert_eq!(migration_audit_count(&store), 1);
        assert!(!raw.contains(LOCATOR));
    }

    #[test]
    fn staging_rejects_matching_symlink_and_hardlink() {
        let dir = tempdir().unwrap();
        let bytes = b"exact target";
        let expected = content_hash(bytes);
        let victim = dir.path().join("victim");
        fs::write(&victim, bytes).unwrap();

        let hardlink = dir.path().join("hardlink.new");
        fs::hard_link(&victim, &hardlink).unwrap();
        let error = stage_target(&hardlink, bytes, &expected, "safe")
            .unwrap_err()
            .to_string();
        assert!(!error.contains("exact target"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let symlink_path = dir.path().join("symlink.new");
            symlink(&victim, &symlink_path).unwrap();
            let error = stage_target(&symlink_path, bytes, &expected, "safe")
                .unwrap_err()
                .to_string();
            assert!(!error.contains("exact target"));
        }
    }

    #[test]
    fn post_install_same_content_replacement_never_commits_or_audits() {
        let alias = "post-install-replacement";
        let (_dir, store, _path) = setup(alias);
        let mut hook = ReplaceAfterInstall {
            fired: false,
            stage: MigrationStage::AfterInstall,
        };
        let error = migrate_with_hooks(&store, alias, true, &mut hook)
            .unwrap_err()
            .to_string();
        assert!(error.contains("concurrent replacement was not overwritten"));
        assert_eq!(migration_audit_count(&store), 0);
        assert_eq!(migration_readiness(&store).pending, 1);
        let retry = migrate(&store, alias, true).unwrap_err().to_string();
        assert!(retry.contains("concurrent replacement was not overwritten"));
    }

    #[test]
    fn post_install_old_writer_never_commits_or_audits() {
        let alias = "post-install-old-writer";
        let (_dir, store, _path) = setup(alias);
        let mut hook = MutateOldDescriptor::default();
        let error = migrate_with_hooks(&store, alias, true, &mut hook)
            .unwrap_err()
            .to_string();
        assert!(error.contains("concurrent replacement was not overwritten"));
        assert_eq!(migration_audit_count(&store), 0);
        assert_eq!(migration_readiness(&store).pending, 1);
        let retry = migrate(&store, alias, true).unwrap_err().to_string();
        assert!(retry.contains("concurrent replacement was not overwritten"));
    }

    #[test]
    fn replacement_before_audit_stays_pending_and_blocks_retry() {
        let alias = "before-audit-replacement";
        let (_dir, store, _path) = setup(alias);
        let mut hook = ReplaceAfterInstall {
            fired: false,
            stage: MigrationStage::BeforeAudit,
        };
        let result = migrate_with_hooks(&store, alias, true, &mut hook).unwrap();
        assert!(result.written);
        assert!(result.audit_pending);
        assert!(result.recovery_pending);
        assert_eq!(migration_audit_count(&store), 0);
        assert_eq!(migration_readiness(&store).pending, 1);
        let retry = migrate(&store, alias, true).unwrap_err().to_string();
        assert!(retry.contains("concurrent replacement was not overwritten"));
    }

    #[test]
    fn replacement_before_completed_never_marks_journal_complete() {
        let alias = "before-completed-replacement";
        let (_dir, store, _path) = setup(alias);
        let mut hook = ReplaceAfterInstall {
            fired: false,
            stage: MigrationStage::BeforeCompleted,
        };
        let error = migrate_with_hooks(&store, alias, true, &mut hook)
            .unwrap_err()
            .to_string();
        assert!(error.contains("concurrent replacement was not overwritten"));
        assert_eq!(migration_audit_count(&store), 1);
        let journal: MigrationJournal = serde_json::from_str(&journal_text(&store, alias)).unwrap();
        assert_eq!(journal.phase, JournalPhase::Committed);
        assert_eq!(migration_readiness(&store).pending, 1);
        let retry = migrate(&store, alias, true).unwrap_err().to_string();
        assert!(retry.contains("concurrent replacement was not overwritten"));
    }

    #[test]
    fn newly_staged_file_rejects_external_hardlink_before_install() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("random.new");
        let bytes = b"exact target";
        let expected = content_hash(bytes);
        let staged = stage_target(&path, bytes, &expected, "safe").unwrap();
        fs::hard_link(&path, dir.path().join("external-link")).unwrap();
        let error = authenticate_exact(&path, &expected, staged.identity, "safe")
            .unwrap_err()
            .to_string();
        assert!(error.contains("concurrent replacement was not overwritten"));
        assert!(!error.contains("exact target"));
    }
}

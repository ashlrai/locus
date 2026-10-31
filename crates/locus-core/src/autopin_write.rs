//! Private atomic workspace update shared by Store and its regression tests.

use crate::binding::Binding;
use crate::error::{LocusError, Result};
use crate::workspace::WorkspaceConfig;
use rand::RngCore;
use std::{fs, path::Path};

/// Read an existing `.locus.toml` through one `O_NOFOLLOW` fd, checked with
/// `fstat` to be a regular file, so a symlink swapped in can never be read.
/// `Ok(None)` when absent.
pub(crate) fn read_locus_toml_nofollow(file: &Path) -> Result<Option<(String, fs::Permissions)>> {
    use std::io::Read;
    let mut o = fs::OpenOptions::new();
    o.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let mut f = match o.open(file) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        #[cfg(unix)]
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
            return Err(LocusError::msg(format!(
                "{} is a symlink; refusing to write through it",
                file.display()
            )))
        }
        Err(e) => return Err(e.into()),
    };
    let metadata = f.metadata()?;
    if !metadata.is_file() {
        return Err(LocusError::msg(format!(
            "{} is not a regular file; refusing to write it",
            file.display()
        )));
    }
    let mut text = String::new();
    f.read_to_string(&mut text)?;
    Ok(Some((text, metadata.permissions())))
}

/// Atomically set `default_binding` in a `.locus.toml`, keeping other keys.
pub(crate) fn write_default_binding(file: &Path, binding: &Binding) -> Result<()> {
    write_default_binding_with_sync(file, binding, |dir| {
        #[cfg(unix)]
        fs::File::open(dir)?.sync_all()?;
        #[cfg(not(unix))]
        let _ = dir;
        Ok(())
    })
}

pub(crate) fn write_default_binding_with_sync(
    file: &Path,
    binding: &Binding,
    sync_parent: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<()> {
    use std::io::Write;
    let existing = read_locus_toml_nofollow(file)?;
    // Recheck the policy from the exact read used to build the update: a
    // policy changed since planning must not allow a blocked winner through.
    if let Some((text, _)) = &existing {
        let cfg = WorkspaceConfig::parse(text)?;
        if !cfg.allows(&binding.alias) && !cfg.allows(&binding.id) {
            return Err(LocusError::binding_not_allowed(
                &binding.alias,
                file,
                &cfg.allowed_bindings,
            ));
        }
    }
    let mut table = match &existing {
        Some((text, _)) => toml::from_str::<toml::Table>(text)
            .map_err(|e| LocusError::msg(format!("{}: {e}", file.display())))?,
        None => {
            let mut t = toml::Table::new();
            t.insert("version".into(), toml::Value::Integer(1));
            t
        }
    };
    table.insert(
        "default_binding".into(),
        toml::Value::String(binding.alias.clone()),
    );
    let text = toml::to_string_pretty(&table)
        .map_err(|e| LocusError::msg(format!("serialize .locus.toml: {e}")))?;
    let dir = file
        .parent()
        .ok_or_else(|| LocusError::msg("workspace file has no parent"))?;
    let mut nonce = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut nonce);
    let tmp = dir.join(format!(".locus.toml.{}.tmp", hex::encode(nonce)));
    let mut o = fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        use std::os::unix::fs::PermissionsExt;
        let mode = existing
            .as_ref()
            .map(|(_, permissions)| permissions.mode() & 0o777)
            .unwrap_or(0o644);
        o.mode(mode).custom_flags(libc::O_NOFOLLOW);
    }
    let res = (|| -> std::io::Result<()> {
        let mut f = o.open(&tmp)?;
        f.write_all(text.as_bytes())?;
        if let Some((_, permissions)) = existing {
            // fchmod restores bits removed by umask using the same fd whose
            // contents are synced below; never chmod through the target path.
            f.set_permissions(permissions)?;
        }
        f.sync_all()?;
        fs::rename(&tmp, file)?;
        // File fsync alone does not make its new directory entry durable.
        sync_parent(dir)
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res.map_err(Into::into)
}

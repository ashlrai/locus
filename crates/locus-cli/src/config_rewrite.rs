//! Opt-in migration of npx Locus MCP registrations to a native executable.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::value::RawValue;
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

type Object<'a> = BTreeMap<String, &'a RawValue>;

struct Change {
    path: PathBuf,
    original: String,
    updated: String,
}

pub struct Rewrite(Vec<Change>);

fn native_binary() -> Result<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(if cfg!(windows) {
                "locus-mcp.exe"
            } else {
                "locus-mcp"
            }));
        }
    }
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".locus/bin").join(if cfg!(windows) {
            "locus-mcp.exe"
        } else {
            "locus-mcp"
        }));
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|dir| {
            dir.join(if cfg!(windows) {
                "locus-mcp.exe"
            } else {
                "locus-mcp"
            })
        }));
    }
    for candidate in candidates {
        if is_native(&candidate) {
            return candidate.canonicalize().context("resolve locus-mcp binary");
        }
    }
    bail!("native locus-mcp binary not found; install it next to locus or on PATH")
}

fn is_native(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return false;
        }
    }
    let mut magic = [0; 4];
    if fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut magic))
        .is_err()
    {
        return false;
    }
    matches!(
        &magic,
        b"\x7fELF"
            | b"\xcf\xfa\xed\xfe"
            | b"\xfe\xed\xfa\xcf"
            | b"\xca\xfe\xba\xbe"
            | b"\xbe\xba\xfe\xca"
    ) || magic.starts_with(b"MZ")
}

// Only accept the documented package and harmless npx switches before it.
// Everything after the package belongs to the MCP server and is retained.
fn package_offset(command: &str, args: &[String]) -> Option<usize> {
    let name = command.rsplit(['/', '\\']).next()?;
    if name != "npx" && name != "npx.cmd" {
        return None;
    }
    for (i, arg) in args.iter().enumerate() {
        let is_package = ["@ashlrai/locus-mcp", "locus-mcp"].iter().any(|package| {
            arg == *package
                || arg
                    .strip_prefix(&format!("{package}@"))
                    .is_some_and(|version| !version.is_empty() && !version.contains('/'))
        });
        if is_package {
            return Some(i + 1);
        }
        if !matches!(arg.as_str(), "-y" | "--yes" | "--") {
            return None;
        }
    }
    None
}

fn object(raw: &RawValue) -> Result<Object<'_>> {
    Ok(serde_json::from_str(raw.get())?)
}

fn json_servers<'a>(
    obj: &Object<'a>,
    source: &'a str,
    binary: &str,
    edits: &mut Vec<(usize, usize, String)>,
) -> Result<()> {
    let Some(servers) = obj.get("mcpServers") else {
        return Ok(());
    };
    let servers = object(servers).context("mcpServers must be an object")?;
    for server in servers.values() {
        let Ok(entry) = object(server) else { continue };
        let (Some(command), Some(args)) = (entry.get("command"), entry.get("args")) else {
            continue;
        };
        let (Ok(command_value), Ok(args_value)) = (
            serde_json::from_str::<String>(command.get()),
            serde_json::from_str::<Vec<String>>(args.get()),
        ) else {
            continue;
        };
        let Some(offset) = package_offset(&command_value, &args_value) else {
            continue;
        };
        for (raw, replacement) in [
            (*command, serde_json::to_string(binary)?),
            (*args, serde_json::to_string(&args_value[offset..])?),
        ] {
            let start = raw.get().as_ptr() as usize - source.as_ptr() as usize;
            edits.push((start, start + raw.get().len(), replacement));
        }
    }
    Ok(())
}

fn rewrite_json(source: &str, binary: &str, claude: bool) -> Result<String> {
    let root: Object<'_> = serde_json::from_str(source).context("invalid JSON config")?;
    let mut edits = Vec::new();
    json_servers(&root, source, binary, &mut edits)?;
    if claude {
        if let Some(projects) = root.get("projects") {
            for project in object(projects)
                .context("projects must be an object")?
                .values()
            {
                if let Ok(project) = object(project) {
                    json_servers(&project, source, binary, &mut edits)?;
                }
            }
        }
    }
    let mut output = source.to_owned();
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.0));
    for (start, end, replacement) in edits {
        output.replace_range(start..end, &replacement);
    }
    Ok(output)
}

fn rewrite_toml(source: &str, binary: &str) -> Result<String> {
    let mut doc = source
        .parse::<toml_edit::DocumentMut>()
        .map_err(|_| anyhow!("invalid TOML config"))?;
    if let Some(servers) = doc
        .get_mut("mcp_servers")
        .and_then(toml_edit::Item::as_table_like_mut)
    {
        for (_, entry) in servers.iter_mut() {
            let Some(entry) = entry.as_table_like_mut() else {
                continue;
            };
            let Some(command) = entry.get("command").and_then(toml_edit::Item::as_str) else {
                continue;
            };
            let Some(args) = entry.get("args").and_then(toml_edit::Item::as_array) else {
                continue;
            };
            let Some(args_value) = args
                .iter()
                .map(|v| v.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            let Some(offset) = package_offset(command, &args_value) else {
                continue;
            };
            let cmd = entry
                .get_mut("command")
                .and_then(toml_edit::Item::as_value_mut)
                .expect("string value");
            let decor = cmd.decor().clone();
            *cmd = toml_edit::Value::from(binary);
            *cmd.decor_mut() = decor;
            let args = entry
                .get_mut("args")
                .and_then(toml_edit::Item::as_array_mut)
                .expect("array value");
            for _ in 0..offset {
                args.remove(0);
            }
        }
    }
    Ok(doc.to_string())
}

pub fn prepare() -> Result<Rewrite> {
    let binary = native_binary()?;
    let binary = binary.to_str().context("locus-mcp path is not UTF-8")?;
    let home = dirs::home_dir().context("HOME is unavailable")?;
    let paths = [
        (".codex/config.toml", 0),
        (".claude.json", 1),
        (".cursor/mcp.json", 2),
    ];
    let mut changes = Vec::new();
    for (relative, kind) in paths {
        let path = home.join(relative);
        let original = match read_config(&path) {
            Ok((data, _)) => data,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err).with_context(|| format!("read {}", path.display())),
        };
        let updated = if kind == 0 {
            rewrite_toml(&original, binary)
        } else {
            rewrite_json(&original, binary, kind == 1)
        }
        .with_context(|| format!("parse {}", path.display()))?;
        if updated != original {
            changes.push(Change {
                path,
                original,
                updated,
            });
        }
    }
    Ok(Rewrite(changes))
}

// Open without following a final symlink, and inspect the opened file before
// reading it. This also refuses hard links to an unrelated authentication file.
fn read_config(path: &Path) -> std::io::Result<(String, fs::Metadata)> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(not(unix))]
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "config is a symlink",
        ));
    }
    let mut file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "config is not a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "config has multiple hard links",
            ));
        }
    }
    let mut content = String::new();
    file.read_to_string(&mut content)?;
    Ok((content, metadata))
}

impl Rewrite {
    pub fn apply(self) -> Result<()> {
        for change in self.0 {
            // Do not overwrite changes made since preparation.
            let (current, metadata) = read_config(&change.path)?;
            if current != change.original {
                bail!("config changed during pin: {}", change.path.display());
            }
            let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.9fZ");
            let mut backup = change.path.as_os_str().to_os_string();
            backup.push(format!(".{stamp}.bak"));
            let backup = PathBuf::from(backup);
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options
                .open(&backup)
                .with_context(|| format!("create backup {}", backup.display()))?;
            file.write_all(change.original.as_bytes())?;
            file.sync_all()?;
            let mut temp = tempfile::NamedTempFile::new_in(
                change.path.parent().context("config has no parent")?,
            )?;
            temp.as_file().set_permissions(metadata.permissions())?;
            temp.write_all(change.updated.as_bytes())?;
            temp.as_file().sync_all()?;
            temp.persist(&change.path)
                .with_context(|| format!("replace {}", change.path.display()))?;
            eprintln!(
                "Updated {} (backup {})",
                change.path.display(),
                backup.display()
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_known_npx_invocations_are_migrated() {
        let source = r#"{"mcpServers":{"a":{"command":"npx","args":["--yes","@ashlrai/locus-mcp","--http"]},"b":{"command":"npx","args":["--package=other","locus-mcp"]},"c":{"command":"node","args":["locus-mcp"]},"d":{"command":"npx","args":["locus-mcp-extra"]}}}"#;
        let output = rewrite_json(source, "/native/locus-mcp", false).unwrap();
        let v: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(v["mcpServers"]["a"]["args"], serde_json::json!(["--http"]));
        for name in ["b", "c", "d"] {
            let original: serde_json::Value = serde_json::from_str(source).unwrap();
            assert_eq!(v["mcpServers"][name], original["mcpServers"][name]);
        }
        assert_eq!(
            rewrite_json(&output, "/native/locus-mcp", false).unwrap(),
            output
        );
    }

    #[test]
    fn versioned_package_is_supported() {
        assert_eq!(
            package_offset("npx", &["-y".into(), "@ashlrai/locus-mcp@latest".into()]),
            Some(2)
        );
        assert_eq!(package_offset("npx", &["locus-mcp@".into()]), None);
    }

    #[test]
    fn toml_inline_tables_and_other_servers_are_preserved() {
        let source = "# comment\n[mcp_servers]\nlocus = { command = 'npx', args = ['-y', 'locus-mcp', '--http'], env = { KEEP = 'yes' } }\nother = { command = 'npx', args = ['other'] }\n";
        let output = rewrite_toml(source, "/native/locus-mcp").unwrap();
        let doc = output.parse::<toml_edit::DocumentMut>().unwrap();
        assert_eq!(
            doc["mcp_servers"]["locus"]["command"].as_str(),
            Some("/native/locus-mcp")
        );
        assert_eq!(
            doc["mcp_servers"]["locus"]["args"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(
            output.contains("# comment")
                && output.contains("other = { command = 'npx', args = ['other'] }")
        );
        assert_eq!(rewrite_toml(&output, "/native/locus-mcp").unwrap(), output);
    }
}

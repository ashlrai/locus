//! OS / best-effort sandbox preparation for upstream workers.
//!
//! `sandbox = true` / `LOCUS_WORKER_SANDBOX=1` selects a platform backend:
//!
//! | Platform | Backend tag | Strength |
//! |----------|-------------|----------|
//! | macOS | `sandbox-exec` | Seatbelt deny-by-default (required; fail closed if missing) |
//! | Linux | `bwrap` | bubblewrap mount/pid namespace (when `bwrap` is installed) |
//! | Linux | `path` | Restricted PATH + absolute executable only (**best-effort**, not kernel isolation) |
//! | other | — | Fail closed |
//!
//! `LOCUS_WORKER_SANDBOXED=1` and `LOCUS_WORKER_SANDBOX_BACKEND=<tag>` are set only
//! after backend resolution. The `path` tag must never be treated as equivalent to
//! Seatbelt or bubblewrap. This is **not** a VM or full seccomp profile.

use crate::error::{LocusError, Result};
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Component, Path, PathBuf};

/// Env: enable sandbox for all MCP stdio workers (`1` / `true` / `yes`).
pub const ENV_WORKER_SANDBOX: &str = "LOCUS_WORKER_SANDBOX";

/// Marker injected into sandboxed worker children (never a secret).
pub const ENV_WORKER_SANDBOXED: &str = "LOCUS_WORKER_SANDBOXED";

/// Applied sandbox backend tag (`sandbox-exec` / `bwrap` / `path`).
pub const ENV_WORKER_SANDBOX_BACKEND: &str = "LOCUS_WORKER_SANDBOX_BACKEND";

/// Whether global env requests sandbox mode.
pub fn sandbox_from_env() -> bool {
    sandbox_env_value(std::env::var(ENV_WORKER_SANDBOX).ok().as_deref())
}

fn sandbox_env_value(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        let value = value.trim();
        value == "1" || value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("yes")
    })
}

/// Effective sandbox: config/spec flag OR env.
pub fn sandbox_enabled(config_flag: bool) -> bool {
    sandbox_enabled_with_env(config_flag, sandbox_from_env())
}

pub(crate) fn sandbox_enabled_with_env(config_flag: bool, env_flag: bool) -> bool {
    config_flag || env_flag
}

/// Restricted PATH for sandboxed workers.
///
/// The resolved executable's directory is prepended separately so a protected
/// absolute executable keeps working even when it lives outside system paths.
/// On the Linux `path` backend this is the *only* restriction (best-effort).
pub fn restricted_worker_path() -> String {
    ["/usr/bin", "/bin", "/usr/sbin", "/sbin", "/usr/local/bin"].join(":")
}

/// How sandbox was applied for a spawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxBackend {
    /// macOS Seatbelt via `/usr/bin/sandbox-exec`.
    SandboxExec,
    /// Linux bubblewrap (`bwrap`) — mount/pid namespace; network shared for MCP.
    Bwrap,
    /// Restricted PATH + absolute executable only. **Best-effort**, not OS isolation.
    Path,
}

impl SandboxBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SandboxExec => "sandbox-exec",
            Self::Bwrap => "bwrap",
            Self::Path => "path",
        }
    }

    /// True when a kernel/namespace wrapper is used (not the path-only fallback).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_os_isolation(self) -> bool {
        matches!(self, Self::SandboxExec | Self::Bwrap)
    }
}

/// Selected backend + optional wrapper binary (`sandbox-exec` / `bwrap`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct BackendSelection {
    backend: SandboxBackend,
    wrapper: Option<PathBuf>,
}

/// True when macOS sandbox-exec is present.
#[allow(dead_code)]
pub fn sandbox_exec_available() -> bool {
    sandbox_exec_path().is_some()
}

/// True when Linux bubblewrap is present on a fixed path.
#[allow(dead_code)]
pub fn bwrap_available() -> bool {
    bwrap_path().is_some()
}

fn sandbox_exec_path() -> Option<PathBuf> {
    let path = PathBuf::from("/usr/bin/sandbox-exec");
    path.is_file().then_some(path)
}

fn bwrap_path() -> Option<PathBuf> {
    for candidate in ["/usr/bin/bwrap", "/bin/bwrap", "/usr/local/bin/bwrap"] {
        let path = PathBuf::from(candidate);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

/// Choose backend for the current platform (production entry).
fn select_sandbox_backend() -> Result<BackendSelection> {
    select_sandbox_backend_with_probe(
        sandbox_exec_path(),
        bwrap_path(),
        cfg!(target_os = "macos"),
        cfg!(target_os = "linux"),
    )
}

/// Pure selection logic (unit-testable without the host OS).
///
/// - macOS: require `sandbox-exec` (fail closed).
/// - Linux: prefer `bwrap`; otherwise best-effort `path`.
/// - other: fail closed.
fn select_sandbox_backend_with_probe(
    sandbox_exec: Option<PathBuf>,
    bwrap: Option<PathBuf>,
    is_macos: bool,
    is_linux: bool,
) -> Result<BackendSelection> {
    if is_macos {
        let wrapper = sandbox_exec.ok_or_else(|| {
            LocusError::msg(
                "sandbox requested but no supported OS isolation backend is available; refusing to spawn",
            )
        })?;
        return Ok(BackendSelection {
            backend: SandboxBackend::SandboxExec,
            wrapper: Some(wrapper),
        });
    }
    if is_linux {
        if let Some(wrapper) = bwrap {
            return Ok(BackendSelection {
                backend: SandboxBackend::Bwrap,
                wrapper: Some(wrapper),
            });
        }
        // Best-effort only — callers must tag backend as `path` and document limits.
        return Ok(BackendSelection {
            backend: SandboxBackend::Path,
            wrapper: None,
        });
    }
    Err(LocusError::msg(
        "sandbox requested but no supported OS isolation backend is available; refusing to spawn",
    ))
}

fn canonical_existing_dir(path: &Path, label: &str) -> Result<PathBuf> {
    let canonical = path.canonicalize().map_err(|e| {
        LocusError::msg(format!(
            "sandbox {label} `{}` is unavailable: {e}",
            path.display()
        ))
    })?;
    if !canonical.is_dir() {
        return Err(LocusError::msg(format!(
            "sandbox {label} `{}` is not a directory",
            path.display()
        )));
    }
    Ok(canonical)
}

fn locus_home_from_worker_home(worker_home: &Path) -> Result<PathBuf> {
    let worker_home = canonical_existing_dir(worker_home, "worker home")?;
    let workers = worker_home
        .parent()
        .ok_or_else(|| LocusError::msg("sandbox worker home has no workers parent"))?;
    if workers.file_name().and_then(|name| name.to_str()) != Some("workers") {
        return Err(LocusError::msg(format!(
            "sandbox worker home `{}` is not rooted under LOCUS_HOME/workers",
            worker_home.display()
        )));
    }
    workers
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| LocusError::msg("sandbox worker home has no LOCUS_HOME parent"))
}

fn resolve_executable(
    program: &str,
    work_dir: &Path,
    search_path: Option<&str>,
) -> Result<PathBuf> {
    let trimmed = program.trim();
    if trimmed.is_empty() {
        return Err(LocusError::msg("sandbox executable must be non-empty"));
    }

    let requested = Path::new(trimmed);
    let candidate = if requested.is_absolute() {
        requested.to_path_buf()
    } else if requested.components().count() > 1
        || matches!(
            requested.components().next(),
            Some(Component::CurDir | Component::ParentDir)
        )
    {
        work_dir.join(requested)
    } else {
        let path = search_path.ok_or_else(|| {
            LocusError::msg(format!(
                "sandbox executable `{trimmed}` is unavailable: PATH is empty"
            ))
        })?;
        std::env::split_paths(path)
            .map(|dir| dir.join(requested))
            .find(|candidate| candidate.is_file())
            .ok_or_else(|| {
                LocusError::msg(format!(
                    "sandbox executable `{trimmed}` is unavailable on the protected parent PATH"
                ))
            })?
    };

    let executable = candidate.canonicalize().map_err(|e| {
        LocusError::msg(format!(
            "sandbox executable `{}` cannot be resolved: {e}",
            candidate.display()
        ))
    })?;
    if !executable.is_file() {
        return Err(LocusError::msg(format!(
            "sandbox executable `{}` is not a file",
            executable.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if executable.metadata()?.permissions().mode() & 0o111 == 0 {
            return Err(LocusError::msg(format!(
                "sandbox executable `{}` is not executable",
                executable.display()
            )));
        }
    }
    Ok(executable)
}

#[derive(Debug)]
struct RuntimeAccess {
    executable: PathBuf,
    interpreter: Option<PathBuf>,
    read_roots: Vec<PathBuf>,
}

fn shebang_interpreter(executable: &Path, search_path: Option<&str>) -> Result<Option<PathBuf>> {
    let mut first_line = Vec::new();
    BufReader::new(File::open(executable)?)
        .take(4096)
        .read_until(b'\n', &mut first_line)?;
    if !first_line.starts_with(b"#!") {
        return Ok(None);
    }
    let line = std::str::from_utf8(&first_line[2..])
        .map_err(|_| LocusError::msg("sandbox executable has a non-UTF-8 shebang"))?;
    let mut parts = line.split_whitespace();
    let requested = parts
        .next()
        .ok_or_else(|| LocusError::msg("sandbox executable has an empty shebang"))?;
    let interpreter = if Path::new(requested).file_name().and_then(|v| v.to_str()) == Some("env") {
        parts
            .find(|part| !part.starts_with('-'))
            .ok_or_else(|| LocusError::msg("sandbox env shebang does not name an interpreter"))?
    } else {
        requested
    };
    resolve_executable(interpreter, Path::new("/"), search_path).map(Some)
}

fn node_package_root(executable: &Path) -> Option<PathBuf> {
    let components: Vec<_> = executable.components().collect();
    let node_modules = components
        .windows(2)
        .position(|pair| pair[0].as_os_str() == "lib" && pair[1].as_os_str() == "node_modules")?;
    let package = node_modules + 2;
    let package_end = if components
        .get(package)?
        .as_os_str()
        .to_string_lossy()
        .starts_with('@')
    {
        package + 2
    } else {
        package + 1
    };
    if package_end >= components.len() {
        return None;
    }
    Some(components[..package_end].iter().collect())
}

fn resolve_runtime_access(executable: &Path, search_path: Option<&str>) -> Result<RuntimeAccess> {
    let executable = executable.canonicalize().map_err(|e| {
        LocusError::msg(format!(
            "sandbox executable `{}` cannot be canonicalized: {e}",
            executable.display()
        ))
    })?;
    let interpreter = shebang_interpreter(&executable, search_path)?;
    let node_script = interpreter
        .as_ref()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        == Some("node");
    let read_root = if node_script {
        node_package_root(&executable)
    } else {
        None
    }
    .or_else(|| executable.parent().map(Path::to_path_buf))
    .ok_or_else(|| LocusError::msg("sandbox executable has no readable parent"))?
    .canonicalize()
    .map_err(|e| LocusError::msg(format!("sandbox runtime root is unavailable: {e}")))?;

    Ok(RuntimeAccess {
        executable,
        interpreter,
        read_roots: vec![read_root],
    })
}

fn validate_recursive_grant(path: &Path, locus_home: &Path, label: &str) -> Result<()> {
    if path == locus_home || path.starts_with(locus_home) || locus_home.starts_with(path) {
        return Err(LocusError::msg(format!(
            "sandbox {label} `{}` overlaps or contains protected LOCUS_HOME authority state",
            path.display()
        )));
    }
    Ok(())
}

fn validate_runtime_access(runtime: &RuntimeAccess, locus_home: &Path) -> Result<()> {
    if runtime.executable.starts_with(locus_home)
        || runtime
            .interpreter
            .as_ref()
            .is_some_and(|path| path.starts_with(locus_home))
    {
        return Err(LocusError::msg(
            "sandbox executable or interpreter cannot be loaded from protected LOCUS_HOME",
        ));
    }
    for root in &runtime.read_roots {
        validate_recursive_grant(root, locus_home, "runtime root")?;
    }
    Ok(())
}

/// Deny-by-default Seatbelt profile for one worker.
///
/// The current work tree and worker home are the only caller-owned trees made
/// readable. The rest of LOCUS_HOME (including daemon.key, bindings, sessions,
/// approvals, and audit) and the user's ambient home remain inaccessible.
/// Network is outbound TCP/UDP only because upstream MCP servers are provider
/// clients. Application listeners and non-system Unix-domain sockets are not
/// part of the worker contract; imported Apple system profiles retain narrowly
/// scoped system-service IPC such as logging.
#[allow(dead_code)]
pub fn seatbelt_profile_for_worker(
    work_dir: &Path,
    worker_home: &Path,
    executable: &Path,
) -> Result<String> {
    seatbelt_profile_for_worker_with_path(
        work_dir,
        worker_home,
        executable,
        std::env::var("PATH").ok().as_deref(),
    )
}

fn seatbelt_profile_for_worker_with_path(
    work_dir: &Path,
    worker_home: &Path,
    executable: &Path,
    search_path: Option<&str>,
) -> Result<String> {
    let runtime = resolve_runtime_access(executable, search_path)?;
    seatbelt_profile_for_runtime(work_dir, worker_home, &runtime)
}

/// Canonical work dir, worker home, and LOCUS_HOME after authority overlap checks.
struct SandboxPaths {
    work_dir: PathBuf,
    worker_home: PathBuf,
    #[allow(dead_code)]
    locus_home: PathBuf,
}

fn validate_sandbox_paths(
    work_dir: &Path,
    worker_home: &Path,
    runtime: &RuntimeAccess,
) -> Result<SandboxPaths> {
    let wd = canonical_existing_dir(work_dir, "work directory")?;
    let wh = canonical_existing_dir(worker_home, "worker home")?;
    let locus_home = locus_home_from_worker_home(&wh)?;
    if !wd.starts_with(&wh) {
        validate_recursive_grant(&wd, &locus_home, "work directory")?;
    }
    validate_runtime_access(runtime, &locus_home)?;
    Ok(SandboxPaths {
        work_dir: wd,
        worker_home: wh,
        locus_home,
    })
}

fn seatbelt_profile_for_runtime(
    work_dir: &Path,
    worker_home: &Path,
    runtime: &RuntimeAccess,
) -> Result<String> {
    let paths = validate_sandbox_paths(work_dir, worker_home, runtime)?;
    let wd = &paths.work_dir;
    let wh = &paths.worker_home;

    let mut exec_rules = String::new();
    let mut metadata_rules = String::new();
    let mut read_rules = String::new();
    for root in &runtime.read_roots {
        let root = seatbelt_escape(&root.display().to_string());
        exec_rules.push_str(&format!("    (subpath \"{root}\")\n"));
        metadata_rules.push_str(&format!("    (path-ancestors \"{root}\")\n"));
        read_rules.push_str(&format!("    (subpath \"{root}\")\n"));
    }
    let mut runtime_literals = vec![runtime.executable.as_path()];
    if let Some(interpreter) = runtime.interpreter.as_deref() {
        runtime_literals.push(interpreter);
    }
    for literal in runtime_literals {
        let literal = seatbelt_escape(&literal.display().to_string());
        exec_rules.push_str(&format!("    (literal \"{literal}\")\n"));
        metadata_rules.push_str(&format!("    (path-ancestors \"{literal}\")\n"));
        read_rules.push_str(&format!("    (literal \"{literal}\")\n"));
    }

    Ok(format!(
        r#"(version 1)
(deny default)
(import "system.sb")
; Locus worker: deny-by-default filesystem, outbound provider network only.
(allow process-fork)
(allow process-info*)
(allow process-exec
    (subpath "/bin")
    (subpath "/usr/bin")
    (subpath "/usr/sbin")
    (subpath "/sbin")
    (subpath "{wd}")
    (subpath "{wh}")
{exec_rules})
(allow signal (target self))
(allow ipc-posix*)
(allow system-socket)
(system-network)
(allow network-outbound (remote tcp) (remote udp))
(allow file-read-metadata file-test-existence
    (path-ancestors "{wd}")
    (path-ancestors "{wh}")
{metadata_rules})
(allow file-read* file-map-executable
    (subpath "{wd}")
    (subpath "{wh}")
{read_rules})
(allow file-write* (subpath "{wd}") (subpath "{wh}"))
"#,
        wd = seatbelt_escape(&wd.display().to_string()),
        wh = seatbelt_escape(&wh.display().to_string()),
    ))
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn seatbelt_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Resolved spawn line after sandbox wrapping (or path-only best-effort).
#[derive(Debug, Clone)]
pub struct SandboxSpawn {
    pub program: String,
    pub args: Vec<String>,
    pub backend: SandboxBackend,
    /// Restricted PATH value, with the protected executable directory first.
    pub path: String,
    /// Canonical executable selected before the child environment is rebuilt.
    #[allow(dead_code)]
    pub executable: PathBuf,
}

fn restricted_path_for_runtime(runtime: &RuntimeAccess) -> String {
    let mut path_dirs = Vec::new();
    if let Some(parent) = runtime.interpreter.as_deref().and_then(Path::parent) {
        path_dirs.push(parent.display().to_string());
    }
    if let Some(parent) = runtime.executable.parent() {
        let parent = parent.display().to_string();
        if !path_dirs.contains(&parent) {
            path_dirs.push(parent);
        }
    }
    path_dirs.push(restricted_worker_path());
    path_dirs.join(":")
}

/// Read-only system roots for the Linux bubblewrap profile.
///
/// Uses `--ro-bind-try` so missing merged-usr paths do not fail the wrap.
/// Network stays shared (no `--unshare-net`) so MCP provider HTTPS works.
const BWRAP_RO_ROOTS: &[&str] = &[
    "/usr",
    "/bin",
    "/lib",
    "/lib64",
    "/sbin",
    "/usr/local",
    "/etc", // DNS + TLS trust store; not LOCUS_HOME authority state
];

/// Build `bwrap` argv (without the `bwrap` program itself).
///
/// Profile goals (best-effort, not a VM):
/// - RO system roots
/// - RW bind of work tree + session worker home only (not full `LOCUS_HOME`)
/// - tmpfs `/tmp`; private HOME via env (worker home)
/// - network allowed (shared netns) for MCP stdio → provider APIs
/// - no bind of `LOCUS_HOME/bindings` or host ambient home
fn bwrap_args_for_runtime(
    work_dir: &Path,
    worker_home: &Path,
    runtime: &RuntimeAccess,
    args: &[String],
) -> Vec<String> {
    let mut out: Vec<String> = vec![
        "--die-with-parent".into(),
        "--unshare-pid".into(),
        // Intentionally no --unshare-net: MCP servers need outbound provider traffic.
        "--proc".into(),
        "/proc".into(),
        "--dev".into(),
        "/dev".into(),
        "--tmpfs".into(),
        "/tmp".into(),
    ];

    for root in BWRAP_RO_ROOTS {
        out.push("--ro-bind-try".into());
        out.push((*root).into());
        out.push((*root).into());
    }

    // Session-private trees only — never the LOCUS_HOME parent (bindings/, daemon.key).
    out.push("--bind".into());
    out.push(work_dir.display().to_string());
    out.push(work_dir.display().to_string());
    out.push("--bind".into());
    out.push(worker_home.display().to_string());
    out.push(worker_home.display().to_string());

    let mut already = vec![work_dir.to_path_buf(), worker_home.to_path_buf()];
    for root in &runtime.read_roots {
        if already
            .iter()
            .any(|p| root.starts_with(p) || p.starts_with(root))
        {
            continue;
        }
        if BWRAP_RO_ROOTS
            .iter()
            .any(|sys| root.starts_with(Path::new(sys)))
        {
            continue;
        }
        out.push("--ro-bind-try".into());
        out.push(root.display().to_string());
        out.push(root.display().to_string());
        already.push(root.clone());
    }

    for literal in runtime
        .interpreter
        .iter()
        .chain(std::iter::once(&runtime.executable))
    {
        if already.iter().any(|p| literal.starts_with(p)) {
            continue;
        }
        if BWRAP_RO_ROOTS
            .iter()
            .any(|sys| literal.starts_with(Path::new(sys)))
        {
            continue;
        }
        out.push("--ro-bind-try".into());
        out.push(literal.display().to_string());
        out.push(literal.display().to_string());
    }

    out.push("--chdir".into());
    out.push(work_dir.display().to_string());
    out.push("--".into());
    out.push(runtime.executable.display().to_string());
    out.extend(args.iter().cloned());
    out
}

/// Build program/args/backend for a sandboxed spawn (does not touch env yet).
///
/// Platform rules: macOS requires Seatbelt; Linux prefers bubblewrap and falls
/// back to best-effort `path` when `bwrap` is missing; other OS fail closed.
pub fn resolve_sandbox_spawn(
    program: &str,
    args: &[String],
    work_dir: &Path,
    worker_home: &Path,
) -> Result<SandboxSpawn> {
    let selection = select_sandbox_backend()?;
    resolve_sandbox_spawn_for(
        program,
        args,
        work_dir,
        worker_home,
        std::env::var("PATH").ok().as_deref(),
        selection,
    )
}

/// Test helper: force a Seatbelt wrapper path (or fail closed when `None`).
#[cfg(test)]
fn resolve_sandbox_spawn_with_backend(
    program: &str,
    args: &[String],
    work_dir: &Path,
    worker_home: &Path,
    search_path: Option<&str>,
    backend_path: Option<&Path>,
) -> Result<SandboxSpawn> {
    let selection = match backend_path {
        Some(path) => BackendSelection {
            backend: SandboxBackend::SandboxExec,
            wrapper: Some(path.to_path_buf()),
        },
        None => {
            return Err(LocusError::msg(
                "sandbox requested but no supported OS isolation backend is available; refusing to spawn",
            ));
        }
    };
    resolve_sandbox_spawn_for(program, args, work_dir, worker_home, search_path, selection)
}

fn resolve_sandbox_spawn_for(
    program: &str,
    args: &[String],
    work_dir: &Path,
    worker_home: &Path,
    search_path: Option<&str>,
    selection: BackendSelection,
) -> Result<SandboxSpawn> {
    let requested_executable = resolve_executable(program, work_dir, search_path)?;
    let runtime = resolve_runtime_access(&requested_executable, search_path)?;
    let paths = validate_sandbox_paths(work_dir, worker_home, &runtime)?;
    let path = restricted_path_for_runtime(&runtime);
    let executable = runtime.executable.clone();

    match selection.backend {
        SandboxBackend::SandboxExec => {
            let profile = seatbelt_profile_for_runtime(work_dir, worker_home, &runtime)?;
            let wrapper = selection.wrapper.ok_or_else(|| {
                LocusError::msg("sandbox-exec backend selected without wrapper path")
            })?;
            let backend = wrapper.canonicalize().map_err(|e| {
                LocusError::msg(format!(
                    "sandbox backend `{}` is unavailable: {e}",
                    wrapper.display()
                ))
            })?;
            let mut wrapped = Vec::with_capacity(3 + args.len());
            wrapped.push("-p".into());
            wrapped.push(profile);
            wrapped.push(executable.display().to_string());
            wrapped.extend(args.iter().cloned());
            Ok(SandboxSpawn {
                program: backend.display().to_string(),
                args: wrapped,
                backend: SandboxBackend::SandboxExec,
                path,
                executable,
            })
        }
        SandboxBackend::Bwrap => {
            let wrapper = selection
                .wrapper
                .ok_or_else(|| LocusError::msg("bwrap backend selected without wrapper path"))?;
            let backend = wrapper.canonicalize().map_err(|e| {
                LocusError::msg(format!(
                    "sandbox backend `{}` is unavailable: {e}",
                    wrapper.display()
                ))
            })?;
            let wrapped =
                bwrap_args_for_runtime(&paths.work_dir, &paths.worker_home, &runtime, args);
            Ok(SandboxSpawn {
                program: backend.display().to_string(),
                args: wrapped,
                backend: SandboxBackend::Bwrap,
                path,
                executable,
            })
        }
        SandboxBackend::Path => {
            // Best-effort only: absolute executable + restricted PATH.
            Ok(SandboxSpawn {
                program: executable.display().to_string(),
                args: args.to_vec(),
                backend: SandboxBackend::Path,
                path,
                executable,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf, PathBuf) {
        let dir = tempdir().unwrap();
        let locus_home = dir.path().join("custom-locus-home");
        let worker_home = locus_home.join("workers").join("sess_test");
        let work_dir = dir.path().join("repo");
        let bin_dir = dir.path().join("toolchain").join("bin");
        fs::create_dir_all(&worker_home).unwrap();
        fs::create_dir_all(&work_dir).unwrap();
        fs::create_dir_all(&bin_dir).unwrap();
        fs::write(locus_home.join("daemon.key"), "authority-canary").unwrap();
        let executable = bin_dir.join("custom-mcp");
        fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let backend = dir.path().join("sandbox-exec");
        fs::write(&backend, "backend").unwrap();
        (dir, worker_home, work_dir, executable, backend)
    }

    #[cfg(unix)]
    fn make_executable(path: &Path, contents: &str) {
        use std::os::unix::fs::PermissionsExt;
        fs::write(path, contents).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn restricted_path_has_only_core_bins() {
        let path = restricted_worker_path();
        assert_eq!(path, "/usr/bin:/bin:/usr/sbin:/sbin:/usr/local/bin");
        assert!(!path.split(':').any(|part| part.is_empty()));
    }

    #[test]
    fn sandbox_env_values_and_config_compose_without_process_mutation() {
        for value in [Some("1"), Some(" true "), Some("YES"), Some("yes")] {
            assert!(sandbox_env_value(value), "value={value:?}");
        }
        for value in [None, Some(""), Some("0"), Some("false"), Some("no")] {
            assert!(!sandbox_env_value(value), "value={value:?}");
        }
        assert!(sandbox_enabled_with_env(false, true));
        assert!(sandbox_enabled_with_env(true, false));
        assert!(!sandbox_enabled_with_env(false, false));
    }

    #[test]
    fn profile_binds_custom_locus_home_and_outbound_network() {
        let (_dir, worker_home, work_dir, executable, _backend) = fixture();
        let profile = seatbelt_profile_for_worker(&work_dir, &worker_home, &executable).unwrap();
        assert!(profile.contains("(deny default)"));
        assert!(profile.contains("(import \"system.sb\")"));
        assert!(profile.contains("(allow system-socket)"));
        assert!(profile.contains("(system-network)"));
        assert!(profile.contains("(allow network-outbound (remote tcp) (remote udp))"));
        assert!(!profile.contains("network-inbound"));
        assert!(profile.contains(&worker_home.canonicalize().unwrap().display().to_string()));
        assert!(!profile.contains("(allow default)"));
        assert!(!profile.contains("daemon.key"));
    }

    #[test]
    fn missing_backend_fails_closed_without_path_marker_fallback() {
        let (_dir, worker_home, work_dir, executable, _backend) = fixture();
        let path = executable.parent().unwrap().display().to_string();
        let err = resolve_sandbox_spawn_with_backend(
            "custom-mcp",
            &[],
            &work_dir,
            &worker_home,
            Some(&path),
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("no supported OS isolation backend"), "{err}");
    }

    #[test]
    fn linux_selects_path_backend_when_bwrap_missing() {
        let sel = select_sandbox_backend_with_probe(None, None, false, true).unwrap();
        assert_eq!(sel.backend, SandboxBackend::Path);
        assert_eq!(sel.backend.as_str(), "path");
        assert!(!sel.backend.is_os_isolation());
        assert!(sel.wrapper.is_none());
    }

    #[test]
    fn linux_prefers_bwrap_when_available() {
        let dir = tempdir().unwrap();
        let bwrap = dir.path().join("bwrap");
        fs::write(&bwrap, "wrapper").unwrap();
        let sel =
            select_sandbox_backend_with_probe(None, Some(bwrap.clone()), false, true).unwrap();
        assert_eq!(sel.backend, SandboxBackend::Bwrap);
        assert_eq!(sel.backend.as_str(), "bwrap");
        assert!(sel.backend.is_os_isolation());
        assert_eq!(sel.wrapper.as_deref(), Some(bwrap.as_path()));
    }

    #[test]
    fn macos_requires_sandbox_exec_and_ignores_bwrap() {
        let dir = tempdir().unwrap();
        let bwrap = dir.path().join("bwrap");
        fs::write(&bwrap, "wrapper").unwrap();
        let err = select_sandbox_backend_with_probe(None, Some(bwrap), true, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("no supported OS isolation backend"), "{err}");

        let se = dir.path().join("sandbox-exec");
        fs::write(&se, "wrapper").unwrap();
        let sel = select_sandbox_backend_with_probe(Some(se.clone()), None, true, false).unwrap();
        assert_eq!(sel.backend, SandboxBackend::SandboxExec);
        assert_eq!(sel.wrapper.as_deref(), Some(se.as_path()));
    }

    #[test]
    fn unsupported_platform_fails_closed() {
        let err = select_sandbox_backend_with_probe(None, None, false, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("no supported OS isolation backend"), "{err}");
    }

    #[test]
    fn path_backend_resolve_uses_absolute_executable_and_restricted_path() {
        let (_dir, worker_home, work_dir, executable, _backend) = fixture();
        let search_path = executable.parent().unwrap().display().to_string();
        let custom_args = vec!["--flag".into(), "value".into()];
        let spawn = resolve_sandbox_spawn_for(
            "custom-mcp",
            &custom_args,
            &work_dir,
            &worker_home,
            Some(&search_path),
            BackendSelection {
                backend: SandboxBackend::Path,
                wrapper: None,
            },
        )
        .unwrap();
        let canonical = executable.canonicalize().unwrap();
        assert_eq!(spawn.backend, SandboxBackend::Path);
        assert_eq!(spawn.backend.as_str(), "path");
        assert!(!spawn.backend.is_os_isolation());
        assert_eq!(spawn.program, canonical.display().to_string());
        assert_eq!(spawn.args, custom_args);
        assert_eq!(spawn.executable, canonical);
        assert!(spawn.path.ends_with(&restricted_worker_path()));
        assert!(spawn
            .path
            .split(':')
            .any(|part| part == canonical.parent().unwrap().display().to_string()));
    }

    #[test]
    fn path_backend_still_refuses_work_dir_over_locus_home() {
        let (dir, worker_home, _work_dir, executable, _backend) = fixture();
        let search_path = executable.parent().unwrap().display().to_string();
        let err = resolve_sandbox_spawn_for(
            "custom-mcp",
            &[],
            dir.path(),
            &worker_home,
            Some(&search_path),
            BackendSelection {
                backend: SandboxBackend::Path,
                wrapper: None,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("work directory") && err.contains("LOCUS_HOME"),
            "{err}"
        );
    }

    #[test]
    fn bwrap_args_bind_worker_not_bindings_and_keep_network() {
        let (dir, worker_home, work_dir, executable, _backend) = fixture();
        let locus_home = dir.path().join("custom-locus-home");
        let bindings = locus_home.join("bindings");
        fs::create_dir_all(&bindings).unwrap();
        fs::write(bindings.join("acme.toml"), "alias = \"acme\"").unwrap();

        let runtime = resolve_runtime_access(&executable, None).unwrap();
        let wd = work_dir.canonicalize().unwrap();
        let wh = worker_home.canonicalize().unwrap();
        let args = bwrap_args_for_runtime(&wd, &wh, &runtime, &["--ping".into()]);

        assert!(args.iter().any(|a| a == "--die-with-parent"));
        assert!(args.iter().any(|a| a == "--unshare-pid"));
        assert!(
            !args.iter().any(|a| a == "--unshare-net"),
            "MCP needs shared network: {args:?}"
        );
        assert!(args
            .windows(3)
            .any(|w| { w[0] == "--ro-bind-try" && w[1] == "/usr" && w[2] == "/usr" }));
        assert!(args.windows(3).any(|w| {
            w[0] == "--bind" && w[1] == wd.display().to_string() && w[2] == wd.display().to_string()
        }));
        assert!(args.windows(3).any(|w| {
            w[0] == "--bind" && w[1] == wh.display().to_string() && w[2] == wh.display().to_string()
        }));
        // Must not bind LOCUS_HOME root or bindings/ (deny authority state).
        let locus_s = locus_home.canonicalize().unwrap().display().to_string();
        let bindings_s = bindings.canonicalize().unwrap().display().to_string();
        for window in args.windows(3) {
            if window[0] == "--bind" || window[0] == "--ro-bind" || window[0] == "--ro-bind-try" {
                assert_ne!(window[1], locus_s, "must not bind LOCUS_HOME root");
                assert_ne!(window[1], bindings_s, "must not bind bindings/");
                assert!(!window[1].ends_with("/bindings"), "must not bind bindings/");
            }
        }
        assert!(args.iter().any(|a| a == "--tmpfs"));
        let sep = args
            .iter()
            .position(|a| a == "--")
            .expect("bwrap -- separator");
        assert_eq!(args[sep + 1], runtime.executable.display().to_string());
        assert_eq!(args[sep + 2], "--ping");
    }

    #[test]
    fn bwrap_backend_resolve_wraps_with_bwrap_argv() {
        let (dir, worker_home, work_dir, executable, _backend) = fixture();
        let bwrap = dir.path().join("bwrap");
        fs::write(&bwrap, "wrapper").unwrap();
        let search_path = executable.parent().unwrap().display().to_string();
        let spawn = resolve_sandbox_spawn_for(
            "custom-mcp",
            &["a".into()],
            &work_dir,
            &worker_home,
            Some(&search_path),
            BackendSelection {
                backend: SandboxBackend::Bwrap,
                wrapper: Some(bwrap.clone()),
            },
        )
        .unwrap();
        assert_eq!(spawn.backend, SandboxBackend::Bwrap);
        assert_eq!(
            spawn.program,
            bwrap.canonicalize().unwrap().display().to_string()
        );
        assert!(spawn.args.iter().any(|a| a == "--die-with-parent"));
        assert!(!spawn.args.iter().any(|a| a == "--unshare-net"));
        assert_eq!(spawn.args.last().map(String::as_str), Some("a"));
    }

    #[test]
    fn unavailable_npx_fails_before_spawn() {
        let (_dir, worker_home, work_dir, _executable, backend) = fixture();
        let err = resolve_sandbox_spawn_with_backend(
            "npx",
            &["-y".into(), "@pkg".into()],
            &work_dir,
            &worker_home,
            Some("/definitely/no/bin"),
            Some(&backend),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("npx") && err.contains("unavailable"), "{err}");
    }

    #[test]
    fn custom_args_and_executable_provenance_are_preserved() {
        let (_dir, worker_home, work_dir, executable, backend) = fixture();
        let search_path = executable.parent().unwrap().display().to_string();
        let custom_args = vec![
            "--scope".into(),
            "value with spaces".into(),
            "--literal=$TOKEN".into(),
        ];
        let spawn = resolve_sandbox_spawn_with_backend(
            "custom-mcp",
            &custom_args,
            &work_dir,
            &worker_home,
            Some(&search_path),
            Some(&backend),
        )
        .unwrap();
        let canonical = executable.canonicalize().unwrap();
        assert_eq!(spawn.executable, canonical);
        assert_eq!(spawn.args[2], canonical.display().to_string());
        assert_eq!(&spawn.args[3..], custom_args.as_slice());
        assert!(spawn
            .path
            .split(':')
            .any(|part| part == canonical.parent().unwrap().display().to_string()));
        let profile = &spawn.args[1];
        assert!(profile.contains(&format!(
            "(subpath \"{}\")",
            canonical.parent().unwrap().display()
        )));
        assert!(!profile.contains(&format!(
            "(subpath \"{}\")",
            canonical.parent().unwrap().parent().unwrap().display()
        )));
    }

    #[cfg(unix)]
    #[test]
    fn user_root_node_layout_never_grants_the_user_root() {
        let dir = tempdir().unwrap();
        let user_root = dir.path().join("user-root");
        let work_dir = user_root.join("project");
        let worker_home = user_root.join(".locus/workers/sess_test");
        let bin_dir = user_root.join("bin");
        fs::create_dir_all(&work_dir).unwrap();
        fs::create_dir_all(&worker_home).unwrap();
        fs::create_dir_all(&bin_dir).unwrap();
        fs::write(user_root.join(".locus/daemon.key"), "authority-canary").unwrap();
        let executable = work_dir.join("custom-mcp");
        let node = bin_dir.join("node");
        make_executable(&executable, "#!/usr/bin/env node\n");
        make_executable(&node, "#!/bin/sh\n/bin/cat \"$2\"\n");

        let search_path = bin_dir.display().to_string();
        let profile = seatbelt_profile_for_worker_with_path(
            &work_dir,
            &worker_home,
            &executable,
            Some(&search_path),
        )
        .unwrap();
        let canonical_root = user_root.canonicalize().unwrap();
        assert!(!profile.contains(&format!("(subpath \"{}\")", canonical_root.display())));
        assert!(profile.contains(&format!(
            "(literal \"{}\")",
            node.canonicalize().unwrap().display()
        )));
        assert!(!profile.contains("daemon.key"));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_alias_to_broad_runtime_root_fails_closed() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let user_root = dir.path().join("user-root");
        let work_dir = user_root.join("project");
        let worker_home = user_root.join(".locus/workers/sess_test");
        fs::create_dir_all(&work_dir).unwrap();
        fs::create_dir_all(&worker_home).unwrap();
        fs::write(user_root.join(".locus/daemon.key"), "authority-canary").unwrap();
        let broad_target = user_root.join("custom-mcp-real");
        make_executable(&broad_target, "#!/bin/sh\nexit 0\n");
        let alias = work_dir.join("custom-mcp");
        symlink(&broad_target, &alias).unwrap();

        let err = seatbelt_profile_for_worker_with_path(
            &work_dir,
            &worker_home,
            &alias,
            Some("/usr/bin:/bin"),
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("runtime root") && err.contains("LOCUS_HOME"),
            "{err}"
        );
    }

    #[test]
    fn broad_work_directory_containing_locus_home_fails_closed() {
        let (dir, worker_home, _work_dir, executable, _backend) = fixture();
        let err = seatbelt_profile_for_worker(dir.path(), &worker_home, &executable)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("work directory") && err.contains("LOCUS_HOME"),
            "{err}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn daemon_key_is_not_readable_inside_real_sandbox() {
        let (dir, worker_home, work_dir, _executable, _backend) = fixture();
        let daemon_key = dir.path().join("custom-locus-home").join("daemon.key");
        let spawn = resolve_sandbox_spawn(
            "/bin/cat",
            &[daemon_key.display().to_string()],
            &work_dir,
            &worker_home,
        )
        .unwrap();
        let output = std::process::Command::new(&spawn.program)
            .args(&spawn.args)
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "sandbox unexpectedly read daemon.key: status={} stdout={:?} stderr={:?}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("authority-canary"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn user_root_node_layout_cannot_read_daemon_key_in_real_sandbox() {
        let dir = tempdir().unwrap();
        let user_root = dir.path().join("user-root");
        let work_dir = user_root.join("project");
        let worker_home = user_root.join(".locus/workers/sess_test");
        let bin_dir = user_root.join("bin");
        fs::create_dir_all(&work_dir).unwrap();
        fs::create_dir_all(&worker_home).unwrap();
        fs::create_dir_all(&bin_dir).unwrap();
        let daemon_key = user_root.join(".locus/daemon.key");
        fs::write(&daemon_key, "authority-canary").unwrap();
        make_executable(&work_dir.join("custom-mcp"), "#!/usr/bin/env node\n");
        make_executable(&bin_dir.join("node"), "#!/bin/sh\n/bin/cat \"$2\"\n");

        let spawn = resolve_sandbox_spawn_with_backend(
            "custom-mcp",
            &[daemon_key.display().to_string()],
            &work_dir,
            &worker_home,
            Some(&format!(
                "{}:{}:/usr/bin:/bin",
                work_dir.display(),
                bin_dir.display()
            )),
            Some(Path::new("/usr/bin/sandbox-exec")),
        )
        .unwrap();
        let output = std::process::Command::new(&spawn.program)
            .args(&spawn.args)
            .current_dir(&work_dir)
            .env_clear()
            .env("PATH", &spawn.path)
            .env("HOME", &worker_home)
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "sandbox unexpectedly read daemon.key"
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("authority-canary"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn oauth_callback_listener_cannot_bind_in_real_sandbox() {
        use std::process::Stdio;
        use std::time::Duration;

        let (_dir, worker_home, work_dir, _executable, _backend) = fixture();
        let spawn = resolve_sandbox_spawn(
            "/usr/bin/nc",
            &["-l".into(), "127.0.0.1".into(), "0".into()],
            &work_dir,
            &worker_home,
        )
        .unwrap();
        let mut child = std::process::Command::new(&spawn.program)
            .args(&spawn.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        for _ in 0..50 {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(
                    !status.success(),
                    "sandbox unexpectedly allowed an OAuth callback listener"
                );
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = child.kill();
        let _ = child.wait();
        panic!("sandbox unexpectedly allowed an OAuth callback listener to remain active");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn docker_style_unix_socket_is_denied_in_real_sandbox() {
        use std::os::unix::net::UnixListener;
        use std::time::{Duration, Instant};

        let (dir, worker_home, work_dir, _executable, _backend) = fixture();
        let socket_path = dir.path().join("docker.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let accept = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                match listener.accept() {
                    Ok(_) => return true,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => return false,
                }
            }
            false
        });
        let spawn = resolve_sandbox_spawn(
            "/usr/bin/nc",
            &["-U".into(), socket_path.display().to_string()],
            &work_dir,
            &worker_home,
        )
        .unwrap();
        let output = std::process::Command::new(&spawn.program)
            .args(&spawn.args)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        let connected = accept.join().unwrap();
        assert!(
            !output.status.success() && !connected,
            "sandbox unexpectedly reached a Docker-style Unix socket: status={} stderr={:?}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn live_docker_socket_is_denied_in_real_sandbox_when_present() {
        let socket_path = Path::new("/var/run/docker.sock");
        if !socket_path.exists() {
            return;
        }
        let (_dir, worker_home, work_dir, _executable, _backend) = fixture();
        let spawn = resolve_sandbox_spawn(
            "/usr/bin/nc",
            &["-U".into(), socket_path.display().to_string()],
            &work_dir,
            &worker_home,
        )
        .unwrap();
        let output = std::process::Command::new(&spawn.program)
            .args(&spawn.args)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "sandbox unexpectedly connected to the live Docker daemon socket"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn outbound_network_policy_allows_provider_connections() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port().to_string();
        let (_dir, worker_home, work_dir, _executable, _backend) = fixture();
        let spawn = resolve_sandbox_spawn(
            "/usr/bin/nc",
            &[
                "-z".into(),
                "-w".into(),
                "1".into(),
                "127.0.0.1".into(),
                port,
            ],
            &work_dir,
            &worker_home,
        )
        .unwrap();
        let output = std::process::Command::new(&spawn.program)
            .args(&spawn.args)
            .current_dir(&work_dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "sandbox blocked intended outbound connection: {:?}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn available_npx_keeps_canonical_provenance_inside_real_sandbox() {
        let (_dir, worker_home, work_dir, _executable, _backend) = fixture();
        if resolve_executable("npx", &work_dir, std::env::var("PATH").ok().as_deref()).is_err() {
            return;
        }
        let spawn =
            resolve_sandbox_spawn("npx", &["--version".into()], &work_dir, &worker_home).unwrap();
        fs::create_dir_all(worker_home.join("tmp")).unwrap();
        let output = std::process::Command::new(&spawn.program)
            .args(&spawn.args)
            .current_dir(&work_dir)
            .env_clear()
            .env("PATH", &spawn.path)
            .env("HOME", &worker_home)
            .env("TMPDIR", worker_home.join("tmp"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "protected npx failed: status={} stderr={:?}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(spawn.args[2].starts_with('/'));
    }
}

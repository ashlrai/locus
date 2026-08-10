//! Fail-closed OS sandbox preparation for upstream workers.
//!
//! `sandbox = true` means the worker is wrapped by a supported operating-system
//! isolation backend. Environment markers and PATH filtering are diagnostics;
//! they are never treated as isolation by themselves.

use crate::error::{LocusError, Result};
use std::path::{Component, Path, PathBuf};

/// Env: enable sandbox for all MCP stdio workers (`1` / `true` / `yes`).
pub const ENV_WORKER_SANDBOX: &str = "LOCUS_WORKER_SANDBOX";

/// Marker injected into sandboxed worker children (never a secret).
pub const ENV_WORKER_SANDBOXED: &str = "LOCUS_WORKER_SANDBOXED";

/// Applied OS isolation backend.
pub const ENV_WORKER_SANDBOX_BACKEND: &str = "LOCUS_WORKER_SANDBOX_BACKEND";

/// Whether global env requests sandbox mode.
pub fn sandbox_from_env() -> bool {
    match std::env::var(ENV_WORKER_SANDBOX) {
        Ok(v) => {
            let t = v.trim();
            t == "1" || t.eq_ignore_ascii_case("true") || t.eq_ignore_ascii_case("yes")
        }
        Err(_) => false,
    }
}

/// Effective sandbox: config/spec flag OR env.
pub fn sandbox_enabled(config_flag: bool) -> bool {
    config_flag || sandbox_from_env()
}

/// Restricted PATH for sandboxed workers.
///
/// The resolved executable's directory is prepended separately so a protected
/// absolute executable keeps working even when it lives outside system paths.
pub fn restricted_worker_path() -> String {
    ["/usr/bin", "/bin", "/usr/sbin", "/sbin", "/usr/local/bin"].join(":")
}

/// How sandbox was applied for a spawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxBackend {
    /// macOS Seatbelt via `/usr/bin/sandbox-exec`.
    SandboxExec,
}

impl SandboxBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SandboxExec => "sandbox-exec",
        }
    }
}

/// True only when a supported OS isolation backend is installed.
#[allow(dead_code)]
pub fn sandbox_exec_available() -> bool {
    sandbox_backend_path().is_some()
}

fn sandbox_backend_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let path = PathBuf::from("/usr/bin/sandbox-exec");
        path.is_file().then_some(path)
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
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

fn executable_read_root(executable: &Path) -> &Path {
    if let Some(node_root) = executable
        .ancestors()
        .find(|ancestor| ancestor.join("bin").join("node").is_file())
    {
        return node_root;
    }
    let parent = executable.parent().unwrap_or(Path::new("/nonexistent"));
    parent
}

/// Deny-by-default Seatbelt profile for one worker.
///
/// The current work tree and worker home are the only caller-owned trees made
/// readable. The rest of LOCUS_HOME (including daemon.key, bindings, sessions,
/// approvals, and audit) and the user's ambient home remain inaccessible.
/// Network is outbound-only because upstream MCP servers are provider clients;
/// listening sockets are not part of the worker contract.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn seatbelt_profile_for_worker(
    work_dir: &Path,
    worker_home: &Path,
    executable: &Path,
) -> Result<String> {
    let wd = canonical_existing_dir(work_dir, "work directory")?;
    let wh = canonical_existing_dir(worker_home, "worker home")?;
    let locus_home = locus_home_from_worker_home(&wh)?;
    if wd.starts_with(&locus_home) && !wd.starts_with(&wh) {
        return Err(LocusError::msg(
            "sandbox work directory overlaps protected LOCUS_HOME authority state",
        ));
    }
    let executable = executable.canonicalize().map_err(|e| {
        LocusError::msg(format!(
            "sandbox executable `{}` cannot be canonicalized: {e}",
            executable.display()
        ))
    })?;
    if executable.starts_with(&locus_home) {
        return Err(LocusError::msg(
            "sandbox executable cannot be loaded from protected LOCUS_HOME",
        ));
    }
    let executable_root = executable_read_root(&executable);

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
    (subpath "{exe_root}")
    (literal "{exe}"))
(allow signal (target self))
(allow ipc-posix*)
(allow system-socket)
(system-network)
(allow network-outbound)
(allow file-read-metadata file-test-existence
    (path-ancestors "{wd}")
    (path-ancestors "{wh}")
    (path-ancestors "{exe_root}")
    (path-ancestors "{exe}"))
(allow file-read* file-map-executable
    (subpath "{wd}")
    (subpath "{wh}")
    (subpath "{exe_root}")
    (literal "{exe}"))
(allow file-write* (subpath "{wd}") (subpath "{wh}"))
"#,
        wd = seatbelt_escape(&wd.display().to_string()),
        wh = seatbelt_escape(&wh.display().to_string()),
        exe_root = seatbelt_escape(&executable_root.display().to_string()),
        exe = seatbelt_escape(&executable.display().to_string()),
    ))
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn seatbelt_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Resolved spawn line after mandatory OS sandbox wrapping.
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

/// Build program/args/backend for a sandboxed spawn (does not touch env yet).
///
/// Missing backend or executable is an error. There is deliberately no
/// PATH-only fallback.
pub fn resolve_sandbox_spawn(
    program: &str,
    args: &[String],
    work_dir: &Path,
    worker_home: &Path,
) -> Result<SandboxSpawn> {
    resolve_sandbox_spawn_with_backend(
        program,
        args,
        work_dir,
        worker_home,
        std::env::var("PATH").ok().as_deref(),
        sandbox_backend_path().as_deref(),
    )
}

fn resolve_sandbox_spawn_with_backend(
    program: &str,
    args: &[String],
    work_dir: &Path,
    worker_home: &Path,
    search_path: Option<&str>,
    backend_path: Option<&Path>,
) -> Result<SandboxSpawn> {
    let backend_path = backend_path.ok_or_else(|| {
        LocusError::msg(
            "sandbox requested but no supported OS isolation backend is available; refusing to spawn",
        )
    })?;
    let backend = backend_path.canonicalize().map_err(|e| {
        LocusError::msg(format!(
            "sandbox backend `{}` is unavailable: {e}",
            backend_path.display()
        ))
    })?;
    let executable = resolve_executable(program, work_dir, search_path)?;
    let profile = seatbelt_profile_for_worker(work_dir, worker_home, &executable)?;
    let mut wrapped = Vec::with_capacity(3 + args.len());
    wrapped.push("-p".into());
    wrapped.push(profile);
    wrapped.push(executable.display().to_string());
    wrapped.extend(args.iter().cloned());

    let executable_dir = executable
        .parent()
        .unwrap_or(Path::new("/nonexistent"))
        .display()
        .to_string();
    let executable_root = executable_read_root(&executable);
    let runtime_bin = executable_root.join("bin");
    let path = if runtime_bin.join("node").is_file() {
        format!(
            "{}:{executable_dir}:{}",
            runtime_bin.display(),
            restricted_worker_path()
        )
    } else {
        format!("{executable_dir}:{}", restricted_worker_path())
    };
    Ok(SandboxSpawn {
        program: backend.display().to_string(),
        args: wrapped,
        backend: SandboxBackend::SandboxExec,
        path,
        executable,
    })
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

    #[test]
    fn restricted_path_has_only_core_bins() {
        let path = restricted_worker_path();
        assert_eq!(path, "/usr/bin:/bin:/usr/sbin:/sbin:/usr/local/bin");
        assert!(!path.split(':').any(|part| part.is_empty()));
    }

    #[test]
    fn sandbox_from_env_truthy() {
        let prev = std::env::var(ENV_WORKER_SANDBOX).ok();
        std::env::set_var(ENV_WORKER_SANDBOX, "1");
        assert!(sandbox_from_env());
        assert!(sandbox_enabled(false));
        std::env::set_var(ENV_WORKER_SANDBOX, "true");
        assert!(sandbox_from_env());
        std::env::set_var(ENV_WORKER_SANDBOX, "0");
        assert!(!sandbox_from_env());
        assert!(sandbox_enabled(true));
        match prev {
            Some(v) => std::env::set_var(ENV_WORKER_SANDBOX, v),
            None => std::env::remove_var(ENV_WORKER_SANDBOX),
        }
    }

    #[test]
    fn profile_binds_custom_locus_home_and_outbound_network() {
        let (_dir, worker_home, work_dir, executable, _backend) = fixture();
        let profile = seatbelt_profile_for_worker(&work_dir, &worker_home, &executable).unwrap();
        assert!(profile.contains("(deny default)"));
        assert!(profile.contains("(allow network-outbound)"));
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
            .starts_with(canonical.parent().unwrap().display().to_string().as_str()));
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
    fn inbound_network_cannot_fall_through_real_sandbox() {
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
                assert!(!status.success(), "sandbox unexpectedly allowed a listener");
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = child.kill();
        let _ = child.wait();
        panic!("sandbox unexpectedly allowed an inbound listener to remain active");
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

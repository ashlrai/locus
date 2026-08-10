//! Best-effort worker sandbox options.
//!
//! When enabled (`LOCUS_WORKER_SANDBOX=1`, [`super::McpStdioConfig::sandbox`], or
//! binding `upstream.sandbox = true`):
//!
//! 1. Set marker `LOCUS_WORKER_SANDBOXED=1` (and optional backend tag).
//! 2. Restrict `PATH` to `/usr/bin:/bin:/usr/local/bin` + cargo bin when present.
//! 3. On macOS, optionally wrap the spawn with `sandbox-exec` when available
//!    (best-effort — never fail closed if `sandbox-exec` is missing).
//!
//! Ambient identity scrub is already done by isolation; this layer is additive.

use std::path::{Path, PathBuf};

/// Env: enable sandbox for all MCP stdio workers (`1` / `true` / `yes`).
pub const ENV_WORKER_SANDBOX: &str = "LOCUS_WORKER_SANDBOX";

/// Marker injected into sandboxed worker children (never a secret).
pub const ENV_WORKER_SANDBOXED: &str = "LOCUS_WORKER_SANDBOXED";

/// Optional backend tag: `path` | `sandbox-exec`.
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
/// Always: `/usr/bin:/bin:/usr/local/bin`.
/// Plus `$CARGO_HOME/bin` or `$HOME/.cargo/bin` when that directory exists.
pub fn restricted_worker_path() -> String {
    let mut parts: Vec<String> = vec!["/usr/bin".into(), "/bin".into(), "/usr/local/bin".into()];
    for cargo_bin in cargo_bin_dirs() {
        let s = cargo_bin.display().to_string();
        if !parts.iter().any(|p| p == &s) {
            parts.push(s);
        }
    }
    parts.join(":")
}

fn cargo_bin_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(cargo_home) = std::env::var("CARGO_HOME") {
        let p = PathBuf::from(cargo_home).join("bin");
        if p.is_dir() {
            out.push(p);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home).join(".cargo").join("bin");
        if p.is_dir() && !out.iter().any(|x| x == &p) {
            out.push(p);
        }
    }
    out
}

/// How sandbox was applied for a spawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxBackend {
    /// PATH restriction + marker only.
    Path,
    /// macOS `sandbox-exec` wrap + PATH restriction.
    SandboxExec,
}

impl SandboxBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Path => "path",
            Self::SandboxExec => "sandbox-exec",
        }
    }
}

/// True when `sandbox-exec` binary exists (macOS Seatbelt wrapper).
pub fn sandbox_exec_available() -> bool {
    #[cfg(target_os = "macos")]
    {
        which_sandbox_exec().is_some()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

#[cfg(target_os = "macos")]
fn which_sandbox_exec() -> Option<PathBuf> {
    if Path::new("/usr/bin/sandbox-exec").is_file() {
        return Some(PathBuf::from("/usr/bin/sandbox-exec"));
    }
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            let candidate = Path::new(dir).join("sandbox-exec");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Mild Seatbelt profile for best-effort OS wrap.
///
/// Isolation already scrubs ambient identity. This profile denies reading other
/// bindings under `~/.locus/bindings` while allowing the worker home, work dir,
/// temp, and default operations (network included for upstream MCP).
pub fn seatbelt_profile_for_worker(work_dir: &Path, worker_home: &Path) -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/nonexistent".into());
    let wd = seatbelt_escape(&work_dir.display().to_string());
    let wh = seatbelt_escape(&worker_home.display().to_string());
    let home_e = seatbelt_escape(&home);
    format!(
        r#"(version 1)
(allow default)
; Locus worker sandbox — best-effort seatbelt wrap (PATH already restricted)
(deny file-read* (subpath "{home_e}/.locus/bindings"))
(deny file-write* (subpath "{home_e}/.locus"))
(allow file-read* (subpath "{wd}") (subpath "{wh}") (subpath "/tmp") (subpath "/private/tmp") (subpath "/var/folders"))
(allow file-write* (subpath "{wd}") (subpath "{wh}") (subpath "/tmp") (subpath "/private/tmp") (subpath "/var/folders"))
"#
    )
}

fn seatbelt_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Resolved spawn line after optional sandbox wrap.
#[derive(Debug, Clone)]
pub struct SandboxSpawn {
    pub program: String,
    pub args: Vec<String>,
    pub backend: SandboxBackend,
    /// Restricted PATH value.
    pub path: String,
}

/// Build program/args/backend for a sandboxed spawn (does not touch env yet).
///
/// When `sandbox-exec` is available on macOS, wraps:
/// `sandbox-exec -p PROFILE -- <program> <args…>`
/// Otherwise keeps the original program and applies PATH-only backend.
pub fn resolve_sandbox_spawn(
    program: &str,
    args: &[String],
    work_dir: &Path,
    worker_home: &Path,
) -> SandboxSpawn {
    let path = restricted_worker_path();

    #[cfg(target_os = "macos")]
    {
        // Use the public probe so availability stays consistent with wrap path.
        if sandbox_exec_available() {
            if let Some(se) = which_sandbox_exec() {
                let profile = seatbelt_profile_for_worker(work_dir, worker_home);
                let mut wrapped = Vec::with_capacity(3 + args.len());
                wrapped.push("-p".into());
                wrapped.push(profile);
                wrapped.push(program.to_string());
                wrapped.extend(args.iter().cloned());
                return SandboxSpawn {
                    program: se.display().to_string(),
                    args: wrapped,
                    backend: SandboxBackend::SandboxExec,
                    path,
                };
            }
        }
    }

    let _ = (work_dir, worker_home);
    SandboxSpawn {
        program: program.to_string(),
        args: args.to_vec(),
        backend: SandboxBackend::Path,
        path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restricted_path_has_core_bins() {
        let p = restricted_worker_path();
        assert!(p.contains("/usr/bin"));
        assert!(p.contains("/bin"));
        assert!(p.contains("/usr/local/bin"));
        assert!(!p.split(':').any(|s| s.is_empty()));
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
        assert!(sandbox_enabled(true)); // config flag still wins
        match prev {
            Some(v) => std::env::set_var(ENV_WORKER_SANDBOX, v),
            None => std::env::remove_var(ENV_WORKER_SANDBOX),
        }
    }

    #[test]
    fn seatbelt_profile_mentions_work_dir() {
        let p = seatbelt_profile_for_worker(Path::new("/tmp/wd"), Path::new("/tmp/wh"));
        assert!(p.contains("/tmp/wd"));
        assert!(p.contains("/tmp/wh"));
        assert!(p.contains("version 1"));
    }

    #[test]
    fn sandbox_exec_available_is_bool() {
        // Probe must not panic; true only when binary exists (macOS).
        let _ = sandbox_exec_available();
        #[cfg(not(target_os = "macos"))]
        assert!(!sandbox_exec_available());
    }

    #[test]
    fn resolve_path_backend_keeps_program() {
        // Force path backend by not relying on macos wrap success for identity.
        let s = resolve_sandbox_spawn(
            "python3",
            &["-u".into(), "-c".into(), "pass".into()],
            Path::new("/tmp/wd"),
            Path::new("/tmp/wh"),
        );
        assert!(!s.path.is_empty());
        // Either path-only or sandbox-exec wrap on macOS — program is non-empty either way.
        assert!(!s.program.is_empty());
        if s.backend == SandboxBackend::Path {
            assert_eq!(s.program, "python3");
            assert_eq!(s.args.len(), 3);
        } else {
            assert_eq!(s.backend, SandboxBackend::SandboxExec);
            assert!(s.args.iter().any(|a| a == "python3"));
        }
    }
}

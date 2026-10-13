//! CredentialRef resolution.
//!
//! Formats:
//! - `phm:NAME`     — retained reference; resolution requires an unavailable scoped bridge
//! - `env:VAR`      — read from process environment
//! - `test:VALUE`   — compiled unit tests only; release binaries always reject it
//!
//! Values are held in `Zeroizing<String>` and must only be injected into
//! worker env maps — never returned over MCP or printed to agent-facing stdout.

use crate::error::{LocusError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::process::{Command, Stdio};
#[cfg(test)]
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

/// Process-lifetime cache of `phantom --version` success.
///
/// Doctor, agent report, forensics, and the dashboard all need to know whether
/// Phantom is on PATH. Shelling out on every probe is slow (and dashboard polls
/// `/api/doctor` often). Probe once per process; result is sticky for the life
/// of the binary.
pub fn phantom_on_path() -> bool {
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(probe_phantom_version)
}

/// Hard deadline for the `phantom --version` PATH probe. Same fail-closed
/// treatment as any metadata probe: a wedged binary must never hang doctor,
/// verify, or the dashboard poll — probe failure just means "not on PATH".
const PHANTOM_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

fn probe_phantom_version() -> bool {
    let mut cmd = Command::new("phantom");
    cmd.arg("--version");
    run_capture_stdout_with_deadline(cmd, PHANTOM_PROBE_TIMEOUT, "phantom --version")
        .map(|(status, _)| status.success())
        .unwrap_or(false)
}

/// Parsed credential reference (no secret material).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CredentialRef {
    /// Phantom vault secret name.
    Phantom { name: String },
    /// Environment variable in the parent process.
    Env { var: String },
    /// Test-only plaintext (gated).
    Test { value: String },
    /// Unrecognized / unsupported scheme.
    Unknown { raw: String },
}

impl CredentialRef {
    pub fn parse(raw: &str) -> Self {
        let raw = raw.trim();
        if let Some(rest) = raw.strip_prefix("phm:") {
            return Self::Phantom {
                name: rest.to_string(),
            };
        }
        if let Some(rest) = raw.strip_prefix("env:") {
            return Self::Env {
                var: rest.to_string(),
            };
        }
        if let Some(rest) = raw.strip_prefix("test:") {
            return Self::Test {
                value: rest.to_string(),
            };
        }
        Self::Unknown {
            raw: raw.to_string(),
        }
    }

    /// Parse and validate a reference accepted in a binding.
    ///
    /// Only explicit supported schemes are accepted. `test:` is compiled out
    /// of production acceptance and cannot be enabled by environment.
    pub fn validate(raw: &str) -> Result<Self> {
        if raw != raw.trim() {
            return Err(invalid_ref());
        }
        let parsed = Self::parse(raw);
        match &parsed {
            Self::Phantom { name } if valid_phantom_name(name) => Ok(parsed),
            Self::Env { var } if valid_env_name(var) => Ok(parsed),
            Self::Test { value } if value.is_empty() => Err(invalid_ref()),
            Self::Test { .. } if cfg!(test) => Ok(parsed),
            _ => Err(invalid_ref()),
        }
    }

    /// Safe source label for agent-facing metadata. Never includes the ref name.
    pub fn source(&self) -> &'static str {
        match self {
            Self::Phantom { .. } => "phantom",
            Self::Env { .. } => "environment",
            Self::Test { .. } => "test",
            Self::Unknown { .. } => "unsupported",
        }
    }
}

/// Agent-safe credential metadata. The reference name/value is intentionally absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialMetadata {
    pub present: bool,
    pub source: String,
}

pub fn credential_metadata(raw: &str) -> CredentialMetadata {
    let parsed = CredentialRef::parse(raw);
    CredentialMetadata {
        present: !raw.trim().is_empty(),
        source: parsed.source().to_string(),
    }
}

/// Safe resolution failure metadata. Locator names and provider stderr are absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialResolutionIssue {
    pub provider: String,
    pub source: String,
    pub code: String,
}

impl fmt::Display for CredentialResolutionIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "provider={} source={} code={}",
            self.provider, self.source, self.code
        )
    }
}

#[derive(Debug)]
pub struct ResolvedBindingSecrets {
    pub env: BTreeMap<String, Zeroizing<String>>,
    pub issues: Vec<CredentialResolutionIssue>,
}

fn safe_provider_label(provider: &str) -> String {
    if !provider.is_empty()
        && provider.len() <= 64
        && provider
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    {
        provider.to_ascii_lowercase()
    } else {
        "unknown".into()
    }
}

fn valid_phantom_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphanumeric())
        && chars.all(|c| c == '_' || c == '-' || c == '.' || c.is_ascii_alphanumeric())
}

fn valid_env_name(var: &str) -> bool {
    let mut chars = var.chars();
    matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn invalid_ref() -> LocusError {
    LocusError::msg("invalid credential_ref: use explicit phm:NAME or env:VAR")
}

/// Convert only a conservative legacy bare Phantom name. Unsafe input is never returned.
pub fn migrate_legacy_phantom_ref(raw: &str) -> Option<String> {
    let mut chars = raw.chars();
    let conservative_name = matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_uppercase())
        && chars.all(|c| {
            c == '_' || c == '-' || c == '.' || c.is_ascii_uppercase() || c.is_ascii_digit()
        });
    if raw == raw.trim() && !raw.contains(':') && conservative_name {
        Some(format!("phm:{raw}"))
    } else {
        None
    }
}

/// Resolve a credential ref to a secret value.
///
/// # Safety
/// Caller must not log or serialize the returned value into agent context.
pub fn resolve(cred: &CredentialRef) -> Result<Zeroizing<String>> {
    match cred {
        CredentialRef::Phantom { .. } => Err(LocusError::PhantomCredentialIntegrationUnsupported),
        CredentialRef::Env { var } => {
            let v = std::env::var(var)
                .map_err(|_| LocusError::msg("credential unavailable (source=environment)"))?;
            Ok(Zeroizing::new(v))
        }
        CredentialRef::Test { value } => {
            if !cfg!(test) {
                return Err(LocusError::msg("credential source unsupported"));
            }
            Ok(Zeroizing::new(value.clone()))
        }
        CredentialRef::Unknown { .. } => Err(invalid_ref()),
    }
}

/// Preflight only operations that request credential resolution.
/// Reference parsing/storage remains available for migration and metadata.
pub fn ensure_binding_credential_resolution_supported(
    binding: &crate::binding::Binding,
) -> Result<()> {
    if binding.providers.iter().any(|provider| {
        matches!(
            CredentialRef::parse(&provider.credential_ref),
            CredentialRef::Phantom { .. }
        )
    }) {
        return Err(LocusError::PhantomCredentialIntegrationUnsupported);
    }
    Ok(())
}

/// Map provider → standard env var names that receive the resolved secret.
pub fn inject_keys_for_provider(provider: &str) -> &'static [&'static str] {
    match provider.to_ascii_lowercase().as_str() {
        "supabase" => &["SUPABASE_ACCESS_TOKEN"],
        "github" => &["GH_TOKEN", "GITHUB_TOKEN", "GITHUB_PERSONAL_ACCESS_TOKEN"],
        "vercel" => &["VERCEL_TOKEN"],
        "cloudflare" => &["CLOUDFLARE_API_TOKEN"],
        "aws" => &["AWS_SECRET_ACCESS_KEY"], // incomplete without key id — phase 2
        "resend" => &["RESEND_API_KEY"],
        "stripe" => &["STRIPE_API_KEY", "STRIPE_SECRET_KEY"],
        "openai" => &["OPENAI_API_KEY"],
        "anthropic" => &["ANTHROPIC_API_KEY"],
        "xai" => &["XAI_API_KEY"],
        _ => &[],
    }
}

/// Report unsupported Phantom credential references without invoking the
/// binary or opening a vault. Installation/name presence cannot prove that
/// a scoped credential bridge is available.
pub fn collect_unresolved_phm_refs(
    store: &crate::store::Store,
    _phantom_on_path: bool,
) -> Result<Vec<CredentialResolutionIssue>> {
    let mut unresolved = Vec::new();
    for summary in store.list_bindings()? {
        let binding = match store.load_binding(&summary.alias) {
            Ok(binding) => binding,
            Err(_) => continue,
        };
        for provider in binding.providers {
            if matches!(
                CredentialRef::parse(&provider.credential_ref),
                CredentialRef::Phantom { .. }
            ) {
                unresolved.push(CredentialResolutionIssue {
                    provider: safe_provider_label(&provider.provider),
                    source: "phantom".into(),
                    code: "unsupported-integration".into(),
                });
            }
        }
    }
    unresolved.sort_by(|a, b| a.provider.cmp(&b.provider));
    unresolved.dedup_by(|a, b| a.provider == b.provider);
    Ok(unresolved)
}

/// Cap on concurrent stdout-reader helper threads (short-lived readers plus
/// any left behind after a deadline kill by a grandchild that escaped the
/// process-group kill and still holds the pipe). At the cap, new
/// deadline-guarded children are refused (fail closed) — a repeatedly wedged
/// phantom can never accumulate unbounded blocked threads.
const MAX_LIVE_READERS: usize = 16;

static LIVE_READERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// RAII slot in the live-reader budget; held by the reader thread for its
/// whole life so the count stays honest even for detached threads.
struct ReaderSlot;

impl ReaderSlot {
    fn try_acquire() -> Option<Self> {
        use std::sync::atomic::Ordering;
        let prev = LIVE_READERS.fetch_add(1, Ordering::SeqCst);
        if prev >= MAX_LIVE_READERS {
            LIVE_READERS.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Self)
    }
}

impl Drop for ReaderSlot {
    fn drop(&mut self) {
        LIVE_READERS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(test)]
fn live_reader_count() -> usize {
    LIVE_READERS.load(std::sync::atomic::Ordering::SeqCst)
}

/// Kill the child and (on Unix) its whole process group — a wrapper shell
/// that forked the real binary would otherwise leave a grandchild holding
/// the stdout pipe open forever. The child was made its own process-group
/// leader at spawn (`setpgid` in `pre_exec`).
fn kill_child_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        // Negative pid targets the process group. Best-effort: if setpgid
        // failed at spawn this is a no-op and the plain kill below applies.
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Bounded reap of the stdout reader thread after the child tree was killed:
/// the group kill closes the pipe in the overwhelmingly common case, so the
/// thread finishes almost immediately — join it. A pathological survivor
/// (grandchild that re-set its own process group) leaves the thread detached;
/// its [`ReaderSlot`] keeps the live count honest and [`MAX_LIVE_READERS`]
/// bounds accumulation (fail closed at the cap).
fn reap_reader_bounded(reader: std::thread::JoinHandle<Vec<u8>>) {
    let deadline = Instant::now() + Duration::from_millis(500);
    while !reader.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    if reader.is_finished() {
        let _ = reader.join();
    }
}

/// Run `cmd` capturing stdout, hard-killing the child (and, on Unix, its
/// process group) at `timeout` — std `try_wait` poll loop with a small sleep.
/// Stdout is drained on a helper thread so a full pipe can never block the
/// child before the deadline check; the thread budget is capped so timeouts
/// can never leak unbounded blocked readers.
fn run_capture_stdout_with_deadline(
    mut cmd: Command,
    timeout: Duration,
    label: &str,
) -> Result<(std::process::ExitStatus, Vec<u8>)> {
    let Some(slot) = ReaderSlot::try_acquire() else {
        return Err(LocusError::msg(format!(
            "cannot run {label}: helper thread budget exhausted (wedged children?)"
        )));
    };
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Own process group so the deadline kill takes out grandchildren too
        // (the stdout pipe then closes and the reader thread exits instead of
        // blocking forever).
        unsafe {
            cmd.pre_exec(|| {
                libc::setpgid(0, 0);
                Ok(())
            });
        }
    }
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| LocusError::msg(format!("cannot run {label}")))?;
    let stdout_pipe = child.stdout.take();
    let reader = match std::thread::Builder::new()
        .name(format!("locus-cred-reader ({label})"))
        .spawn(move || {
            let _slot = slot;
            let mut buf = Vec::new();
            if let Some(mut out) = stdout_pipe {
                use std::io::Read;
                let _ = out.read_to_end(&mut buf);
            }
            buf
        }) {
        Ok(handle) => handle,
        Err(_) => {
            kill_child_tree(&mut child);
            return Err(LocusError::msg(format!("cannot run {label}")));
        }
    };

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = reader.join().unwrap_or_default();
                return Ok((status, stdout));
            }
            Ok(None) if Instant::now() >= deadline => {
                kill_child_tree(&mut child);
                reap_reader_bounded(reader);
                return Err(LocusError::msg(format!(
                    "{label} timed out after {timeout:?} — child killed"
                )));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => {
                kill_child_tree(&mut child);
                reap_reader_bounded(reader);
                return Err(LocusError::msg(format!("cannot run {label}")));
            }
        }
    }
}

/// Resolve all provider credentials for a binding into env var injections.
/// Returns map of env_key → secret. Values must not be logged.
pub fn resolve_binding_secrets(binding: &crate::binding::Binding) -> ResolvedBindingSecrets {
    let mut out = BTreeMap::new();
    let mut issues = Vec::new();
    for p in &binding.providers {
        let cred = CredentialRef::parse(&p.credential_ref);
        let value = match resolve(&cred) {
            Ok(v) => v,
            Err(error) => {
                issues.push(CredentialResolutionIssue {
                    provider: safe_provider_label(&p.provider),
                    source: cred.source().into(),
                    code: if matches!(error, LocusError::PhantomCredentialIntegrationUnsupported) {
                        "unsupported-integration"
                    } else {
                        "unavailable"
                    }
                    .into(),
                });
                continue;
            }
        };
        for key in inject_keys_for_provider(&p.provider) {
            out.insert(
                (*key).to_string(),
                Zeroizing::new(value.as_str().to_string()),
            );
        }
        // Also set LOCUS_<PROVIDER>_RESOLVED=1 (not the secret) for debugging
        let flag = format!("LOCUS_{}_CREDENTIAL_RESOLVED", p.provider.to_uppercase());
        // Don't put secrets in out under flag — use empty marker via separate path
        let _ = flag;
    }
    ResolvedBindingSecrets { env: out, issues }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_formats() {
        assert_eq!(
            CredentialRef::parse("phm:FOO"),
            CredentialRef::Phantom { name: "FOO".into() }
        );
        assert_eq!(
            CredentialRef::parse("env:BAR"),
            CredentialRef::Env { var: "BAR".into() }
        );
        assert!(matches!(
            CredentialRef::parse("BARE"),
            CredentialRef::Unknown { .. }
        ));
    }

    #[test]
    fn validation_rejects_bare_tokens_and_bad_explicit_refs_without_echoing_them() {
        for raw in ["sk_live_canary_secret", "ghp_canary_secret", "oauth:token"] {
            let err = CredentialRef::validate(raw).unwrap_err().to_string();
            assert!(!err.contains(raw), "validation error leaked candidate ref");
        }
        for raw in ["phm:", "env:NOT-A-VAR", " env:GOOD"] {
            assert!(CredentialRef::validate(raw).is_err());
        }
    }

    #[test]
    fn metadata_never_contains_reference_name_or_value() {
        let metadata = credential_metadata("phm:TOP_SECRET_CANARY");
        let json = serde_json::to_string(&metadata).unwrap();
        assert_eq!(metadata.source, "phantom");
        assert!(metadata.present);
        assert!(!json.contains("TOP_SECRET_CANARY"));
    }

    #[test]
    fn resolve_env_and_test() {
        std::env::set_var("LOCUS_TEST_SECRET_XYZ", "s3cret");
        let v = resolve(&CredentialRef::Env {
            var: "LOCUS_TEST_SECRET_XYZ".into(),
        })
        .unwrap();
        assert_eq!(v.as_str(), "s3cret");
        std::env::remove_var("LOCUS_TEST_SECRET_XYZ");

        assert!(CredentialRef::validate("test:tval").is_ok());
        let v = resolve(&CredentialRef::Test {
            value: "tval".into(),
        })
        .unwrap();
        assert_eq!(v.as_str(), "tval");
    }

    #[test]
    fn legacy_migration_accepts_only_conservative_bare_names() {
        assert_eq!(
            migrate_legacy_phantom_ref("GH_TOKEN_ACME").as_deref(),
            Some("phm:GH_TOKEN_ACME")
        );
        for unsafe_raw in [
            "ghp_secret_value",
            "ghp_secret/value",
            " name",
            "name:other",
            "name\nnext",
        ] {
            assert!(migrate_legacy_phantom_ref(unsafe_raw).is_none());
        }
    }

    #[test]
    fn inject_keys() {
        assert!(inject_keys_for_provider("supabase").contains(&"SUPABASE_ACCESS_TOKEN"));
        assert!(inject_keys_for_provider("github").contains(&"GH_TOKEN"));
    }

    fn phm_binding(credential_ref: &str) -> crate::binding::Binding {
        crate::binding::Binding::from_body(crate::binding::BindingBody {
            id: "bnd_cred".into(),
            alias: "cred".into(),
            tenant: "tenant".into(),
            principal: None,
            description: None,
            policy: crate::binding::Policy::default(),
            providers: vec![crate::binding::ProviderBinding {
                provider: "github".into(),
                account: "acct".into(),
                credential_ref: credential_ref.into(),
                scope: crate::binding::Scope::default(),
                upstream: None,
            }],
        })
    }

    // These read-only credential fixtures do not authorize control mutations
    // or start an authority broker. Store loading still validates the TOML.
    fn write_binding_fixture(store: &crate::store::Store, binding: &crate::binding::Binding) {
        std::fs::write(
            store.bindings_dir().join(format!("{}.toml", binding.alias)),
            binding.to_toml().unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn collect_unresolved_phm_refs_reports_when_phantom_missing() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::store::Store::open(dir.path()).unwrap();
        write_binding_fixture(&store, &phm_binding("phm:CRED_TEST_MISSING_CANARY"));

        let issues = collect_unresolved_phm_refs(&store, false).unwrap();
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert_eq!(issues[0].provider, "github");
        assert_eq!(issues[0].source, "phantom");
        assert_eq!(issues[0].code, "unsupported-integration");
        // Locator names never leak into the metadata.
        let json = serde_json::to_string(&issues).unwrap();
        assert!(!json.contains("CRED_TEST_MISSING_CANARY"), "{json}");
    }

    #[test]
    fn collect_unresolved_phm_refs_empty_without_phm_refs() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::store::Store::open(dir.path()).unwrap();
        write_binding_fixture(&store, &phm_binding("env:SOME_VAR"));
        // No phm refs → empty result and no `phantom list` shell-out.
        assert!(collect_unresolved_phm_refs(&store, false)
            .unwrap()
            .is_empty());
        assert!(collect_unresolved_phm_refs(&store, true)
            .unwrap()
            .is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn deadline_runner_kills_slow_child_and_errors() {
        let start = Instant::now();
        let mut cmd = Command::new("sleep");
        cmd.arg("5");
        let result =
            run_capture_stdout_with_deadline(cmd, Duration::from_millis(150), "phantom list");
        let err = result.expect_err("slow child must produce an error");
        assert!(
            err.to_string().contains("phantom list timed out"),
            "timeout error must name the operation: {err}"
        );
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "deadline runner must not wait for the child's natural exit"
        );
    }

    /// Regression: a killed wrapper whose grandchild still holds the stdout
    /// pipe must not leak a permanently-blocked reader thread — the process-
    /// group kill closes the pipe and the reader exits promptly.
    #[cfg(unix)]
    #[test]
    fn deadline_runner_group_kills_grandchildren_and_reaps_reader() {
        let start = Instant::now();
        let mut cmd = Command::new("/bin/sh");
        // The backgrounded grandchild inherits stdout; without the group kill
        // it would hold the read pipe open for 30s after the child is killed.
        cmd.arg("-c").arg("sleep 30 & exec sleep 30");
        let err = run_capture_stdout_with_deadline(cmd, Duration::from_millis(150), "phantom list")
            .expect_err("wedged child must time out");
        assert!(err.to_string().contains("timed out"), "{err}");
        assert!(start.elapsed() < Duration::from_secs(5));

        // The reader thread exits once the group kill closes the pipe; the
        // live count drains back to its idle level (other tests' readers are
        // all short-lived too).
        let deadline = Instant::now() + Duration::from_secs(5);
        while live_reader_count() > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            live_reader_count(),
            0,
            "reader thread leaked after group kill"
        );
    }

    #[cfg(unix)]
    #[test]
    fn deadline_runner_captures_stdout_of_fast_child() {
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg("printf 'A\\nB\\n'");
        let (status, out) =
            run_capture_stdout_with_deadline(cmd, Duration::from_secs(5), "phantom list").unwrap();
        assert!(status.success());
        assert_eq!(String::from_utf8_lossy(&out), "A\nB\n");
    }

    /// Serialize the PATH-override tests: PATH is process-global.
    #[cfg(unix)]
    fn path_override_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    /// Run `f` with a fake `phantom` executable (with the given script body)
    /// prepended to PATH, restoring PATH afterwards.
    #[cfg(unix)]
    fn with_fake_phantom<T>(script_body: &str, f: impl FnOnce() -> T) -> T {
        let guard = match path_override_lock().lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("phantom");
        std::fs::write(&bin, format!("#!/bin/sh\n{script_body}\n")).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let old_path = std::env::var_os("PATH");
        std::env::set_var(
            "PATH",
            format!(
                "{}:{}",
                dir.path().display(),
                old_path
                    .as_deref()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default()
            ),
        );
        let out = f();
        match old_path {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
        drop(guard);
        out
    }

    #[cfg(unix)]
    #[test]
    fn phantom_resolution_fails_before_binary_invocation() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("phantom-invoked");
        let script = format!("printf invoked > '{}'", marker.display());
        with_fake_phantom(&script, || {
            let error = resolve(&CredentialRef::Phantom {
                name: "REFERENCE_NAME_CANARY".into(),
            })
            .expect_err("no supported scoped bridge exists");
            assert!(matches!(
                error,
                LocusError::PhantomCredentialIntegrationUnsupported
            ));
            let message = error.to_string();
            assert!(message.contains("env:VAR"));
            assert!(!message.contains("REFERENCE_NAME_CANARY"));
            assert!(
                !marker.exists(),
                "unsupported resolution must not invoke phantom"
            );
        });
    }

    #[test]
    fn binding_preflight_and_safe_issue_metadata_reject_phantom_without_ambient_fallback() {
        let binding = phm_binding("phm:REFERENCE_NAME_CANARY");
        assert!(matches!(
            ensure_binding_credential_resolution_supported(&binding),
            Err(LocusError::PhantomCredentialIntegrationUnsupported)
        ));
        let resolved = resolve_binding_secrets(&binding);
        assert!(resolved.env.is_empty());
        assert_eq!(resolved.issues.len(), 1);
        assert_eq!(resolved.issues[0].code, "unsupported-integration");
        assert!(!serde_json::to_string(&resolved.issues)
            .unwrap()
            .contains("REFERENCE_NAME_CANARY"));
        ensure_binding_credential_resolution_supported(&phm_binding("env:UNUSED")).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn doctor_flags_unsupported_integration_even_with_phantom_installed_without_invocation() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("phantom-invoked");
        let script = format!("printf invoked > '{}'", marker.display());
        let store = crate::store::Store::open(dir.path().join("store")).unwrap();
        write_binding_fixture(&store, &phm_binding("phm:REFERENCE_NAME_CANARY"));
        with_fake_phantom(&script, || {
            let issues = collect_unresolved_phm_refs(&store, true).unwrap();
            assert_eq!(issues.len(), 1);
            assert_eq!(issues[0].code, "unsupported-integration");
            assert!(!marker.exists());
        });
    }

    #[test]
    fn phantom_probe_deadline_stays_bounded() {
        assert_eq!(PHANTOM_PROBE_TIMEOUT, Duration::from_secs(2));
    }
}

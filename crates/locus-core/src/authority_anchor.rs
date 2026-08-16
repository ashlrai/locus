//! Capability-authenticated, supervised local authority generations.
//!
//! The durable store contains only an endpoint description. Issuance is a
//! two-step, capability-bound control operation. Control and executor
//! capabilities exist only in trusted parent and supervised child environments;
//! bearer values are never placed in argv or durable state.

use crate::error::{LocusError, Result};
use fs2::FileExt;
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub const AUTHORITY_ANCHOR_VERSION: u32 = 1;
const AUTHORITY_ENDPOINT_VERSION: u32 = 3;
pub const CONTROL_CAPABILITY_ENV: &str = "LOCUS_CONTROL_CAPABILITY";
pub const EXECUTOR_CAPABILITY_ENV: &str = "LOCUS_EXECUTOR_CAPABILITY";
const SERVER_ENV: &str = "LOCUS_INTERNAL_AUTHORITY_ANCHOR_SERVER";
const MAX_MESSAGE_BYTES: usize = 16 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(2);
/// Production wait for broker handoff (spawn + SHA-256 identity + socket bind).
/// 3s proved flaky under CI load after heavy `cargo test` + large debug binaries.
const START_TIMEOUT: Duration = Duration::from_secs(10);
const TEST_START_TIMEOUT: Duration = Duration::from_secs(15);
/// Optional override: `LOCUS_AUTHORITY_BROKER_START_TIMEOUT_MS` (clamped 500–120_000).
const BROKER_START_TIMEOUT_ENV: &str = "LOCUS_AUTHORITY_BROKER_START_TIMEOUT_MS";
const CONTROL_TTL: Duration = Duration::from_secs(2);
const MAX_CLIENTS: usize = 64;
const SUPERVISOR_POLL: Duration = Duration::from_millis(250);
static EXECUTOR_ONLY_VALIDATION: AtomicBool = AtomicBool::new(false);

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutableIdentity {
    pub sha256: String,
    pub length: u64,
    pub modified_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inode: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SupervisorIdentity {
    pub pid: u32,
    pub uid: String,
    pub start_token: String,
    pub executable: String,
    pub executable_identity: ExecutableIdentity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthorityAnchorEndpoint {
    pub version: u32,
    pub transport: String,
    pub address: String,
    pub epoch: String,
    pub endpoint_generation: u64,
    pub owner_pid: u32,
    pub owner_executable: String,
    pub owner_executable_identity: ExecutableIdentity,
    pub supervisor: SupervisorIdentity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionAuthorityAnchor {
    pub version: u32,
    pub epoch: String,
    pub generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValidationMode {
    Manual,
    Executor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ServerStartup {
    home: PathBuf,
    epoch: String,
    endpoint_generation: u64,
    launcher_executable: String,
    supervisor: SupervisorIdentity,
    control_capability: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ControlOperation {
    Issue {
        session_id: String,
        backing_type: String,
        subject_digest: String,
    },
    GrantExecutor {
        session_id: String,
        backing_type: String,
        generation: u64,
        subject_digest: String,
    },
    Revoke {
        session_id: String,
        backing_type: String,
        generation: u64,
    },
    Retire,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum AnchorRequest {
    Ping,
    BeginControl {
        operation: ControlOperation,
    },
    Issue {
        challenge: String,
        session_id: String,
        backing_type: String,
        subject_digest: String,
    },
    GrantExecutor {
        challenge: String,
        session_id: String,
        backing_type: String,
        epoch: String,
        generation: u64,
        subject_digest: String,
    },
    Validate {
        session_id: String,
        backing_type: String,
        epoch: String,
        generation: u64,
        subject_digest: String,
    },
    Revoke {
        challenge: String,
        session_id: String,
        backing_type: String,
        epoch: String,
        generation: u64,
    },
    Retire {
        challenge: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum RequestCredential {
    Control,
    Executor { capability_id: String },
}

#[derive(Debug, Serialize, Deserialize)]
struct AuthenticatedRequest {
    version: u32,
    nonce: String,
    credential: RequestCredential,
    request: AnchorRequest,
    mac: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct AuthenticatedResponse {
    version: u32,
    nonce: String,
    response: AnchorResponse,
    mac: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct AnchorResponse {
    ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    epoch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    challenge: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    executor_capability: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error_code: Option<AnchorErrorCode>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum AnchorErrorCode {
    StaleGeneration,
    ExecutorCapabilityRequired,
}

#[derive(Debug)]
struct PendingControl {
    peer_pid: u32,
    operation: ControlOperation,
    expires: Instant,
}

#[derive(Debug)]
struct ExecutorGrant {
    session_id: String,
    backing_type: String,
    generation: u64,
    subject_digest: String,
    key: [u8; 32],
}

#[derive(Debug)]
struct AnchorState {
    epoch: String,
    control_key: [u8; 32],
    next_generation: u64,
    active: Option<(String, u64, String)>,
    ephemeral: HashMap<(String, String), (u64, String)>,
    controls: HashMap<String, PendingControl>,
    executors: HashMap<String, ExecutorGrant>,
}

#[derive(Debug, Clone)]
struct PeerIdentity {
    pid: u32,
    uid: String,
    executable: PathBuf,
}

#[derive(Clone)]
struct BrokerIdentity {
    uid: String,
}

#[derive(Clone)]
struct RequestAuth {
    credential: RequestCredential,
    key: [u8; 32],
}

/// Executables call this before argument parsing. Startup data arrives over an
/// anonymous pipe; no control bearer is placed in argv, env, or durable state.
pub fn run_authority_anchor_server_if_requested() -> Option<Result<()>> {
    std::env::var_os(SERVER_ENV)?;
    Some(read_startup().and_then(run_child_server))
}

pub(crate) fn issue(
    home: &Path,
    session_id: &str,
    backing_type: &str,
    subject_digest: &str,
) -> Result<SessionAuthorityAnchor> {
    let auth = control_auth()?;
    let endpoint = ensure_broker(home, &auth)?;
    let operation = ControlOperation::Issue {
        session_id: session_id.to_string(),
        backing_type: backing_type.to_string(),
        subject_digest: subject_digest.to_string(),
    };
    let challenge = begin_control(&endpoint, operation, &auth)?;
    let response = request(
        &endpoint,
        &AnchorRequest::Issue {
            challenge,
            session_id: session_id.to_string(),
            backing_type: backing_type.to_string(),
            subject_digest: subject_digest.to_string(),
        },
        &auth,
    )?;
    if !response.ok || response.epoch.as_deref() != Some(endpoint.epoch.as_str()) {
        return Err(control_error(
            response,
            "live anchor refused authority issuance",
        ));
    }
    Ok(SessionAuthorityAnchor {
        version: AUTHORITY_ANCHOR_VERSION,
        epoch: endpoint.epoch,
        generation: response.generation.ok_or_else(|| {
            LocusError::AuthorityAnchorUnavailable("anchor generation missing".into())
        })?,
    })
}

/// Authenticate the operator-held control capability and ensure its supervised
/// broker is current without issuing or mutating a session generation.
pub(crate) fn authorize_control(home: &Path) -> Result<()> {
    let auth = control_auth()?;
    let endpoint = ensure_broker(home, &auth)?;
    ping(home, &endpoint, &auth)
}

/// Authenticate the operator control capability for **wedged-session
/// teardown** (`locus leave --force`).
///
/// Unlike [`authorize_control`], this never requires — and never starts — a
/// live supervised broker: the whole point of forced teardown is recovering
/// when the supervisor/broker is gone and every anchor-validated path fails
/// closed. The gate stays real (fail closed):
///
/// - the env capability must be present and well-formed ([`control_auth`]);
/// - when anything answers the recorded broker endpoint, the capability must
///   authenticate against it exactly like any other control operation — a
///   live broker refusing the capability refuses teardown; only a truly
///   unreachable endpoint is the recovery path;
/// - when the broker is unreachable but a persisted operator capability
///   exists (0600 file minted at init/quickstart), the env value must match
///   it byte-for-byte (constant-time);
/// - when the broker is unreachable and NO persisted capability exists (the
///   deliberate `locus init --no-persist-capability` strict posture), no
///   verifier remains, so teardown is refused unless the operator explicitly
///   acknowledged the degraded gate (`locus leave --force --no-verifier`,
///   `allow_unverified`). The acknowledgement never overrides a verifier
///   that is present and disagrees.
///
/// Teardown authority can only delete session state — it never mints,
/// validates, or issues session generations.
pub(crate) fn authorize_control_teardown(home: &Path, allow_unverified: bool) -> Result<()> {
    let auth = control_auth()?;
    authorize_control_teardown_with(home, allow_unverified, &auth)
}

fn authorize_control_teardown_with(
    home: &Path,
    allow_unverified: bool,
    auth: &RequestAuth,
) -> Result<()> {
    if let Ok(endpoint) = read_endpoint(home) {
        match ping(home, &endpoint, auth) {
            Ok(()) => return Ok(()),
            Err(_) => {
                // Distinguish an authenticated rejection from a gone broker:
                // if anything still answers the endpoint socket, the refusal
                // is authoritative and teardown fails closed — exactly like
                // normal `locus leave` against the same broker.
                if platform::connect(&endpoint, IO_TIMEOUT).is_ok() {
                    return Err(LocusError::ExecutorAuthorityUnavailable(
                        "a live authority broker answered but refused this control capability — \
                         refusing forced session teardown"
                            .into(),
                    ));
                }
                // Endpoint recorded but nothing answers: the broker is gone;
                // fall through to the persisted-capability verifier.
            }
        }
    }
    match read_persisted_control_capability(home)? {
        Some(persisted) => {
            let persisted_key = decode_capability(&persisted, "persisted control")?;
            let mismatch = auth
                .key
                .iter()
                .zip(persisted_key.iter())
                .fold(0u8, |acc, (a, b)| acc | (a ^ b));
            if mismatch != 0 {
                return Err(LocusError::ExecutorAuthorityUnavailable(
                    "control capability does not match the persisted operator capability — \
                     refusing forced session teardown"
                        .into(),
                ));
            }
            Ok(())
        }
        None if allow_unverified => Ok(()),
        None => Err(LocusError::ExecutorAuthorityUnavailable(
            "no verifier is available for forced teardown (authority broker unreachable and no \
             persisted operator capability under this home) — re-run as \
             `locus leave --force --no-verifier` to explicitly acknowledge tearing down without \
             capability verification"
                .into(),
        )),
    }
}

pub(crate) fn validate(
    home: &Path,
    lease: &SessionAuthorityAnchor,
    session_id: &str,
    backing_type: &str,
    subject_digest: &str,
) -> Result<ValidationMode> {
    let (auth, mode) = validation_auth()?;
    let endpoint = read_endpoint(home).map_err(|error| {
        LocusError::AuthorityAnchorUnavailable(format!("no current broker endpoint: {error}"))
    })?;
    validate_endpoint_shape(home, &endpoint)?;
    if lease.version != AUTHORITY_ANCHOR_VERSION || lease.epoch != endpoint.epoch {
        return Err(LocusError::AuthorityAnchorMismatch);
    }
    let response = request(
        &endpoint,
        &AnchorRequest::Validate {
            session_id: session_id.to_string(),
            backing_type: backing_type.to_string(),
            epoch: lease.epoch.clone(),
            generation: lease.generation,
            subject_digest: subject_digest.to_string(),
        },
        &auth,
    )?;
    if response.ok {
        Ok(mode)
    } else if mode == ValidationMode::Executor
        && response.error_code == Some(AnchorErrorCode::ExecutorCapabilityRequired)
    {
        Err(LocusError::ExecutorAuthorityUnavailable(
            response
                .error
                .unwrap_or_else(|| "executor capability rejected".into()),
        ))
    } else {
        Err(LocusError::AuthorityAnchorMismatch)
    }
}

pub(crate) fn grant_executor(
    home: &Path,
    lease: &SessionAuthorityAnchor,
    session_id: &str,
    backing_type: &str,
    subject_digest: &str,
) -> Result<String> {
    let auth = control_auth()?;
    let endpoint = read_endpoint(home)?;
    let operation = ControlOperation::GrantExecutor {
        session_id: session_id.to_string(),
        backing_type: backing_type.to_string(),
        generation: lease.generation,
        subject_digest: subject_digest.to_string(),
    };
    let challenge = begin_control(&endpoint, operation, &auth)?;
    let response = request(
        &endpoint,
        &AnchorRequest::GrantExecutor {
            challenge,
            session_id: session_id.to_string(),
            backing_type: backing_type.to_string(),
            epoch: lease.epoch.clone(),
            generation: lease.generation,
            subject_digest: subject_digest.to_string(),
        },
        &auth,
    )?;
    if response.ok {
        response.executor_capability.ok_or_else(|| {
            LocusError::AuthorityAnchorUnavailable("executor capability missing".into())
        })
    } else {
        Err(control_error(response, "executor grant refused"))
    }
}

pub(crate) fn revoke(
    home: &Path,
    lease: &SessionAuthorityAnchor,
    session_id: &str,
    backing_type: &str,
) -> Result<()> {
    let auth = control_auth()?;
    let endpoint = read_endpoint(home)?;
    let operation = ControlOperation::Revoke {
        session_id: session_id.to_string(),
        backing_type: backing_type.to_string(),
        generation: lease.generation,
    };
    let challenge = begin_control(&endpoint, operation, &auth)?;
    let response = request(
        &endpoint,
        &AnchorRequest::Revoke {
            challenge,
            session_id: session_id.to_string(),
            backing_type: backing_type.to_string(),
            epoch: lease.epoch.clone(),
            generation: lease.generation,
        },
        &auth,
    )?;
    if response.ok {
        Ok(())
    } else {
        Err(control_error(response, "authority revoke refused"))
    }
}

#[cfg(all(test, unix))]
pub(crate) fn retire_for_test(home: &Path) -> Result<()> {
    let auth = control_auth()?;
    let endpoint = read_endpoint(home)?;
    let challenge = begin_control(&endpoint, ControlOperation::Retire, &auth)?;
    let response = request(&endpoint, &AnchorRequest::Retire { challenge }, &auth)?;
    if response.ok {
        Ok(())
    } else {
        Err(control_error(response, "anchor retirement refused"))
    }
}

fn validation_auth() -> Result<(RequestAuth, ValidationMode)> {
    if let Some(capability) = capability_from_env(EXECUTOR_CAPABILITY_ENV)? {
        let key = decode_capability(&capability, "executor")?;
        return Ok((
            RequestAuth {
                credential: RequestCredential::Executor {
                    capability_id: hash_capability(&capability),
                },
                key,
            },
            ValidationMode::Executor,
        ));
    }
    if EXECUTOR_ONLY_VALIDATION.load(Ordering::SeqCst) {
        return Err(LocusError::ExecutorAuthorityUnavailable(
            "this process requires a supervised executor capability".into(),
        ));
    }
    Ok((control_auth()?, ValidationMode::Manual))
}

/// Permanently narrow this process to executor validation authority.
///
/// Agent-facing transports call this before starting worker threads. The flag
/// only removes authority: once set, a control capability inherited from a
/// parent cannot be used to validate provider execution in this process.
pub fn restrict_validation_to_executor() {
    EXECUTOR_ONLY_VALIDATION.store(true, Ordering::SeqCst);
}

fn control_auth() -> Result<RequestAuth> {
    let capability = match capability_from_env(CONTROL_CAPABILITY_ENV)? {
        Some(capability) => capability,
        None if is_test_harness() => test_control_capability(),
        None => {
            return Err(LocusError::ExecutorAuthorityUnavailable(
                "LOCUS_CONTROL_CAPABILITY is required for control-plane authority.\n  \
                 fix (fresh setup): locus quickstart   # mints + persists an operator capability (0600), then adopts it\n  \
                 fix (manual):      export LOCUS_CONTROL_CAPABILITY=\"$(openssl rand -hex 32)\"\n  \
                 fix (persisted):   eval \"$(locus hook zsh)\"   # exports $LOCUS_HOME/control_capability if present\n  \
                 check:             locus doctor"
                    .into(),
            ))
        }
    };
    Ok(RequestAuth {
        credential: RequestCredential::Control,
        key: decode_capability(&capability, "control")?,
    })
}

fn capability_from_env(name: &str) -> Result<Option<String>> {
    match std::env::var(name) {
        Ok(value) if value.trim().is_empty() => Err(LocusError::ExecutorAuthorityUnavailable(
            format!("{name} is empty or invalid"),
        )),
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(LocusError::ExecutorAuthorityUnavailable(
            format!("{name} is not valid Unicode"),
        )),
    }
}

fn decode_capability(value: &str, label: &str) -> Result<[u8; 32]> {
    let raw = hex::decode(value.trim()).map_err(|_| {
        LocusError::ExecutorAuthorityUnavailable(format!(
            "{label} capability must be 32 bytes encoded as lowercase hex"
        ))
    })?;
    if raw.len() != 32 || value.trim().len() != 64 || value.trim() != value.trim().to_lowercase() {
        return Err(LocusError::ExecutorAuthorityUnavailable(format!(
            "{label} capability must be 32 bytes encoded as lowercase hex"
        )));
    }
    let mut key = [0_u8; 32];
    key.copy_from_slice(&raw);
    Ok(key)
}

fn test_control_capability() -> String {
    static CAPABILITY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CAPABILITY.get_or_init(|| random_hex(32)).clone()
}

// ── Operator capability persistence (onboarding convenience) ───────────────
//
// The control gate itself stays env-only: `control_auth` never reads durable
// state. What *is* persisted (with explicit operator intent via
// `locus quickstart` / `locus init`) is a 0600 operator-owned file that
// `locus hook` exports back into the shell — the same trust boundary as the
// seal key living in the same directory. Fail closed: an existing file is
// never silently replaced.

/// Path of the persisted operator control capability under a Locus home.
pub fn control_capability_file(home: &Path) -> PathBuf {
    home.join("control_capability")
}

/// Read + validate the persisted operator control capability, if present.
pub fn read_persisted_control_capability(home: &Path) -> Result<Option<String>> {
    let path = control_capability_file(home);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path).map_err(|error| {
        LocusError::msg(format!(
            "cannot read control capability at {}: {error}",
            path.display()
        ))
    })?;
    let value = raw.trim().to_string();
    decode_capability(&value, "persisted control").map_err(|_| {
        LocusError::msg(format!(
            "persisted control capability at {} is invalid (expected 64 lowercase hex chars) — \
             delete it deliberately and re-run `locus quickstart` to mint a fresh one",
            path.display()
        ))
    })?;
    Ok(Some(value))
}

/// Mint a fresh operator control capability and persist it 0600.
///
/// Fail closed: never overwrites an existing file (silently replacing a
/// mismatched capability would mask a real control-plane conflict).
pub fn mint_persisted_control_capability(home: &Path) -> Result<String> {
    let path = control_capability_file(home);
    if path.exists() {
        return Err(LocusError::msg(format!(
            "refusing to overwrite existing control capability at {} — export it instead \
             (eval \"$(locus hook zsh)\") or delete the file deliberately before re-minting",
            path.display()
        )));
    }
    let value = random_hex(32);
    write_control_capability_file(home, &value)?;
    Ok(value)
}

/// Mint a fresh control capability value WITHOUT persisting it.
///
/// Strict-posture onboarding (`locus init --no-persist-capability`): the value
/// lives only in the process env; the operator keeps a copy via the printed
/// export line in their shell profile.
pub fn mint_ephemeral_control_capability() -> String {
    random_hex(32)
}

/// Persist an already-held control capability 0600 (`locus capability persist`).
///
/// Idempotent when the persisted file already carries the same value; fail
/// closed on any mismatch or invalid existing file — Locus never silently
/// replaces a capability.
pub fn persist_control_capability(home: &Path, value: &str) -> Result<()> {
    let value = value.trim();
    decode_capability(value, "control")?;
    if let Some(existing) = read_persisted_control_capability(home)? {
        if existing == value {
            return Ok(());
        }
        return Err(LocusError::msg(format!(
            "refusing to overwrite existing control capability at {} with a different value — \
             remove it deliberately first (locus capability unpersist)",
            control_capability_file(home).display()
        )));
    }
    write_control_capability_file(home, value)
}

/// Remove the persisted control capability file (`locus capability unpersist`).
///
/// Returns whether a file was removed. Removal only narrows durable state —
/// the capability already exported in operator shells keeps working.
pub fn unpersist_control_capability(home: &Path) -> Result<bool> {
    let path = control_capability_file(home);
    if !path.exists() {
        return Ok(false);
    }
    fs::remove_file(&path).map_err(|error| {
        LocusError::msg(format!(
            "cannot remove control capability at {}: {error}",
            path.display()
        ))
    })?;
    Ok(true)
}

fn write_control_capability_file(home: &Path, value: &str) -> Result<()> {
    fs::create_dir_all(home)?;
    let path = control_capability_file(home);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path).map_err(|error| {
        LocusError::msg(format!(
            "cannot create control capability at {}: {error}",
            path.display()
        ))
    })?;
    file.write_all(value.as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
}

/// Operator-facing control-capability readiness (never carries bearer values).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlCapabilityStatus {
    /// `LOCUS_CONTROL_CAPABILITY` is set (non-empty).
    pub env_present: bool,
    /// Env value decodes as a valid capability (64 lowercase hex chars).
    pub env_valid: bool,
    /// A persisted capability file exists under the home.
    pub persisted: bool,
    /// The persisted file decodes as a valid capability.
    pub persisted_valid: bool,
    /// Persisted file is not readable by group/other (unix only; true elsewhere).
    pub persisted_permissions_ok: bool,
    /// Some(true/false) only when both env and file are valid.
    pub matches_persisted: Option<bool>,
    /// Test-harness fallback capability is in effect (mirrors `control_auth`).
    pub test_fallback: bool,
}

impl ControlCapabilityStatus {
    /// Whether `control_auth()` would succeed in this process right now.
    pub fn satisfied(&self) -> bool {
        self.env_valid || self.test_fallback
    }
}

/// Compute control-capability readiness for `home` from the process env.
pub fn control_capability_status(home: &Path) -> ControlCapabilityStatus {
    let env_value = std::env::var(CONTROL_CAPABILITY_ENV).ok();
    control_capability_status_with_env(home, env_value.as_deref())
}

fn control_capability_status_with_env(
    home: &Path,
    env_value: Option<&str>,
) -> ControlCapabilityStatus {
    let env_present = env_value.map(|v| !v.trim().is_empty()).unwrap_or(false);
    let env_valid = env_value
        .map(|v| decode_capability(v, "control").is_ok())
        .unwrap_or(false);

    let path = control_capability_file(home);
    let persisted_raw = fs::read_to_string(&path).ok();
    let persisted = path.exists();
    let persisted_valid = persisted_raw
        .as_deref()
        .map(|v| decode_capability(v.trim(), "control").is_ok())
        .unwrap_or(false);

    let persisted_permissions_ok = if !persisted {
        true
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            fs::metadata(&path)
                .map(|m| m.mode() & 0o077 == 0)
                .unwrap_or(false)
        }
        #[cfg(not(unix))]
        {
            true
        }
    };

    let matches_persisted = match (env_valid, persisted_valid) {
        (true, true) => Some(env_value.map(str::trim) == persisted_raw.as_deref().map(str::trim)),
        _ => None,
    };

    ControlCapabilityStatus {
        env_present,
        env_valid,
        persisted,
        persisted_valid,
        persisted_permissions_ok,
        matches_persisted,
        test_fallback: !env_present && is_test_harness(),
    }
}

fn begin_control(
    endpoint: &AuthorityAnchorEndpoint,
    operation: ControlOperation,
    auth: &RequestAuth,
) -> Result<String> {
    let response = request(endpoint, &AnchorRequest::BeginControl { operation }, auth)?;
    if response.ok {
        response.challenge.ok_or_else(|| {
            LocusError::AuthorityAnchorUnavailable("control challenge missing".into())
        })
    } else {
        Err(control_error(response, "control authorization refused"))
    }
}

fn control_error(response: AnchorResponse, fallback: &str) -> LocusError {
    LocusError::AuthorityAnchorUnavailable(response.error.unwrap_or_else(|| fallback.to_string()))
}

fn ensure_broker(home: &Path, auth: &RequestAuth) -> Result<AuthorityAnchorEndpoint> {
    if let Ok(endpoint) = read_endpoint(home) {
        if ping(home, &endpoint, auth).is_ok() {
            return Ok(endpoint);
        }
    }
    start_broker(home, auth)
}

fn ping(home: &Path, endpoint: &AuthorityAnchorEndpoint, auth: &RequestAuth) -> Result<()> {
    validate_endpoint_shape(home, endpoint)?;
    let response = request(endpoint, &AnchorRequest::Ping, auth)?;
    if response.ok && response.epoch.as_deref() == Some(endpoint.epoch.as_str()) {
        Ok(())
    } else {
        Err(LocusError::AuthorityAnchorUnavailable(
            "broker endpoint generation is not current".into(),
        ))
    }
}

fn start_broker(home: &Path, auth: &RequestAuth) -> Result<AuthorityAnchorEndpoint> {
    if auth.credential != RequestCredential::Control {
        return Err(LocusError::ExecutorAuthorityUnavailable(
            "only a control capability can start the authority broker".into(),
        ));
    }
    fs::create_dir_all(runtime_dir(home))?;
    let executable = std::env::current_exe().map_err(|error| {
        LocusError::AuthorityAnchorUnavailable(format!("cannot locate current executable: {error}"))
    })?;
    let executable = fs::canonicalize(executable)?;
    let startup = ServerStartup {
        home: canonical_home(home)?,
        epoch: random_hex(16),
        endpoint_generation: random_u64(),
        launcher_executable: executable.display().to_string(),
        supervisor: platform::capture_supervisor_identity()?,
        control_capability: hex::encode(auth.key),
    };
    if should_host_in_process() {
        return start_in_process(startup).or_else(|_| retry_current_endpoint(home, auth));
    }

    let mut child = Command::new(&executable)
        .env_clear()
        .env(SERVER_ENV, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // Drain stderr so a chatty failure cannot block on a full pipe, and so
        // timeout errors can include a short diagnostic snip.
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            LocusError::AuthorityAnchorUnavailable(format!(
                "cannot start authority broker: {error}"
            ))
        })?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        LocusError::AuthorityAnchorUnavailable("broker startup pipe unavailable".into())
    })?;
    serde_json::to_writer(&mut stdin, &startup)?;
    stdin.write_all(b"\n")?;
    drop(stdin);

    let stdout = child.stdout.take().ok_or_else(|| {
        LocusError::AuthorityAnchorUnavailable("broker handoff pipe unavailable".into())
    })?;
    let stderr = child.stderr.take();
    let (err_sender, err_receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut buf = String::new();
        if let Some(stderr) = stderr {
            let _ = BufReader::new(stderr).read_to_string(&mut buf);
        }
        let _ = err_sender.send(buf);
    });
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut line = String::new();
        let result = BufReader::new(stdout).read_line(&mut line).map(|_| line);
        let _ = sender.send(result);
    });
    let line = match receiver.recv_timeout(broker_start_timeout()) {
        Ok(Ok(line)) => line,
        Ok(Err(error)) => {
            let _ = child.kill();
            let _ = child.wait();
            let detail = drain_broker_stderr(&err_receiver);
            return Err(LocusError::AuthorityAnchorUnavailable(format!(
                "broker handoff failed: {error}{detail}"
            )));
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            let detail = drain_broker_stderr(&err_receiver);
            return retry_current_endpoint(home, auth).map_err(|_| {
                LocusError::AuthorityAnchorUnavailable(format!("broker startup timed out{detail}"))
            });
        }
    };
    match serde_json::from_str::<AuthorityAnchorEndpoint>(line.trim()) {
        Ok(endpoint) => {
            ping(home, &endpoint, auth)?;
            Ok(endpoint)
        }
        Err(_) => retry_current_endpoint(home, auth),
    }
}

fn retry_current_endpoint(home: &Path, auth: &RequestAuth) -> Result<AuthorityAnchorEndpoint> {
    let deadline = Instant::now() + broker_start_timeout();
    while Instant::now() < deadline {
        if let Ok(endpoint) = read_endpoint(home) {
            if ping(home, &endpoint, auth).is_ok() {
                return Ok(endpoint);
            }
        }
        thread::sleep(Duration::from_millis(25));
    }
    Err(LocusError::AuthorityAnchorUnavailable(
        "no supervised broker owns this Locus home".into(),
    ))
}

fn start_in_process(startup: ServerStartup) -> Result<AuthorityAnchorEndpoint> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let server_sender = sender.clone();
    thread::Builder::new()
        .name("locus-test-authority-broker".into())
        .spawn(move || {
            if let Err(error) = run_server(startup, Some(server_sender)) {
                let _ = sender.send(Err(error));
            }
        })?;
    let endpoint = receiver
        .recv_timeout(broker_start_timeout())
        .map_err(|_| {
            LocusError::AuthorityAnchorUnavailable("test broker startup timed out".into())
        })??;
    Ok(endpoint)
}

fn read_startup() -> Result<ServerStartup> {
    let mut line = String::new();
    BufReader::new(std::io::stdin().take(MAX_MESSAGE_BYTES as u64)).read_line(&mut line)?;
    let startup: ServerStartup = serde_json::from_str(&line)?;
    if startup.epoch.len() != 32 || startup.endpoint_generation == 0 {
        return Err(LocusError::AuthorityAnchorUnavailable(
            "invalid broker startup generation".into(),
        ));
    }
    Ok(startup)
}

fn run_child_server(startup: ServerStartup) -> Result<()> {
    run_server(startup, None)
}

fn run_server(
    startup: ServerStartup,
    ready: Option<mpsc::SyncSender<Result<AuthorityAnchorEndpoint>>>,
) -> Result<()> {
    let home = canonical_home(&startup.home)?;
    fs::create_dir_all(runtime_dir(&home))?;
    let lock_path = broker_lock_path(&home);
    let lock = open_singleton_lock(&lock_path)?;
    if lock.try_lock_exclusive().is_err() {
        if let Some(sender) = ready {
            let _ = sender.send(Err(LocusError::AuthorityAnchorUnavailable(
                "another broker owns this Locus home".into(),
            )));
        }
        return Err(LocusError::AuthorityAnchorUnavailable(
            "another broker owns this Locus home".into(),
        ));
    }

    let owner_executable = fs::canonicalize(&startup.launcher_executable).map_err(|error| {
        LocusError::AuthorityAnchorUnavailable(format!("invalid broker launcher: {error}"))
    })?;
    let owner_executable_identity = executable_identity(&owner_executable)?;
    if !platform::supervisor_matches_identity(&startup.supervisor)? {
        return Err(LocusError::AuthorityAnchorUnavailable(
            "authority supervisor identity is no longer current".into(),
        ));
    }
    let control_key = decode_capability(&startup.control_capability, "control")?;
    let broker_identity = BrokerIdentity {
        uid: platform::current_user_identity()?,
    };
    let endpoint = AuthorityAnchorEndpoint {
        version: AUTHORITY_ENDPOINT_VERSION,
        transport: platform::TRANSPORT.into(),
        address: platform::endpoint_address(&home, &startup.epoch),
        epoch: startup.epoch.clone(),
        endpoint_generation: startup.endpoint_generation,
        owner_pid: std::process::id(),
        owner_executable: owner_executable.display().to_string(),
        owner_executable_identity,
        supervisor: startup.supervisor.clone(),
    };
    let listener = platform::bind(&endpoint)?;
    write_endpoint_atomic(&home, &endpoint)?;
    if let Some(sender) = ready {
        let _ = sender.send(Ok(endpoint.clone()));
    } else {
        println!("{}", serde_json::to_string(&endpoint)?);
        std::io::stdout().flush()?;
    }

    let state = Arc::new(Mutex::new(AnchorState {
        epoch: startup.epoch,
        control_key,
        next_generation: 0,
        active: None,
        ephemeral: HashMap::new(),
        controls: HashMap::new(),
        executors: HashMap::new(),
    }));
    let retired = Arc::new(AtomicBool::new(false));
    let clients = Arc::new(AtomicUsize::new(0));

    let mut next_supervisor_check = Instant::now();
    while !retired.load(Ordering::Acquire) {
        if !home.exists() {
            break;
        }
        if Instant::now() >= next_supervisor_check {
            if !platform::supervisor_is_current(&startup.supervisor)? {
                break;
            }
            next_supervisor_check = Instant::now() + SUPERVISOR_POLL;
        }
        match platform::accept(&listener) {
            Ok(Some(connection)) => {
                if clients.fetch_add(1, Ordering::AcqRel) >= MAX_CLIENTS {
                    clients.fetch_sub(1, Ordering::AcqRel);
                    continue;
                }
                let state = Arc::clone(&state);
                let retired = Arc::clone(&retired);
                let clients = Arc::clone(&clients);
                let identity = broker_identity.clone();
                let error_home = home.clone();
                thread::spawn(move || {
                    if let Err(error) = serve_connection(connection, &identity, &state, &retired) {
                        record_broker_error(&error_home, &error);
                    }
                    clients.fetch_sub(1, Ordering::AcqRel);
                });
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                retire_endpoint_if_current(&home, &endpoint);
                return Err(error);
            }
        }
    }
    platform::retire(&endpoint);
    retire_endpoint_if_current(&home, &endpoint);
    drop(lock);
    Ok(())
}

fn serve_connection(
    mut connection: platform::Connection,
    broker: &BrokerIdentity,
    state: &Arc<Mutex<AnchorState>>,
    retired: &Arc<AtomicBool>,
) -> Result<()> {
    platform::set_deadlines(&connection, IO_TIMEOUT)?;
    let peer = connection_peer_identity(&connection)?;
    authenticate_peer(broker, &peer)?;
    let raw = platform::read_message(&mut connection, MAX_MESSAGE_BYTES, IO_TIMEOUT)?;
    let envelope: AuthenticatedRequest = serde_json::from_slice(&raw).map_err(|_| {
        LocusError::AuthorityAnchorUnavailable("invalid authenticated broker request".into())
    })?;
    if envelope.version != AUTHORITY_ENDPOINT_VERSION
        || envelope.nonce.len() != 64
        || !envelope.nonce.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(LocusError::AuthorityAnchorUnavailable(
            "invalid authenticated broker request shape".into(),
        ));
    }

    let mut guard = state.lock().expect("authority state poisoned");
    let key = request_key(&guard, &envelope.credential).ok_or_else(|| {
        LocusError::AuthorityAnchorUnavailable("unknown authority credential".into())
    })?;
    verify_request_mac(&key, &envelope)?;
    if !credential_allows_request(&envelope.credential, &envelope.request) {
        return write_authenticated_response(
            &mut connection,
            &envelope.nonce,
            response_error("authority credential cannot perform this operation"),
            &key,
        );
    }
    let response = handle_request(
        &mut guard,
        &peer,
        &envelope.credential,
        envelope.request,
        retired,
    );
    drop(guard);
    write_authenticated_response(&mut connection, &envelope.nonce, response, &key)
}

fn request_key(state: &AnchorState, credential: &RequestCredential) -> Option<[u8; 32]> {
    match credential {
        RequestCredential::Control => Some(state.control_key),
        RequestCredential::Executor { capability_id } => {
            state.executors.get(capability_id).map(|grant| grant.key)
        }
    }
}

fn credential_allows_request(credential: &RequestCredential, request: &AnchorRequest) -> bool {
    match credential {
        RequestCredential::Control => true,
        RequestCredential::Executor { .. } => {
            matches!(
                request,
                AnchorRequest::Ping | AnchorRequest::Validate { .. }
            )
        }
    }
}

fn write_authenticated_response(
    connection: &mut platform::Connection,
    nonce: &str,
    response: AnchorResponse,
    key: &[u8; 32],
) -> Result<()> {
    let mac = response_mac(key, nonce, &response)?;
    let envelope = AuthenticatedResponse {
        version: AUTHORITY_ENDPOINT_VERSION,
        nonce: nonce.to_string(),
        response,
        mac,
    };
    platform::write_message(connection, &serde_json::to_vec(&envelope)?, IO_TIMEOUT)
}

fn authenticate_peer(broker: &BrokerIdentity, peer: &PeerIdentity) -> Result<()> {
    if peer.uid != broker.uid {
        return Err(LocusError::AuthorityAnchorUnavailable(
            "broker peer user does not match owner".into(),
        ));
    }
    Ok(())
}

fn handle_request(
    state: &mut AnchorState,
    peer: &PeerIdentity,
    credential: &RequestCredential,
    request: AnchorRequest,
    retired: &AtomicBool,
) -> AnchorResponse {
    state
        .controls
        .retain(|_, control| control.expires > Instant::now());
    match request {
        AnchorRequest::Ping => response_ok(state, None),
        AnchorRequest::BeginControl { operation } => {
            let challenge = random_hex(32);
            state.controls.insert(
                hash_capability(&challenge),
                PendingControl {
                    peer_pid: peer.pid,
                    operation,
                    expires: Instant::now() + CONTROL_TTL,
                },
            );
            AnchorResponse {
                ok: true,
                epoch: Some(state.epoch.clone()),
                generation: None,
                challenge: Some(challenge),
                executor_capability: None,
                error: None,
                error_code: None,
            }
        }
        AnchorRequest::Issue {
            challenge,
            session_id,
            backing_type,
            subject_digest,
        } => {
            let expected = ControlOperation::Issue {
                session_id: session_id.clone(),
                backing_type: backing_type.clone(),
                subject_digest: subject_digest.clone(),
            };
            if !consume_control(state, &challenge, peer.pid, &expected) {
                return response_error(
                    "issue challenge is missing, expired, reused, or out of scope",
                );
            }
            if !valid_subject_digest(&subject_digest) {
                return response_error("invalid session authority subject digest");
            }
            state.next_generation = state.next_generation.saturating_add(1);
            let generation = state.next_generation;
            if backing_type == "active" {
                state.active = Some((session_id, generation, subject_digest));
            } else if backing_type == "run" || backing_type == "ci" {
                state
                    .ephemeral
                    .insert((backing_type, session_id), (generation, subject_digest));
            } else {
                return response_error("invalid backing type");
            }
            response_ok(state, Some(generation))
        }
        AnchorRequest::GrantExecutor {
            challenge,
            session_id,
            backing_type,
            epoch,
            generation,
            subject_digest,
        } => {
            let expected = ControlOperation::GrantExecutor {
                session_id: session_id.clone(),
                backing_type: backing_type.clone(),
                generation,
                subject_digest: subject_digest.clone(),
            };
            if epoch != state.epoch || !consume_control(state, &challenge, peer.pid, &expected) {
                return response_error("executor grant challenge is invalid");
            }
            if !lease_is_current(
                state,
                &session_id,
                &backing_type,
                generation,
                &subject_digest,
            ) {
                return response_error("executor grant targets stale authority");
            }
            let capability = random_hex(32);
            let key = match decode_capability(&capability, "executor") {
                Ok(key) => key,
                Err(_) => return response_error("executor capability generation failed"),
            };
            state.executors.insert(
                hash_capability(&capability),
                ExecutorGrant {
                    session_id,
                    backing_type,
                    generation,
                    subject_digest,
                    key,
                },
            );
            AnchorResponse {
                ok: true,
                epoch: Some(state.epoch.clone()),
                generation: Some(generation),
                challenge: None,
                executor_capability: Some(capability),
                error: None,
                error_code: None,
            }
        }
        AnchorRequest::Validate {
            session_id,
            backing_type,
            epoch,
            generation,
            subject_digest,
        } => {
            if epoch != state.epoch
                || !lease_is_current(
                    state,
                    &session_id,
                    &backing_type,
                    generation,
                    &subject_digest,
                )
            {
                return response_error_code(
                    "stale or unknown authority generation",
                    AnchorErrorCode::StaleGeneration,
                );
            }
            if matches!(credential, RequestCredential::Control) {
                return response_ok(state, Some(generation));
            }
            let valid_executor = match credential {
                RequestCredential::Executor { capability_id } => {
                    state.executors.get(capability_id).is_some_and(|grant| {
                        grant.session_id == session_id
                            && grant.backing_type == backing_type
                            && grant.generation == generation
                            && grant.subject_digest == subject_digest
                    })
                }
                RequestCredential::Control => false,
            };
            if valid_executor {
                response_ok(state, Some(generation))
            } else {
                response_error_code(
                    "a live supervised executor capability is required",
                    AnchorErrorCode::ExecutorCapabilityRequired,
                )
            }
        }
        AnchorRequest::Revoke {
            challenge,
            session_id,
            backing_type,
            epoch,
            generation,
        } => {
            let expected = ControlOperation::Revoke {
                session_id: session_id.clone(),
                backing_type: backing_type.clone(),
                generation,
            };
            if epoch != state.epoch || !consume_control(state, &challenge, peer.pid, &expected) {
                return response_error("revoke challenge is invalid");
            }
            if !lease_generation_is_current(state, &session_id, &backing_type, generation) {
                return response_error_code(
                    "revoke targets stale or unknown authority generation",
                    AnchorErrorCode::StaleGeneration,
                );
            }
            if backing_type == "active" {
                if state
                    .active
                    .as_ref()
                    .is_some_and(|(id, current, _)| id == &session_id && *current == generation)
                {
                    state.active = None;
                }
            } else if state
                .ephemeral
                .get(&(backing_type.clone(), session_id.clone()))
                .is_some_and(|(current, _)| *current == generation)
            {
                state
                    .ephemeral
                    .remove(&(backing_type.clone(), session_id.clone()));
            }
            state.executors.retain(|_, grant| {
                grant.session_id != session_id
                    || grant.backing_type != backing_type
                    || grant.generation != generation
            });
            response_ok(state, Some(generation))
        }
        AnchorRequest::Retire { challenge } => {
            if !consume_control(state, &challenge, peer.pid, &ControlOperation::Retire) {
                return response_error("retirement challenge is invalid");
            }
            retired.store(true, Ordering::Release);
            response_ok(state, None)
        }
    }
}

fn consume_control(
    state: &mut AnchorState,
    challenge: &str,
    peer_pid: u32,
    expected: &ControlOperation,
) -> bool {
    state
        .controls
        .remove(&hash_capability(challenge))
        .is_some_and(|control| {
            control.peer_pid == peer_pid
                && control.expires > Instant::now()
                && &control.operation == expected
        })
}

fn lease_is_current(
    state: &AnchorState,
    session_id: &str,
    backing_type: &str,
    generation: u64,
    subject_digest: &str,
) -> bool {
    if backing_type == "active" {
        state.active.as_ref().is_some_and(|(id, current, digest)| {
            id == session_id && *current == generation && digest == subject_digest
        })
    } else {
        state
            .ephemeral
            .get(&(backing_type.to_string(), session_id.to_string()))
            .is_some_and(|(current, digest)| *current == generation && digest == subject_digest)
    }
}

fn lease_generation_is_current(
    state: &AnchorState,
    session_id: &str,
    backing_type: &str,
    generation: u64,
) -> bool {
    if backing_type == "active" {
        state
            .active
            .as_ref()
            .is_some_and(|(id, current, _)| id == session_id && *current == generation)
    } else {
        state
            .ephemeral
            .get(&(backing_type.to_string(), session_id.to_string()))
            .is_some_and(|(current, _)| *current == generation)
    }
}

fn valid_subject_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn response_ok(state: &AnchorState, generation: Option<u64>) -> AnchorResponse {
    AnchorResponse {
        ok: true,
        epoch: Some(state.epoch.clone()),
        generation,
        challenge: None,
        executor_capability: None,
        error: None,
        error_code: None,
    }
}

fn response_error(error: &str) -> AnchorResponse {
    AnchorResponse {
        ok: false,
        epoch: None,
        generation: None,
        challenge: None,
        executor_capability: None,
        error: Some(error.into()),
        error_code: None,
    }
}

fn response_error_code(error: &str, error_code: AnchorErrorCode) -> AnchorResponse {
    AnchorResponse {
        ok: false,
        epoch: None,
        generation: None,
        challenge: None,
        executor_capability: None,
        error: Some(error.into()),
        error_code: Some(error_code),
    }
}

fn request(
    endpoint: &AuthorityAnchorEndpoint,
    request: &AnchorRequest,
    auth: &RequestAuth,
) -> Result<AnchorResponse> {
    let mut connection = platform::connect(endpoint, IO_TIMEOUT)?;
    platform::set_deadlines(&connection, IO_TIMEOUT)?;
    authenticate_server(endpoint, &connection)?;
    let nonce = random_hex(32);
    let envelope = AuthenticatedRequest {
        version: AUTHORITY_ENDPOINT_VERSION,
        nonce: nonce.clone(),
        credential: auth.credential.clone(),
        request: serde_json::from_value(serde_json::to_value(request)?)?,
        mac: request_mac(&auth.key, &nonce, &auth.credential, request)?,
    };
    let encoded = serde_json::to_vec(&envelope)?;
    if encoded.len() > MAX_MESSAGE_BYTES {
        return Err(LocusError::AuthorityAnchorUnavailable(
            "authority request exceeds protocol limit".into(),
        ));
    }
    platform::write_message(&mut connection, &encoded, IO_TIMEOUT)?;
    let raw = platform::read_message(&mut connection, MAX_MESSAGE_BYTES, IO_TIMEOUT)?;
    let response: AuthenticatedResponse = serde_json::from_slice(&raw).map_err(|error| {
        LocusError::AuthorityAnchorUnavailable(format!("invalid broker response: {error}"))
    })?;
    if response.version != AUTHORITY_ENDPOINT_VERSION || response.nonce != nonce {
        return Err(LocusError::AuthorityAnchorUnavailable(
            "broker response is not bound to this request".into(),
        ));
    }
    verify_response_mac(&auth.key, &response)?;
    Ok(response.response)
}

fn request_mac(
    key: &[u8; 32],
    nonce: &str,
    credential: &RequestCredential,
    request: &AnchorRequest,
) -> Result<String> {
    let material = serde_json::to_vec(&(AUTHORITY_ENDPOINT_VERSION, nonce, credential, request))?;
    Ok(mac_hex(key, &material))
}

fn response_mac(key: &[u8; 32], nonce: &str, response: &AnchorResponse) -> Result<String> {
    let material = serde_json::to_vec(&(AUTHORITY_ENDPOINT_VERSION, nonce, response))?;
    Ok(mac_hex(key, &material))
}

fn mac_hex(key: &[u8; 32], material: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts a 32-byte key");
    mac.update(material);
    hex::encode(mac.finalize().into_bytes())
}

fn verify_request_mac(key: &[u8; 32], envelope: &AuthenticatedRequest) -> Result<()> {
    let expected = request_mac(
        key,
        &envelope.nonce,
        &envelope.credential,
        &envelope.request,
    )?;
    verify_mac(key, expected.as_bytes(), envelope.mac.as_bytes())
}

fn verify_response_mac(key: &[u8; 32], envelope: &AuthenticatedResponse) -> Result<()> {
    let expected = response_mac(key, &envelope.nonce, &envelope.response)?;
    verify_mac(key, expected.as_bytes(), envelope.mac.as_bytes())
}

fn verify_mac(_key: &[u8; 32], expected: &[u8], actual: &[u8]) -> Result<()> {
    if expected.len() != actual.len() {
        return Err(LocusError::AuthorityAnchorUnavailable(
            "authority protocol authentication failed".into(),
        ));
    }
    let mut difference = 0_u8;
    for (left, right) in expected.iter().zip(actual) {
        difference |= left ^ right;
    }
    if difference != 0 {
        return Err(LocusError::AuthorityAnchorUnavailable(
            "authority protocol authentication failed".into(),
        ));
    }
    Ok(())
}

fn authenticate_server(
    endpoint: &AuthorityAnchorEndpoint,
    connection: &platform::Connection,
) -> Result<()> {
    let peer = connection_peer_identity(connection)?;
    let current_uid = platform::current_user_identity()?;
    let expected_executable = fs::canonicalize(&endpoint.owner_executable).map_err(|_| {
        LocusError::AuthorityAnchorUnavailable("broker owner executable is unavailable".into())
    })?;
    let peer_executable = fs::canonicalize(&peer.executable).map_err(|_| {
        LocusError::AuthorityAnchorUnavailable("broker server executable is unavailable".into())
    })?;
    if peer.uid != current_uid
        || peer.pid != endpoint.owner_pid
        || peer_executable != expected_executable
        || !executable_metadata_matches(&peer_executable, &endpoint.owner_executable_identity)?
    {
        return Err(LocusError::AuthorityAnchorUnavailable(
            "authority endpoint peer does not match its recorded broker owner".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
fn connection_peer_identity(_connection: &platform::Connection) -> Result<PeerIdentity> {
    Ok(PeerIdentity {
        pid: std::process::id(),
        uid: platform::current_user_identity()?,
        executable: std::env::current_exe()?,
    })
}

#[cfg(not(test))]
fn connection_peer_identity(connection: &platform::Connection) -> Result<PeerIdentity> {
    platform::peer_identity(connection)
}

fn open_singleton_lock(path: &Path) -> Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .mode(0o600);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000);
    }
    let file = options.open(path)?;
    let opened = file.metadata()?;
    let linked = fs::symlink_metadata(path)?;
    if !opened.is_file() || !linked.is_file() || linked.file_type().is_symlink() {
        return Err(LocusError::AuthorityAnchorUnavailable(
            "authority singleton lock is not a regular file".into(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if opened.dev() != linked.dev()
            || opened.ino() != linked.ino()
            || opened.nlink() != 1
            || linked.nlink() != 1
        {
            return Err(LocusError::AuthorityAnchorUnavailable(
                "authority singleton lock was linked or replaced".into(),
            ));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if opened.file_attributes() & 0x0000_0400 != 0
            || linked.file_attributes() & 0x0000_0400 != 0
        {
            return Err(LocusError::AuthorityAnchorUnavailable(
                "authority singleton lock is a reparse point".into(),
            ));
        }
    }
    Ok(file)
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn execution_authority_fails_closed_without_native_peer_authenticated_pipe() {
        let dir = tempdir().unwrap();
        let error = issue(
            dir.path(),
            "ses_windows",
            "active",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .unwrap_err();
        assert!(matches!(error, LocusError::AuthorityAnchorUnavailable(_)));
        assert!(!endpoint_path(dir.path()).exists());
    }
}

fn write_endpoint_atomic(home: &Path, endpoint: &AuthorityAnchorEndpoint) -> Result<()> {
    let path = endpoint_path(home);
    let temp = runtime_dir(home).join(format!(".authority.{}.tmp", random_hex(8)));
    let bytes = serde_json::to_vec(endpoint)?;
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temp, &path)?;
    Ok(())
}

fn read_endpoint(home: &Path) -> Result<AuthorityAnchorEndpoint> {
    let path = endpoint_path(home);
    let link_metadata = fs::symlink_metadata(&path)?;
    if !link_metadata.is_file() || link_metadata.file_type().is_symlink() {
        return Err(LocusError::AuthorityAnchorUnavailable(
            "authority endpoint store entry is not a regular file".into(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if link_metadata.nlink() != 1 {
            return Err(LocusError::AuthorityAnchorUnavailable(
                "authority endpoint store entry has unexpected hard links".into(),
            ));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if link_metadata.file_attributes() & 0x0000_0400 != 0 {
            return Err(LocusError::AuthorityAnchorUnavailable(
                "authority endpoint store entry is a reparse point".into(),
            ));
        }
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000);
    }
    let file = options.open(&path)?;
    let opened = file.metadata()?;
    #[cfg(not(unix))]
    let _ = &opened;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if opened.dev() != link_metadata.dev()
            || opened.ino() != link_metadata.ino()
            || opened.nlink() != 1
        {
            return Err(LocusError::AuthorityAnchorUnavailable(
                "authority endpoint store entry changed while opening".into(),
            ));
        }
    }
    let mut raw = Vec::new();
    file.take(MAX_MESSAGE_BYTES as u64).read_to_end(&mut raw)?;
    let endpoint: AuthorityAnchorEndpoint = serde_json::from_slice(&raw)?;
    validate_endpoint_shape(home, &endpoint)?;
    Ok(endpoint)
}

fn validate_endpoint_shape(home: &Path, endpoint: &AuthorityAnchorEndpoint) -> Result<()> {
    if endpoint.version != AUTHORITY_ENDPOINT_VERSION
        || endpoint.transport != platform::TRANSPORT
        || endpoint.epoch.len() != 32
        || endpoint.endpoint_generation == 0
        || endpoint.owner_pid == 0
        || endpoint.owner_executable_identity.sha256.len() != 64
        || endpoint.owner_executable_identity.length == 0
        || endpoint.owner_executable_identity.modified_token.is_empty()
        || endpoint.supervisor.pid == 0
        || endpoint.supervisor.start_token.is_empty()
        || endpoint.supervisor.executable_identity.sha256.len() != 64
    {
        return Err(LocusError::AuthorityAnchorUnavailable(
            "invalid authority endpoint metadata".into(),
        ));
    }
    let home = canonical_home(home)?;
    if endpoint.address != platform::endpoint_address(&home, &endpoint.epoch) {
        return Err(LocusError::AuthorityAnchorUnavailable(
            "authority endpoint is not bound to the canonical home and epoch".into(),
        ));
    }
    platform::validate_address(&home, &endpoint.address)?;
    if !platform::supervisor_is_current(&endpoint.supervisor)? {
        return Err(LocusError::AuthorityAnchorUnavailable(
            "authority supervisor is no longer current".into(),
        ));
    }
    Ok(())
}

fn retire_endpoint_if_current(home: &Path, endpoint: &AuthorityAnchorEndpoint) {
    if read_endpoint(home).ok().as_ref() == Some(endpoint) {
        let _ = fs::remove_file(endpoint_path(home));
    }
}

fn runtime_dir(home: &Path) -> PathBuf {
    home.join("runtime")
}

fn endpoint_path(home: &Path) -> PathBuf {
    runtime_dir(home).join("authority.json")
}

fn broker_lock_path(home: &Path) -> PathBuf {
    runtime_dir(home).join("authority.lock")
}

fn canonical_home(home: &Path) -> Result<PathBuf> {
    fs::create_dir_all(home)?;
    Ok(fs::canonicalize(home)?)
}

fn should_host_in_process() -> bool {
    is_test_harness()
}

fn broker_start_timeout() -> Duration {
    if let Ok(raw) = std::env::var(BROKER_START_TIMEOUT_ENV) {
        if let Ok(ms) = raw.trim().parse::<u64>() {
            // Floor keeps accidental "0" from busy-looping; ceiling caps runaway CI.
            return Duration::from_millis(ms.clamp(500, 120_000));
        }
    }
    if is_test_harness() {
        TEST_START_TIMEOUT
    } else {
        START_TIMEOUT
    }
}

/// Collect a short stderr snip from the broker child (if any) for timeout errors.
fn drain_broker_stderr(receiver: &mpsc::Receiver<String>) -> String {
    let raw = match receiver.recv_timeout(Duration::from_millis(200)) {
        Ok(s) => s,
        Err(_) => return String::new(),
    };
    let snip: String = raw
        .chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .take(240)
        .collect();
    let snip = snip.trim();
    if snip.is_empty() {
        String::new()
    } else {
        format!(" ({snip})")
    }
}

fn is_test_harness() -> bool {
    if cfg!(test) {
        return true;
    }
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.ends_with("deps")))
        .unwrap_or(false)
}

fn executable_identity(path: &Path) -> Result<ExecutableIdentity> {
    let linked = fs::symlink_metadata(path)?;
    if !linked.is_file() || linked.file_type().is_symlink() {
        return Err(LocusError::AuthorityAnchorUnavailable(
            "authority executable is not an unlinked regular file".into(),
        ));
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000);
    }
    let mut file = options.open(path)?;
    let before = file.metadata()?;
    if !before.is_file() {
        return Err(LocusError::AuthorityAnchorUnavailable(
            "authority executable handle is not a regular file".into(),
        ));
    }
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    let after = file.metadata()?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err(LocusError::AuthorityAnchorUnavailable(
            "authority executable changed while hashing".into(),
        ));
    }

    #[cfg(unix)]
    let (device, inode) = {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != linked.dev() || before.ino() != linked.ino() {
            return Err(LocusError::AuthorityAnchorUnavailable(
                "authority executable path was replaced while opening".into(),
            ));
        }
        (Some(before.dev()), Some(before.ino()))
    };
    #[cfg(not(unix))]
    let (device, inode) = (None, None);

    Ok(ExecutableIdentity {
        sha256: hex::encode(digest.finalize()),
        length: before.len(),
        modified_token: metadata_modified_token(&before)?,
        device,
        inode,
    })
}

fn executable_metadata_matches(path: &Path, expected: &ExecutableIdentity) -> Result<bool> {
    let linked = fs::symlink_metadata(path)?;
    if !linked.is_file() || linked.file_type().is_symlink() {
        return Ok(false);
    }
    if linked.len() != expected.length
        || metadata_modified_token(&linked)? != expected.modified_token
    {
        return Ok(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(expected.device == Some(linked.dev()) && expected.inode == Some(linked.ino()))
    }
    #[cfg(not(unix))]
    {
        Ok(true)
    }
}

fn metadata_modified_token(metadata: &fs::Metadata) -> Result<String> {
    let modified = metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| {
            LocusError::AuthorityAnchorUnavailable(
                "authority executable modification time is invalid".into(),
            )
        })?;
    Ok(format!(
        "{}:{}",
        modified.as_secs(),
        modified.subsec_nanos()
    ))
}

fn record_broker_error(home: &Path, error: &LocusError) {
    #[cfg(test)]
    eprintln!("authority broker connection error: {error}");
    let path = runtime_dir(home).join("authority-errors.log");
    if fs::symlink_metadata(&path)
        .ok()
        .is_some_and(|metadata| metadata.file_type().is_symlink() || !metadata.is_file())
    {
        return;
    }
    if fs::metadata(&path)
        .ok()
        .is_some_and(|metadata| metadata.len() >= 64 * 1024)
    {
        return;
    }
    let mut options = fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .mode(0o600);
    }
    let Ok(mut file) = options.open(path) else {
        return;
    };
    let message = error.to_string().replace(['\r', '\n'], " ");
    let message = message.chars().take(512).collect::<String>();
    let _ = writeln!(file, "{message}");
}

fn hash_capability(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn random_hex(bytes: usize) -> String {
    let mut value = vec![0_u8; bytes];
    rand::thread_rng().fill_bytes(&mut value);
    hex::encode(value)
}

fn random_u64() -> u64 {
    let mut bytes = [0_u8; 8];
    rand::thread_rng().fill_bytes(&mut bytes);
    u64::from_le_bytes(bytes).max(1)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;
    use std::process::Command;
    use tempfile::tempdir;

    const TEST_SUBJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn auth() -> RequestAuth {
        control_auth().unwrap()
    }

    fn broker(home: &Path) -> AuthorityAnchorEndpoint {
        let endpoint = ensure_broker(home, &auth()).unwrap();
        ping(home, &endpoint, &auth()).unwrap();
        endpoint
    }

    fn call(endpoint: &AuthorityAnchorEndpoint, request_value: &AnchorRequest) -> AnchorResponse {
        request(endpoint, request_value, &auth()).unwrap()
    }

    fn executor_auth(capability: &str) -> RequestAuth {
        RequestAuth {
            credential: RequestCredential::Executor {
                capability_id: hash_capability(capability),
            },
            key: decode_capability(capability, "executor").unwrap(),
        }
    }

    fn retire(home: &Path) {
        let _ = retire_for_test(home);
        thread::sleep(Duration::from_millis(40));
    }

    #[test]
    fn mint_persists_0600_and_never_overwrites() {
        let dir = tempdir().unwrap();
        let value = mint_persisted_control_capability(dir.path()).unwrap();
        decode_capability(&value, "control").expect("minted value is a valid capability");

        let path = control_capability_file(dir.path());
        use std::os::unix::fs::MetadataExt;
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);

        // Round-trips through the persisted reader.
        assert_eq!(
            read_persisted_control_capability(dir.path()).unwrap(),
            Some(value.clone())
        );

        // Fail closed: a second mint must refuse rather than replace.
        let err = mint_persisted_control_capability(dir.path()).unwrap_err();
        assert!(err.to_string().contains("refusing to overwrite"));
        assert_eq!(
            read_persisted_control_capability(dir.path()).unwrap(),
            Some(value)
        );
    }

    #[test]
    fn persist_unpersist_round_trip_is_fail_closed() {
        let dir = tempdir().unwrap();

        // Ephemeral mint never touches the filesystem.
        let value = mint_ephemeral_control_capability();
        decode_capability(&value, "control").expect("ephemeral value is a valid capability");
        assert!(!control_capability_file(dir.path()).exists());

        // Persist writes 0600 and round-trips.
        persist_control_capability(dir.path(), &value).unwrap();
        let path = control_capability_file(dir.path());
        use std::os::unix::fs::MetadataExt;
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        assert_eq!(
            read_persisted_control_capability(dir.path()).unwrap(),
            Some(value.clone())
        );

        // Idempotent for the same value; fail closed for a different one.
        persist_control_capability(dir.path(), &value).unwrap();
        let err = persist_control_capability(dir.path(), &random_hex(32)).unwrap_err();
        assert!(err.to_string().contains("refusing to overwrite"));

        // Invalid values are rejected before any write.
        assert!(persist_control_capability(dir.path(), "beef").is_err());

        // Unpersist removes the file exactly once.
        assert!(unpersist_control_capability(dir.path()).unwrap());
        assert!(!path.exists());
        assert!(!unpersist_control_capability(dir.path()).unwrap());
    }

    #[test]
    fn teardown_live_broker_rejection_fails_closed() {
        let dir = tempdir().unwrap();
        broker(dir.path());
        // The correct capability authorizes teardown against the live broker.
        authorize_control_teardown(dir.path(), false).unwrap();
        // A wrong-but-well-formed capability is refused by the live broker —
        // even with the unverified-teardown acknowledgement.
        let wrong = RequestAuth {
            credential: RequestCredential::Control,
            key: decode_capability(&random_hex(32), "control").unwrap(),
        };
        for allow_unverified in [false, true] {
            let err =
                authorize_control_teardown_with(dir.path(), allow_unverified, &wrong).unwrap_err();
            assert!(
                err.to_string().contains("refused"),
                "expected live-broker refusal, got: {err}"
            );
        }
        retire(dir.path());
    }

    #[test]
    fn teardown_without_any_verifier_requires_explicit_acknowledgement() {
        let dir = tempdir().unwrap();
        // No broker, no endpoint file, no persisted capability: fail closed…
        let err = authorize_control_teardown(dir.path(), false).unwrap_err();
        assert!(err.to_string().contains("no verifier"), "err={err}");
        // …unless the operator explicitly acknowledges the degraded gate.
        authorize_control_teardown(dir.path(), true).unwrap();
    }

    #[test]
    fn teardown_persisted_capability_must_match_byte_for_byte() {
        let dir = tempdir().unwrap();
        let value = mint_persisted_control_capability(dir.path()).unwrap();
        let good = RequestAuth {
            credential: RequestCredential::Control,
            key: decode_capability(&value, "control").unwrap(),
        };
        authorize_control_teardown_with(dir.path(), false, &good).unwrap();
        let wrong = RequestAuth {
            credential: RequestCredential::Control,
            key: decode_capability(&random_hex(32), "control").unwrap(),
        };
        // A present-but-mismatching verifier is never overridden — not even
        // with the no-verifier acknowledgement.
        for allow_unverified in [false, true] {
            let err =
                authorize_control_teardown_with(dir.path(), allow_unverified, &wrong).unwrap_err();
            assert!(err.to_string().contains("does not match"), "err={err}");
        }
    }

    #[test]
    fn read_persisted_rejects_invalid_content() {
        let dir = tempdir().unwrap();
        fs::write(control_capability_file(dir.path()), "not-hex\n").unwrap();
        let err = read_persisted_control_capability(dir.path()).unwrap_err();
        assert!(err.to_string().contains("invalid"));
    }

    #[test]
    fn capability_status_reflects_env_and_persisted_state() {
        let dir = tempdir().unwrap();

        // Nothing anywhere (env injected as None; test fallback mirrors the gate).
        let s = control_capability_status_with_env(dir.path(), None);
        assert!(!s.env_present && !s.env_valid && !s.persisted);
        assert!(s.test_fallback, "cfg(test) mirrors control_auth fallback");
        assert!(s.satisfied());
        assert_eq!(s.matches_persisted, None);

        // Invalid env value.
        let s = control_capability_status_with_env(dir.path(), Some("beef"));
        assert!(s.env_present && !s.env_valid && !s.test_fallback);
        assert!(!s.satisfied());

        // Valid env + matching persisted file.
        let minted = mint_persisted_control_capability(dir.path()).unwrap();
        let s = control_capability_status_with_env(dir.path(), Some(minted.as_str()));
        assert!(s.env_valid && s.persisted && s.persisted_valid);
        assert!(s.persisted_permissions_ok);
        assert_eq!(s.matches_persisted, Some(true));

        // Valid env that mismatches the persisted file.
        let other = random_hex(32);
        let s = control_capability_status_with_env(dir.path(), Some(other.as_str()));
        assert_eq!(s.matches_persisted, Some(false));
    }

    /// Wait until two consecutive process-identity snapshots agree.
    ///
    /// Linux `Command::spawn` is fork+exec: immediately after `spawn` returns,
    /// `/proc/<pid>/exe` can still point at the pre-exec image under scheduler
    /// load. Production supervisors are long-lived parents (no race); this only
    /// stabilizes short-lived children used by unit tests.
    fn wait_stable_process_identity(pid: u32) -> SupervisorIdentity {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut previous: Option<SupervisorIdentity> = None;
        while Instant::now() < deadline {
            match platform::process_identity_for_test(pid) {
                Ok(current) => {
                    if previous.as_ref() == Some(&current)
                        && platform::supervisor_matches_identity(&current).unwrap_or(false)
                        && platform::supervisor_is_current(&current).unwrap_or(false)
                    {
                        return current;
                    }
                    previous = Some(current);
                }
                Err(_) => {
                    previous = None;
                }
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("process identity for pid {pid} did not stabilize before deadline");
    }

    #[test]
    fn endpoint_contains_no_bearer_or_control_capability() {
        let dir = tempdir().unwrap();
        let endpoint = broker(dir.path());
        let encoded = serde_json::to_string(&endpoint).unwrap();
        assert!(!encoded.contains("control_capability"));
        assert!(!encoded.contains("executor_capability"));
        assert_eq!(endpoint.transport, "unix_socket");
        retire(dir.path());
    }

    #[test]
    fn split_brain_startup_converges_on_one_owned_generation() {
        let dir = tempdir().unwrap();
        let home = Arc::new(dir.path().to_path_buf());
        let mut starts = Vec::new();
        for _ in 0..12 {
            let home = Arc::clone(&home);
            starts.push(thread::spawn(move || broker(&home)));
        }
        let endpoints: Vec<_> = starts
            .into_iter()
            .map(|start| start.join().unwrap())
            .collect();
        let first = &endpoints[0];
        assert!(endpoints.iter().all(|endpoint| {
            endpoint.epoch == first.epoch
                && endpoint.endpoint_generation == first.endpoint_generation
                && endpoint.owner_pid == first.owner_pid
                && endpoint.address == first.address
        }));
        retire(&home);
    }

    #[test]
    fn slowloris_client_does_not_block_concurrent_validation() {
        let dir = tempdir().unwrap();
        let endpoint = broker(dir.path());
        let lease = issue(dir.path(), "ses_slowloris", "active", TEST_SUBJECT).unwrap();
        let mut slow = UnixStream::connect(&endpoint.address).unwrap();
        slow.write_all(b"{\"action\":\"ping\"").unwrap();

        let started = Instant::now();
        let mode = validate(dir.path(), &lease, "ses_slowloris", "active", TEST_SUBJECT).unwrap();
        assert_eq!(mode, ValidationMode::Manual);
        assert!(started.elapsed() < IO_TIMEOUT);

        thread::sleep(IO_TIMEOUT + Duration::from_millis(100));
        let _ = slow.write_all(b"}\n");
        ping(dir.path(), &endpoint, &auth()).unwrap();
        retire(dir.path());
    }

    #[test]
    fn raw_issue_challenge_is_operation_scoped_and_single_use() {
        let dir = tempdir().unwrap();
        let endpoint = broker(dir.path());
        let operation = ControlOperation::Issue {
            session_id: "ses_one".into(),
            backing_type: "active".into(),
            subject_digest: TEST_SUBJECT.into(),
        };
        let challenge = begin_control(&endpoint, operation, &auth()).unwrap();

        let wrong = call(
            &endpoint,
            &AnchorRequest::Issue {
                challenge: challenge.clone(),
                session_id: "ses_other".into(),
                backing_type: "active".into(),
                subject_digest: TEST_SUBJECT.into(),
            },
        );
        assert!(!wrong.ok);

        let replay = call(
            &endpoint,
            &AnchorRequest::Issue {
                challenge,
                session_id: "ses_one".into(),
                backing_type: "active".into(),
                subject_digest: TEST_SUBJECT.into(),
            },
        );
        assert!(!replay.ok);

        let no_challenge = call(
            &endpoint,
            &AnchorRequest::Issue {
                challenge: "00".repeat(32),
                session_id: "ses_one".into(),
                backing_type: "active".into(),
                subject_digest: TEST_SUBJECT.into(),
            },
        );
        assert!(!no_challenge.ok);
        retire(dir.path());
    }

    #[test]
    fn forged_control_capability_cannot_begin_control_operation() {
        let dir = tempdir().unwrap();
        let endpoint = broker(dir.path());
        let forged = RequestAuth {
            credential: RequestCredential::Control,
            key: decode_capability(&random_hex(32), "control").unwrap(),
        };
        assert!(request(
            &endpoint,
            &AnchorRequest::BeginControl {
                operation: ControlOperation::Issue {
                    session_id: "ses_agent_issue".into(),
                    backing_type: "active".into(),
                    subject_digest: TEST_SUBJECT.into(),
                },
            },
            &forged,
        )
        .is_err());
        retire(dir.path());
    }

    #[test]
    fn unknown_executor_capability_cannot_validate() {
        let dir = tempdir().unwrap();
        let endpoint = broker(dir.path());
        let lease = issue(dir.path(), "ses_agent", "active", TEST_SUBJECT).unwrap();
        let unknown = random_hex(32);
        assert!(request(
            &endpoint,
            &AnchorRequest::Validate {
                session_id: "ses_agent".into(),
                backing_type: "active".into(),
                epoch: lease.epoch,
                generation: lease.generation,
                subject_digest: TEST_SUBJECT.into(),
            },
            &executor_auth(&unknown),
        )
        .is_err());
        retire(dir.path());
    }

    #[test]
    fn executor_grant_is_bound_to_one_session_and_generation() {
        let dir = tempdir().unwrap();
        let endpoint = broker(dir.path());
        let lease = issue(dir.path(), "ses_scoped", "active", TEST_SUBJECT).unwrap();
        let capability =
            grant_executor(dir.path(), &lease, "ses_scoped", "active", TEST_SUBJECT).unwrap();

        let executor = executor_auth(&capability);
        let accepted = request(
            &endpoint,
            &AnchorRequest::Validate {
                session_id: "ses_scoped".into(),
                backing_type: "active".into(),
                epoch: lease.epoch.clone(),
                generation: lease.generation,
                subject_digest: TEST_SUBJECT.into(),
            },
            &executor,
        )
        .unwrap();
        assert!(accepted.ok);

        let wrong_session = request(
            &endpoint,
            &AnchorRequest::Validate {
                session_id: "ses_other".into(),
                backing_type: "active".into(),
                epoch: lease.epoch,
                generation: lease.generation,
                subject_digest: TEST_SUBJECT.into(),
            },
            &executor,
        )
        .unwrap();
        assert!(!wrong_session.ok);
        assert_eq!(
            wrong_session.error_code,
            Some(AnchorErrorCode::StaleGeneration)
        );
        retire(dir.path());
    }

    #[test]
    fn broker_rejects_same_generation_with_different_session_subject() {
        let dir = tempdir().unwrap();
        let endpoint = broker(dir.path());
        let lease = issue(dir.path(), "ses_subject", "ci", TEST_SUBJECT).unwrap();
        let capability =
            grant_executor(dir.path(), &lease, "ses_subject", "ci", TEST_SUBJECT).unwrap();
        let forged = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let response = request(
            &endpoint,
            &AnchorRequest::Validate {
                session_id: "ses_subject".into(),
                backing_type: "ci".into(),
                epoch: lease.epoch,
                generation: lease.generation,
                subject_digest: forged.into(),
            },
            &executor_auth(&capability),
        )
        .unwrap();
        assert!(!response.ok);
        assert_eq!(response.error_code, Some(AnchorErrorCode::StaleGeneration));
        retire(dir.path());
    }

    #[test]
    fn endpoint_owner_substitution_is_rejected_by_server_peer_authentication() {
        let dir = tempdir().unwrap();
        let endpoint = broker(dir.path());
        let mut tampered = endpoint.clone();
        tampered.owner_pid = endpoint.owner_pid.wrapping_add(1).max(1);
        write_endpoint_atomic(dir.path(), &tampered).unwrap();
        assert!(ping(dir.path(), &tampered, &auth())
            .unwrap_err()
            .to_string()
            .contains("recorded broker owner"));
        write_endpoint_atomic(dir.path(), &endpoint).unwrap();
        retire(dir.path());
    }

    #[test]
    fn linked_singleton_lock_is_rejected_before_broker_start() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(runtime_dir(dir.path())).unwrap();
        let lock = broker_lock_path(dir.path());
        fs::write(&lock, b"").unwrap();
        let linked = runtime_dir(dir.path()).join("linked-authority.lock");
        fs::hard_link(&lock, &linked).unwrap();
        assert!(ensure_broker(dir.path(), &auth()).is_err());
    }

    #[test]
    fn retired_broker_and_replayed_endpoint_cannot_restore_authority() {
        let dir = tempdir().unwrap();
        let old_endpoint = broker(dir.path());
        let old_lease = issue(dir.path(), "ses_old", "active", TEST_SUBJECT).unwrap();
        let old_record = fs::read(endpoint_path(dir.path())).unwrap();
        retire(dir.path());

        let new_endpoint = broker(dir.path());
        assert_ne!(new_endpoint.epoch, old_endpoint.epoch);
        fs::write(endpoint_path(dir.path()), old_record).unwrap();
        assert!(validate(dir.path(), &old_lease, "ses_old", "active", TEST_SUBJECT).is_err());
        write_endpoint_atomic(dir.path(), &new_endpoint).unwrap();
        retire(dir.path());
    }

    #[test]
    fn concurrent_retirement_and_restart_yields_one_new_current_epoch() {
        let dir = tempdir().unwrap();
        let old_endpoint = broker(dir.path());
        let old_lease = issue(dir.path(), "ses_before_restart", "active", TEST_SUBJECT).unwrap();
        let home = Arc::new(dir.path().to_path_buf());
        let barrier = Arc::new(std::sync::Barrier::new(3));

        let retiring_home = Arc::clone(&home);
        let retiring_barrier = Arc::clone(&barrier);
        let retiring = thread::spawn(move || {
            retiring_barrier.wait();
            retire_for_test(&retiring_home).unwrap();
        });
        let racing_home = Arc::clone(&home);
        let racing_barrier = Arc::clone(&barrier);
        let racing_start = thread::spawn(move || {
            racing_barrier.wait();
            let _ = ensure_broker(&racing_home, &auth());
        });
        barrier.wait();
        retiring.join().unwrap();
        racing_start.join().unwrap();
        thread::sleep(Duration::from_millis(80));

        let current = broker(&home);
        assert_ne!(current.epoch, old_endpoint.epoch);
        assert!(validate(
            &home,
            &old_lease,
            "ses_before_restart",
            "active",
            TEST_SUBJECT
        )
        .is_err());
        let new_lease = issue(&home, "ses_after_restart", "active", TEST_SUBJECT).unwrap();
        assert_eq!(new_lease.epoch, current.epoch);
        retire(&home);
    }

    #[test]
    fn linked_endpoint_store_entry_is_rejected() {
        let dir = tempdir().unwrap();
        let endpoint = broker(dir.path());
        let path = endpoint_path(dir.path());
        let linked = runtime_dir(dir.path()).join("linked-authority.json");
        fs::hard_link(&path, &linked).unwrap();
        assert!(read_endpoint(dir.path())
            .unwrap_err()
            .to_string()
            .contains("hard links"));
        fs::remove_file(linked).unwrap();
        write_endpoint_atomic(dir.path(), &endpoint).unwrap();
        retire(dir.path());
    }

    #[test]
    fn supervisor_start_identity_rejects_pid_reuse_shape() {
        // Long enough that CI scheduling delay cannot exit before we sample.
        let mut child = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let identity = wait_stable_process_identity(child.id());
        assert!(platform::supervisor_matches_identity(&identity).unwrap());
        assert!(platform::supervisor_is_current(&identity).unwrap());
        child.kill().unwrap();
        child.wait().unwrap();
        // Reaped PIDs (and later PID-reuse of the same number) must not match
        // the captured start-token binding. Poll briefly: procfs teardown is
        // not always visible on the very next read under load.
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut stale = false;
        while Instant::now() < deadline {
            if !platform::supervisor_is_current(&identity).unwrap_or(false) {
                stale = true;
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert!(
            stale,
            "supervisor identity remained current after process reaped"
        );
    }

    #[test]
    fn executable_identity_detects_same_path_replacement() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("trusted-locus");
        fs::copy(std::env::current_exe().unwrap(), &path).unwrap();
        let before = executable_identity(&path).unwrap();
        let replacement = dir.path().join("replacement");
        fs::write(&replacement, b"not the trusted executable").unwrap();
        fs::rename(&replacement, &path).unwrap();
        let after = executable_identity(&path).unwrap();
        assert_ne!(before, after);
    }

    #[test]
    fn private_socket_parent_rejects_symlink() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("target");
        let linked = dir.path().join("linked");
        fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, &linked).unwrap();
        assert!(platform::secure_socket_parent_for_test(&linked).is_err());
    }

    #[test]
    fn parallel_authenticated_requests_do_not_break_pipes() {
        let dir = tempdir().unwrap();
        let home = Arc::new(dir.path().to_path_buf());
        broker(&home);
        let mut workers = Vec::new();
        for index in 0..48 {
            let home = Arc::clone(&home);
            workers.push(thread::spawn(move || {
                let session_id = format!("ses_parallel_{index}");
                let lease = issue(&home, &session_id, "ci", TEST_SUBJECT).unwrap();
                let capability =
                    grant_executor(&home, &lease, &session_id, "ci", TEST_SUBJECT).unwrap();
                let executor = executor_auth(&capability);
                let endpoint = read_endpoint(&home).unwrap();
                let response = request(
                    &endpoint,
                    &AnchorRequest::Validate {
                        session_id,
                        backing_type: "ci".into(),
                        epoch: lease.epoch,
                        generation: lease.generation,
                        subject_digest: TEST_SUBJECT.into(),
                    },
                    &executor,
                )
                .unwrap();
                assert!(response.ok);
            }));
        }
        for worker in workers {
            worker.join().unwrap();
        }
        retire(&home);
    }
}

#[cfg(unix)]
mod platform {
    use super::*;
    #[cfg(not(test))]
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{
        DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt,
    };
    use std::os::unix::net::{UnixListener, UnixStream};

    pub const TRANSPORT: &str = "unix_socket";
    pub type Listener = UnixListener;
    pub type Connection = UnixStream;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct DirectoryIdentity {
        device: u64,
        inode: u64,
    }

    fn socket_parent() -> PathBuf {
        let base = fs::canonicalize("/tmp").unwrap_or_else(|_| PathBuf::from("/tmp"));
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        base.join(format!("locus-authority-{uid}"))
    }

    pub fn endpoint_address(home: &Path, epoch: &str) -> String {
        let digest = Sha256::digest(format!("{}:{epoch}", home.display()).as_bytes());
        socket_parent()
            .join(format!("{}.sock", &hex::encode(digest)[..32]))
            .display()
            .to_string()
    }

    pub fn validate_address(home: &Path, address: &str) -> Result<()> {
        let path = PathBuf::from(address);
        let parent = socket_parent();
        if path.parent() != Some(parent.as_path())
            || path.extension().and_then(|ext| ext.to_str()) != Some("sock")
        {
            return Err(LocusError::AuthorityAnchorUnavailable(
                "authority socket is outside the canonical local socket directory".into(),
            ));
        }
        secure_socket_parent(&parent)?;
        let _ = home;
        Ok(())
    }

    pub fn bind(endpoint: &AuthorityAnchorEndpoint) -> Result<Listener> {
        let path = PathBuf::from(&endpoint.address);
        let parent = path.parent().ok_or_else(|| {
            LocusError::AuthorityAnchorUnavailable("authority socket has no parent".into())
        })?;
        let before = secure_socket_parent(parent)?;
        if fs::symlink_metadata(&path).is_ok() {
            return Err(LocusError::AuthorityAnchorUnavailable(
                "authority socket path already exists".into(),
            ));
        }
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        let after = secure_socket_parent(parent)?;
        let socket = fs::symlink_metadata(&path)?;
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        if before != after
            || !socket.file_type().is_socket()
            || socket.uid() != uid
            || socket.mode() & 0o777 != 0o600
        {
            return Err(LocusError::AuthorityAnchorUnavailable(
                "authority socket or private parent changed during bind".into(),
            ));
        }
        listener.set_nonblocking(true)?;
        Ok(listener)
    }

    fn secure_socket_parent(parent: &Path) -> Result<DirectoryIdentity> {
        match fs::symlink_metadata(parent) {
            Ok(metadata) => {
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err(LocusError::AuthorityAnchorUnavailable(
                        "authority socket parent is linked or not a directory".into(),
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut builder = fs::DirBuilder::new();
                builder.mode(0o700);
                if let Err(create_error) = builder.create(parent) {
                    if create_error.kind() != std::io::ErrorKind::AlreadyExists {
                        return Err(create_error.into());
                    }
                }
            }
            Err(error) => return Err(error.into()),
        }

        let linked = fs::symlink_metadata(parent)?;
        if !linked.is_dir() || linked.file_type().is_symlink() {
            return Err(LocusError::AuthorityAnchorUnavailable(
                "authority socket parent is linked or not a directory".into(),
            ));
        }
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        if linked.uid() != uid {
            return Err(LocusError::AuthorityAnchorUnavailable(
                "authority socket parent is not owned by the current user".into(),
            ));
        }
        if linked.mode() & 0o777 != 0o700 {
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }

        let mut options = fs::OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let directory = options.open(parent)?;
        let opened = directory.metadata()?;
        let current = fs::symlink_metadata(parent)?;
        if !opened.is_dir()
            || !current.is_dir()
            || current.file_type().is_symlink()
            || opened.uid() != uid
            || current.uid() != uid
            || opened.mode() & 0o777 != 0o700
            || current.mode() & 0o777 != 0o700
            || opened.dev() != current.dev()
            || opened.ino() != current.ino()
        {
            return Err(LocusError::AuthorityAnchorUnavailable(
                "authority socket parent was replaced while opening".into(),
            ));
        }
        Ok(DirectoryIdentity {
            device: opened.dev(),
            inode: opened.ino(),
        })
    }

    #[cfg(test)]
    pub(super) fn secure_socket_parent_for_test(parent: &Path) -> Result<()> {
        secure_socket_parent(parent).map(|_| ())
    }

    pub fn accept(listener: &Listener) -> Result<Option<Connection>> {
        match listener.accept() {
            Ok((stream, _)) => {
                // Darwin may propagate O_NONBLOCK from the listener. Every
                // accepted connection uses bounded blocking I/O with explicit
                // read/write deadlines, so clear it before handing off.
                stream.set_nonblocking(false)?;
                Ok(Some(stream))
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn connect(endpoint: &AuthorityAnchorEndpoint, _timeout: Duration) -> Result<Connection> {
        validate_address(Path::new("/"), &endpoint.address)?;
        UnixStream::connect(&endpoint.address).map_err(|error| {
            LocusError::AuthorityAnchorUnavailable(format!(
                "local authority socket unavailable: {error}"
            ))
        })
    }

    pub fn set_deadlines(connection: &Connection, timeout: Duration) -> Result<()> {
        connection.set_read_timeout(Some(timeout))?;
        connection.set_write_timeout(Some(timeout))?;
        Ok(())
    }

    pub fn read_message(
        connection: &mut Connection,
        limit: usize,
        _timeout: Duration,
    ) -> Result<Vec<u8>> {
        let mut raw = Vec::new();
        let count =
            BufReader::new(connection.take((limit + 1) as u64)).read_until(b'\n', &mut raw)?;
        if count == 0 || raw.len() > limit || !raw.ends_with(b"\n") {
            return Err(LocusError::AuthorityAnchorUnavailable(
                "authority message was empty, incomplete, or oversized".into(),
            ));
        }
        raw.pop();
        Ok(raw)
    }

    pub fn write_message(
        connection: &mut Connection,
        value: &[u8],
        _timeout: Duration,
    ) -> Result<()> {
        connection.write_all(value)?;
        connection.write_all(b"\n")?;
        connection.flush()?;
        Ok(())
    }

    #[cfg(not(test))]
    pub fn peer_identity(connection: &Connection) -> Result<PeerIdentity> {
        let (pid, uid) = peer_process(connection)?;
        let executable = peer_executable(pid)?;
        Ok(PeerIdentity {
            pid,
            uid,
            executable,
        })
    }

    #[cfg(all(not(test), target_os = "linux"))]
    fn peer_process(connection: &Connection) -> Result<(u32, String)> {
        let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
        let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: credentials and length are valid writable buffers for SO_PEERCRED.
        let rc = unsafe {
            libc::getsockopt(
                connection.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut credentials as *mut libc::ucred).cast(),
                &mut length,
            )
        };
        if rc != 0 || credentials.pid <= 0 {
            return Err(LocusError::AuthorityAnchorUnavailable(
                "Unix peer credentials are unavailable".into(),
            ));
        }
        Ok((credentials.pid as u32, credentials.uid.to_string()))
    }

    #[cfg(all(not(test), any(target_os = "macos", target_os = "ios")))]
    fn peer_process(connection: &Connection) -> Result<(u32, String)> {
        let mut uid: libc::uid_t = 0;
        let mut gid: libc::gid_t = 0;
        // SAFETY: uid and gid are valid writable values for getpeereid.
        if unsafe { libc::getpeereid(connection.as_raw_fd(), &mut uid, &mut gid) } != 0 {
            return Err(LocusError::AuthorityAnchorUnavailable(
                "Unix peer identity is unavailable".into(),
            ));
        }
        let mut pid: libc::pid_t = 0;
        let mut length = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
        const LOCAL_PEERPID: libc::c_int = 0x002;
        // SAFETY: pid and length are valid writable buffers for LOCAL_PEERPID.
        let rc = unsafe {
            libc::getsockopt(
                connection.as_raw_fd(),
                0,
                LOCAL_PEERPID,
                (&mut pid as *mut libc::pid_t).cast(),
                &mut length,
            )
        };
        if rc != 0 || pid <= 0 {
            return Err(LocusError::AuthorityAnchorUnavailable(
                "Unix peer PID is unavailable".into(),
            ));
        }
        Ok((pid as u32, uid.to_string()))
    }

    #[cfg(all(
        not(test),
        not(any(target_os = "linux", target_os = "macos", target_os = "ios"))
    ))]
    fn peer_process(_connection: &Connection) -> Result<(u32, String)> {
        Err(LocusError::AuthorityAnchorUnavailable(
            "peer PID authentication is unsupported on this Unix platform".into(),
        ))
    }

    #[cfg(target_os = "linux")]
    fn peer_executable(pid: u32) -> Result<PathBuf> {
        fs::read_link(format!("/proc/{pid}/exe")).map_err(|error| {
            LocusError::AuthorityAnchorUnavailable(format!("peer executable unavailable: {error}"))
        })
    }

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    fn peer_executable(pid: u32) -> Result<PathBuf> {
        const PROC_PIDPATHINFO_MAXSIZE: usize = 4096;
        unsafe extern "C" {
            fn proc_pidpath(
                pid: libc::c_int,
                buffer: *mut libc::c_void,
                buffersize: u32,
            ) -> libc::c_int;
        }
        let mut buffer = vec![0_u8; PROC_PIDPATHINFO_MAXSIZE];
        // SAFETY: buffer is writable for its full declared size.
        let count = unsafe {
            proc_pidpath(
                pid as libc::c_int,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
            )
        };
        if count <= 0 {
            return Err(LocusError::AuthorityAnchorUnavailable(
                "peer executable lineage is unavailable".into(),
            ));
        }
        buffer.truncate(count as usize);
        Ok(PathBuf::from(String::from_utf8_lossy(&buffer).into_owned()))
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios")))]
    fn peer_executable(_pid: u32) -> Result<PathBuf> {
        Err(LocusError::AuthorityAnchorUnavailable(
            "peer executable lineage is unsupported on this Unix platform".into(),
        ))
    }

    pub fn current_user_identity() -> Result<String> {
        // SAFETY: geteuid has no preconditions.
        Ok(unsafe { libc::geteuid() }.to_string())
    }

    pub fn capture_supervisor_identity() -> Result<SupervisorIdentity> {
        // SAFETY: getppid has no preconditions.
        let pid = unsafe { libc::getppid() };
        if pid <= 0 {
            return Err(LocusError::AuthorityAnchorUnavailable(
                "authority supervisor PID is unavailable".into(),
            ));
        }
        process_identity(pid as u32)
    }

    pub fn supervisor_matches_identity(expected: &SupervisorIdentity) -> Result<bool> {
        Ok(process_identity(expected.pid)? == *expected)
    }

    pub fn supervisor_is_current(expected: &SupervisorIdentity) -> Result<bool> {
        if process_start_token(expected.pid)? != expected.start_token {
            return Ok(false);
        }
        let executable = fs::canonicalize(peer_executable(expected.pid)?)?;
        if executable.display().to_string() != expected.executable {
            return Ok(false);
        }
        let metadata = fs::metadata(&executable)?;
        Ok(expected.uid == current_user_identity()?
            && metadata.len() == expected.executable_identity.length
            && metadata_modified_token(&metadata)? == expected.executable_identity.modified_token
            && expected.executable_identity.device == Some(metadata.dev())
            && expected.executable_identity.inode == Some(metadata.ino()))
    }

    fn process_identity(pid: u32) -> Result<SupervisorIdentity> {
        let executable = fs::canonicalize(peer_executable(pid)?)?;
        Ok(SupervisorIdentity {
            pid,
            uid: current_user_identity()?,
            start_token: process_start_token(pid)?,
            executable: executable.display().to_string(),
            executable_identity: executable_identity(&executable)?,
        })
    }

    #[cfg(test)]
    pub(super) fn process_identity_for_test(pid: u32) -> Result<SupervisorIdentity> {
        process_identity(pid)
    }

    #[cfg(target_os = "linux")]
    fn process_start_token(pid: u32) -> Result<String> {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
        let end = stat.rfind(')').ok_or_else(|| {
            LocusError::AuthorityAnchorUnavailable("invalid supervisor process stat".into())
        })?;
        let fields: Vec<&str> = stat[end + 1..].split_whitespace().collect();
        fields
            .get(19)
            .map(|value| (*value).to_string())
            .ok_or_else(|| {
                LocusError::AuthorityAnchorUnavailable(
                    "supervisor start time is unavailable".into(),
                )
            })
    }

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    fn process_start_token(pid: u32) -> Result<String> {
        #[repr(C)]
        struct ProcBsdInfo {
            pbi_flags: u32,
            pbi_status: u32,
            pbi_xstatus: u32,
            pbi_pid: u32,
            pbi_ppid: u32,
            pbi_uid: u32,
            pbi_gid: u32,
            pbi_ruid: u32,
            pbi_rgid: u32,
            pbi_svuid: u32,
            pbi_svgid: u32,
            rfu_1: u32,
            pbi_comm: [u8; 16],
            pbi_name: [u8; 32],
            pbi_nfiles: u32,
            pbi_pgid: u32,
            pbi_pjobc: u32,
            e_tdev: u32,
            e_tpgid: u32,
            pbi_nice: i32,
            pbi_start_tvsec: u64,
            pbi_start_tvusec: u64,
        }
        unsafe extern "C" {
            fn proc_pidinfo(
                pid: libc::c_int,
                flavor: libc::c_int,
                arg: u64,
                buffer: *mut libc::c_void,
                buffersize: libc::c_int,
            ) -> libc::c_int;
        }
        const PROC_PIDTBSDINFO: libc::c_int = 3;
        let mut info: ProcBsdInfo = unsafe { std::mem::zeroed() };
        // SAFETY: info is a writable buffer of the exact declared structure size.
        let count = unsafe {
            proc_pidinfo(
                pid as libc::c_int,
                PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut ProcBsdInfo).cast(),
                std::mem::size_of::<ProcBsdInfo>() as libc::c_int,
            )
        };
        if count != std::mem::size_of::<ProcBsdInfo>() as libc::c_int {
            return Err(LocusError::AuthorityAnchorUnavailable(
                "supervisor start time is unavailable".into(),
            ));
        }
        Ok(format!(
            "{}:{}",
            info.pbi_start_tvsec, info.pbi_start_tvusec
        ))
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios")))]
    fn process_start_token(_pid: u32) -> Result<String> {
        Err(LocusError::AuthorityAnchorUnavailable(
            "supervisor start identity is unsupported on this Unix platform".into(),
        ))
    }

    pub fn retire(endpoint: &AuthorityAnchorEndpoint) {
        let path = PathBuf::from(&endpoint.address);
        let Some(parent) = path.parent() else {
            return;
        };
        if secure_socket_parent(parent).is_err() {
            return;
        }
        if fs::symlink_metadata(&path).ok().is_some_and(|metadata| {
            // SAFETY: geteuid has no preconditions.
            metadata.file_type().is_socket() && metadata.uid() == unsafe { libc::geteuid() }
        }) {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::*;

    pub const TRANSPORT: &str = "windows_named_pipe";
    pub struct Listener;
    pub struct Connection;

    pub fn endpoint_address(_home: &Path, epoch: &str) -> String {
        format!(r"\\.\pipe\locus-authority-{epoch}")
    }

    pub fn validate_address(_home: &Path, address: &str) -> Result<()> {
        if address.starts_with(r"\\.\pipe\locus-authority-") {
            Ok(())
        } else {
            Err(LocusError::AuthorityAnchorUnavailable(
                "invalid Windows authority pipe name".into(),
            ))
        }
    }

    pub fn bind(_endpoint: &AuthorityAnchorEndpoint) -> Result<Listener> {
        Err(LocusError::AuthorityAnchorUnavailable(
            "native Windows peer-authenticated named-pipe authority is required but unavailable"
                .into(),
        ))
    }
    pub fn accept(_listener: &Listener) -> Result<Option<Connection>> {
        Ok(None)
    }
    pub fn connect(_endpoint: &AuthorityAnchorEndpoint, _timeout: Duration) -> Result<Connection> {
        Err(LocusError::AuthorityAnchorUnavailable(
            "native Windows peer-authenticated named-pipe authority is required but unavailable"
                .into(),
        ))
    }
    pub fn set_deadlines(_connection: &Connection, _timeout: Duration) -> Result<()> {
        Ok(())
    }
    pub fn read_message(
        _connection: &mut Connection,
        _limit: usize,
        _timeout: Duration,
    ) -> Result<Vec<u8>> {
        Err(LocusError::AuthorityAnchorUnavailable(
            "Windows authority unavailable".into(),
        ))
    }
    pub fn write_message(
        _connection: &mut Connection,
        _value: &[u8],
        _timeout: Duration,
    ) -> Result<()> {
        Err(LocusError::AuthorityAnchorUnavailable(
            "Windows authority unavailable".into(),
        ))
    }
    #[cfg(not(test))]
    pub fn peer_identity(_connection: &Connection) -> Result<PeerIdentity> {
        Err(LocusError::AuthorityAnchorUnavailable(
            "Windows peer identity unavailable".into(),
        ))
    }
    pub fn current_user_identity() -> Result<String> {
        Err(LocusError::AuthorityAnchorUnavailable(
            "Windows SID identity unavailable".into(),
        ))
    }
    pub fn capture_supervisor_identity() -> Result<SupervisorIdentity> {
        Err(LocusError::AuthorityAnchorUnavailable(
            "Windows supervisor identity unavailable".into(),
        ))
    }
    pub fn supervisor_matches_identity(_expected: &SupervisorIdentity) -> Result<bool> {
        Err(LocusError::AuthorityAnchorUnavailable(
            "Windows supervisor identity unavailable".into(),
        ))
    }
    pub fn supervisor_is_current(_expected: &SupervisorIdentity) -> Result<bool> {
        Err(LocusError::AuthorityAnchorUnavailable(
            "Windows supervisor identity unavailable".into(),
        ))
    }
    pub fn retire(_endpoint: &AuthorityAnchorEndpoint) {}
}

#[cfg(not(any(unix, windows)))]
mod platform {
    use super::*;
    pub const TRANSPORT: &str = "unsupported";
    pub struct Listener;
    pub struct Connection;
    pub fn endpoint_address(_home: &Path, _epoch: &str) -> String {
        String::new()
    }
    pub fn validate_address(_home: &Path, _address: &str) -> Result<()> {
        unavailable()
    }
    pub fn bind(_endpoint: &AuthorityAnchorEndpoint) -> Result<Listener> {
        unavailable()
    }
    pub fn accept(_listener: &Listener) -> Result<Option<Connection>> {
        unavailable()
    }
    pub fn connect(_endpoint: &AuthorityAnchorEndpoint, _timeout: Duration) -> Result<Connection> {
        unavailable()
    }
    pub fn set_deadlines(_connection: &Connection, _timeout: Duration) -> Result<()> {
        unavailable()
    }
    pub fn read_message(
        _connection: &mut Connection,
        _limit: usize,
        _timeout: Duration,
    ) -> Result<Vec<u8>> {
        unavailable()
    }
    pub fn write_message(
        _connection: &mut Connection,
        _value: &[u8],
        _timeout: Duration,
    ) -> Result<()> {
        unavailable()
    }
    #[cfg(not(test))]
    pub fn peer_identity(_connection: &Connection) -> Result<PeerIdentity> {
        unavailable()
    }
    pub fn current_user_identity() -> Result<String> {
        unavailable()
    }
    pub fn capture_supervisor_identity() -> Result<SupervisorIdentity> {
        Err(LocusError::AuthorityAnchorUnavailable(
            "authority supervisor identity unsupported on this platform".into(),
        ))
    }
    pub fn supervisor_matches_identity(_expected: &SupervisorIdentity) -> Result<bool> {
        Ok(false)
    }
    pub fn supervisor_is_current(_expected: &SupervisorIdentity) -> Result<bool> {
        Ok(false)
    }
    pub fn retire(_endpoint: &AuthorityAnchorEndpoint) {}
    fn unavailable<T>() -> Result<T> {
        Err(LocusError::AuthorityAnchorUnavailable(
            "peer-authenticated local IPC is unsupported on this platform".into(),
        ))
    }
}

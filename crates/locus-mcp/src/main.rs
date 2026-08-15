//! locus-mcp — MCP multiplexor hard-scoped to the active Locus pin.
//!
//! Agents see only control tools when unpinned, and control + provider tools
//! when pinned. Agents cannot pin themselves (`locus_request_pin` only).
//!
//! AI-native surface:
//! - **Tools** — control + pinned providers; descriptions tagged `[locus:<alias|unpinned>]`
//! - **Resources** — `locus://session`, `locus://doctor`, `locus://bindings`
//! - **Prompts** — `locus_context` system fragment
//! - **Auto-pin** — optional silent pin from workspace `default_binding` (never force)
//!
//! Transports:
//! - **stdio** (default) — Content-Length or NDJSON for Claude Code / Cursor
//! - **HTTP** — `locus-mcp --http 127.0.0.1:8742` or `LOCUS_MCP_HTTP=1`
//!   Streamable-HTTP-lite: JSON-RPC POST `/mcp`, GET `/mcp` capabilities,
//!   GET `/mcp/sse` session ticks, GET `/health`; requires `LOCUS_MCP_HTTP_TOKEN`.
//!   `Mcp-Session-Id` memory cache + file-backed resume under
//!   `$LOCUS_HOME/http-sessions/` (or `LOCUS_MCP_SESSION_DIR`; TTL + max N;
//!   no secrets on disk). SSE: single-event by default; multi-message
//!   (progress/chunks + final) for large `tools/call` when Accept prefers
//!   `text/event-stream`.

mod anchor;
mod http_session;

use anchor::{AnchorDecision, SessionAnchor};
use anyhow::{bail, Context, Result};
use http_session::{
    resolve_http_session_dir, HttpSessionError, HttpSessionMap, HttpSessionPinSummary,
};
use locus_core::{
    build_doctor_report, call_tool_gated, compute_safe_next, control_tools, enforce_policy,
    find_workspace, gather_doctor_external, load_config, split_namespaced_tool, tools_for_binding,
    verify_claim, verify_session, AdapterTool, ApprovalGate, Binding, Session, Store, VERSION,
};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

/// Process-wide worker manager (synthetic + per-provider upstream MCP).
fn worker_manager() -> &'static Mutex<CompositeWorkerManagerGuard> {
    static MGR: OnceLock<Mutex<CompositeWorkerManagerGuard>> = OnceLock::new();
    MGR.get_or_init(|| Mutex::new(CompositeWorkerManagerGuard::new()))
}

/// Thin alias so we can keep the type local without re-export noise.
type CompositeWorkerManagerGuard = locus_core::CompositeWorkerManager;

/// Advisory auto-pin probe attempted once per process (start / first tools/list).
/// The probe never pins — see [`maybe_mcp_auto_pin`].
static AUTO_PIN_ATTEMPTED: AtomicBool = AtomicBool::new(false);

/// Process-wide stdio session anchor: the pinned identity this MCP session
/// observed at initialize (or first healthy pinned observation), plus the
/// mismatch-audit dedupe pair. See [`anchor`].
#[derive(Debug, Default)]
struct ProcessAnchorState {
    anchor: Option<SessionAnchor>,
    last_reported_mismatch: Option<(String, String)>,
}

fn process_anchor() -> &'static Mutex<ProcessAnchorState> {
    static STATE: OnceLock<Mutex<ProcessAnchorState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(ProcessAnchorState::default()))
}

/// Where the current MCP session stores its identity anchor.
#[derive(Debug, Clone)]
enum AnchorScope {
    /// stdio transport — one anchor per process.
    Process,
    /// HTTP transport — per `Mcp-Session-Id`, file-backed via [`HttpSessionMap`].
    Http(String),
    /// Anchorless read-only context (GET /mcp capabilities probe without an
    /// `Mcp-Session-Id`). Never used for POSTs: stateless JSON-RPC requests
    /// share the process-level anchor ([`AnchorScope::Process`]) so provider
    /// `tools/call` stays pin-swap protected even without the header.
    None,
}

impl AnchorScope {
    fn get(&self) -> Option<SessionAnchor> {
        match self {
            AnchorScope::Process => process_anchor()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .anchor
                .clone(),
            AnchorScope::Http(id) => http_session_map()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .anchor(id),
            AnchorScope::None => None,
        }
    }

    /// Compare-and-set against a healthy full observation (see [`anchor::decide`]).
    fn observe(&self, obs: &SessionAnchor, allow_establish: bool) -> Option<AnchorDecision> {
        match self {
            AnchorScope::Process => {
                let mut state = process_anchor().lock().unwrap_or_else(|e| e.into_inner());
                anchor::decide(&mut state.anchor, obs, allow_establish)
            }
            AnchorScope::Http(id) => http_session_map()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .observe_anchor(id, obs, allow_establish),
            AnchorScope::None => None,
        }
    }

    /// Initialize-only overwrite (explicit re-initialize adopts the current pin).
    fn reset(&self, new_anchor: Option<SessionAnchor>) {
        match self {
            AnchorScope::Process => {
                let mut state = process_anchor().lock().unwrap_or_else(|e| e.into_inner());
                state.anchor = new_anchor;
                state.last_reported_mismatch = None;
            }
            AnchorScope::Http(id) => {
                let mut map = http_session_map().lock().unwrap_or_else(|e| e.into_inner());
                let _ = map.reset_anchor(id, new_anchor);
            }
            AnchorScope::None => {}
        }
    }

    /// Mismatch audit dedupe: true when this (anchored_session_id,
    /// current_session_id) pair has not been reported yet.
    fn note_mismatch(&self, anchored: &SessionAnchor, current: &SessionAnchor) -> bool {
        let key = anchor::mismatch_key(anchored, current);
        match self {
            AnchorScope::Process => {
                let mut state = process_anchor().lock().unwrap_or_else(|e| e.into_inner());
                if state.last_reported_mismatch.as_ref() == Some(&key) {
                    return false;
                }
                state.last_reported_mismatch = Some(key);
                true
            }
            AnchorScope::Http(id) => http_session_map()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .note_mismatch(id, key),
            AnchorScope::None => false,
        }
    }
}

/// Audit an anchor event — aliases/tenants/binding_ids/session_ids only,
/// never secrets. Best-effort (anchor enforcement never depends on audit IO).
fn audit_anchor_event(op: &str, alias: &str, detail: Value) {
    if let Ok(s) = store() {
        let _ = s.audit(op, alias, Some(detail));
    }
}

/// Current healthy pinned identity for anchor establishment / initialize
/// adoption. Requires `drift.ok` and a loaded unfrozen session — an anchor is
/// never established from an unhealthy runtime.
fn current_healthy_anchor() -> Option<SessionAnchor> {
    let s = store().ok()?;
    let drift = s.check_drift_and_freeze().ok()?;
    if !drift.ok {
        return None;
    }
    let (session, bindings) = active_session_bindings().ok()??;
    if session.is_frozen() {
        return None;
    }
    Some(anchor::observation(&session, &bindings))
}

/// `pin_changed` refusal body + deduped `mcp.anchor_mismatch` audit.
/// Session-local: never mutates or freezes the store session.
fn pin_changed_refusal(
    scope: &AnchorScope,
    anchored: &SessionAnchor,
    current: &SessionAnchor,
    underlying_issues: &[String],
) -> Value {
    if scope.note_mismatch(anchored, current) {
        audit_anchor_event(
            "mcp.anchor_mismatch",
            &anchored.binding_alias,
            json!({
                "anchored": anchor::identity_json(anchored),
                "current": anchor::identity_json(current),
                "underlying_issues": underlying_issues,
            }),
        );
    }
    tool_text(
        anchor::pin_changed_error(anchored, current, underlying_issues),
        true,
    )
}

/// Anchor context for `not_pinned` refusals: distinguishes "pin vanished after
/// this session anchored" from "never pinned". Unpinned observations never
/// clear the anchor — a later re-pin to a different alias still refuses.
fn attach_anchor_context(body: &mut Value, session_anchor: Option<&SessionAnchor>) {
    if let Some(a) = session_anchor {
        body["anchor"] = json!({
            "anchored_alias": a.binding_alias,
            "anchored_tenant": a.tenant,
            "anchored_session_id": a.session_id,
        });
        body["hint"] = json!(format!(
            "previous pin `{}` vanished (locus leave?); this session stays locked to it. \
             Human: `locus enter {}` to restore, or re-initialize this client after pinning a different alias.",
            a.binding_alias, a.binding_alias
        ));
    }
}

/// Current pin identity for health surfaces — built from the SAME inputs the
/// provider gate compares against: a full observation (mode + namespaces)
/// when the active session + bindings load, else the primary-only drift
/// identity, else `None` (unpinned / vanished pin).
fn current_identity_observation(s: &Store) -> Option<SessionAnchor> {
    if let Ok(Some((session, bindings))) = active_session_bindings() {
        return Some(anchor::observation(&session, &bindings));
    }
    let drift = s.check_drift_and_freeze().ok()?;
    anchor::drift_observation(&drift)
}

/// Gate-equivalent identity comparison for health surfaces: full
/// `same_identity` when a full observation is available; primary-only
/// (`same_primary_identity`) only when just the drift identity exists (empty
/// mode). No observation never matches (fail closed) — health surfaces must
/// report unhealthy whenever provider tools would refuse with `pin_changed`.
fn anchor_matches_current(a: &SessionAnchor, current: Option<&SessionAnchor>) -> bool {
    match current {
        Some(obs) if obs.mode.is_empty() && obs.namespaces.is_empty() => {
            a.same_primary_identity(obs)
        }
        Some(obs) => a.same_identity(obs),
        None => false,
    }
}

/// Additive `mcp_anchor` block for control tools (omitted when no anchor
/// exists). Returns the report plus whether the current pin matches the
/// anchored identity — using the SAME comparison as the provider gate
/// (`anchor_matches_current`), so a mode/namespace identity change that would
/// refuse provider tools also reads as a mismatch here. `who` is display-only.
fn mcp_anchor_report(
    a: &SessionAnchor,
    who: Option<&locus_core::Whoami>,
    current: Option<&SessionAnchor>,
) -> (Value, bool) {
    let matches = anchor_matches_current(a, current);
    let hint = if matches {
        Value::Null
    } else if who.is_some() {
        json!(format!(
            "global pin changed after this MCP session anchored to `{}`; re-initialize this client to adopt, or `locus enter {}` to restore",
            a.binding_alias, a.binding_alias
        ))
    } else {
        json!(format!(
            "previous pin `{}` vanished; this session stays locked to it",
            a.binding_alias
        ))
    };
    (
        json!({
            "anchored_alias": a.binding_alias,
            "anchored_tenant": a.tenant,
            "anchored_binding_id": a.binding_id,
            "anchored_session_id": a.session_id,
            "current_alias": who.map(|w| w.binding_alias.clone()),
            "current_tenant": who.map(|w| w.tenant.clone()),
            "match": matches,
            "hint": hint,
        }),
        matches,
    )
}

/// Default HTTP bind when `--http` / `LOCUS_MCP_HTTP=1` without an address.
const DEFAULT_HTTP_ADDR: &str = "127.0.0.1:8742";

/// MCP HTTP session idle TTL. Clients must re-initialize after expiry.
const HTTP_SESSION_TTL: Duration = Duration::from_secs(30 * 60);
/// Cap concurrent opaque `Mcp-Session-Id` entries (memory + on-disk resume map).
const HTTP_SESSION_MAX: usize = 256;

/// Max HTTP request body accepted pre-auth (413 above this) — prevents a
/// trivial OOM DoS via a huge Content-Length before the token check runs.
const HTTP_MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
/// Max total bytes for the request line + headers (431 above this).
const HTTP_MAX_HEADER_BYTES: usize = 32 * 1024;
/// Max number of header fields (431 above this).
const HTTP_MAX_HEADER_COUNT: usize = 128;

/// Serialized JSON-RPC body size (bytes) above which SSE prefers multi-message
/// (progress / progressive text chunks + final complete response).
const DEFAULT_SSE_MULTI_THRESHOLD: usize = 4096;
/// Progressive `locus.sse.chunk` text slice size when multi-message SSE is used.
const DEFAULT_SSE_CHUNK_BYTES: usize = 2048;
/// Default tick interval for GET `/mcp/sse` hub heartbeats.
const DEFAULT_SSE_SESSION_INTERVAL: Duration = Duration::from_secs(5);

/// Wire framing chosen per message so mixed clients stay happy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Framing {
    /// `Content-Length: N\r\n\r\n{body}` (MCP stdio transport).
    ContentLength,
    /// Single JSON object terminated by `\n`.
    Ndjson,
}

#[derive(Debug, Clone)]
struct RunMode {
    /// When set, serve HTTP JSON-RPC instead of stdio.
    http_addr: Option<SocketAddr>,
}

fn main() {
    if let Some(result) = locus_core::run_authority_anchor_server_if_requested() {
        if let Err(error) = result {
            eprintln!("authority anchor error: {error}");
            std::process::exit(1);
        }
        return;
    }
    locus_core::restrict_validation_to_executor();
    // MCP servers must not pollute stdout with logs (stdio mode).
    if let Err(e) = run() {
        eprintln!("locus-mcp error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mode = parse_run_mode(std::env::args().skip(1))?;
    match mode.http_addr {
        Some(addr) => run_http(addr),
        None => run_stdio(),
    }
}

/// Parse CLI + env for transport selection.
///
/// ```text
/// locus-mcp                         # stdio (default)
/// locus-mcp --http                  # 127.0.0.1:8742
/// locus-mcp --http 127.0.0.1:9000
/// LOCUS_MCP_HTTP=1 locus-mcp        # same as --http
/// LOCUS_MCP_HTTP_ADDR=127.0.0.1:9…  # address when HTTP enabled via env
/// ```
fn parse_run_mode(args: impl IntoIterator<Item = String>) -> Result<RunMode> {
    let args: Vec<String> = args.into_iter().collect();
    let mut http_flag = false;
    let mut http_arg: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--http" | "-H" => {
                http_flag = true;
                if let Some(next) = args.get(i + 1) {
                    if !next.starts_with('-') && next.contains(':') {
                        http_arg = Some(next.clone());
                        i += 1;
                    }
                }
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other if other.starts_with("--http=") => {
                http_flag = true;
                http_arg = Some(other.trim_start_matches("--http=").to_string());
            }
            other => {
                bail!("unknown argument: {other} (try --help)");
            }
        }
        i += 1;
    }

    let env_http = env_truthy("LOCUS_MCP_HTTP");
    if !http_flag && !env_http {
        return Ok(RunMode { http_addr: None });
    }

    let addr_str = http_arg
        .or_else(|| std::env::var("LOCUS_MCP_HTTP_ADDR").ok())
        .unwrap_or_else(|| DEFAULT_HTTP_ADDR.to_string());
    let addr: SocketAddr = addr_str
        .parse()
        .with_context(|| format!("invalid HTTP bind address: {addr_str}"))?;
    Ok(RunMode {
        http_addr: Some(addr),
    })
}

fn print_help() {
    eprintln!(
        "locus-mcp {VERSION} — MCP multiplexor (pin-scoped tools)\n\n\
         Usage:\n\
           locus-mcp                 stdio MCP (Claude Code / Cursor default)\n\
           locus-mcp --http [ADDR]   Streamable-HTTP-lite on ADDR (default {DEFAULT_HTTP_ADDR})\n\n\
         HTTP endpoints:\n\
           GET  /health              unauthenticated liveness\n\
           GET  /mcp                 capabilities + tool names (token; values-free)\n\
           GET  /mcp/sse             session_ok SSE ticks (token; hub heartbeat)\n\
           POST /mcp                 JSON-RPC 2.0 (token; Accept: application/json\n\
                                     and/or text/event-stream; mints/binds Mcp-Session-Id;\n\
                                     multi-event SSE for large tools/call)\n\
           DELETE /mcp               terminate Mcp-Session-Id (token)\n\n\
         Env:\n\
           LOCUS_MCP_HTTP=1            enable HTTP (same as --http)\n\
           LOCUS_MCP_HTTP_ADDR         bind address when HTTP enabled\n\
           LOCUS_MCP_HTTP_TOKEN        required bearer/token for HTTP auth\n\
           LOCUS_MCP_HTTP_ALLOW_REMOTE=1  allow non-loopback bind (default: loopback only)\n\
           LOCUS_MCP_SSE_MULTI_BYTES   multi-message SSE threshold (default 4096)\n\
           LOCUS_MCP_SSE_CHUNK_BYTES   progressive text chunk size (default 2048)\n\
           LOCUS_MCP_SSE_INTERVAL      GET /mcp/sse tick interval (default 5s)\n\
           LOCUS_HOME                  store root (pin + bindings + http-sessions)\n\
           LOCUS_MCP_SESSION_DIR       override HTTP Mcp-Session-Id disk map\n\
                                       (default: $LOCUS_HOME/http-sessions)\n\
           LOCUS_WORKER_IDLE_SECS      optional idle teardown for upstream workers\n\
           LOCUS_WORKER_SANDBOX=1      require supported OS isolation or fail closed\n\
           LOCUS_WORKER_SANDBOX_NO_NETWORK=1  opt-in deny network (bwrap/Seatbelt)\n"
    );
}

fn env_truthy(key: &str) -> bool {
    std::env::var(key)
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            matches!(v.as_str(), "1" | "true" | "on" | "yes")
        })
        .unwrap_or(false)
}

fn run_stdio() -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut reader = stdin.lock();

    loop {
        let Some((msg, framing)) = read_message(&mut reader)? else {
            break; // EOF
        };

        if let Some(response) = dispatch_rpc(&msg, &AnchorScope::Process) {
            write_message(&mut stdout, &response, framing)?;
        }
    }
    Ok(())
}

/// Dispatch one JSON-RPC request/notification.
/// Returns `None` for notifications (no response).
fn dispatch_rpc(msg: &Value, scope: &AnchorScope) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(json!({}));

    if id.is_none() {
        handle_notification(method, &params);
        return None;
    }

    let result = match method {
        "initialize" => Ok(handle_initialize(&params, scope)),
        "ping" => Ok(json!({})),
        "tools/list" => handle_tools_list(scope),
        "tools/call" => handle_tools_call(&params, scope),
        "resources/list" => handle_resources_list(),
        "resources/read" => handle_resources_read(&params, scope),
        "prompts/list" => handle_prompts_list(),
        "prompts/get" => handle_prompts_get(&params),
        other => Err(rpc_error(-32601, format!("method not found: {other}"))),
    };

    Some(match result {
        Ok(r) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": r,
        }),
        Err(err) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": err,
        }),
    })
}

// ─── HTTP transport (Streamable-HTTP-lite: POST /mcp + GET /mcp[/sse] + /health) ──

/// How the client wants the MCP response encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HttpResponseMode {
    /// Single JSON object (`Content-Type: application/json`) — default / preferred.
    Json,
    /// SSE stream (`text/event-stream`): single event, or multi-message when the
    /// JSON-RPC body is large / `tools/call` exceeds the multi threshold.
    Sse,
}

/// Values-free pin summary for optional HTTP session disk annotation.
fn current_http_session_pin_summary() -> Option<HttpSessionPinSummary> {
    let s = store().ok()?;
    let w = s.whoami().ok()?;
    Some(HttpSessionPinSummary {
        binding_alias: Some(w.binding_alias),
        tenant: Some(w.tenant),
        mode: Some(w.mode),
        seal_ok: Some(w.seal_ok),
    })
}

fn http_session_map() -> &'static Mutex<HttpSessionMap> {
    static MAP: OnceLock<Mutex<HttpSessionMap>> = OnceLock::new();
    MAP.get_or_init(|| {
        Mutex::new(
            HttpSessionMap::new(HTTP_SESSION_TTL, HTTP_SESSION_MAX)
                .with_persist_dir(resolve_http_session_dir())
                .with_pin_summary_fn(Some(current_http_session_pin_summary)),
        )
    })
}

/// Resolve `Mcp-Session-Id` for an authenticated MCP HTTP request.
///
/// - Missing header + `mint_if_missing` → mint (initialize / first POST path).
/// - Present valid → touch idle TTL and reuse.
/// - Present unknown/expired → fail closed (`Unknown`).
/// - Present empty → `Invalid`.
fn resolve_mcp_http_session(
    map: &mut HttpSessionMap,
    headers: &[(String, String)],
    mint_if_missing: bool,
) -> Result<Option<String>, HttpSessionError> {
    match header_value(headers, "mcp-session-id").map(str::trim) {
        Some("") => Err(HttpSessionError::Invalid),
        Some(id) => {
            if map.touch(id) {
                Ok(Some(id.to_string()))
            } else {
                Err(HttpSessionError::Unknown)
            }
        }
        None if mint_if_missing => map.mint().map(Some),
        None => Ok(None),
    }
}

fn session_error_body(err: &HttpSessionError) -> (u16, Value) {
    match err {
        HttpSessionError::Unknown => (
            404,
            json!({
                "error": "unknown_session",
                "hint": "Mcp-Session-Id not found or expired; POST initialize without the header to mint a new session",
            }),
        ),
        HttpSessionError::Invalid => (
            400,
            json!({
                "error": "invalid_session",
                "hint": "Mcp-Session-Id must be a non-empty opaque id",
            }),
        ),
        HttpSessionError::Capacity => (
            503,
            json!({
                "error": "session_capacity",
                "hint": "Too many concurrent MCP HTTP sessions; retry later or DELETE /mcp with an idle Mcp-Session-Id",
            }),
        ),
    }
}

fn run_http(addr: SocketAddr) -> Result<()> {
    if !addr.ip().is_loopback() && !env_truthy("LOCUS_MCP_HTTP_ALLOW_REMOTE") {
        bail!(
            "refusing non-loopback bind {addr} — use 127.0.0.1 or set LOCUS_MCP_HTTP_ALLOW_REMOTE=1"
        );
    }
    // Prefer explicit loopback default even if caller passed IPv6 localhost only via env.
    if matches!(addr.ip(), IpAddr::V4(ip) if ip == Ipv4Addr::UNSPECIFIED)
        && !env_truthy("LOCUS_MCP_HTTP_ALLOW_REMOTE")
    {
        bail!("refusing 0.0.0.0 bind without LOCUS_MCP_HTTP_ALLOW_REMOTE=1");
    }

    let token = std::env::var("LOCUS_MCP_HTTP_TOKEN").unwrap_or_default();
    let token = token.trim().to_string();
    if token.is_empty() {
        bail!("LOCUS_MCP_HTTP_TOKEN is required for HTTP mode (set a non-empty shared secret)");
    }

    let listener = TcpListener::bind(addr).with_context(|| format!("bind {addr}"))?;
    eprintln!(
        "locus-mcp: HTTP listening on http://{addr}  (GET|POST|DELETE /mcp, GET /mcp/sse, GET /health)  token auth + Mcp-Session-Id"
    );

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let token = token.clone();
                // Detach per connection so health checks aren't blocked by tools/call.
                thread::spawn(move || {
                    if let Err(e) = handle_http_connection(stream, &token) {
                        eprintln!("locus-mcp http: {e:#}");
                    }
                });
            }
            Err(e) => eprintln!("locus-mcp http accept: {e}"),
        }
    }
    Ok(())
}

fn handle_http_connection(mut stream: TcpStream, expected_token: &str) -> Result<()> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));

    let mut reader = io::BufReader::new(stream.try_clone().context("clone stream")?);
    // Size caps run pre-auth and fail closed with an explicit HTTP status
    // (never an unbounded allocation or a silently dropped connection).
    let (method, path, headers, body) = match read_http_request(&mut reader) {
        Ok(req) => req,
        Err(HttpReadError::PayloadTooLarge { content_length }) => {
            let body = json!({
                "error": "payload_too_large",
                "hint": format!(
                    "Content-Length {content_length} exceeds max {HTTP_MAX_BODY_BYTES} bytes"
                ),
            });
            return write_http_json(&mut stream, 413, &body, None);
        }
        Err(HttpReadError::HeadersTooLarge) => {
            let body = json!({
                "error": "request_header_fields_too_large",
                "hint": format!(
                    "headers exceed {HTTP_MAX_HEADER_COUNT} fields / {HTTP_MAX_HEADER_BYTES} bytes"
                ),
            });
            return write_http_json(&mut stream, 431, &body, None);
        }
        Err(HttpReadError::Fatal(e)) => return Err(e),
    };

    let path_only = path.split('?').next().unwrap_or(path.as_str());
    let query = path.split_once('?').map(|(_, q)| q).unwrap_or("");
    let accept_caps = http_accept_caps(&headers);
    let response_mode = http_response_mode_from_caps(accept_caps);
    let is_mcp_path = path_only == "/mcp" || path_only == "/mcp/" || path_only == "/jsonrpc";

    // Health is unauthenticated so orchestrators can probe liveness without the token.
    if method.eq_ignore_ascii_case("GET")
        && (path_only == "/health" || path_only == "/healthz" || path_only == "/")
    {
        let body = json!({
            "ok": true,
            "service": "locus-mcp",
            "version": VERSION,
            "transport": "streamable-http-lite",
            "endpoints": {
                "health": "GET /health",
                "capabilities": "GET /mcp (token)",
                "session_sse": "GET /mcp/sse (token)",
                "rpc": "POST /mcp (token, JSON-RPC 2.0)",
                "session": "Mcp-Session-Id header (minted on initialize)"
            },
        });
        return write_http_json(&mut stream, 200, &body, None);
    }

    if !http_token_ok(&headers, expected_token) {
        let body = json!({
            "error": "unauthorized",
            "hint": "set Authorization: Bearer <LOCUS_MCP_HTTP_TOKEN> or X-Locus-Token header",
        });
        return write_http_json(&mut stream, 401, &body, Some("unauthorized"));
    }

    // Reject Accept that cannot receive either JSON or SSE (streamable clients list both).
    // GET /mcp/sse is SSE-only — require event-stream (or */* / missing).
    if path_only == "/mcp/sse" || path_only == "/mcp/sse/" {
        if let Some(accept) = header_value(&headers, "accept") {
            let lower = accept.to_ascii_lowercase();
            if !lower.trim().is_empty()
                && !lower.contains("*/*")
                && !lower.contains("text/event-stream")
            {
                let body = json!({
                    "error": "not_acceptable",
                    "hint": "GET /mcp/sse requires Accept: text/event-stream",
                });
                return write_http_json(&mut stream, 406, &body, None);
            }
        }
    } else if let Some(accept) = header_value(&headers, "accept") {
        if !http_accept_allows_mcp(accept) {
            let body = json!({
                "error": "not_acceptable",
                "hint": "Accept must allow application/json and/or text/event-stream (MCP streamable HTTP)",
            });
            return write_http_json(&mut stream, 406, &body, None);
        }
    }

    if method.eq_ignore_ascii_case("GET") && (path_only == "/mcp/sse" || path_only == "/mcp/sse/") {
        return handle_mcp_sse_session_stream(&mut stream, query);
    }

    if method.eq_ignore_ascii_case("GET") && is_mcp_path {
        // Capabilities probe: session optional. Unknown id fails closed.
        let session_id = {
            let mut map = http_session_map().lock().unwrap_or_else(|e| e.into_inner());
            match resolve_mcp_http_session(&mut map, &headers, false) {
                Ok(id) => id,
                Err(err) => {
                    let (status, body) = session_error_body(&err);
                    return write_http_json(&mut stream, status, &body, None);
                }
            }
        };
        // Auth'd capabilities probe — tool *names* only, never secret values.
        // Read-only anchor report when an Mcp-Session-Id was presented.
        let anchor_scope = match session_id.as_deref() {
            Some(id) => AnchorScope::Http(id.to_string()),
            None => AnchorScope::None,
        };
        let caps = http_mcp_capabilities(&anchor_scope);
        return write_http_mcp_body(
            &mut stream,
            200,
            &caps,
            response_mode,
            session_id.as_deref(),
            None,
        );
    }

    if method.eq_ignore_ascii_case("DELETE") && is_mcp_path {
        // Explicit session teardown (MCP streamable HTTP).
        let provided = header_value(&headers, "mcp-session-id").map(str::trim);
        match provided {
            None | Some("") => {
                let body = json!({
                    "error": "invalid_session",
                    "hint": "DELETE /mcp requires a non-empty Mcp-Session-Id header",
                });
                return write_http_json(&mut stream, 400, &body, None);
            }
            Some(id) => {
                let removed = {
                    let mut map = http_session_map().lock().unwrap_or_else(|e| e.into_inner());
                    map.remove(id)
                };
                if removed {
                    return write_http_response(&mut stream, 204, "text/plain", b"", None);
                }
                let (status, body) = session_error_body(&HttpSessionError::Unknown);
                return write_http_json(&mut stream, status, &body, None);
            }
        }
    }

    if method.eq_ignore_ascii_case("POST") && is_mcp_path {
        // Content-Type: prefer application/json; allow missing (legacy CI) and
        // application/*+json. Reject clearly wrong types fail-closed.
        if let Some(ct) = header_value(&headers, "content-type") {
            if !http_content_type_ok(ct) {
                let body = json!({
                    "error": "unsupported_media_type",
                    "hint": "Content-Type must be application/json (or application/*+json)",
                });
                return write_http_json(&mut stream, 415, &body, None);
            }
        }

        // Parse JSON-RPC before any session work: garbage must not mint or
        // persist session state (capacity wedge), and parse failures get an
        // explicit 400 instead of a dropped connection.
        let msg: Value = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                let body = json!({
                    "jsonrpc": "2.0",
                    "id": Value::Null,
                    "error": { "code": -32700, "message": format!("parse error: {e}") },
                });
                return write_http_json(&mut stream, 400, &body, None);
            }
        };
        let rpc_method = msg.get("method").and_then(|m| m.as_str());

        // Bind an existing Mcp-Session-Id when the header is present; mint only
        // on initialize so arbitrary POSTs cannot exhaust session capacity.
        let session_id = {
            let mut map = http_session_map().lock().unwrap_or_else(|e| e.into_inner());
            let mint_if_missing = rpc_method == Some("initialize");
            match resolve_mcp_http_session(&mut map, &headers, mint_if_missing) {
                Ok(id) => id,
                Err(err) => {
                    let (status, body) = session_error_body(&err);
                    return write_http_json(&mut stream, status, &body, None);
                }
            }
        };

        // Optional strictness: refuse sessionless provider tools/call outright
        // (default off — stateless CI POSTs keep working, protected by the
        // shared process-level anchor below; conforming streamable-HTTP
        // clients always carry the header).
        if session_id.is_none()
            && rpc_method == Some("tools/call")
            && env_truthy("LOCUS_MCP_HTTP_REQUIRE_SESSION")
        {
            let tool = msg
                .get("params")
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("");
            if !tool.starts_with("locus_") {
                let body = json!({
                    "error": "session_required",
                    "hint": "LOCUS_MCP_HTTP_REQUIRE_SESSION=1: provider tools/call requires an Mcp-Session-Id (POST initialize first)",
                });
                return write_http_json(&mut stream, 400, &body, None);
            }
        }

        // Stateless POSTs (no Mcp-Session-Id, non-initialize) share the
        // process-level anchor: provider tools/call stays pin-swap protected
        // (fail closed) without breaking legacy stateless clients — the same
        // decide() machinery, one anchor per server process (stdio parity).
        let anchor_scope = match session_id.as_deref() {
            Some(id) => AnchorScope::Http(id.to_string()),
            None => AnchorScope::Process,
        };
        // A fresh initialize without the header (stateless client) is the
        // adoption path for the shared process anchor too — mirrors stdio,
        // where initialize re-anchors the process. Session-bound initializes
        // only reset their own session anchor.
        if rpc_method == Some("initialize") && header_value(&headers, "mcp-session-id").is_none() {
            AnchorScope::Process.reset(current_healthy_anchor());
        }
        match dispatch_rpc(&msg, &anchor_scope) {
            Some(response) => {
                // When Accept lists both JSON and SSE, still upgrade large tools/call
                // to multi-message SSE so hubs get progressive chunks without SSE-only Accept.
                let mode = resolve_post_response_mode(accept_caps, rpc_method, &response);
                write_http_mcp_body(
                    &mut stream,
                    200,
                    &response,
                    mode,
                    session_id.as_deref(),
                    rpc_method,
                )
            }
            None => {
                // Notification — 202 Accepted, empty JSON object (still respect Accept).
                write_http_mcp_body(
                    &mut stream,
                    202,
                    &json!({}),
                    response_mode,
                    session_id.as_deref(),
                    rpc_method,
                )
            }
        }
    } else if method.eq_ignore_ascii_case("OPTIONS") {
        // Minimal CORS preflight for local browser tools (CI usually doesn't need it).
        write_http_response(
            &mut stream,
            204,
            "text/plain",
            b"",
            Some(vec![
                ("Access-Control-Allow-Origin", "*"),
                (
                    "Access-Control-Allow-Headers",
                    "Authorization, Content-Type, Accept, X-Locus-Token, Mcp-Session-Id",
                ),
                (
                    "Access-Control-Expose-Headers",
                    "Mcp-Session-Id, X-Locus-Streamable",
                ),
                ("Access-Control-Allow-Methods", "GET, POST, DELETE, OPTIONS"),
            ]),
        )
    } else {
        let body = json!({
            "error": "not_found",
            "hint": "GET /health · GET /mcp (capabilities) · GET /mcp/sse (session ticks) · POST /mcp (JSON-RPC 2.0) · DELETE /mcp (session)",
        });
        write_http_json(&mut stream, 404, &body, None)
    }
}

/// Values-free GET /mcp body: pin summary + tool names + advertised capabilities.
/// `scope` is used for a read-only anchor report only — GET never establishes
/// or resets an anchor.
fn http_mcp_capabilities(scope: &AnchorScope) -> Value {
    let _ = maybe_mcp_auto_pin();

    let pin = match store() {
        Ok(s) => {
            let _ = s.check_drift_and_freeze();
            match s.whoami() {
                Ok(w) => json!({
                    "pinned": true,
                    "binding_alias": w.binding_alias,
                    "tenant": w.tenant,
                    "mode": w.mode,
                    "frozen": w.frozen,
                    // Provider + account names only — no credential refs or secret values.
                    "providers": w.providers.iter().map(|p| json!({
                        "provider": p.provider,
                        "account": p.account,
                    })).collect::<Vec<_>>(),
                }),
                Err(_) => json!({
                    "pinned": false,
                    "hint": "Human must `locus pin <alias>` / `locus enter <alias>` (or CI mint). Agents cannot pin."
                }),
            }
        }
        Err(_) => json!({
            "pinned": false,
            "hint": "Store unavailable under LOCUS_HOME"
        }),
    };

    // Tool *names* only via the same exclusive catalog as tools/list — no schemas,
    // descriptions, or secret-bearing fields. Scope::None keeps this probe
    // read-only with respect to anchors (the global catalog is reported;
    // anchor_ok below carries the per-session verdict).
    let tool_names: Vec<String> = match handle_tools_list(&AnchorScope::None) {
        Ok(listed) => listed
            .get("tools")
            .and_then(|t| t.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| t.get("name").and_then(|n| n.as_str()).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        Err(_) => control_tools(false).into_iter().map(|t| t.name).collect(),
    };

    let mut caps = json!({
        "ok": true,
        "service": "locus-mcp",
        "version": VERSION,
        "transport": "streamable-http-lite",
        "protocolVersion": "2024-11-05",
        "capabilities": {
            "tools": { "listChanged": true },
            "resources": { "subscribe": false, "listChanged": true },
            "prompts": { "listChanged": true }
        },
        "pin": pin,
        "tools": tool_names,
        "endpoints": {
            "health": "GET /health",
            "capabilities": "GET /mcp",
            "session_sse": "GET /mcp/sse",
            "rpc": "POST /mcp",
            "session_delete": "DELETE /mcp"
        },
        "content_types": {
            "request": ["application/json"],
            "response": ["application/json", "text/event-stream"]
        },
        "streamable": {
            "mode": "json-preferred",
            "sse": "multi-message-for-large-tools-call",
            "session_sse": "GET /mcp/sse heartbeats session_ok ticks",
            "multi_threshold_bytes": sse_multi_threshold(),
            "session": {
                "header": "Mcp-Session-Id",
                "ttl_seconds": HTTP_SESSION_TTL.as_secs(),
                "max_sessions": HTTP_SESSION_MAX,
                "mint": "POST initialize without Mcp-Session-Id header",
                "storage": "memory-cache-plus-disk",
                "disk": "$LOCUS_HOME/http-sessions or LOCUS_MCP_SESSION_DIR",
                "disk_fields": "id + timestamps + optional pin summary (never secrets)"
            },
            "note": "Mcp-Session-Id file-backed resume + multi-message SSE for large tools/call landed. Multi-tenant remote multiplexor still open."
        }
    });

    // Values-free per-session anchor verdict — only when an Mcp-Session-Id
    // header was presented (sessionless probes keep the exact legacy shape).
    if matches!(scope, AnchorScope::Http(_)) {
        match scope.get() {
            Some(a) => {
                // Same identity comparison as the provider gate — a
                // mode/namespace change must read anchor_ok=false too.
                let anchor_ok = match store().ok() {
                    Some(s) => {
                        anchor_matches_current(&a, current_identity_observation(&s).as_ref())
                    }
                    None => false,
                };
                caps["anchor_ok"] = json!(anchor_ok);
                caps["anchor"] = json!({ "alias": a.binding_alias });
            }
            None => {
                caps["anchor_ok"] = json!(true);
            }
        }
    }
    caps
}

fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// Parsed Accept capabilities for streamable MCP responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HttpAcceptCaps {
    json: bool,
    sse: bool,
}

fn http_accept_caps(headers: &[(String, String)]) -> HttpAcceptCaps {
    let Some(accept) = header_value(headers, "accept") else {
        // Missing Accept → JSON legacy clients.
        return HttpAcceptCaps {
            json: true,
            sse: false,
        };
    };
    let lower = accept.to_ascii_lowercase();
    if lower.trim().is_empty() || lower.contains("*/*") {
        return HttpAcceptCaps {
            json: true,
            sse: true,
        };
    }
    HttpAcceptCaps {
        json: lower.contains("application/json"),
        sse: lower.contains("text/event-stream"),
    }
}

/// Streamable HTTP Accept: allow `application/json` and/or `text/event-stream`.
/// Also allows `*/*` and empty (legacy clients).
fn http_accept_allows_mcp(accept: &str) -> bool {
    let lower = accept.to_ascii_lowercase();
    if lower.trim().is_empty() || lower.contains("*/*") {
        return true;
    }
    lower.contains("application/json") || lower.contains("text/event-stream")
}

/// Prefer JSON when the client lists it (or omits Accept). SSE when the client
/// accepts event-stream and does **not** list application/json.
fn http_response_mode_from_caps(caps: HttpAcceptCaps) -> HttpResponseMode {
    if caps.sse && !caps.json {
        HttpResponseMode::Sse
    } else {
        HttpResponseMode::Json
    }
}

/// POST response mode: SSE-only Accept → SSE; both allowed → JSON unless
/// body exceeds multi threshold and SSE is allowed.
fn resolve_post_response_mode(
    caps: HttpAcceptCaps,
    rpc_method: Option<&str>,
    response: &Value,
) -> HttpResponseMode {
    if caps.sse && !caps.json {
        return HttpResponseMode::Sse;
    }
    if caps.sse && should_use_multi_sse(rpc_method, response) {
        return HttpResponseMode::Sse;
    }
    HttpResponseMode::Json
}

fn http_content_type_ok(content_type: &str) -> bool {
    let main = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    main.is_empty()
        || main == "application/json"
        || main == "application/json-rpc"
        || main.ends_with("+json")
}

fn http_token_ok(headers: &[(String, String)], expected: &str) -> bool {
    for (k, v) in headers {
        let k = k.to_ascii_lowercase();
        if (k == "x-locus-token" || k == "x-locus-mcp-token")
            && constant_time_eq(v.trim(), expected)
        {
            return true;
        }
        if k == "authorization" {
            let v = v.trim();
            if let Some(rest) = v
                .strip_prefix("Bearer ")
                .or_else(|| v.strip_prefix("bearer "))
            {
                if constant_time_eq(rest.trim(), expected) {
                    return true;
                }
            }
            // Also accept raw token in Authorization (some CI helpers).
            if constant_time_eq(v, expected) {
                return true;
            }
        }
    }
    false
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn sse_multi_threshold() -> usize {
    std::env::var("LOCUS_MCP_SSE_MULTI_BYTES")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .filter(|n: &usize| *n > 0)
        .unwrap_or(DEFAULT_SSE_MULTI_THRESHOLD)
}

fn sse_chunk_bytes() -> usize {
    std::env::var("LOCUS_MCP_SSE_CHUNK_BYTES")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .filter(|n: &usize| *n > 0)
        .unwrap_or(DEFAULT_SSE_CHUNK_BYTES)
}

fn sse_session_interval() -> Duration {
    let raw = std::env::var("LOCUS_MCP_SSE_INTERVAL").unwrap_or_default();
    parse_sse_interval(&raw).unwrap_or(DEFAULT_SSE_SESSION_INTERVAL)
}

fn parse_sse_interval(s: &str) -> Option<Duration> {
    let s = s.trim().to_ascii_lowercase();
    if s.is_empty() {
        return None;
    }
    if let Ok(n) = s.parse::<u64>() {
        return Some(Duration::from_secs(n.max(1)));
    }
    if let Some(num) = s.strip_suffix('s') {
        let n: u64 = num.parse().ok()?;
        return Some(Duration::from_secs(n.max(1)));
    }
    if let Some(num) = s.strip_suffix('m') {
        let n: u64 = num.parse().ok()?;
        return Some(Duration::from_secs(n.saturating_mul(60).max(1)));
    }
    None
}

fn query_flag(query: &str, name: &str) -> bool {
    for part in query.split('&') {
        let part = part.trim();
        if part == name {
            return true;
        }
        if let Some((k, v)) = part.split_once('=') {
            if k == name {
                let v = v.trim().to_ascii_lowercase();
                return matches!(v.as_str(), "1" | "true" | "yes" | "on");
            }
        }
    }
    false
}

fn query_value<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    for part in query.split('&') {
        if let Some((k, v)) = part.split_once('=') {
            if k == name {
                return Some(v);
            }
        }
    }
    None
}

/// Whether this JSON-RPC response should use multi-message SSE framing.
fn should_use_multi_sse(rpc_method: Option<&str>, response: &Value) -> bool {
    let bytes = serde_json::to_vec(response).map(|b| b.len()).unwrap_or(0);
    let threshold = sse_multi_threshold();
    if bytes >= threshold {
        return true;
    }
    // tools/call: multi when over a soft floor, or primary text exceeds chunk size.
    if rpc_method == Some("tools/call") {
        if bytes >= threshold / 2 && threshold > 1 {
            return true;
        }
        if let Some(text) = extract_tool_result_text(response) {
            if text.len() >= sse_chunk_bytes() {
                return true;
            }
        }
    }
    false
}

/// Pull the first text content blob from a tools/call JSON-RPC response (if any).
fn extract_tool_result_text(response: &Value) -> Option<&str> {
    response
        .get("result")?
        .get("content")?
        .as_array()?
        .iter()
        .find_map(|c| {
            if c.get("type").and_then(|t| t.as_str()) == Some("text") {
                c.get("text").and_then(|t| t.as_str())
            } else {
                None
            }
        })
}

/// Split `text` into ~`chunk_size` byte slices on UTF-8 char boundaries.
fn split_text_chunks(text: &str, chunk_size: usize) -> Vec<String> {
    if chunk_size == 0 || text.is_empty() {
        return vec![text.to_string()];
    }
    let mut out = Vec::new();
    let mut buf = String::new();
    for ch in text.chars() {
        buf.push(ch);
        if buf.len() >= chunk_size {
            out.push(std::mem::take(&mut buf));
        }
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

/// Build the ordered list of JSON-RPC messages for an SSE response body.
///
/// Multi-message layout (JSON-RPC correct: intermediate events are notifications
/// without `id`; final event is the complete response with the original `id`):
/// 1. optional `notifications/message` progress (start)
/// 2. optional progressive text chunks for large tool results
/// 3. final complete JSON-RPC response (authoritative)
fn build_sse_rpc_messages(body: &Value, rpc_method: Option<&str>) -> Vec<Value> {
    if !should_use_multi_sse(rpc_method, body) {
        return vec![body.clone()];
    }

    let mut events = Vec::new();
    let rpc_id = body.get("id").cloned().unwrap_or(Value::Null);
    let total_bytes = serde_json::to_vec(body).map(|b| b.len()).unwrap_or(0);

    events.push(json!({
        "jsonrpc": "2.0",
        "method": "notifications/message",
        "params": {
            "level": "info",
            "data": {
                "kind": "locus.sse.progress",
                "phase": "start",
                "rpc_id": rpc_id,
                "rpc_method": rpc_method,
                "bytes": total_bytes
            }
        }
    }));

    // Progressive text chunks for large tools/call bodies (display only).
    // Final event always carries the complete JSON-RPC result for correctness.
    // Char-aware packing keeps UTF-8 intact at chunk boundaries.
    if let Some(text) = extract_tool_result_text(body) {
        let chunk_size = sse_chunk_bytes();
        if text.len() >= chunk_size {
            let chunks = split_text_chunks(text, chunk_size);
            let total = chunks.len();
            for (index, chunk) in chunks.into_iter().enumerate() {
                events.push(json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/message",
                    "params": {
                        "level": "info",
                        "data": {
                            "kind": "locus.sse.chunk",
                            "index": index,
                            "total": total,
                            "text": chunk
                        }
                    }
                }));
            }
        }
    }

    // Authoritative final JSON-RPC response (keeps id + full result/error).
    events.push(body.clone());
    events
}

fn encode_sse_messages(messages: &[Value]) -> Result<Vec<u8>> {
    let mut sse = Vec::new();
    for msg in messages {
        let json_bytes = serde_json::to_vec(msg)?;
        sse.extend_from_slice(b"event: message\ndata: ");
        sse.extend_from_slice(&json_bytes);
        sse.extend_from_slice(b"\n\n");
    }
    Ok(sse)
}

/// Compact values-free session tick for GET `/mcp/sse` (hub heartbeat over HTTP).
///
/// Shape mirrors `locus watch` NDJSON: `session_ok`, pin alias, doctor verdict,
/// safe_next action — never secrets or credential refs.
fn http_session_tick() -> Value {
    let pack = match store() {
        // Real external facts (Phantom PATH probe + unresolved phm refs) so the
        // tick matches `locus verify session --json`; gather failure ⇒ fail closed.
        Ok(s) => gather_doctor_external(&s, cwd())
            .ok()
            .and_then(|external| verify_session(&s, &cwd(), external).ok()),
        Err(_) => None,
    };

    match pack {
        Some(pack) => {
            let whoami_alias = pack.whoami.as_ref().map(|w| w.binding_alias.clone());
            let pinned =
                whoami_alias.is_some() || pack.doctor.pin.is_some() || pack.doctor.runtime.pinned;
            json!({
                "jsonrpc": "2.0",
                "method": "notifications/message",
                "params": {
                    "level": "info",
                    "data": {
                        "kind": "locus.session_tick",
                        "session_ok": pack.session_ok,
                        "whoami": whoami_alias,
                        "doctor_verdict": pack.doctor.verdict.as_str(),
                        "safe_next": pack.safe_next.action,
                        "pinned": pinned,
                        "frozen": pack.doctor.runtime.frozen,
                        "version": VERSION
                    }
                }
            })
        }
        None => json!({
            "jsonrpc": "2.0",
            "method": "notifications/message",
            "params": {
                "level": "info",
                "data": {
                    "kind": "locus.session_tick",
                    "session_ok": false,
                    "whoami": null,
                    "doctor_verdict": "UNSAFE",
                    "safe_next": "init",
                    "pinned": false,
                    "frozen": false,
                    "version": VERSION,
                    "hint": "store unavailable under LOCUS_HOME"
                }
            }
        }),
    }
}

/// Long-lived (or `?once=1`) SSE stream of session_ok ticks for hub heartbeats.
///
/// Auth is checked by the caller before this runs. Fail closed: no soft-allow path.
fn handle_mcp_sse_session_stream(stream: &mut TcpStream, query: &str) -> Result<()> {
    let once = query_flag(query, "once");
    let interval = query_value(query, "interval")
        .and_then(parse_sse_interval)
        .unwrap_or_else(sse_session_interval);

    // Long-lived streams need a generous write timeout; read side stays idle.
    let _ = stream.set_write_timeout(Some(Duration::from_secs(120)));
    write_http_sse_headers(
        stream,
        200,
        &[
            ("Cache-Control", "no-cache"),
            ("X-Locus-Streamable", "sse-session"),
        ],
    )?;
    // Comment preamble so proxies flush headers.
    stream.write_all(b": locus-mcp session stream\n\n")?;
    stream.flush()?;

    loop {
        let tick = http_session_tick();
        write_sse_event(stream, "message", &tick)?;
        if once {
            break;
        }
        // Keepalive comment between ticks (clients ignore `:` lines).
        stream.write_all(b": heartbeat\n\n")?;
        stream.flush()?;
        thread::sleep(interval);
        // Next write failure (client gone) ends the loop via ? below.
    }
    Ok(())
}

/// Pre-auth HTTP read failures that map to explicit status codes (fail closed).
#[derive(Debug)]
enum HttpReadError {
    /// Declared Content-Length exceeds [`HTTP_MAX_BODY_BYTES`] → 413.
    PayloadTooLarge { content_length: usize },
    /// Request line + headers exceed byte/count caps → 431.
    HeadersTooLarge,
    /// IO / framing error — nothing sane to answer; connection is dropped.
    Fatal(anyhow::Error),
}

#[allow(clippy::type_complexity)]
fn read_http_request<R: BufRead>(
    reader: &mut R,
) -> std::result::Result<(String, String, Vec<(String, String)>, Vec<u8>), HttpReadError> {
    // Budget the whole header section (request line included) so a huge line
    // without a newline cannot allocate unboundedly before the token check.
    let mut limited = io::Read::take(&mut *reader, HTTP_MAX_HEADER_BYTES as u64);
    let mut request_line = String::new();
    limited
        .read_line(&mut request_line)
        .map_err(|e| HttpReadError::Fatal(anyhow::Error::new(e).context("read request line")))?;
    if request_line.is_empty() {
        return Err(HttpReadError::Fatal(anyhow::anyhow!("empty HTTP request")));
    }
    if !request_line.ends_with('\n') && limited.limit() == 0 {
        return Err(HttpReadError::HeadersTooLarge);
    }
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        return Err(HttpReadError::Fatal(anyhow::anyhow!(
            "malformed request line: {request_line:?}"
        )));
    }
    let method = parts[0].to_string();
    let path = parts[1].to_string();

    let mut headers = Vec::new();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        let n = limited
            .read_line(&mut line)
            .map_err(|e| HttpReadError::Fatal(anyhow::Error::new(e).context("read header")))?;
        if n == 0 {
            // EOF (truncated request) or header byte budget exhausted.
            return Err(if limited.limit() == 0 {
                HttpReadError::HeadersTooLarge
            } else {
                HttpReadError::Fatal(anyhow::anyhow!("truncated HTTP headers"))
            });
        }
        if !line.ends_with('\n') && limited.limit() == 0 {
            return Err(HttpReadError::HeadersTooLarge);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if headers.len() >= HTTP_MAX_HEADER_COUNT {
            return Err(HttpReadError::HeadersTooLarge);
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            let key = k.trim().to_string();
            let val = v.trim().to_string();
            if key.eq_ignore_ascii_case("content-length") {
                content_length = val.parse().unwrap_or(0);
            }
            headers.push((key, val));
        }
    }

    // Cap before allocating the body buffer (pre-auth OOM guard).
    if content_length > HTTP_MAX_BODY_BYTES {
        return Err(HttpReadError::PayloadTooLarge { content_length });
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader
            .read_exact(&mut body)
            .map_err(|e| HttpReadError::Fatal(anyhow::Error::new(e).context("read HTTP body")))?;
    }
    Ok((method, path, headers, body))
}

fn write_http_json(
    stream: &mut TcpStream,
    status: u16,
    body: &Value,
    _reason: Option<&str>,
) -> Result<()> {
    write_http_json_with_session(stream, status, body, None)
}

fn write_http_json_with_session(
    stream: &mut TcpStream,
    status: u16,
    body: &Value,
    session_id: Option<&str>,
) -> Result<()> {
    let bytes = serde_json::to_vec(body)?;
    let mut extra: Vec<(&str, &str)> = Vec::new();
    if let Some(id) = session_id {
        extra.push(("Mcp-Session-Id", id));
    }
    write_http_response(
        stream,
        status,
        "application/json",
        &bytes,
        if extra.is_empty() { None } else { Some(extra) },
    )
}

/// Write an MCP HTTP body as `application/json` or SSE `event: message` frame(s).
fn write_http_mcp_body(
    stream: &mut TcpStream,
    status: u16,
    body: &Value,
    mode: HttpResponseMode,
    session_id: Option<&str>,
    rpc_method: Option<&str>,
) -> Result<()> {
    match mode {
        HttpResponseMode::Json => write_http_json_with_session(stream, status, body, session_id),
        HttpResponseMode::Sse => {
            let messages = build_sse_rpc_messages(body, rpc_method);
            let multi = messages.len() > 1;
            let sse = encode_sse_messages(&messages)?;
            let tag = if multi { "sse-multi" } else { "sse-single" };
            let mut extra: Vec<(&str, &str)> =
                vec![("Cache-Control", "no-cache"), ("X-Locus-Streamable", tag)];
            if let Some(id) = session_id {
                extra.push(("Mcp-Session-Id", id));
            }
            write_http_response(stream, status, "text/event-stream", &sse, Some(extra))
        }
    }
}

fn http_status_reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        406 => "Not Acceptable",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "OK",
    }
}

fn write_http_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
    extra_headers: Option<Vec<(&str, &str)>>,
) -> Result<()> {
    let reason = http_status_reason(status);
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if let Some(extra) = extra_headers {
        for (k, v) in extra {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()?;
    Ok(())
}

/// SSE response headers without Content-Length (long-lived stream until client disconnect).
fn write_http_sse_headers(
    stream: &mut TcpStream,
    status: u16,
    extra: &[(&str, &str)],
) -> Result<()> {
    let reason = http_status_reason(status);
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\n"
    );
    for (k, v) in extra {
        // Avoid duplicating Cache-Control if callers pass it.
        if k.eq_ignore_ascii_case("cache-control") || k.eq_ignore_ascii_case("content-type") {
            continue;
        }
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.flush()?;
    Ok(())
}

fn write_sse_event(stream: &mut TcpStream, event: &str, data: &Value) -> Result<()> {
    let json_bytes = serde_json::to_vec(data)?;
    stream.write_all(format!("event: {event}\ndata: ").as_bytes())?;
    stream.write_all(&json_bytes)?;
    stream.write_all(b"\n\n")?;
    stream.flush()?;
    Ok(())
}

/// Read one MCP message. Supports Content-Length headers and NDJSON lines.
fn read_message<R: BufRead>(reader: &mut R) -> Result<Option<(Value, Framing)>> {
    let mut first = String::new();
    let n = reader.read_line(&mut first)?;
    if n == 0 {
        return Ok(None);
    }
    let trimmed = first.trim();
    if trimmed.is_empty() {
        // Skip blank lines (common between framed messages)
        return read_message(reader);
    }

    // Content-Length framed: headers until blank line, then body bytes.
    if trimmed.to_ascii_lowercase().starts_with("content-length:")
        || trimmed.to_ascii_lowercase().starts_with("content-type:")
    {
        let mut content_length: Option<usize> = None;
        let mut line = first;
        loop {
            let lower = line.trim().to_ascii_lowercase();
            if lower.starts_with("content-length:") {
                let v = lower
                    .trim_start_matches("content-length:")
                    .trim()
                    .parse::<usize>()
                    .context("Content-Length parse")?;
                content_length = Some(v);
            }
            // blank line ends headers
            if line.trim().is_empty() {
                break;
            }
            line.clear();
            let n = reader.read_line(&mut line)?;
            if n == 0 {
                return Ok(None);
            }
        }
        let len = content_length.context("Content-Length header missing")?;
        let mut buf = vec![0u8; len];
        reader
            .read_exact(&mut buf)
            .context("read Content-Length body")?;
        let msg: Value = serde_json::from_slice(&buf).context("json body")?;
        return Ok(Some((msg, Framing::ContentLength)));
    }

    // NDJSON: first non-empty line is the JSON object.
    match serde_json::from_str::<Value>(trimmed) {
        Ok(msg) => Ok(Some((msg, Framing::Ndjson))),
        Err(e) => {
            eprintln!("locus-mcp: bad json: {e}");
            // Skip garbage line; keep serving
            read_message(reader)
        }
    }
}

fn write_message<W: Write>(out: &mut W, msg: &Value, framing: Framing) -> Result<()> {
    let body = serde_json::to_vec(msg)?;
    match framing {
        Framing::ContentLength => {
            // MCP stdio uses CRLF headers + raw body (no trailing newline required).
            write!(out, "Content-Length: {}\r\n\r\n", body.len())?;
            out.write_all(&body)?;
        }
        Framing::Ndjson => {
            out.write_all(&body)?;
            out.write_all(b"\n")?;
        }
    }
    out.flush()?;
    Ok(())
}

/// Notifications never receive a JSON-RPC response.
fn handle_notification(method: &str, _params: &Value) {
    match method {
        "notifications/initialized" | "initialized" => {
            // Client finished initialize handshake — ready for tools/list etc.
            // No server-side state required (initialize already ran the advisory auto-pin probe).
        }
        "notifications/cancelled" => {}
        _ => {
            // Ignore unknown notifications (forward-compatible).
        }
    }
}

fn rpc_error(code: i64, message: String) -> Value {
    json!({ "code": code, "message": message })
}

// ─── Initialize / agent instructions ────────────────────────────────────────

/// Crisp agent rules for `initialize.instructions` — pin state reflects the
/// operator-sealed store (the server never pins on its own).
fn agent_instructions(session_anchor: Option<&SessionAnchor>) -> String {
    let pin_line = match store() {
        Ok(s) => {
            let _ = s.check_drift_and_freeze();
            match s.whoami() {
                Ok(w) => format!(
                    "• Active pin: `{}` (tenant `{}`, mode {}). Catalog is exclusive to this binding — no ambient accounts.",
                    w.binding_alias,
                    w.tenant,
                    w.mode
                ),
                Err(_) => "• Currently unpinned — only locus_* control tools are available. There is no ambient personal fallthrough.".into(),
            }
        }
        Err(_) => "• Store unavailable — treat session as unpinned.".into(),
    };

    let mut lines: Vec<String> = vec![
        "Locus identity plane — tools are hard-scoped to the active sealed pin.".into(),
        pin_line,
        "• ALWAYS call locus_whoami or locus_safe_next (or read locus://session) before infrastructure mutations when context is unclear.".into(),
        "• You CANNOT pin or switch tenants. Use locus_request_pin / locus_enter_hint; surface the command so a human runs `locus pin <alias>` / `locus enter <alias>`.".into(),
        "• When stuck, unpinned, approval-blocked, or doctor-unhealthy: call locus_safe_next — it returns the single best next action.".into(),
        "• Hub session pack: locus_verify_session returns doctor + whoami + safe_next + session_ok (same as `locus verify session --json`). Gate on session_ok; available unpinned.".into(),
        "• Frozen scopes (project_ref, team_id, account_id, orgs/repos) cannot be overridden — provider-native and camelCase alias spellings (projectId, teamId, owner/org, …) are frozen too; scope freeze on mismatch is expected and correct.".into(),
        "• Resources always reflect current pin: locus://session, locus://doctor, locus://bindings. Prompt: locus_context. Re-read after pin changes.".into(),
        "• Never invent alternate project_ref/team/org. Never claim you re-pinned. Never log or request raw secrets.".into(),
    ];
    if let Some(a) = session_anchor {
        lines.push(format!(
            "• This MCP session is anchored to `{}`; if the global pin changes, provider tools refuse until the client re-initializes.",
            a.binding_alias
        ));
    }
    lines.join("\n")
}

fn handle_initialize(_params: &Value, scope: &AnchorScope) -> Value {
    // Advisory auto-pin probe once at MCP start (see maybe_mcp_auto_pin): it
    // audits the workspace default but never pins — instructions reflect the
    // operator-controlled pin state only.
    let _ = maybe_mcp_auto_pin();

    // Anchor adoption: an explicit initialize re-anchors this MCP session to
    // the current healthy pinned identity (the identity a human already made
    // globally active — agents still cannot pin), or clears the anchor when
    // unpinned/unhealthy. Audited when it replaces a different identity.
    let old_anchor = scope.get();
    let new_anchor = current_healthy_anchor();
    let replaced_identity = match (&old_anchor, &new_anchor) {
        (Some(o), Some(n)) => !o.same_identity(n),
        (Some(_), None) => true,
        _ => false,
    };
    if replaced_identity {
        if let Some(old) = old_anchor.as_ref() {
            audit_anchor_event(
                "mcp.anchor_reset",
                &old.binding_alias,
                json!({
                    "old": anchor::identity_json(old),
                    "new": new_anchor.as_ref().map(anchor::identity_json),
                }),
            );
        }
    } else if old_anchor.is_none() {
        if let Some(new) = new_anchor.as_ref() {
            audit_anchor_event(
                "mcp.anchor_established",
                &new.binding_alias,
                json!({ "anchor": anchor::identity_json(new) }),
            );
        }
    }
    scope.reset(new_anchor.clone());

    // listChanged=true: catalog may change after a human pin/leave; clients should re-list.
    json!({
        "protocolVersion": "2024-11-05",
        "capabilities": {
            "tools": { "listChanged": true },
            "resources": { "subscribe": false, "listChanged": true },
            "prompts": { "listChanged": true }
        },
        "serverInfo": {
            "name": "locus-mcp",
            "version": VERSION
        },
        "instructions": agent_instructions(new_anchor.as_ref())
    })
}

fn store() -> Result<Store> {
    Store::open_default().context("open locus store")
}

/// Active pin plus resolved bindings (alias, Binding) for exclusive or namespaced mode.
type ActiveBindings = (Session, Vec<(String, Binding)>);

/// Load active pin + all bindings (exclusive: one; namespaced: many).
/// Fails closed on invalid seal / expiry. Frozen sessions still return `Some`
/// so callers can emit `session_frozen` tool errors.
fn active_session_bindings() -> Result<Option<ActiveBindings>> {
    let s = store()?;
    match s.require_active() {
        Err(locus_core::LocusError::NotPinned) => Ok(None),
        Err(error) => Err(error.into()),
        Ok(session) => {
            let mut bindings = Vec::new();
            for alias in session.all_aliases() {
                let b = s.load_binding(&alias)?;
                bindings.push((alias, b));
            }
            Ok(Some((session, bindings)))
        }
    }
}

fn primary_binding(bindings: &[(String, Binding)]) -> Option<&Binding> {
    bindings.first().map(|(_, b)| b)
}

fn cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

// ─── MCP auto-pin ───────────────────────────────────────────────────────────

/// Whether the advisory auto-pin probe may run from workspace/cwd signals.
///
/// The knobs are parsed but currently **inert for authority**: the probe never
/// pins (see [`maybe_mcp_auto_pin`]); it only audits the refusal. Enabled when:
/// - `LOCUS_MCP_AUTO_PIN=1` (explicit), or
/// - `LOCUS_AUTO_PIN=cwd` / `clients.auto_pin=cwd`, or
/// - workspace `.locus.toml` has `require_pin = true`, or
/// - workspace has `default_binding`
///
/// Disabled when `LOCUS_MCP_AUTO_PIN=0|false|off`.
fn mcp_auto_pin_policy_enabled(home: &Path) -> locus_core::Result<bool> {
    if let Ok(v) = std::env::var("LOCUS_MCP_AUTO_PIN") {
        let v = v.trim().to_ascii_lowercase();
        if matches!(v.as_str(), "0" | "false" | "off" | "no") {
            return Ok(false);
        }
        if matches!(v.as_str(), "1" | "true" | "on" | "yes") {
            return Ok(true);
        }
    }

    if env_truthy_cwd("LOCUS_AUTO_PIN") {
        return Ok(true);
    }

    let cfg = load_config(home);
    if cfg
        .clients
        .auto_pin
        .as_deref()
        .map(|s| s.eq_ignore_ascii_case("cwd"))
        .unwrap_or(false)
    {
        return Ok(true);
    }

    // Preferred default: workspace with default_binding and/or require_pin.
    if let Some((_, ws)) = find_workspace(&cwd())? {
        if ws.require_pin {
            return Ok(true);
        }
        if ws
            .default_binding
            .as_ref()
            .map(|a| !a.is_empty())
            .unwrap_or(false)
        {
            return Ok(true);
        }
    }

    Ok(false)
}

fn env_truthy_cwd(key: &str) -> bool {
    std::env::var(key)
        .map(|v| v.trim().eq_ignore_ascii_case("cwd"))
        .unwrap_or(false)
}

/// Advisory workspace auto-pin probe when unpinned and policy allows.
///
/// MCP auto-pin is **advisory only — the server never pins**. The
/// `LOCUS_AUTO_PIN` / `LOCUS_MCP_AUTO_PIN` / workspace knobs are parsed, but
/// `Store::pin_auto_delegated` refuses: pinning requires operator authority,
/// and an agent-facing process cannot self-issue session authority (the
/// workspace `.locus.toml` is repo-local and agent-writable, and executor
/// grants are bound to an operator-supervised launch). Pending an explicit
/// operator-delegation design, the probe:
///
/// - Only runs when unpinned
/// - Only when workspace has `require_pin` or non-empty `default_binding`
/// - Resolves the workspace target read-only (never force past allowlist)
/// - Audits `session.auto_pin_denied` with the honest refusal reason
/// - Leaves the session unpinned (control tools + `locus_request_pin` only)
/// - At most once per process (initialize and/or first tools/list)
fn maybe_mcp_auto_pin() -> Option<String> {
    if AUTO_PIN_ATTEMPTED.swap(true, Ordering::SeqCst) {
        return None;
    }

    let s = match store() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("locus-mcp: auto-pin skipped (store): {e:#}");
            return None;
        }
    };

    match mcp_auto_pin_policy_enabled(s.home()) {
        Ok(true) => {}
        Ok(false) => return None,
        Err(e) => {
            eprintln!("locus-mcp: auto-pin blocked (workspace): {e}");
            return None;
        }
    }

    // Already pinned → nothing to do.
    match s.active_session() {
        Ok(Some(_)) => return None,
        Ok(None) => {}
        Err(e) => {
            eprintln!("locus-mcp: auto-pin skipped (active session): {e}");
            return None;
        }
    }

    let cwd = cwd();
    let ws = match find_workspace(&cwd) {
        Ok(ws) => ws,
        Err(e) => {
            eprintln!("locus-mcp: auto-pin blocked (workspace): {e}");
            return None;
        }
    };
    let (_, ref cfg) = ws?;
    // Only pin when workspace declares require_pin or default_binding.
    let has_default = cfg
        .default_binding
        .as_ref()
        .map(|a| !a.trim().is_empty())
        .unwrap_or(false);
    if !cfg.require_pin && !has_default {
        return None;
    }

    // Advisory-only resolve so the audit records which binding the workspace
    // suggested. Read-only: never forces past the allowlist, never pins.
    let advisory = s.resolve_auto_pin(&cwd).ok();
    match s.pin_auto_delegated(&cwd, Some("mcp-auto".into()), false) {
        Ok(session) => {
            // Unreachable today: pin_auto_delegated always refuses. Kept so a
            // future operator-delegation design slots in with the audit trail
            // already wired.
            let _ = s.audit(
                "session.auto_pin",
                &session.binding_alias,
                Some(json!({
                    "session_id": session.session_id,
                    "tenant": session.tenant,
                    "source": session.source,
                    "client": session.client,
                    "cwd": cwd.display().to_string(),
                    "reason": "mcp_auto_pin",
                })),
            );
            eprintln!(
                "locus-mcp: auto-pinned `{}` (tenant {}) from workspace",
                session.binding_alias, session.tenant
            );
            Some(session.binding_alias)
        }
        Err(e) => {
            // Honest fail-closed: leave unpinned, audit the refusal, and point
            // agents at control tools + locus_request_pin.
            let advisory_alias = advisory.as_ref().map(|t| t.alias.as_str());
            let _ = s.audit(
                "session.auto_pin_denied",
                advisory_alias.unwrap_or("-"),
                Some(json!({
                    "cwd": cwd.display().to_string(),
                    "advisory_binding": advisory_alias,
                    "reason": e.to_string(),
                })),
            );
            eprintln!("locus-mcp: auto-pin unavailable (staying unpinned): {e}");
            None
        }
    }
}

// ─── Resources ──────────────────────────────────────────────────────────────

const RESOURCE_SESSION: &str = "locus://session";
const RESOURCE_DOCTOR: &str = "locus://doctor";
const RESOURCE_BINDINGS: &str = "locus://bindings";

/// Live pin tag for resource/prompt descriptions (operator-controlled pin state).
fn pin_label_for_catalog() -> String {
    match store() {
        Ok(s) => {
            let _ = s.check_drift_and_freeze();
            match s.whoami() {
                Ok(w) => format!("locus:{}", w.binding_alias),
                Err(_) => "locus:unpinned".into(),
            }
        }
        Err(_) => "locus:unpinned".into(),
    }
}

fn handle_resources_list() -> std::result::Result<Value, Value> {
    // Run the once-per-process advisory auto-pin probe before describing resources.
    let _ = maybe_mcp_auto_pin();
    let pin = pin_label_for_catalog();
    Ok(json!({
        "resources": [
            {
                "uri": RESOURCE_SESSION,
                "name": "session",
                "title": "Active Locus pin (whoami)",
                "description": format!(
                    "[{pin}] Current pin whoami JSON: tenant, binding, providers, frozen scopes. Live after human pin/leave. Never includes secrets."
                ),
                "mimeType": "application/json"
            },
            {
                "uri": RESOURCE_DOCTOR,
                "name": "doctor",
                "title": "Locus doctor lite",
                "description": format!(
                    "[{pin}] Doctor-lite / runtime drift: pin health, seal, freeze, workspace. Never includes secrets."
                ),
                "mimeType": "application/json"
            },
            {
                "uri": RESOURCE_BINDINGS,
                "name": "bindings",
                "title": "Configured bindings",
                "description": format!(
                    "[{pin}] Binding summaries (alias, tenant, providers). No secrets."
                ),
                "mimeType": "application/json"
            }
        ]
    }))
}

fn handle_resources_read(params: &Value, scope: &AnchorScope) -> std::result::Result<Value, Value> {
    // Run the once-per-process advisory auto-pin probe (if initialize was skipped).
    let _ = maybe_mcp_auto_pin();
    let uri = params
        .get("uri")
        .and_then(|u| u.as_str())
        .ok_or_else(|| rpc_error(-32602, "missing resource uri".into()))?;

    let body = match uri {
        RESOURCE_SESSION => resource_session_json(scope)?,
        RESOURCE_DOCTOR => resource_doctor_json()?,
        RESOURCE_BINDINGS => resource_bindings_json()?,
        other => {
            return Err(rpc_error(-32002, format!("resource not found: {other}")));
        }
    };

    let text = serde_json::to_string(&body).unwrap_or_else(|_| body.to_string());
    Ok(json!({
        "contents": [{
            "uri": uri,
            "mimeType": "application/json",
            "text": text
        }]
    }))
}

fn resource_session_json(scope: &AnchorScope) -> std::result::Result<Value, Value> {
    let s = store().map_err(|e| rpc_error(-32000, e.to_string()))?;
    let _ = s.check_drift_and_freeze();
    let who = s.whoami().ok();
    let mut body = match &who {
        Some(w) => serde_json::to_value(w).unwrap_or(json!({})),
        None => json!({
            "pinned": false,
            "hint": "No active pin. Human: `locus pin <alias>` or `locus enter <alias>`. Agents: locus_request_pin / locus_enter_hint."
        }),
    };
    // Additive anchor block (omitted when this session has no anchor).
    if let Some(a) = scope.get() {
        let current = current_identity_observation(&s);
        let (report, _) = mcp_anchor_report(&a, who.as_ref(), current.as_ref());
        body["mcp_anchor"] = report;
    }
    Ok(body)
}

fn resource_doctor_json() -> std::result::Result<Value, Value> {
    let s = store().map_err(|e| rpc_error(-32000, e.to_string()))?;
    // Full structured report with real external facts (Phantom PATH probe +
    // unresolved phm refs) so it matches `locus doctor --json`. Never secrets.
    let external =
        gather_doctor_external(&s, cwd()).map_err(|e| rpc_error(-32000, e.to_string()))?;
    let report = build_doctor_report(&s, external).map_err(|e| rpc_error(-32000, e.to_string()))?;
    Ok(serde_json::to_value(report).unwrap_or(json!({})))
}

fn resource_bindings_json() -> std::result::Result<Value, Value> {
    let s = store().map_err(|e| rpc_error(-32000, e.to_string()))?;
    let list = s
        .list_bindings()
        .map_err(|e| rpc_error(-32000, e.to_string()))?;
    Ok(serde_json::to_value(list).unwrap_or(json!([])))
}

// ─── Prompts ────────────────────────────────────────────────────────────────

fn handle_prompts_list() -> std::result::Result<Value, Value> {
    let _ = maybe_mcp_auto_pin();
    let pin = pin_label_for_catalog();
    Ok(json!({
        "prompts": [{
            "name": "locus_context",
            "title": "Locus identity context",
            "description": format!(
                "[{pin}] System prompt fragment: active tenant, frozen scopes, agents cannot pin. Re-fetch after pin changes."
            ),
            "arguments": []
        }]
    }))
}

fn handle_prompts_get(params: &Value) -> std::result::Result<Value, Value> {
    let _ = maybe_mcp_auto_pin();
    let name = params
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| rpc_error(-32602, "missing prompt name".into()))?;

    match name {
        "locus_context" => {
            let text = build_locus_context_prompt();
            let pin = pin_label_for_catalog();
            Ok(json!({
                "description": format!("[{pin}] Locus identity context for the agent system prompt"),
                "messages": [{
                    "role": "user",
                    "content": {
                        "type": "text",
                        "text": text
                    }
                }]
            }))
        }
        other => Err(rpc_error(-32602, format!("unknown prompt: {other}"))),
    }
}

fn build_locus_context_prompt() -> String {
    let s = match store() {
        Ok(s) => s,
        Err(e) => {
            return format!(
                "## Locus identity context\n\nStore unavailable ({e}). Treat session as unpinned. Agents cannot pin — ask the human to run `locus pin <alias>`."
            );
        }
    };
    let _ = s.check_drift_and_freeze();

    let mut lines = vec![
        "## Locus identity context".into(),
        String::new(),
        "You are operating under the Locus identity plane. Credentials and account selectors are resolved at the gate — not in the prompt.".into(),
        String::new(),
        "### Hard rules".into(),
        "- You **cannot pin** or switch tenants. If the wrong account is active, ask the human to run `locus pin <alias>` / `locus enter <alias>` (or surface `locus_request_pin` / `locus_enter_hint`).".into(),
        "- Call `locus_whoami`, `locus_safe_next`, or read resource `locus://session` before infrastructure mutations.".into(),
        "- When stuck (unpinned, approval-blocked, freeze, doctor issues): call `locus_safe_next` for the single best next action.".into(),
        "- Do **not** invent or override frozen `project_ref`, `team_id`, orgs, or repos.".into(),
        "- Unpinned sessions only expose `locus_*` control tools — there is no ambient personal fallthrough.".into(),
        String::new(),
    ];

    match s.whoami() {
        Ok(w) => {
            lines.push("### Active pin".into());
            lines.push(format!("- **binding**: `{}`", w.binding_alias));
            lines.push(format!("- **tenant**: `{}`", w.tenant));
            lines.push(format!("- **binding_id**: `{}`", w.binding_id));
            lines.push(format!("- **session_id**: `{}`", w.session_id));
            lines.push(format!("- **seal_ok**: {}", w.seal_ok));
            lines.push(format!("- **frozen**: {}", w.frozen));
            if let Some(ref reason) = w.frozen_reason {
                lines.push(format!("- **frozen_reason**: {reason}"));
            }
            lines.push(format!("- **mode**: {}", w.mode));
            if !w.namespaces.is_empty() {
                lines.push(format!("- **namespaces**: {}", w.namespaces.join(", ")));
            }
            lines.push(format!("- **expires_at**: {}", w.expires_at));
            lines.push(String::new());
            lines.push("### Frozen scopes (providers)".into());
            if w.providers.is_empty() {
                lines.push("- (no providers on this binding)".into());
            } else {
                for p in &w.providers {
                    let mut scope_bits = Vec::new();
                    if let Some(ref r) = p.project_ref {
                        scope_bits.push(format!("project_ref={r}"));
                    }
                    if let Some(ref t) = p.team_id {
                        scope_bits.push(format!("team_id={t}"));
                    }
                    if let Some(ref a) = p.account_id {
                        scope_bits.push(format!("account_id={a}"));
                    }
                    if !p.orgs.is_empty() {
                        scope_bits.push(format!("orgs={}", p.orgs.join(",")));
                    }
                    if p.read_only == Some(true) {
                        scope_bits.push("read_only".into());
                    }
                    let scope = if scope_bits.is_empty() {
                        "(open within provider adapter)".into()
                    } else {
                        scope_bits.join("; ")
                    };
                    lines.push(format!(
                        "- **{}** account=`{}` credential=`{}` — {scope}",
                        p.provider, p.account, p.credential.source
                    ));
                }
            }
        }
        Err(_) => {
            lines.push("### Active pin".into());
            lines.push("- **pinned**: false".into());
            lines.push(
                "- No sealed session. Provider tools are unavailable until a human pins a binding."
                    .into(),
            );
        }
    }

    match find_workspace(&cwd()) {
        Ok(Some((path, cfg))) => {
            lines.push(String::new());
            lines.push("### Workspace".into());
            lines.push(format!("- **path**: `{}`", path.display()));
            if let Some(ref d) = cfg.default_binding {
                lines.push(format!("- **default_binding**: `{d}`"));
            }
            if !cfg.allowed_bindings.is_empty() {
                lines.push(format!(
                    "- **allowed_bindings**: {}",
                    cfg.allowed_bindings.join(", ")
                ));
            }
            lines.push(format!("- **require_pin**: {}", cfg.require_pin));
        }
        Ok(None) => {}
        Err(_) => {
            lines.push(String::new());
            lines.push("### Workspace".into());
            lines.push("- **status**: `unsafe` — policy is invalid or unreadable; do not use provider tools and run `locus doctor`.".into());
        }
    }

    lines.push(String::new());
    lines.push("### Resources".into());
    lines.push("- `locus://session` — whoami JSON".into());
    lines.push("- `locus://doctor` — doctor lite / drift".into());
    lines.push("- `locus://bindings` — configured binding summaries".into());

    lines.join("\n")
}

// ─── Tools list / call ──────────────────────────────────────────────────────

/// Map tools to MCP list payload; ensure `locus_whoami` first + locus tags.
fn tools_list_payload(mut tools: Vec<AdapterTool>, pin_alias: Option<&str>) -> Value {
    tag_tool_descriptions(&mut tools, pin_alias);
    ensure_whoami_first(&mut tools);
    let list: Vec<Value> = tools
        .into_iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "inputSchema": t.input_schema,
            })
        })
        .collect();
    json!({ "tools": list })
}

fn ensure_whoami_first(tools: &mut Vec<AdapterTool>) {
    if let Some(i) = tools.iter().position(|t| t.name == "locus_whoami") {
        if i != 0 {
            let t = tools.remove(i);
            tools.insert(0, t);
        }
    }
}

/// Prefix every tool description with `[locus:<alias|unpinned>]`.
fn tag_tool_descriptions(tools: &mut [AdapterTool], pin_alias: Option<&str>) {
    for t in tools.iter_mut() {
        if t.description.starts_with("[locus:") {
            continue;
        }
        let tag = if t.name.starts_with("locus_") {
            match pin_alias {
                Some(a) => format!("[locus:{a}]"),
                None => "[locus:unpinned]".into(),
            }
        } else if let Some((alias, _)) = split_namespaced_tool(&t.name) {
            format!("[locus:{alias}]")
        } else {
            match pin_alias {
                Some(a) => format!("[locus:{a}]"),
                None => "[locus:unpinned]".into(),
            }
        };
        // Drop legacy `[alias] ` prefix from namespaced soft-fail path if present.
        let rest = t.description.as_str();
        let rest = if rest.starts_with('[') && !rest.starts_with("[locus:") {
            if let Some(idx) = rest.find("] ") {
                &rest[idx + 2..]
            } else {
                rest
            }
        } else {
            rest
        };
        t.description = format!("{tag} {rest}");
    }
}

/// Unpinned / frozen / invalid seal ⇒ only locus_* control tools.
/// Healthy pin ⇒ control + provider tools (synthetic + upstream MCP when declared).
/// Namespaced multi-bind prefixes tools as `alias__tool`.
fn handle_tools_list(scope: &AnchorScope) -> std::result::Result<Value, Value> {
    // Advisory auto-pin probe when still unpinned (once per process; never grants authority).
    let _ = maybe_mcp_auto_pin();

    let s = store().map_err(|e| rpc_error(-32000, e.to_string()))?;
    // Heartbeat on every tools/list: freeze session if binding material drifted.
    let drift = s
        .check_drift_and_freeze()
        .map_err(|e| rpc_error(-32000, e.to_string()))?;

    // Control tools always. `locus_providers` when a pin exists (even frozen).
    let mut tools: Vec<AdapterTool> = control_tools(drift.pinned);
    let pin_alias = drift.binding_alias.clone();

    // Anchor check BEFORE the drift early-return so the catalog reflects the
    // anchored identity even when a cross-process re-pin staled the executor
    // grant (drift unhealthy but identity fields populated).
    let session_anchor = scope.get();
    if let (Some(anchored), true) = (&session_anchor, drift.pinned) {
        if let Some(current) = anchor::drift_observation(&drift) {
            if !anchored.same_primary_identity(&current) {
                return Ok(tools_list_payload(
                    control_tools(true),
                    Some(anchored.binding_alias.as_str()),
                ));
            }
        }
    }

    // Privileged provider catalog only when runtime is healthy (pinned, seal ok,
    // unfrozen, unexpired, binding matches). Fail closed otherwise.
    if !drift.ok {
        debug_assert!(tools.iter().all(|t| t.name.starts_with("locus_")));
        return Ok(tools_list_payload(tools, pin_alias.as_deref()));
    }

    let pinned = active_session_bindings().map_err(|e| rpc_error(-32000, e.to_string()))?;
    if let Some((ref session, ref bindings)) = pinned {
        // Belt + suspenders: frozen session never lists provider tools.
        if session.is_frozen() {
            return Ok(tools_list_payload(
                tools,
                Some(session.binding_alias.as_str()),
            ));
        }
        // Anchor observe on the healthy catalog path: establish on the first
        // healthy pinned observation; on mismatch collapse to control tools
        // tagged with the ANCHORED alias (fail closed, session-local).
        let obs = anchor::observation(session, bindings);
        match scope.observe(&obs, true) {
            Some(AnchorDecision::Mismatch { anchored }) => {
                if scope.note_mismatch(&anchored, &obs) {
                    audit_anchor_event(
                        "mcp.anchor_mismatch",
                        &anchored.binding_alias,
                        json!({
                            "anchored": anchor::identity_json(&anchored),
                            "current": anchor::identity_json(&obs),
                            "underlying_issues": [],
                        }),
                    );
                }
                return Ok(tools_list_payload(
                    tools,
                    Some(anchored.binding_alias.as_str()),
                ));
            }
            Some(AnchorDecision::Established) => {
                audit_anchor_event(
                    "mcp.anchor_established",
                    &obs.binding_alias,
                    json!({ "anchor": anchor::identity_json(&obs) }),
                );
            }
            Some(AnchorDecision::Repinned) => {
                audit_anchor_event(
                    "mcp.anchor_repin",
                    &obs.binding_alias,
                    json!({ "anchor": anchor::identity_json(&obs) }),
                );
            }
            Some(AnchorDecision::Match) | None => {}
        }
        let mgr = worker_manager()
            .lock()
            .map_err(|_| rpc_error(-32000, "worker manager lock poisoned".into()))?;
        // Discovery is side-effect free: merge synthetic tools and schemas from
        // workers that were already started by an authorized call. Never spawn
        // a credential-bearing child from tools/list.
        tools.extend(mgr.tools_for_session(session, bindings));
        return Ok(tools_list_payload(
            tools,
            Some(session.binding_alias.as_str()),
        ));
    }
    Ok(tools_list_payload(tools, pin_alias.as_deref()))
}

fn handle_tools_call(params: &Value, scope: &AnchorScope) -> std::result::Result<Value, Value> {
    let name = params
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| rpc_error(-32602, "missing tool name".into()))?;
    let args = params.get("arguments").cloned().unwrap_or(json!({}));

    // Control tools (allowed even when frozen — whoami/status/heartbeat report freeze)
    if name.starts_with("locus_") {
        return call_control(name, &args, scope);
    }

    // Provider tools require a healthy pin
    let s = store().map_err(|e| rpc_error(-32000, e.to_string()))?;

    // Continuous drift check — freezes session if binding file mutated.
    let drift = s
        .check_drift_and_freeze()
        .map_err(|e| rpc_error(-32000, e.to_string()))?;

    // Anchored-identity check BEFORE the drift early-returns: a cross-process
    // re-pin stales the executor grant, so drift is already unhealthy — the
    // wrong-account refusal (`pin_changed`) must outrank `runtime_unhealthy`.
    // RuntimeDrift carries identity fields even when unhealthy; drift.issues
    // ride along as underlying_issues so the authority-plane facts stay
    // visible. Establishment never happens here (unhealthy observations).
    let session_anchor = scope.get();
    if let (Some(anchored), true) = (&session_anchor, drift.pinned) {
        if let Some(current) = anchor::drift_observation(&drift) {
            if !anchored.same_primary_identity(&current) {
                return Ok(pin_changed_refusal(
                    scope,
                    anchored,
                    &current,
                    &drift.issues,
                ));
            }
        }
    }

    // Fail closed on any unhealthy runtime (invalid seal, freeze, expiry, drift).
    if !drift.ok {
        if !drift.pinned {
            let mut body = json!({
                "error": "not_pinned",
                "issues": drift.issues,
                "hint": "Human must run: locus enter <alias> (or `locus pin <alias>`). Agents: locus_enter_hint / locus_request_pin."
            });
            attach_anchor_context(&mut body, session_anchor.as_ref());
            return Ok(tool_text(body, true));
        }
        if !drift.seal_ok {
            return Ok(tool_text(
                json!({
                    "error": "invalid_seal",
                    "issues": drift.issues,
                    "hint": "Session seal is invalid. Human must re-pin: `locus leave` then `locus pin <alias>`.",
                }),
                true,
            ));
        }
        if drift.frozen {
            return Ok(tool_text(
                json!({
                    "error": "session_frozen: re-pin",
                    "reason": drift.issues,
                    "hint": "Binding changed under the active pin. Human must run `locus leave` then `locus pin <alias>`.",
                }),
                true,
            ));
        }
        if drift.expired {
            return Ok(tool_text(
                json!({
                    "error": "session_expired",
                    "issues": drift.issues,
                    "hint": "Pin TTL expired. Human must re-pin: `locus pin <alias>`.",
                }),
                true,
            ));
        }
        return Ok(tool_text(
            json!({
                "error": "runtime_unhealthy",
                "issues": drift.issues,
                "hint": "Identity drift detected. Human: `locus heartbeat` / `locus doctor`, then re-pin if needed. Agents: call locus_heartbeat.",
            }),
            true,
        ));
    }

    let pinned = active_session_bindings().map_err(|e| rpc_error(-32000, e.to_string()))?;
    let Some((session, bindings)) = pinned else {
        let mut body = json!({
            "error": "not_pinned",
            "hint": "Human must run: locus pin <alias>. Agents: call locus_request_pin or locus_enter_hint."
        });
        attach_anchor_context(&mut body, session_anchor.as_ref());
        return Ok(tool_text(body, true));
    };

    if session.is_frozen() {
        return Ok(tool_text(
            json!({
                "error": "session_frozen: re-pin",
                "reason": session.frozen_reason,
                "hint": "Human must re-pin after binding drift."
            }),
            true,
        ));
    }

    // Healthy-path anchor observe against the SAME loaded (session, bindings)
    // handed to the provider gate below (window identical to today's drift
    // window; no store mutation). Establish on first healthy observation;
    // same-identity re-pin (`locus enter <same>`) re-anchors silently;
    // different identity fails closed with `pin_changed`.
    let obs = anchor::observation(&session, &bindings);
    match scope.observe(&obs, true) {
        Some(AnchorDecision::Established) => {
            audit_anchor_event(
                "mcp.anchor_established",
                &obs.binding_alias,
                json!({ "anchor": anchor::identity_json(&obs) }),
            );
        }
        Some(AnchorDecision::Repinned) => {
            audit_anchor_event(
                "mcp.anchor_repin",
                &obs.binding_alias,
                json!({ "anchor": anchor::identity_json(&obs) }),
            );
        }
        Some(AnchorDecision::Mismatch { anchored }) => {
            return Ok(pin_changed_refusal(scope, &anchored, &obs, &[]));
        }
        Some(AnchorDecision::Match) | None => {}
    }

    // Resolve target binding + un-prefixed tool name for namespaced sessions.
    let (binding, tool_name): (&Binding, &str) = if session.is_namespaced() {
        match split_namespaced_tool(name) {
            Some((alias, rest)) => {
                let b = bindings
                    .iter()
                    .find(|(a, _)| a == alias)
                    .map(|(_, b)| b)
                    .ok_or_else(|| {
                        rpc_error(
                            -32602,
                            format!("unknown namespace alias `{alias}` for tool `{name}`"),
                        )
                    })?;
                // Alias must be in this session
                if !session.all_aliases().iter().any(|a| a == alias) {
                    return Ok(tool_text(
                        json!({
                            "error": "namespace_not_in_session",
                            "alias": alias,
                            "tool": name,
                        }),
                        true,
                    ));
                }
                (b, rest)
            }
            None => {
                return Ok(tool_text(
                    json!({
                        "error": "namespaced_tool_required",
                        "detail": "This session is namespaced; call tools as `alias__tool` (e.g. acme__github.scope).",
                        "tool": name,
                        "namespaces": session.all_aliases(),
                    }),
                    true,
                ));
            }
        }
    } else {
        let b = primary_binding(&bindings)
            .ok_or_else(|| rpc_error(-32000, "pinned session has no bindings".into()))?;
        (b, name)
    };

    let principal_owned = session.principal.clone();
    let gate = ApprovalGate {
        store: &s,
        session_id: &session.session_id,
        principal: principal_owned.as_deref(),
    };

    let synthetic = tools_for_binding(binding);
    let is_synthetic = synthetic.iter().any(|t| t.name == tool_name);

    if is_synthetic {
        match call_tool_gated(binding, tool_name, &args, Some(gate)) {
            Ok(r) => {
                let ticket = if r.ok {
                    s.mint_capability_ticket(&session.session_id, &binding.id, tool_name)
                        .ok()
                } else {
                    None
                };
                let ticket_id = ticket.as_ref().map(|t| t.ticket_id.as_str());
                audit_tool_block(&s, &binding.alias, tool_name, &r.content);
                audit_tool_call(
                    &s,
                    &binding.alias,
                    tool_name,
                    &session.session_id,
                    ticket_id,
                    r.ok,
                );
                Ok(tool_text(r.content, !r.ok))
            }
            Err(e) => Ok(tool_text(
                scope_or_err(&s, &binding.alias, tool_name, &args, e),
                true,
            )),
        }
    } else {
        // Upstream (or unknown) tool: policy + worker fan-out.
        match enforce_policy(binding, tool_name, &args, Some(gate)) {
            Ok(Some(blocked)) => {
                audit_tool_block(&s, &binding.alias, tool_name, &blocked.content);
                Ok(tool_text(blocked.content, true))
            }
            Ok(None) => {
                let Some(provider) = tool_name.split('.').next().filter(|p| !p.is_empty()) else {
                    return Ok(tool_text(
                        json!({ "error": "invalid_tool_name", "tool": name }),
                        true,
                    ));
                };
                let mut mgr = worker_manager()
                    .lock()
                    .map_err(|_| rpc_error(-32000, "worker manager lock poisoned".into()))?;
                if let Err(e) = mgr.ensure_provider(&session, binding, provider) {
                    return Ok(tool_text(
                        json!({
                            "error": "worker_ensure_failed",
                            "detail": e.to_string(),
                            "tool": name,
                        }),
                        true,
                    ));
                }
                let ticket = s
                    .mint_capability_ticket(&session.session_id, &binding.id, tool_name)
                    .ok();
                let ticket_id = ticket.as_ref().map(|t| t.ticket_id.as_str());
                match mgr.call_tool(&session, binding, tool_name, &args) {
                    Ok(r) => {
                        audit_tool_call(
                            &s,
                            &binding.alias,
                            tool_name,
                            &session.session_id,
                            ticket_id,
                            r.ok,
                        );
                        Ok(tool_text(r.content, !r.ok))
                    }
                    Err(e) => Ok(tool_text(
                        json!({ "error": e.to_string(), "tool": name }),
                        true,
                    )),
                }
            }
            Err(e) => Ok(tool_text(json!({ "error": e.to_string() }), true)),
        }
    }
}

/// Audit successful tools/call path with optional capability `ticket_id` (not a secret).
fn audit_tool_call(
    s: &Store,
    alias: &str,
    tool: &str,
    session_id: &str,
    ticket_id: Option<&str>,
    ok: bool,
) {
    let _ = s.audit(
        "mcp.tools_call",
        alias,
        Some(json!({
            "tool": tool,
            "session_id": session_id,
            "ticket_id": ticket_id,
            "ok": ok,
        })),
    );
}

fn audit_tool_block(s: &Store, alias: &str, tool: &str, content: &Value) {
    // Never log raw tool args — digests + meta only (secrets stay out of audit).
    if let Some(err) = content.get("error").and_then(|v| v.as_str()) {
        if err == "requires_approval" {
            let _ = s.audit(
                "mcp.require_approval",
                alias,
                Some(json!({
                    "tool": tool,
                    "status": "pending",
                    "approval_id": content.get("approval_id"),
                    "args_digest": content.get("args_digest"),
                    "dual_control": content.get("dual_control"),
                    "grants": content.get("grants"),
                    "required_grants": content.get("required_grants"),
                    "approval_authority": content.get("approval_authority"),
                    "authoritative_path_enabled": content.get("authoritative_path_enabled"),
                    "detail": content.get("detail"),
                })),
            );
        }
    }
}

fn scope_or_err(
    s: &Store,
    alias: &str,
    tool: &str,
    args: &Value,
    e: locus_core::LocusError,
) -> Value {
    let msg = e.to_string();
    if msg.contains("scope freeze") {
        // Digest only — raw args may contain secrets or PII
        let _ = s.audit(
            "mcp.scope_freeze",
            alias,
            Some(json!({
                "tool": tool,
                "error": msg,
                "args_digest": locus_core::args_digest(args),
            })),
        );
    }
    json!({ "error": msg })
}

fn call_control(
    name: &str,
    args: &Value,
    scope: &AnchorScope,
) -> std::result::Result<Value, Value> {
    let s = store().map_err(|e| rpc_error(-32000, e.to_string()))?;
    // Heartbeat: detect drift and freeze when identity control tools are polled.
    if matches!(
        name,
        "locus_whoami"
            | "locus_status"
            | "locus_providers"
            | "locus_heartbeat"
            | "locus_safe_next"
            | "locus_verify_session"
    ) {
        let _ = s.check_drift_and_freeze();
    }
    // Additive `mcp_anchor` block for identity-reporting control tools —
    // omitted entirely when this MCP session has no anchor (keeps unpinned
    // response shapes untouched). (report, current_matches_anchor).
    let anchor_report: Option<(Value, bool)> = if matches!(
        name,
        "locus_whoami"
            | "locus_status"
            | "locus_heartbeat"
            | "locus_safe_next"
            | "locus_verify_session"
    ) {
        scope.get().map(|a| {
            let who = s.whoami().ok();
            let current = current_identity_observation(&s);
            mcp_anchor_report(&a, who.as_ref(), current.as_ref())
        })
    } else {
        None
    };
    let attach_report = |mut body: Value| -> Value {
        if let Some((report, _)) = &anchor_report {
            body["mcp_anchor"] = report.clone();
        }
        body
    };
    // Anchored-identity mismatch while a *different* pin is globally active
    // (not merely unpinned) — drives safe_next / verify_session overrides.
    let anchor_mismatch_active = anchor_report
        .as_ref()
        .map(|(report, matches)| {
            !matches
                && report
                    .get("current_alias")
                    .map(|v| !v.is_null())
                    .unwrap_or(false)
        })
        .unwrap_or(false);
    match name {
        "locus_safe_next" => {
            let next =
                compute_safe_next(&s, &cwd()).map_err(|e| rpc_error(-32000, e.to_string()))?;
            let mut body = attach_report(serde_json::to_value(&next).unwrap_or(json!({})));
            let mut is_err = !next.ready;
            if anchor_mismatch_active {
                // Session-local override: the only safe next action is to
                // re-initialize this client (or restore the anchored pin).
                let anchored_alias = anchor_report
                    .as_ref()
                    .and_then(|(r, _)| r["anchored_alias"].as_str())
                    .unwrap_or("")
                    .to_string();
                body["action"] = json!("reinitialize_client");
                body["ready"] = json!(false);
                body["command"] = json!(format!("locus enter {anchored_alias}"));
                is_err = true;
            }
            // Informational: isError only when not ready so agents notice the gate.
            Ok(tool_text(body, is_err))
        }
        "locus_whoami" => match s.whoami() {
            Ok(w) => Ok(tool_text(
                attach_report(serde_json::to_value(w).unwrap_or(json!({}))),
                false,
            )),
            Err(e) => Ok(tool_text(
                attach_report(json!({
                    "pinned": false,
                    "error": e.to_string(),
                    "hint": "Run `locus pin <alias>` in this workspace. Agents: locus_enter_hint."
                })),
                true,
            )),
        },
        "locus_status" => {
            let active = s
                .active_session()
                .map_err(|e| rpc_error(-32000, e.to_string()))?;
            match active {
                None => Ok(tool_text(
                    attach_report(json!({ "pinned": false, "status": "unpinned" })),
                    false,
                )),
                Some(session) => {
                    let key = s.seal_key().map_err(|e| rpc_error(-32000, e.to_string()))?;
                    let seal_ok = session.verify_seal(&key).is_ok();
                    Ok(tool_text(
                        attach_report(json!({
                            "pinned": true,
                            "binding": session.binding_alias,
                            "tenant": session.tenant,
                            "session_id": session.session_id,
                            "seal_ok": seal_ok,
                            "expired": session.is_expired(),
                            "frozen": session.frozen,
                            "frozen_reason": session.frozen_reason,
                            "mode": if session.is_namespaced() { "namespaced" } else { "exclusive" },
                            "namespaces": session.all_aliases(),
                        })),
                        false,
                    ))
                }
            }
        }
        "locus_heartbeat" => {
            // Doctor-lite: full RuntimeDrift + operator hint. Never secrets.
            let drift = s
                .check_drift_and_freeze()
                .map_err(|e| rpc_error(-32000, e.to_string()))?;
            let hint = if drift.ok {
                None
            } else if !drift.pinned {
                Some("not pinned — human: `locus enter <alias>` (or `locus pin <alias>`)")
            } else if drift.frozen {
                Some("session frozen after drift — human: `locus leave` then `locus pin <alias>`")
            } else if !drift.seal_ok {
                Some("invalid seal — human must re-pin")
            } else if drift.expired {
                Some("pin TTL expired — human must re-pin")
            } else {
                Some("runtime unhealthy — run `locus doctor`")
            };
            let body = attach_report(json!({
                "ok": drift.ok,
                "pinned": drift.pinned,
                "seal_ok": drift.seal_ok,
                "frozen": drift.frozen,
                "expired": drift.expired,
                "binding": drift.binding_alias,
                "tenant": drift.tenant_session,
                "binding_id_match": drift.binding_id_match,
                "tenant_match": drift.tenant_match,
                "providers_match": drift.providers_match,
                "issues": drift.issues,
                "providers": drift.providers,
                "hint": hint,
                "runtime": drift,
            }));
            // Informational probe — isError only when unhealthy so agents notice.
            Ok(tool_text(body, !drift.ok))
        }
        "locus_enter_hint" => {
            let alias = args
                .get("alias")
                .and_then(|a| a.as_str())
                .map(str::trim)
                .filter(|a| !a.is_empty());
            let (enter_cmd, pin_cmd) = match alias {
                Some(a) => (format!("locus enter {a}"), format!("locus pin {a}")),
                None => ("locus enter".into(), "locus pin".into()),
            };
            let exists = alias.map(|a| s.load_binding(a).is_ok());
            if let Some(a) = alias {
                let _ = s.audit(
                    "mcp.enter_hint",
                    a,
                    Some(json!({ "binding_exists": exists })),
                );
            }
            let message = match alias {
                Some(_) => format!(
                    "Agents cannot pin. Ask the human to run `{enter_cmd}` (or `{pin_cmd}`) in a terminal, then continue."
                ),
                None => "Agents cannot pin. Ask the human to run `locus enter <alias>` (or `locus pin <alias>`) in a terminal.".into(),
            };
            Ok(tool_text(
                json!({
                    "agents_cannot_pin": true,
                    "command": enter_cmd,
                    "pin_command": pin_cmd,
                    "alias": alias,
                    "binding_exists": exists,
                    "message": message,
                }),
                false,
            ))
        }
        "locus_list_bindings" => {
            let list = s
                .list_bindings()
                .map_err(|e| rpc_error(-32000, e.to_string()))?;
            Ok(tool_text(
                serde_json::to_value(list).unwrap_or(json!([])),
                false,
            ))
        }
        "locus_request_pin" => {
            let alias = args.get("alias").and_then(|a| a.as_str()).unwrap_or("");
            if alias.is_empty() {
                return Ok(tool_text(json!({ "error": "alias required" }), true));
            }
            // Verify binding exists
            let exists = s.load_binding(alias).is_ok();
            let _ = s.audit("mcp.request_pin", alias, Some(json!({ "exists": exists })));
            Ok(tool_text(
                json!({
                    "requested": alias,
                    "binding_exists": exists,
                    "message": format!(
                        "Pin request recorded for `{alias}`. Agents cannot pin themselves. \
                         Human: run `locus enter {alias}` (or `locus pin {alias}`) in the terminal, then continue."
                    ),
                    "command": format!("locus enter {alias}"),
                    "pin_command": format!("locus pin {alias}"),
                }),
                false,
            ))
        }
        "locus_providers" => match s.whoami() {
            Ok(w) => Ok(tool_text(json!({ "providers": w.providers }), false)),
            Err(e) => Ok(tool_text(json!({ "error": e.to_string() }), true)),
        },
        "locus_verify_claim" => {
            let text = args
                .get("text")
                .or_else(|| args.get("claim"))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|t| !t.is_empty());
            let Some(text) = text else {
                return Ok(tool_text(
                    json!({ "error": "text (or claim) required — free-text assertion to score" }),
                    true,
                ));
            };
            let who = s.whoami().ok();
            let result = verify_claim(text, who.as_ref());
            let binding = who
                .as_ref()
                .map(|w| w.binding_alias.as_str())
                .unwrap_or("-");
            let _ = s.audit(
                "mcp.verify_claim",
                binding,
                Some(json!({
                    "confidence": result.confidence.as_str(),
                    "needs_tool": result.needs_tool,
                    "signals": result.signals,
                    // Truncate claim in audit — never store huge blobs.
                    "claim_len": result.claim.len(),
                    "claim_preview": result.claim.chars().take(120).collect::<String>(),
                })),
            );
            Ok(tool_text(
                serde_json::to_value(&result).unwrap_or(json!({})),
                false,
            ))
        }
        "locus_verify_session" => {
            // Same pack as `locus verify session --json`. Available unpinned.
            // isError only on hard store failures — agents/hub gate on session_ok.
            let external =
                gather_doctor_external(&s, cwd()).map_err(|e| rpc_error(-32000, e.to_string()))?;
            let pack = verify_session(&s, &cwd(), external)
                .map_err(|e| rpc_error(-32000, e.to_string()))?;
            let binding = pack
                .whoami
                .as_ref()
                .map(|w| w.binding_alias.as_str())
                .unwrap_or("-");
            let _ = s.audit(
                "mcp.verify_session",
                binding,
                Some(json!({
                    // Keys aligned with the CLI's `verify.session` audit event.
                    "session_ok": pack.session_ok,
                    "safe_next": pack.safe_next.action,
                    "doctor_verdict": pack.doctor.verdict,
                    "doctor_ok": pack.doctor.ok,
                    "has_whoami": pack.whoami.is_some(),
                })),
            );
            let mut body = attach_report(serde_json::to_value(&pack).unwrap_or(json!({})));
            if anchor_mismatch_active {
                // Hub gating: an anchored-identity mismatch is a per-session
                // failure even when the global pin itself is healthy. The hub
                // gates on session_ok — force it false; isError stays false
                // (the pack itself returned fine).
                let anchored_alias = anchor_report
                    .as_ref()
                    .and_then(|(r, _)| r["anchored_alias"].as_str())
                    .unwrap_or("")
                    .to_string();
                body["session_ok"] = json!(false);
                body["mcp_anchor_mismatch"] = json!(true);
                body["safe_next"]["action"] = json!("reinitialize_client");
                body["safe_next"]["ready"] = json!(false);
                body["safe_next"]["command"] = json!(format!("locus enter {anchored_alias}"));
            }
            Ok(tool_text(body, false))
        }
        other => Ok(tool_text(
            json!({ "error": format!("unknown control tool: {other}") }),
            true,
        )),
    }
}

fn tool_text(value: Value, is_error: bool) -> Value {
    // Compact JSON only — pretty-print embeds raw newlines that break NDJSON stdio.
    let text = match &value {
        Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_else(|_| other.to_string()),
    };
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error
    })
}

#[cfg(test)]
mod http_read_tests {
    use super::*;
    use std::io::Cursor;

    #[allow(clippy::type_complexity)]
    fn parse(
        raw: &[u8],
    ) -> std::result::Result<(String, String, Vec<(String, String)>, Vec<u8>), HttpReadError> {
        let mut reader = Cursor::new(raw.to_vec());
        read_http_request(&mut reader)
    }

    #[test]
    fn parses_simple_post_with_body() {
        let raw = b"POST /mcp HTTP/1.1\r\nHost: x\r\nContent-Length: 4\r\n\r\nabcd";
        let (method, path, headers, body) = parse(raw).expect("parse");
        assert_eq!(method, "POST");
        assert_eq!(path, "/mcp");
        assert!(headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("host") && v == "x"));
        assert_eq!(body, b"abcd");
    }

    #[test]
    fn oversized_content_length_rejected_before_allocation() {
        let raw = format!(
            "POST /mcp HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            HTTP_MAX_BODY_BYTES + 1
        );
        match parse(raw.as_bytes()) {
            Err(HttpReadError::PayloadTooLarge { content_length }) => {
                assert_eq!(content_length, HTTP_MAX_BODY_BYTES + 1);
            }
            other => panic!("expected PayloadTooLarge, got {other:?}"),
        }
        // At the cap exactly: no 413 (body read may still hit EOF -> Fatal).
        let raw = format!("POST /mcp HTTP/1.1\r\nContent-Length: {HTTP_MAX_BODY_BYTES}\r\n\r\n");
        assert!(matches!(
            parse(raw.as_bytes()),
            Err(HttpReadError::Fatal(_))
        ));
    }

    #[test]
    fn too_many_header_fields_rejected() {
        let mut raw = String::from("GET /mcp HTTP/1.1\r\n");
        for i in 0..(HTTP_MAX_HEADER_COUNT + 1) {
            raw.push_str(&format!("X-H-{i}: v\r\n"));
        }
        raw.push_str("\r\n");
        assert!(matches!(
            parse(raw.as_bytes()),
            Err(HttpReadError::HeadersTooLarge)
        ));
    }

    #[test]
    fn oversized_header_bytes_rejected() {
        // One giant header line without ever reaching the blank line.
        let mut raw = String::from("GET /mcp HTTP/1.1\r\n");
        raw.push_str("X-Big: ");
        raw.push_str(&"a".repeat(HTTP_MAX_HEADER_BYTES + 1024));
        raw.push_str("\r\n\r\n");
        assert!(matches!(
            parse(raw.as_bytes()),
            Err(HttpReadError::HeadersTooLarge)
        ));
    }

    #[test]
    fn giant_request_line_rejected() {
        let mut raw = String::from("GET /");
        raw.push_str(&"a".repeat(HTTP_MAX_HEADER_BYTES + 1024));
        raw.push_str(" HTTP/1.1\r\n\r\n");
        assert!(matches!(
            parse(raw.as_bytes()),
            Err(HttpReadError::HeadersTooLarge)
        ));
    }
}

#[cfg(test)]
mod anchor_health_tests {
    use super::*;
    use crate::anchor::NamespaceAnchor;

    fn sample(alias: &str, id: &str, tenant: &str, mode: &str) -> SessionAnchor {
        SessionAnchor {
            binding_id: id.into(),
            binding_alias: alias.into(),
            tenant: tenant.into(),
            mode: mode.into(),
            namespaces: Vec::new(),
            session_id: "sess".into(),
            backing: None,
            anchored_at_unix: 1,
        }
    }

    /// Regression: health surfaces must use the SAME identity comparison as
    /// the provider gate — a mode/namespace change with identical primary
    /// identity refuses provider tools, so it must read as a mismatch here
    /// too (session_ok=false), not as healthy via primary-only matching.
    #[test]
    fn full_observation_trips_on_mode_and_namespace_changes() {
        let anchored = sample("acme", "bnd_acme", "acme-corp", "exclusive");

        // Same primary identity, different mode → provider gate refuses →
        // health must mismatch.
        let mode_changed = sample("acme", "bnd_acme", "acme-corp", "namespaced");
        assert!(anchored.same_primary_identity(&mode_changed));
        assert!(!anchor_matches_current(&anchored, Some(&mode_changed)));

        // Identical full identity matches.
        let same = sample("acme", "bnd_acme", "acme-corp", "exclusive");
        assert!(anchor_matches_current(&anchored, Some(&same)));

        // Namespace set change with identical primary identity mismatches.
        let mut ns_anchor = sample("acme", "bnd_acme", "acme-corp", "namespaced");
        ns_anchor.namespaces = vec![NamespaceAnchor {
            alias: "alpha".into(),
            binding_id: "bnd_alpha".into(),
            tenant: "alpha-corp".into(),
        }];
        let mut ns_changed = ns_anchor.clone();
        ns_changed.namespaces[0].tenant = "other-corp".into();
        assert!(ns_anchor.same_primary_identity(&ns_changed));
        assert!(!anchor_matches_current(&ns_anchor, Some(&ns_changed)));
        assert!(anchor_matches_current(&ns_anchor, Some(&ns_anchor.clone())));
    }

    /// Primary-only comparison applies only when just the drift identity
    /// exists (empty mode); no observation never matches (fail closed).
    #[test]
    fn drift_only_observation_compares_primary_and_none_never_matches() {
        let anchored = sample("acme", "bnd_acme", "acme-corp", "exclusive");

        let drift_same = sample("acme", "bnd_acme", "acme-corp", "");
        assert!(anchor_matches_current(&anchored, Some(&drift_same)));

        let drift_other = sample("beta", "bnd_beta", "beta-corp", "");
        assert!(!anchor_matches_current(&anchored, Some(&drift_other)));

        assert!(!anchor_matches_current(&anchored, None));
    }

    /// mcp_anchor_report carries the gate-equivalent verdict in `match`.
    #[test]
    fn anchor_report_match_follows_gate_comparison() {
        let anchored = sample("acme", "bnd_acme", "acme-corp", "exclusive");
        let mode_changed = sample("acme", "bnd_acme", "acme-corp", "namespaced");
        let (report, matches) = mcp_anchor_report(&anchored, None, Some(&mode_changed));
        assert!(!matches);
        assert_eq!(report["match"], serde_json::json!(false));

        let same = sample("acme", "bnd_acme", "acme-corp", "exclusive");
        let (report, matches) = mcp_anchor_report(&anchored, None, Some(&same));
        assert!(matches);
        assert_eq!(report["match"], serde_json::json!(true));
    }
}

#[cfg(test)]
mod http_session_tests {
    use super::*;
    use std::time::SystemTime;

    #[test]
    fn resolve_mints_when_missing_and_reuses_header() {
        let mut map = HttpSessionMap::new(Duration::from_secs(60), 8);
        let headers: Vec<(String, String)> = vec![];
        let minted = resolve_mcp_http_session(&mut map, &headers, true)
            .expect("mint")
            .expect("id");
        let with_id = vec![("Mcp-Session-Id".into(), minted.clone())];
        let reused = resolve_mcp_http_session(&mut map, &with_id, true)
            .expect("reuse")
            .expect("id");
        assert_eq!(minted, reused);
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn resolve_unknown_and_invalid_fail_closed() {
        let mut map = HttpSessionMap::new(Duration::from_secs(60), 8);
        let unknown = vec![(
            "mcp-session-id".into(),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        )];
        assert_eq!(
            resolve_mcp_http_session(&mut map, &unknown, true),
            Err(HttpSessionError::Unknown)
        );
        let empty = vec![("Mcp-Session-Id".into(), "   ".into())];
        assert_eq!(
            resolve_mcp_http_session(&mut map, &empty, true),
            Err(HttpSessionError::Invalid)
        );
        assert_eq!(resolve_mcp_http_session(&mut map, &[], false), Ok(None));
    }

    #[test]
    fn session_error_status_codes() {
        assert_eq!(session_error_body(&HttpSessionError::Unknown).0, 404);
        assert_eq!(session_error_body(&HttpSessionError::Invalid).0, 400);
        assert_eq!(session_error_body(&HttpSessionError::Capacity).0, 503);
    }

    #[test]
    fn resolve_resumes_from_disk_after_memory_drop() {
        let dir = tempfile::tempdir().unwrap();
        let mut map = HttpSessionMap::new(Duration::from_secs(60), 8)
            .with_persist_dir(Some(dir.path().to_path_buf()));
        let minted = resolve_mcp_http_session(&mut map, &[], true)
            .unwrap()
            .unwrap();
        map.clear_memory();
        let with_id = vec![("Mcp-Session-Id".into(), minted.clone())];
        let resumed = resolve_mcp_http_session(&mut map, &with_id, true)
            .expect("resume")
            .expect("id");
        assert_eq!(minted, resumed);
    }

    #[test]
    fn resolve_expired_disk_session_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let mut map = HttpSessionMap::new(Duration::from_millis(50), 8)
            .with_persist_dir(Some(dir.path().to_path_buf()));
        let id = map.mint().unwrap();
        map.insert_for_test(&id, SystemTime::now() - Duration::from_secs(10));
        map.clear_memory();
        let with_id = vec![("Mcp-Session-Id".into(), id)];
        assert_eq!(
            resolve_mcp_http_session(&mut map, &with_id, true),
            Err(HttpSessionError::Unknown)
        );
    }
}

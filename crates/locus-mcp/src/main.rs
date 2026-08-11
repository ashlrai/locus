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
//!   GET `/health`; requires `LOCUS_MCP_HTTP_TOKEN`. `Mcp-Session-Id` in-memory
//!   sessions (TTL + max N). Single-event SSE when `Accept: text/event-stream`
//!   only (no multi-message stream rewrite).

use anyhow::{bail, Context, Result};
use locus_core::{
    build_doctor_report, call_tool_gated, compute_safe_next, control_tools, enforce_policy,
    find_workspace, load_config, split_namespaced_tool, tools_for_binding, verify_claim,
    AdapterTool, ApprovalGate, Binding, DoctorExternal, Session, Store, VERSION,
};
use rand::RngCore;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

/// Process-wide worker manager (synthetic + per-provider upstream MCP).
fn worker_manager() -> &'static Mutex<CompositeWorkerManagerGuard> {
    static MGR: OnceLock<Mutex<CompositeWorkerManagerGuard>> = OnceLock::new();
    MGR.get_or_init(|| Mutex::new(CompositeWorkerManagerGuard::new()))
}

/// Thin alias so we can keep the type local without re-export noise.
type CompositeWorkerManagerGuard = locus_core::CompositeWorkerManager;

/// MCP auto-pin attempted once per process (start / first tools/list).
static AUTO_PIN_ATTEMPTED: AtomicBool = AtomicBool::new(false);

/// Default HTTP bind when `--http` / `LOCUS_MCP_HTTP=1` without an address.
const DEFAULT_HTTP_ADDR: &str = "127.0.0.1:8742";

/// In-memory MCP HTTP session TTL (idle). Clients must re-initialize after expiry.
const HTTP_SESSION_TTL: Duration = Duration::from_secs(30 * 60);
/// Cap concurrent opaque `Mcp-Session-Id` entries (process-local; no redis).
const HTTP_SESSION_MAX: usize = 256;

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
           POST /mcp                 JSON-RPC 2.0 (token; Accept: application/json\n\
                                     and/or text/event-stream; mints/binds Mcp-Session-Id)\n\
           DELETE /mcp               terminate Mcp-Session-Id (token)\n\n\
         Env:\n\
           LOCUS_MCP_HTTP=1            enable HTTP (same as --http)\n\
           LOCUS_MCP_HTTP_ADDR         bind address when HTTP enabled\n\
           LOCUS_MCP_HTTP_TOKEN        required bearer/token for HTTP auth\n\
           LOCUS_MCP_HTTP_ALLOW_REMOTE=1  allow non-loopback bind (default: loopback only)\n\
           LOCUS_HOME                  store root (pin + bindings for remote process)\n\
           LOCUS_WORKER_IDLE_SECS      optional idle teardown for upstream workers\n\
           LOCUS_WORKER_SANDBOX=1      require supported OS isolation or fail closed\n           LOCUS_WORKER_SANDBOX_NO_NETWORK=1  opt-in deny network (bwrap/Seatbelt)\n"
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

        if let Some(response) = dispatch_rpc(&msg) {
            write_message(&mut stdout, &response, framing)?;
        }
    }
    Ok(())
}

/// Dispatch one JSON-RPC request/notification.
/// Returns `None` for notifications (no response).
fn dispatch_rpc(msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(json!({}));

    if id.is_none() {
        handle_notification(method, &params);
        return None;
    }

    let result = match method {
        "initialize" => Ok(handle_initialize(&params)),
        "ping" => Ok(json!({})),
        "tools/list" => handle_tools_list(),
        "tools/call" => handle_tools_call(&params),
        "resources/list" => handle_resources_list(),
        "resources/read" => handle_resources_read(&params),
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

// ─── HTTP transport (Streamable-HTTP-lite: POST /mcp + GET /mcp + /health) ──

/// How the client wants the MCP response encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HttpResponseMode {
    /// Single JSON object (`Content-Type: application/json`) — default / preferred.
    Json,
    /// One SSE event carrying the JSON-RPC body, then stream end (no multi-message bus).
    SseSingle,
}

/// Process-local opaque MCP HTTP sessions (`Mcp-Session-Id`). No redis / disk.
#[derive(Debug)]
struct HttpSessionEntry {
    last_seen: Instant,
}

/// In-memory map of streamable-HTTP session ids with idle TTL + hard capacity.
#[derive(Debug)]
struct HttpSessionMap {
    sessions: HashMap<String, HttpSessionEntry>,
    ttl: Duration,
    max: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HttpSessionError {
    /// Client sent a non-empty id that is unknown or past TTL (fail closed).
    Unknown,
    /// Client sent an empty / whitespace-only id.
    Invalid,
    /// Map is at capacity after purge (mint refused).
    Capacity,
}

impl HttpSessionMap {
    fn new(ttl: Duration, max: usize) -> Self {
        Self {
            sessions: HashMap::new(),
            ttl,
            max: max.max(1),
        }
    }

    fn purge_expired(&mut self, now: Instant) {
        let ttl = self.ttl;
        self.sessions
            .retain(|_, e| now.duration_since(e.last_seen) < ttl);
    }

    fn mint(&mut self) -> Result<String, HttpSessionError> {
        let now = Instant::now();
        self.purge_expired(now);
        if self.sessions.len() >= self.max {
            return Err(HttpSessionError::Capacity);
        }
        // Opaque 128-bit id (hex). Collision retry is extremely unlikely.
        for _ in 0..8 {
            let mut bytes = [0u8; 16];
            rand::thread_rng().fill_bytes(&mut bytes);
            let id = hex::encode(bytes);
            if self.sessions.contains_key(&id) {
                continue;
            }
            self.sessions
                .insert(id.clone(), HttpSessionEntry { last_seen: now });
            return Ok(id);
        }
        Err(HttpSessionError::Capacity)
    }

    /// Touch an existing non-expired session. Returns false if unknown/expired.
    fn touch(&mut self, id: &str) -> bool {
        let now = Instant::now();
        self.purge_expired(now);
        if let Some(entry) = self.sessions.get_mut(id) {
            entry.last_seen = now;
            true
        } else {
            false
        }
    }

    fn remove(&mut self, id: &str) -> bool {
        self.sessions.remove(id).is_some()
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Test helper: insert with an explicit last_seen (for TTL).
    #[cfg(test)]
    fn insert_for_test(&mut self, id: impl Into<String>, last_seen: Instant) {
        self.sessions
            .insert(id.into(), HttpSessionEntry { last_seen });
    }
}

fn http_session_map() -> &'static Mutex<HttpSessionMap> {
    static MAP: OnceLock<Mutex<HttpSessionMap>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HttpSessionMap::new(HTTP_SESSION_TTL, HTTP_SESSION_MAX)))
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
                "hint": "Mcp-Session-Id not found or expired; POST initialize (or first POST /mcp) without the header to mint a new session",
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
        "locus-mcp: HTTP listening on http://{addr}  (GET|POST|DELETE /mcp, GET /health)  token auth + Mcp-Session-Id"
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
    let (method, path, headers, body) = read_http_request(&mut reader)?;

    let path_only = path.split('?').next().unwrap_or(path.as_str());
    let response_mode = http_response_mode(&headers);
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
                "rpc": "POST /mcp (token, JSON-RPC 2.0)",
                "session": "Mcp-Session-Id header (mint on initialize/first POST)"
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
    if let Some(accept) = header_value(&headers, "accept") {
        if !http_accept_allows_mcp(accept) {
            let body = json!({
                "error": "not_acceptable",
                "hint": "Accept must allow application/json and/or text/event-stream (MCP streamable HTTP)",
            });
            return write_http_json(&mut stream, 406, &body, None);
        }
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
        let caps = http_mcp_capabilities();
        return write_http_mcp_body(
            &mut stream,
            200,
            &caps,
            response_mode,
            session_id.as_deref(),
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

        // Mint on initialize / first POST when header absent; bind when present.
        let session_id = {
            let mut map = http_session_map().lock().unwrap_or_else(|e| e.into_inner());
            match resolve_mcp_http_session(&mut map, &headers, true) {
                Ok(Some(id)) => id,
                Ok(None) => unreachable!("mint_if_missing always yields an id or error"),
                Err(err) => {
                    let (status, body) = session_error_body(&err);
                    return write_http_json(&mut stream, status, &body, None);
                }
            }
        };

        let msg: Value = serde_json::from_slice(&body).context("json-rpc body")?;
        match dispatch_rpc(&msg) {
            Some(response) => write_http_mcp_body(
                &mut stream,
                200,
                &response,
                response_mode,
                Some(session_id.as_str()),
            ),
            None => {
                // Notification — 202 Accepted, empty JSON object (still respect Accept).
                write_http_mcp_body(
                    &mut stream,
                    202,
                    &json!({}),
                    response_mode,
                    Some(session_id.as_str()),
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
            "hint": "GET /health · GET /mcp (capabilities) · POST /mcp (JSON-RPC 2.0) · DELETE /mcp (session)",
        });
        write_http_json(&mut stream, 404, &body, None)
    }
}

/// Values-free GET /mcp body: pin summary + tool names + advertised capabilities.
fn http_mcp_capabilities() -> Value {
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
    // descriptions, or secret-bearing fields.
    let tool_names: Vec<String> = match handle_tools_list() {
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

    json!({
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
            "rpc": "POST /mcp",
            "session_delete": "DELETE /mcp"
        },
        "content_types": {
            "request": ["application/json"],
            "response": ["application/json", "text/event-stream"]
        },
        "streamable": {
            "mode": "json-preferred",
            "sse": "single-event-when-accept-sse-only",
            "session": {
                "header": "Mcp-Session-Id",
                "ttl_seconds": HTTP_SESSION_TTL.as_secs(),
                "max_sessions": HTTP_SESSION_MAX,
                "mint": "initialize or first POST /mcp without header",
                "storage": "in-memory-process-local"
            },
            "note": "Mcp-Session-Id in-memory sessions landed; full multi-message SSE / cross-process resume still open. Long tool results return as one JSON object or one SSE data event"
        }
    })
}

fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
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

/// Prefer JSON when the client lists it (or omits Accept). SSE only when the
/// client accepts event-stream and does **not** list application/json.
fn http_response_mode(headers: &[(String, String)]) -> HttpResponseMode {
    let Some(accept) = header_value(headers, "accept") else {
        return HttpResponseMode::Json;
    };
    let lower = accept.to_ascii_lowercase();
    let wants_json = lower.contains("application/json") || lower.contains("*/*");
    let wants_sse = lower.contains("text/event-stream");
    if wants_sse && !wants_json {
        HttpResponseMode::SseSingle
    } else {
        HttpResponseMode::Json
    }
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

#[allow(clippy::type_complexity)]
fn read_http_request<R: BufRead>(
    reader: &mut R,
) -> Result<(String, String, Vec<(String, String)>, Vec<u8>)> {
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .context("read request line")?;
    if request_line.is_empty() {
        bail!("empty HTTP request");
    }
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        bail!("malformed request line: {request_line:?}");
    }
    let method = parts[0].to_string();
    let path = parts[1].to_string();

    let mut headers = Vec::new();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).context("read header")?;
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
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

    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body).context("read HTTP body")?;
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

/// Write an MCP HTTP body as `application/json` or a single SSE `data:` event.
fn write_http_mcp_body(
    stream: &mut TcpStream,
    status: u16,
    body: &Value,
    mode: HttpResponseMode,
    session_id: Option<&str>,
) -> Result<()> {
    match mode {
        HttpResponseMode::Json => write_http_json_with_session(stream, status, body, session_id),
        HttpResponseMode::SseSingle => {
            let json_bytes = serde_json::to_vec(body)?;
            // One event, then end stream. Clients that only Accept text/event-stream
            // still get a complete JSON-RPC payload without a multi-message bus.
            let mut sse = Vec::with_capacity(json_bytes.len() + 32);
            sse.extend_from_slice(b"event: message\ndata: ");
            sse.extend_from_slice(&json_bytes);
            sse.extend_from_slice(b"\n\n");
            let mut extra: Vec<(&str, &str)> = vec![
                ("Cache-Control", "no-cache"),
                ("X-Locus-Streamable", "sse-single"),
            ];
            if let Some(id) = session_id {
                extra.push(("Mcp-Session-Id", id));
            }
            write_http_response(stream, status, "text/event-stream", &sse, Some(extra))
        }
    }
}

fn write_http_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
    extra_headers: Option<Vec<(&str, &str)>>,
) -> Result<()> {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        406 => "Not Acceptable",
        415 => "Unsupported Media Type",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "OK",
    };
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
            // No server-side state required beyond optional auto-pin on initialize.
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

/// Crisp agent rules for `initialize.instructions` — pin state is live after auto-pin.
fn agent_instructions() -> String {
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

    [
        "Locus identity plane — tools are hard-scoped to the active sealed pin.".into(),
        pin_line,
        "• ALWAYS call locus_whoami or locus_safe_next (or read locus://session) before infrastructure mutations when context is unclear.".into(),
        "• You CANNOT pin or switch tenants. Use locus_request_pin / locus_enter_hint; surface the command so a human runs `locus pin <alias>` / `locus enter <alias>`.".into(),
        "• When stuck, unpinned, approval-blocked, or doctor-unhealthy: call locus_safe_next — it returns the single best next action.".into(),
        "• Frozen scopes (project_ref, team_id, orgs/repos) cannot be overridden; scope freeze on mismatch is expected and correct.".into(),
        "• Resources always reflect current pin: locus://session, locus://doctor, locus://bindings. Prompt: locus_context. Re-read after pin changes.".into(),
        "• Never invent alternate project_ref/team/org. Never claim you re-pinned. Never log or request raw secrets.".into(),
    ]
    .join("\n")
}

fn handle_initialize(_params: &Value) -> Value {
    // Prefer auto-pin once at MCP start when workspace has default_binding / require_pin
    // or when explicitly enabled (see maybe_mcp_auto_pin). Instructions then include pin state.
    let _ = maybe_mcp_auto_pin();

    // listChanged=true: catalog may change after auto-pin / human pin/leave; clients should re-list.
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
        "instructions": agent_instructions()
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

/// Whether MCP may silently pin from workspace/cwd.
///
/// Enabled when:
/// - `LOCUS_MCP_AUTO_PIN=1` (explicit), or
/// - `LOCUS_AUTO_PIN=cwd` / `clients.auto_pin=cwd`, or
/// - workspace `.locus.toml` has `require_pin = true`, or
/// - workspace has `default_binding` (preferred default: pin once at MCP start)
///
/// Disabled when `LOCUS_MCP_AUTO_PIN=0|false|off`.
///
/// Actual pin still requires `require_pin` or `default_binding` and never uses force.
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

/// Attempt silent workspace auto-pin when unpinned and policy allows.
///
/// - Only when unpinned
/// - Only when workspace has `require_pin` or non-empty `default_binding`
/// - Never force past allowlist (`pin_auto` never uses force for autopin sources)
/// - Audits `session.auto_pin` (in addition to normal `session.pin`)
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

    match s.pin_auto_delegated(&cwd, Some("mcp-auto".into()), false) {
        Ok(session) => {
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
            // Soft: leave unpinned; agents still see control tools + request_pin.
            eprintln!("locus-mcp: auto-pin failed (staying unpinned): {e}");
            None
        }
    }
}

// ─── Resources ──────────────────────────────────────────────────────────────

const RESOURCE_SESSION: &str = "locus://session";
const RESOURCE_DOCTOR: &str = "locus://doctor";
const RESOURCE_BINDINGS: &str = "locus://bindings";

/// Live pin tag for resource/prompt descriptions (after optional auto-pin).
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
    // Stay in sync with auto-pin: attempt once before describing resources.
    let _ = maybe_mcp_auto_pin();
    let pin = pin_label_for_catalog();
    Ok(json!({
        "resources": [
            {
                "uri": RESOURCE_SESSION,
                "name": "session",
                "title": "Active Locus pin (whoami)",
                "description": format!(
                    "[{pin}] Current pin whoami JSON: tenant, binding, providers, frozen scopes. Live after auto-pin. Never includes secrets."
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

fn handle_resources_read(params: &Value) -> std::result::Result<Value, Value> {
    // Re-sync with pin after auto-pin (or if initialize was skipped).
    let _ = maybe_mcp_auto_pin();
    let uri = params
        .get("uri")
        .and_then(|u| u.as_str())
        .ok_or_else(|| rpc_error(-32602, "missing resource uri".into()))?;

    let body = match uri {
        RESOURCE_SESSION => resource_session_json()?,
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

fn resource_session_json() -> std::result::Result<Value, Value> {
    let s = store().map_err(|e| rpc_error(-32000, e.to_string()))?;
    let _ = s.check_drift_and_freeze();
    match s.whoami() {
        Ok(w) => Ok(serde_json::to_value(w).unwrap_or(json!({}))),
        Err(_) => Ok(json!({
            "pinned": false,
            "hint": "No active pin. Human: `locus pin <alias>` or `locus enter <alias>`. Agents: locus_request_pin / locus_enter_hint."
        })),
    }
}

fn resource_doctor_json() -> std::result::Result<Value, Value> {
    let s = store().map_err(|e| rpc_error(-32000, e.to_string()))?;
    // Doctor lite: full structured report with empty external facts (no PATH probe).
    // Never secrets.
    let report = build_doctor_report(
        &s,
        DoctorExternal {
            phantom_on_path: false,
            unresolved_phm: Vec::new(),
            cwd: Some(cwd()),
        },
    )
    .map_err(|e| rpc_error(-32000, e.to_string()))?;
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
fn handle_tools_list() -> std::result::Result<Value, Value> {
    // Silent cwd auto-pin when still unpinned (once per process; no-ops after initialize).
    let _ = maybe_mcp_auto_pin();

    let s = store().map_err(|e| rpc_error(-32000, e.to_string()))?;
    // Heartbeat on every tools/list: freeze session if binding material drifted.
    let drift = s
        .check_drift_and_freeze()
        .map_err(|e| rpc_error(-32000, e.to_string()))?;

    // Control tools always. `locus_providers` when a pin exists (even frozen).
    let mut tools: Vec<AdapterTool> = control_tools(drift.pinned);
    let pin_alias = drift.binding_alias.clone();

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

fn handle_tools_call(params: &Value) -> std::result::Result<Value, Value> {
    let name = params
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| rpc_error(-32602, "missing tool name".into()))?;
    let args = params.get("arguments").cloned().unwrap_or(json!({}));

    // Control tools (allowed even when frozen — whoami/status/heartbeat report freeze)
    if name.starts_with("locus_") {
        return call_control(name, &args);
    }

    // Provider tools require a healthy pin
    let s = store().map_err(|e| rpc_error(-32000, e.to_string()))?;

    // Continuous drift check — freezes session if binding file mutated.
    let drift = s
        .check_drift_and_freeze()
        .map_err(|e| rpc_error(-32000, e.to_string()))?;

    // Fail closed on any unhealthy runtime (invalid seal, freeze, expiry, drift).
    if !drift.ok {
        if !drift.pinned {
            return Ok(tool_text(
                json!({
                    "error": "not_pinned",
                    "issues": drift.issues,
                    "hint": "Human must run: locus enter <alias> (or `locus pin <alias>`). Agents: locus_enter_hint / locus_request_pin."
                }),
                true,
            ));
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
        return Ok(tool_text(
            json!({
                "error": "not_pinned",
                "hint": "Human must run: locus pin <alias>. Agents: call locus_request_pin or locus_enter_hint."
            }),
            true,
        ));
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

fn call_control(name: &str, args: &Value) -> std::result::Result<Value, Value> {
    let s = store().map_err(|e| rpc_error(-32000, e.to_string()))?;
    // Heartbeat: detect drift and freeze when identity control tools are polled.
    if matches!(
        name,
        "locus_whoami" | "locus_status" | "locus_providers" | "locus_heartbeat" | "locus_safe_next"
    ) {
        let _ = s.check_drift_and_freeze();
    }
    match name {
        "locus_safe_next" => {
            let next =
                compute_safe_next(&s, &cwd()).map_err(|e| rpc_error(-32000, e.to_string()))?;
            // Informational: isError only when not ready so agents notice the gate.
            Ok(tool_text(
                serde_json::to_value(&next).unwrap_or(json!({})),
                !next.ready,
            ))
        }
        "locus_whoami" => match s.whoami() {
            Ok(w) => Ok(tool_text(
                serde_json::to_value(w).unwrap_or(json!({})),
                false,
            )),
            Err(e) => Ok(tool_text(
                json!({
                    "pinned": false,
                    "error": e.to_string(),
                    "hint": "Run `locus pin <alias>` in this workspace. Agents: locus_enter_hint."
                }),
                true,
            )),
        },
        "locus_status" => {
            let active = s
                .active_session()
                .map_err(|e| rpc_error(-32000, e.to_string()))?;
            match active {
                None => Ok(tool_text(
                    json!({ "pinned": false, "status": "unpinned" }),
                    false,
                )),
                Some(session) => {
                    let key = s.seal_key().map_err(|e| rpc_error(-32000, e.to_string()))?;
                    let seal_ok = session.verify_seal(&key).is_ok();
                    Ok(tool_text(
                        json!({
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
                        }),
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
            let body = json!({
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
            });
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
mod http_session_tests {
    use super::*;

    #[test]
    fn mint_issues_opaque_hex_id() {
        let mut map = HttpSessionMap::new(Duration::from_secs(60), 8);
        let id = map.mint().expect("mint");
        assert_eq!(id.len(), 32, "16-byte hex id expected, got {id}");
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()), "{id}");
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn touch_reuses_and_unknown_rejects() {
        let mut map = HttpSessionMap::new(Duration::from_secs(60), 8);
        let id = map.mint().unwrap();
        assert!(map.touch(&id), "fresh id must touch");
        assert!(map.touch(&id), "second touch must succeed");
        assert!(!map.touch("deadbeefdeadbeefdeadbeefdeadbeef"));
        assert!(!map.touch("not-a-real-session"));
    }

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
        // Missing without mint is ok (GET capabilities).
        assert_eq!(resolve_mcp_http_session(&mut map, &[], false), Ok(None));
    }

    #[test]
    fn capacity_blocks_new_mints() {
        let mut map = HttpSessionMap::new(Duration::from_secs(60), 2);
        assert!(map.mint().is_ok());
        assert!(map.mint().is_ok());
        assert_eq!(map.mint(), Err(HttpSessionError::Capacity));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn ttl_expiry_purges_and_rejects() {
        let mut map = HttpSessionMap::new(Duration::from_millis(50), 8);
        let id = map.mint().unwrap();
        // Force last_seen into the past beyond TTL.
        map.insert_for_test(&id, Instant::now() - Duration::from_secs(10));
        assert!(!map.touch(&id), "expired session must not touch");
        assert_eq!(map.len(), 0, "purge should drop expired entry");
        // Fresh mint after expiry works.
        let id2 = map.mint().unwrap();
        assert!(map.touch(&id2));
    }

    #[test]
    fn remove_terminates_session() {
        let mut map = HttpSessionMap::new(Duration::from_secs(60), 8);
        let id = map.mint().unwrap();
        assert!(map.remove(&id));
        assert!(!map.touch(&id));
        assert!(!map.remove(&id));
    }

    #[test]
    fn session_error_status_codes() {
        assert_eq!(session_error_body(&HttpSessionError::Unknown).0, 404);
        assert_eq!(session_error_body(&HttpSessionError::Invalid).0, 400);
        assert_eq!(session_error_body(&HttpSessionError::Capacity).0, 503);
    }
}

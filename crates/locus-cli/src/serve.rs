//! Local identity dashboard HTTP server (`locus serve` / `locus dashboard`).
//!
//! - Binds **127.0.0.1 only**
//! - Never resolves or returns secret values (CredentialRefs / digests only)
//! - Optional `LOCUS_DASHBOARD_TOKEN` / `--token` bearer gate

use anyhow::{bail, Context, Result};
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use locus_core::{
    build_doctor_report, external_approval_authority_enabled, filter_audit_events, find_workspace,
    parse_ttl, phantom_on_path, DoctorExternal, Store, VERSION,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use tower_http::set_header::SetResponseHeaderLayer;

/// Embedded dashboard UI (built from apps/dashboard/public/index.html).
const DASHBOARD_HTML: &str = include_str!("../../../apps/dashboard/public/index.html");

/// Default listen port for `locus serve` / `locus dashboard`.
pub const DEFAULT_PORT: u16 = 8750;

#[derive(Clone)]
struct AppState {
    /// When set, every `/api/*` request must present this token.
    token: Option<Arc<str>>,
    /// Override store home (tests). Production uses `Store::open_default()`.
    home: Option<PathBuf>,
}

/// Run the local dashboard HTTP server (blocks until Ctrl-C / fatal error).
pub async fn run_serve(port: u16, token: Option<String>, open_browser: bool) -> Result<()> {
    if port == 0 {
        bail!("port must be non-zero");
    }

    let state = AppState {
        token: token
            .filter(|t| !t.trim().is_empty())
            .map(|t| Arc::from(t.trim())),
        home: None,
    };

    let app = build_router(state.clone());

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("bind {addr}"))?;
    let bound = listener.local_addr().unwrap_or(addr);

    let url = format!("http://127.0.0.1:{}", bound.port());
    let url_with_token = match state.token.as_ref() {
        Some(t) => format!("{url}/?token={t}"),
        None => url.clone(),
    };

    eprintln!("locus serve  {url}  (loopback only)");
    if state.token.is_some() {
        eprintln!("  auth      LOCUS_DASHBOARD_TOKEN required (Bearer / X-Locus-Token / ?token=)");
    } else {
        eprintln!("  auth      none (set LOCUS_DASHBOARD_TOKEN to require a shared secret)");
    }
    eprintln!("  ui        {url}/");
    eprintln!("  api       {url}/api/status · whoami · bindings · approvals · doctor · events");
    eprintln!("  stop      Ctrl-C");

    if open_browser {
        let open_url = if state.token.is_some() {
            url_with_token.as_str()
        } else {
            url.as_str()
        };
        if let Err(e) = try_open_browser(open_url) {
            eprintln!("  browser   open failed: {e:#} — visit {url} manually");
        } else {
            eprintln!("  browser   opened {url}");
        }
    }

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("dashboard server")?;
    eprintln!("locus serve  stopped");
    Ok(())
}

fn build_router(state: AppState) -> Router {
    let api = Router::new()
        .route("/status", get(api_status))
        .route("/whoami", get(api_whoami))
        .route("/bindings", get(api_bindings))
        .route("/approvals", get(api_approvals))
        .route("/doctor", get(api_doctor))
        .route("/events", get(api_events))
        .route("/approve/{id}/grant", post(api_grant))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    Router::new()
        .route("/", get(index))
        .route("/index.html", get(index))
        .route("/api/health", get(api_health))
        .nest("/api", api)
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .with_state(state)
}

async fn index() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        Html(DASHBOARD_HTML),
    )
}

async fn api_health() -> Json<Value> {
    Json(json!({
        "ok": true,
        "service": "locus-dashboard",
        "version": VERSION,
        "bind": "127.0.0.1",
    }))
}

async fn auth_middleware(
    State(state): State<AppState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if let Some(ref expected) = state.token {
        if !token_matches(expected, req.headers(), req.uri().query()) {
            return json_err(
                StatusCode::UNAUTHORIZED,
                "unauthorized: dashboard token required",
            );
        }
    }
    // Method allowlist for API (defense in depth)
    match *req.method() {
        Method::GET | Method::POST | Method::HEAD | Method::OPTIONS => next.run(req).await,
        _ => json_err(StatusCode::METHOD_NOT_ALLOWED, "method not allowed"),
    }
}

fn token_matches(expected: &str, headers: &HeaderMap, query: Option<&str>) -> bool {
    if let Some(auth) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        let auth = auth.trim();
        if let Some(rest) = auth
            .strip_prefix("Bearer ")
            .or_else(|| auth.strip_prefix("bearer "))
        {
            if rest.trim() == expected {
                return true;
            }
        }
        if auth == expected {
            return true;
        }
    }
    if let Some(h) = headers
        .get("x-locus-token")
        .or_else(|| headers.get("X-Locus-Token"))
        .and_then(|v| v.to_str().ok())
    {
        if h.trim() == expected {
            return true;
        }
    }
    if let Some(q) = query {
        for pair in q.split('&') {
            if let Some(v) = pair.strip_prefix("token=") {
                // URL-decoded light: + and %20 rarely used for tokens; compare raw first
                if v == expected {
                    return true;
                }
                if let Ok(decoded) = urlencoding_decode(v) {
                    if decoded == expected {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Minimal percent-decoder (tokens are usually unreserved).
fn urlencoding_decode(s: &str) -> Result<String, ()> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let h = |c: u8| -> Option<u8> {
                    match c {
                        b'0'..=b'9' => Some(c - b'0'),
                        b'a'..=b'f' => Some(c - b'a' + 10),
                        b'A'..=b'F' => Some(c - b'A' + 10),
                        _ => None,
                    }
                };
                match (h(bytes[i + 1]), h(bytes[i + 2])) {
                    (Some(a), Some(b)) => {
                        out.push((a << 4) | b);
                        i += 3;
                    }
                    _ => return Err(()),
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| ())
}

fn open_store(state: &AppState) -> std::result::Result<Store, (StatusCode, Json<Value>)> {
    let res = match &state.home {
        Some(h) => Store::open(h),
        None => Store::open_default(),
    };
    res.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("open store: {e}") })),
        )
    })
}

fn cwd() -> std::path::PathBuf {
    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
}

fn dashboard_capabilities(manual_state: &str) -> Value {
    json!({
        "reporting": "live_runtime",
        "scope": "locus_surfaces_only",
        "manual_cli_command_execution": {
            "state": "surface_dependent",
            "surface_states": {
                "locus exec": manual_state,
                "locus run": "available_with_explicit_binding",
                "locus ci run": "available_with_explicit_binding"
            }
        },
        "agent_command_execution": {
            "state": "not_exposed",
            "surface": "locus-mcp"
        },
        "provider_credential_injection": {
            "state": "surface_dependent",
            "surface_states": {
                "locus exec": if manual_state == "available" { "available_to_manual_cli_child" } else { manual_state },
                "locus run": "available_to_manual_cli_child_with_explicit_binding",
                "locus ci run": "available_to_manual_cli_child_with_explicit_binding"
            },
            "default_for_child_launch_surfaces": ["locus exec", "locus run", "locus ci run"],
            "no_resolve": {
                "classification": "recipe_expanded",
                "resolving_upstream": "fail_closed_before_child_worker_session_or_credential_effect",
                "credential_free_upstream": "allowed"
            }
        }
    })
}

async fn api_status(
    State(state): State<AppState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let s = open_store(&state)?;
    let _ = s.check_drift_and_freeze();
    let require_pin = find_workspace(&cwd())
        .map_err(store_err)?
        .map(|(_, cfg)| cfg.require_pin)
        .unwrap_or(false);

    match s.active_session().map_err(store_err)? {
        None => Ok(Json(json!({
            "pinned": false,
            "require_pin": require_pin,
            "capabilities": dashboard_capabilities("blocked_unpinned"),
        }))),
        Some(session) => {
            let key = s.seal_key().map_err(store_err)?;
            let seal_ok = session.verify_seal(&key).is_ok();
            let manual_state = if seal_ok && !session.frozen && !session.is_expired() {
                "available"
            } else {
                "blocked_unhealthy_session"
            };
            Ok(Json(json!({
                "pinned": true,
                "binding": session.binding_alias,
                "tenant": session.tenant,
                "session_id": session.session_id,
                "seal_ok": seal_ok,
                "frozen": session.frozen,
                "frozen_reason": session.frozen_reason,
                "expired": session.is_expired(),
                "require_pin": require_pin,
                "mode": if session.is_namespaced() { "namespaced" } else { "exclusive" },
                "namespaces": session.all_aliases(),
                "capabilities": dashboard_capabilities(manual_state),
            })))
        }
    }
}

async fn api_whoami(
    State(state): State<AppState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let s = open_store(&state)?;
    let _ = s.check_drift_and_freeze();
    match s.whoami() {
        Ok(w) => {
            // Whoami already exposes CredentialRefs only — never resolved secrets.
            let v = serde_json::to_value(&w).map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": e.to_string() })),
                )
            })?;
            Ok(Json(v))
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("not pinned") || msg.contains("No active") || msg.contains("unpinned") {
                Err((
                    StatusCode::CONFLICT,
                    Json(json!({ "error": "not pinned", "pinned": false })),
                ))
            } else {
                Err(store_err(e))
            }
        }
    }
}

async fn api_bindings(
    State(state): State<AppState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let s = open_store(&state)?;
    let list = s.list_bindings().map_err(store_err)?;
    Ok(Json(json!({ "bindings": list })))
}

async fn api_approvals(
    State(state): State<AppState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let s = open_store(&state)?;
    let pending = s.pending_approvals().map_err(store_err)?;
    let mut out = Vec::with_capacity(pending.len());
    for rec in pending {
        let dual = s.tool_requires_dual_control(&rec.binding, &rec.tool);
        let required = if dual { 2 } else { 1 };
        let mut v = serde_json::to_value(&rec).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?;
        if let Some(obj) = v.as_object_mut() {
            obj.insert("dual_control".into(), json!(dual));
            obj.insert("required_grants".into(), json!(required));
            obj.insert("approval_authority".into(), json!("local_advisory"));
            obj.insert("authoritative_grants".into(), json!(0));
            obj.insert("required_authoritative_grants".into(), json!(required));
            obj.insert("authoritative_path_enabled".into(), json!(false));
            obj.insert("grants_progress".into(), json!(format!("0/{required}")));
            obj.insert("advisory_assertions".into(), json!(rec.grants.len()));
        }
        out.push(v);
    }
    Ok(Json(json!({
        "approvals": out,
        "approval_authority": "local_advisory",
        "authoritative_path_enabled": external_approval_authority_enabled(),
        "authority_blocker": locus_core::EXTERNAL_APPROVAL_AUTHORITY_BLOCKER,
        "peer_authenticated_os_broker_required": true,
        "non_agent_issue_capability_required": true
    })))
}

async fn api_doctor(
    State(state): State<AppState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let s = open_store(&state)?;
    let _ = s.check_drift_and_freeze();
    let report = gather_doctor(&s).map_err(store_err)?;
    let v = serde_json::to_value(&report).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
    })?;
    Ok(Json(v))
}

#[derive(Debug, Deserialize)]
struct EventsQuery {
    #[serde(default = "default_events_last")]
    last: usize,
    op: Option<String>,
    binding: Option<String>,
}

fn default_events_last() -> usize {
    50
}

async fn api_events(
    State(state): State<AppState>,
    Query(q): Query<EventsQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let s = open_store(&state)?;
    let all = s.read_audit_events().map_err(store_err)?;
    let last = q.last.clamp(1, 500);
    let events = filter_audit_events(&all, last, q.op.as_deref(), q.binding.as_deref());
    Ok(Json(json!({ "events": events, "last": last })))
}

#[derive(Debug, Deserialize)]
struct GrantBody {
    principal: String,
    #[serde(default)]
    ttl: Option<String>,
}

async fn api_grant(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<GrantBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let s = open_store(&state)?;
    let principal = body.principal.trim();
    if principal.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "principal is required" })),
        ));
    }
    let ttl_dur = match body.ttl.as_deref() {
        Some(t) if !t.trim().is_empty() => Some(parse_ttl(t).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("invalid ttl: {e}") })),
            )
        })?),
        _ => None,
    };

    let rec = s.grant_approval(&id, ttl_dur, principal).map_err(|e| {
        let msg = e.to_string();
        let code = if msg.contains("not found") || msg.contains("NotFound") {
            StatusCode::NOT_FOUND
        } else if msg.contains("already") || msg.contains("denied") || msg.contains("required") {
            StatusCode::CONFLICT
        } else {
            StatusCode::BAD_REQUEST
        };
        (code, Json(json!({ "error": msg })))
    })?;

    let dual = s.tool_requires_dual_control(&rec.binding, &rec.tool);
    let required = if dual { 2 } else { 1 };
    let mut v = serde_json::to_value(&rec).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
    })?;
    if let Some(obj) = v.as_object_mut() {
        obj.insert("dual_control".into(), json!(dual));
        obj.insert("required_grants".into(), json!(required));
        obj.insert("approval_authority".into(), json!("local_advisory"));
        obj.insert("authoritative_grants".into(), json!(0));
        obj.insert("required_authoritative_grants".into(), json!(required));
        obj.insert("authoritative_path_enabled".into(), json!(false));
        obj.insert("grants_progress".into(), json!(format!("0/{required}")));
        obj.insert("advisory_assertions".into(), json!(rec.grants.len()));
        obj.insert("recorded_label".into(), json!(principal));
        obj.insert(
            "authority_blocker".into(),
            json!(locus_core::EXTERNAL_APPROVAL_AUTHORITY_BLOCKER),
        );
        obj.insert("peer_authenticated_os_broker_required".into(), json!(true));
        obj.insert("non_agent_issue_capability_required".into(), json!(true));
        obj.insert(
            "detail".into(),
            json!("Advisory evidence recorded; provider execution remains blocked."),
        );
    }
    Ok(Json(v))
}

fn gather_doctor(s: &Store) -> locus_core::Result<locus_core::DoctorReport> {
    // Process-cached phantom --version; skip unresolved_phm inventory on the
    // hot dashboard path (full inventory is for `locus doctor` / forensics).
    build_doctor_report(
        s,
        DoctorExternal {
            phantom_on_path: phantom_on_path(),
            unresolved_phm: Vec::new(),
            cwd: Some(cwd()),
        },
    )
}

fn store_err(e: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": e.to_string() })),
    )
}

fn json_err(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg }))).into_response()
}

fn try_open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(url).spawn().context("open")?;
        Ok(())
    }
    #[cfg(target_os = "windows")]
    {
        Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
            .context("start")?;
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        // Linux / BSD
        if Command::new("xdg-open").arg(url).spawn().is_ok() {
            return Ok(());
        }
        if Command::new("gio").args(["open", url]).spawn().is_ok() {
            return Ok(());
        }
        bail!("no browser opener found (tried xdg-open, gio)");
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use locus_core::{Binding, BindingBody, Policy, ProviderBinding, Scope};
    use tower::ServiceExt;

    fn sample_binding(alias: &str, tenant: &str) -> Binding {
        Binding::from_body(BindingBody {
            id: format!("bnd_{alias}"),
            alias: alias.into(),
            tenant: tenant.into(),
            description: Some("test".into()),
            principal: None,
            providers: vec![ProviderBinding {
                provider: "github".into(),
                account: "acme".into(),
                credential_ref: "phm:GH_TOKEN_ACME".into(),
                scope: Scope {
                    orgs: vec!["acme".into()],
                    ..Default::default()
                },
                upstream: None,
            }],
            policy: Policy {
                require_approval: vec!["*.delete*".into()],
                ..Policy::default()
            },
        })
    }

    fn temp_store() -> (tempfile::TempDir, Store, AppState) {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        let _ = s.seal_key().unwrap();
        let state = AppState {
            token: None,
            home: Some(dir.path().to_path_buf()),
        };
        (dir, s, state)
    }

    #[tokio::test]
    async fn health_is_public_and_status_works() {
        let (dir, s, state) = temp_store();
        let mut b = sample_binding("acme", "acme-corp");
        b.policy.require_approval = vec!["github.delete_repo".into()];
        b.policy.dual_control = vec!["github.delete_repo".into()];
        s.save_binding(&b).unwrap();

        let app = build_router(state);
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 1 << 16)
            .await
            .unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["pinned"], false);
        assert_eq!(v["capabilities"]["reporting"], "live_runtime");
        assert_eq!(
            v["capabilities"]["manual_cli_command_execution"]["state"],
            "surface_dependent"
        );
        assert_eq!(
            v["capabilities"]["manual_cli_command_execution"]["surface_states"],
            json!({
                "locus exec": "blocked_unpinned",
                "locus run": "available_with_explicit_binding",
                "locus ci run": "available_with_explicit_binding"
            })
        );
        assert_eq!(
            v["capabilities"]["agent_command_execution"]["state"],
            "not_exposed"
        );
        assert_eq!(
            v["capabilities"]["provider_credential_injection"]["state"],
            "surface_dependent"
        );

        s.pin("acme", dir.path(), None, false).unwrap();
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(res.into_body(), 1 << 16)
            .await
            .unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["pinned"], true);
        assert_eq!(
            v["capabilities"]["manual_cli_command_execution"]["state"],
            "surface_dependent"
        );
        assert_eq!(
            v["capabilities"]["provider_credential_injection"]["state"],
            "surface_dependent"
        );
        assert_eq!(
            v["capabilities"]["manual_cli_command_execution"]["surface_states"],
            json!({
                "locus exec": "available",
                "locus run": "available_with_explicit_binding",
                "locus ci run": "available_with_explicit_binding"
            })
        );
        assert_eq!(
            v["capabilities"]["provider_credential_injection"]["surface_states"],
            json!({
                "locus exec": "available_to_manual_cli_child",
                "locus run": "available_to_manual_cli_child_with_explicit_binding",
                "locus ci run": "available_to_manual_cli_child_with_explicit_binding"
            })
        );
        assert_eq!(
            v["capabilities"]["provider_credential_injection"]["default_for_child_launch_surfaces"],
            json!(["locus exec", "locus run", "locus ci run"])
        );
        assert_eq!(
            v["capabilities"]["provider_credential_injection"]["no_resolve"]["resolving_upstream"],
            "fail_closed_before_child_worker_session_or_credential_effect"
        );
    }

    #[tokio::test]
    async fn token_gate_blocks_api() {
        let (_dir, _s, mut state) = temp_store();
        state.token = Some(Arc::from("secret-token"));
        let app = build_router(state);

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        // HTML remains public (token only gates /api/* nested routes)
        let res = app
            .clone()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/status")
                    .header(header::AUTHORIZATION, "Bearer secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn index_embeds_dashboard() {
        let app = build_router(AppState {
            token: None,
            home: None,
        });
        let res = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Locus"));
        assert!(html.contains("dashboard"));
        assert!(html.contains("status.capabilities"));
        assert!(html.contains("surface_states"));
        assert!(html.contains("manual child launch"));
        assert!(html.contains("unknown / degraded"));
        assert!(!html.contains("manual_identity_only"));
    }

    #[tokio::test]
    async fn bindings_and_grant_no_secrets() {
        let (_dir, s, state) = temp_store();
        let mut b = sample_binding("acme", "acme-corp");
        b.policy.dual_control = vec!["github.delete_repo".into()];
        s.save_binding(&b).unwrap();

        let rec = s
            .create_pending_approval(
                "github.delete_repo",
                "acme",
                &json!({"x": 1}),
                "sess_test",
                "agent",
            )
            .unwrap();

        let app = build_router(state);

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/bindings")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(res.into_body(), 1 << 16)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("acme"));
        assert!(!text.contains("sk-"));
        assert!(!text.contains("ghp_"));

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/approve/{}/grant", rec.id))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"principal":"mason"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 1 << 16)
            .await
            .unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["status"], "pending");
        assert_eq!(v["recorded_label"], "mason");
        assert_eq!(v["approval_authority"], "local_advisory");
        assert_eq!(v["authoritative_grants"], 0);
        assert_eq!(v["required_authoritative_grants"], 2);
        assert_eq!(v["authoritative_path_enabled"], false);
        assert_eq!(v["peer_authenticated_os_broker_required"], true);
        assert_eq!(v["non_agent_issue_capability_required"], true);
        assert_eq!(
            v["authority_blocker"],
            locus_core::EXTERNAL_APPROVAL_AUTHORITY_BLOCKER
        );
        let text = String::from_utf8_lossy(&body);
        assert!(!text.contains("ghp_"));
        assert!(!text.contains("sk-live"));

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/approve/{}/grant", rec.id))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"principal":"company_ceo"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 1 << 16)
            .await
            .unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["status"], "pending");
        assert_eq!(v["advisory_assertions"], 2);
        assert_eq!(v["authoritative_grants"], 0);

        let persisted = s.load_approval(&rec.id).unwrap();
        assert_eq!(persisted.status, locus_core::ApprovalStatus::Pending);
        assert!(!persisted.is_valid_grant());
    }

    #[tokio::test]
    async fn doctor_returns_verdict_keys() {
        let (_dir, s, state) = temp_store();
        let b = sample_binding("personal", "personal");
        s.save_binding(&b).unwrap();

        let app = build_router(state);
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/doctor")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 1 << 18)
            .await
            .unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert!(v.get("verdict").is_some());
        assert!(v.get("home").is_some());
        assert!(v.get("seal_ok").is_some());
        assert!(v.get("bindings").is_some());
        assert!(v.get("secrets").is_none());
        assert!(v.get("credentials").is_none());
    }
}

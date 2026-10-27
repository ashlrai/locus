//! Local, advisory peer discovery. This API never opens a Store or grants authority.

use anyhow::{bail, Context, Result};
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{header, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use clap::Subcommand;
use locus_core::session::{PeerRegistration, PeerRegistry, PeerRegistryError};
use serde_json::{json, Value};
use std::io::{self, Read};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const DEFAULT_PORT: u16 = 8751;
// Field limits apply before JSON escaping; leave room for the escaped form.
const MAX_BODY: usize = 32 * 1024;
const MAX_RESPONSE: u64 = 8 * 1024 * 1024;

#[derive(Debug, Subcommand)]
pub enum PeersCmd {
    /// Run an ephemeral peer registry until Ctrl-C
    Serve {
        /// Only the exact IPv4 loopback address is accepted
        #[arg(long, default_value = "127.0.0.1:8751")]
        bind: SocketAddr,
    },
    /// List live peers; unfamiliar engines may use this read-only operation
    List {
        #[arg(long, default_value_t = DEFAULT_PORT)]
        port: u16,
    },
    /// Register or refresh a live session's advisory metadata
    Register {
        #[arg(long, default_value_t = DEFAULT_PORT)]
        port: u16,
        /// Public session label, never a credential or sealed-session bearer
        #[arg(long)]
        session_id: String,
        /// claude, cursor, codex, grok, or gemini
        #[arg(long)]
        engine_id: String,
        /// Short non-sensitive description
        #[arg(long)]
        task: String,
        /// Absolute working directory
        #[arg(long)]
        cwd: PathBuf,
        /// PID of the long-lived agent session, not this registration command
        #[arg(long)]
        pid: u32,
    },
}

pub fn run(cmd: PeersCmd, as_json: bool) -> Result<()> {
    let (port, registration) = match cmd {
        PeersCmd::Serve { bind } => {
            return tokio::runtime::Runtime::new()?.block_on(serve(bind));
        }
        PeersCmd::List { port } => (port, None),
        PeersCmd::Register {
            port,
            session_id,
            engine_id,
            task,
            cwd,
            pid,
        } => (
            port,
            Some(PeerRegistration {
                session_id,
                engine_id,
                task,
                cwd,
                pid,
            }),
        ),
    };
    let value = request(port, registration.as_ref())?;
    if as_json {
        crate::json_out::print(crate::json_out::PEERS, &value)
    } else {
        println!("{}", serde_json::to_string_pretty(&value)?);
        Ok(())
    }
}

fn request(port: u16, registration: Option<&PeerRegistration>) -> Result<Value> {
    if port == 0 {
        bail!("peer registry port must be non-zero");
    }
    // No proxy discovery or redirects: metadata must never leave loopback.
    let agent = ureq::AgentBuilder::new()
        .try_proxy_from_env(false)
        .redirects(0)
        .timeout(Duration::from_secs(5))
        .build();
    let url = format!("http://127.0.0.1:{port}/peers");
    let result = match registration {
        Some(peer) => agent
            .post(&url)
            .set("Content-Type", "application/json")
            .send_string(&serde_json::to_string(peer)?),
        None => agent.get(&url).call(),
    };
    let response = match result {
        Ok(response) => response,
        Err(ureq::Error::Status(status, _)) => {
            bail!("peer registry refused request (HTTP {status})")
        }
        Err(ureq::Error::Transport(_)) => bail!("peer registry unavailable at 127.0.0.1:{port}"),
    };
    if response.status() != 200 {
        bail!(
            "unexpected peer registry response (HTTP {})",
            response.status()
        );
    }
    // Bound responses even when another local process owns the requested port.
    let mut body = Vec::new();
    response
        .into_reader()
        .take(MAX_RESPONSE + 1)
        .read_to_end(&mut body)?;
    if body.len() as u64 > MAX_RESPONSE {
        bail!("peer registry response too large");
    }
    serde_json::from_slice(&body).context("invalid peer registry response")
}

async fn bind_listener(addr: SocketAddr) -> io::Result<tokio::net::TcpListener> {
    if addr.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "peer registry requires exactly 127.0.0.1",
        ));
    }
    tokio::net::TcpListener::bind(addr).await
}

async fn serve(addr: SocketAddr) -> Result<()> {
    let listener = bind_listener(addr).await?;
    let bound = listener.local_addr()?;
    eprintln!("locus peers: http://{bound}/peers (advisory metadata only)");
    axum::serve(listener, router(bound.port()))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .context("peer registry server")
}

#[derive(Clone)]
struct PeerState {
    registry: Arc<Mutex<PeerRegistry>>,
    authority: String,
}

fn router(port: u16) -> Router {
    let state = PeerState {
        registry: Arc::new(Mutex::new(PeerRegistry::default())),
        authority: format!("127.0.0.1:{port}"),
    };
    Router::new()
        .route("/peers", get(list).post(register))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .layer(middleware::from_fn_with_state(state.clone(), local_request))
        .with_state(state)
}

async fn local_request(
    State(state): State<PeerState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    // A loopback bind alone does not stop DNS rebinding or browser-origin writes.
    // Native clients only; neither forwarded headers nor CORS grant an exception.
    let headers = request.headers();
    if headers.get(header::HOST).and_then(|v| v.to_str().ok()) != Some(&state.authority)
        || headers.contains_key(header::ORIGIN)
        || headers.contains_key("sec-fetch-site")
    {
        return error(StatusCode::FORBIDDEN, "native loopback requests only");
    }
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "no-store".parse().expect("static header"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        "nosniff".parse().expect("static header"),
    );
    response
}

async fn list(State(state): State<PeerState>) -> Response {
    match state.registry.lock() {
        Ok(mut registry) => Json(json!({ "peers": registry.list_peers() })).into_response(),
        Err(_) => error(StatusCode::SERVICE_UNAVAILABLE, "peer registry unavailable"),
    }
}

async fn register(State(state): State<PeerState>, Json(peer): Json<PeerRegistration>) -> Response {
    let Ok(mut registry) = state.registry.lock() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "peer registry unavailable");
    };
    match registry.register(peer) {
        Ok(peer) => Json(json!({ "peer": peer })).into_response(),
        Err(err) => {
            let status = match err {
                PeerRegistryError::ReadOnly => StatusCode::FORBIDDEN,
                PeerRegistryError::Invalid(_) | PeerRegistryError::DeadPid => {
                    StatusCode::BAD_REQUEST
                }
                PeerRegistryError::Capacity => StatusCode::SERVICE_UNAVAILABLE,
                PeerRegistryError::Conflict => StatusCode::CONFLICT,
            };
            error(status, &err.to_string())
        }
    }
}

fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use tower::ServiceExt;

    #[test]
    fn cli_dry_run_never_registers_or_starts_a_server() {
        use clap::Parser;
        for args in [
            vec!["locus", "peers", "serve", "--dry-run"],
            vec![
                "locus",
                "peers",
                "register",
                "--session-id",
                "a",
                "--engine-id",
                "codex",
                "--task",
                "test",
                "--cwd",
                "/work",
                "--pid",
                "1",
                "--dry-run",
            ],
        ] {
            let cli = crate::Cli::try_parse_from(args).unwrap();
            assert_eq!(
                crate::dry_run_support(&cli.command),
                crate::DryRunSupport::Unsupported
            );
        }
        for args in [
            vec!["locus", "peers", "serve", "--dry-run"],
            vec!["locus", "--dry-run", "peers", "register"],
        ] {
            let error = crate::parse_cli(args.into_iter().map(Into::into).collect()).unwrap_err();
            assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
        }
        let cli = crate::Cli::try_parse_from(["locus", "peers", "list", "--dry-run"]).unwrap();
        assert_eq!(
            crate::dry_run_support(&cli.command),
            crate::DryRunSupport::ReadOnly
        );
    }

    fn peer() -> PeerRegistration {
        PeerRegistration {
            session_id: "peer-test".into(),
            engine_id: "codex".into(),
            task: "test discovery".into(),
            cwd: std::env::current_dir().unwrap(),
            pid: std::process::id(),
        }
    }

    fn post(peer: &PeerRegistration) -> Request<Body> {
        Request::post("/peers")
            .header(header::HOST, "127.0.0.1:8751")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(peer).unwrap()))
            .unwrap()
    }

    #[tokio::test]
    async fn non_loopback_bind_refused() {
        for addr in [
            "0.0.0.0:0",
            "192.0.2.1:0",
            "127.0.0.2:0",
            "[::1]:0",
            "[::]:0",
            "[::ffff:127.0.0.1]:0",
        ] {
            assert_eq!(
                bind_listener(addr.parse().unwrap())
                    .await
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::PermissionDenied
            );
        }
        let listener = bind_listener("127.0.0.1:0".parse().unwrap()).await.unwrap();
        assert_eq!(
            listener.local_addr().unwrap().ip(),
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        );
    }

    #[tokio::test]
    async fn register_list_and_unknown_engine_read_only() {
        let app = router(DEFAULT_PORT);
        let expected = peer();
        assert_eq!(
            app.clone().oneshot(post(&expected)).await.unwrap().status(),
            StatusCode::OK
        );
        let mut unknown = expected.clone();
        unknown.engine_id = "unfamiliar".into();
        assert_eq!(
            app.clone().oneshot(post(&unknown)).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
        let request = Request::get("/peers")
            .header(header::HOST, "127.0.0.1:8751")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let body = to_bytes(response.into_body(), MAX_BODY).await.unwrap();
        let result: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(result, json!({ "peers": [expected] }));
    }

    #[tokio::test]
    async fn browser_and_rebinding_requests_cannot_register() {
        for (name, value) in [
            ("host", "attacker.example"),
            ("origin", "http://attacker.example"),
            ("origin", "null"),
            ("sec-fetch-site", "cross-site"),
        ] {
            let app = router(DEFAULT_PORT);
            let mut request = post(&peer());
            request.headers_mut().insert(
                axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                value.parse().unwrap(),
            );
            assert_eq!(
                app.oneshot(request).await.unwrap().status(),
                StatusCode::FORBIDDEN
            );
        }
    }

    #[tokio::test]
    async fn malformed_and_oversized_requests_are_rejected() {
        let app = router(DEFAULT_PORT);
        for body in ["{}".to_string(), "x".repeat(MAX_BODY + 1)] {
            let request = Request::post("/peers")
                .header(header::HOST, "127.0.0.1:8751")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap();
            assert!(app
                .clone()
                .oneshot(request)
                .await
                .unwrap()
                .status()
                .is_client_error());
        }
    }

    #[tokio::test]
    async fn maximum_valid_escaped_metadata_fits_request_limit() {
        let mut maximum = peer();
        maximum.session_id = "\\".repeat(128);
        maximum.task = "\\".repeat(4096);
        maximum.cwd =
            PathBuf::from(if cfg!(windows) { "C:\\" } else { "/" }).join("\"".repeat(4090));
        assert!(serde_json::to_vec(&maximum).unwrap().len() > 16 * 1024);
        assert_eq!(
            router(DEFAULT_PORT)
                .oneshot(post(&maximum))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn real_http_register_and_list() {
        let listener = bind_listener("127.0.0.1:0".parse().unwrap()).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server =
            tokio::spawn(async move { axum::serve(listener, router(port)).await.unwrap() });
        tokio::task::spawn_blocking(move || {
            let expected = peer();
            assert_eq!(
                request(port, Some(&expected)).unwrap(),
                json!({ "peer": expected })
            );
            assert_eq!(request(port, None).unwrap(), json!({ "peers": [expected] }));
        })
        .await
        .unwrap();
        server.abort();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn http_list_prunes_a_dead_pid() {
        struct ChildGuard(std::process::Child);
        impl Drop for ChildGuard {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut child = ChildGuard(
            std::process::Command::new("sleep")
                .arg("30")
                .spawn()
                .unwrap(),
        );
        let app = router(DEFAULT_PORT);
        let mut exiting = peer();
        exiting.pid = child.0.id();
        assert_eq!(
            app.clone().oneshot(post(&exiting)).await.unwrap().status(),
            StatusCode::OK
        );
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        let request = Request::get("/peers")
            .header(header::HOST, "127.0.0.1:8751")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), MAX_BODY).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({ "peers": [] })
        );
    }

    #[tokio::test]
    async fn client_refuses_redirects() {
        let listener = bind_listener("127.0.0.1:0".parse().unwrap()).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = Router::new().route(
            "/peers",
            get(|| async {
                (
                    StatusCode::FOUND,
                    [(header::LOCATION, "http://192.0.2.1/peers")],
                )
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let result = tokio::task::spawn_blocking(move || request(port, None))
            .await
            .unwrap();
        assert!(result.unwrap_err().to_string().contains("HTTP 302"));
        server.abort();
    }
}

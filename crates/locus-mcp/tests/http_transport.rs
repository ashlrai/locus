//! HTTP transport: health probe + token auth reject/accept + JSON-RPC POST /mcp.

use locus_core::{Binding, BindingBody, Policy, ProviderBinding, Scope, Store, UpstreamSpec};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::tempdir;

fn mcp_bin() -> PathBuf {
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_locus-mcp") {
        return PathBuf::from(p);
    }
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p.push("target");
    p.push("debug");
    p.push("locus-mcp");
    p
}

/// Bind an ephemeral localhost port and return it (listener dropped so child can bind).
fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("ephemeral bind");
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

struct HttpServer {
    child: Child,
    addr: SocketAddr,
    token: String,
}

impl HttpServer {
    fn spawn(token: &str) -> Self {
        let dir = tempdir().expect("tempdir");
        // Leak home for process lifetime of this test (child holds LOCUS_HOME).
        let home = dir.path().join("locus-home");
        std::fs::create_dir_all(&home).unwrap();
        // Keep tempdir alive by forgetting it after env is set… better: use a static-ish path.
        let home_owned = home.to_path_buf();
        // Store the TempDir inside a Box that we leak so the directory survives.
        std::mem::forget(dir);

        Self::spawn_with_home(token, &home_owned, &[])
    }

    fn spawn_with_home(token: &str, home: &std::path::Path, extra_env: &[(&str, &str)]) -> Self {
        let port = free_port();
        let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let mut command = Command::new(mcp_bin());
        command
            .arg("--http")
            .arg(addr.to_string())
            .env("LOCUS_HOME", home)
            .env("LOCUS_MCP_HTTP_TOKEN", token)
            .env("LOCUS_MCP_AUTO_PIN", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if !extra_env
            .iter()
            .any(|(key, _)| *key == locus_core::EXECUTOR_CAPABILITY_ENV)
        {
            if let Ok(store) = Store::open(home) {
                if let Ok(Some(session)) = store.active_session() {
                    if let Ok(capability) = store.grant_executor_capability(&session) {
                        command.env(locus_core::EXECUTOR_CAPABILITY_ENV, capability);
                    }
                }
            }
        }
        for (key, value) in extra_env {
            command.env(key, value);
        }
        let mut child = command.spawn().expect("spawn locus-mcp --http");

        // Wait until /health responds or timeout.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if Instant::now() > deadline {
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() {
                    let _ = e.read_to_string(&mut err);
                }
                let _ = child.kill();
                panic!("HTTP server did not become ready: {err}");
            }
            if let Ok(mut stream) = TcpStream::connect(addr) {
                let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
                let _ = stream.write_all(
                    b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
                );
                let mut buf = String::new();
                let _ = stream.read_to_string(&mut buf);
                if buf.contains("200") && buf.contains("\"ok\"") {
                    break;
                }
            }
            thread::sleep(Duration::from_millis(50));
            // Child still alive?
            if let Ok(Some(status)) = child.try_wait() {
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() {
                    let _ = e.read_to_string(&mut err);
                }
                panic!("locus-mcp exited early {status}: {err}");
            }
        }

        Self {
            child,
            addr,
            token: token.to_string(),
        }
    }

    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
        auth: Option<&str>,
    ) -> (u16, String, String) {
        self.request_with_headers(method, path, body, auth, &[])
    }

    fn request_with_headers(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
        auth: Option<&str>,
        extra_headers: &[(&str, &str)],
    ) -> (u16, String, String) {
        let mut stream = TcpStream::connect(self.addr).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let body = body.unwrap_or(b"");
        let mut req = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\nConnection: close\r\n",
            body.len()
        );
        if !body.is_empty() {
            req.push_str("Content-Type: application/json\r\n");
        }
        if let Some(t) = auth {
            req.push_str(&format!("Authorization: Bearer {t}\r\n"));
        }
        for (k, v) in extra_headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str("\r\n");
        stream.write_all(req.as_bytes()).unwrap();
        if !body.is_empty() {
            stream.write_all(body).unwrap();
        }
        stream.flush().unwrap();
        let mut raw = String::new();
        stream.read_to_string(&mut raw).unwrap();
        let (status, headers, body) = parse_http_response(&raw);
        (status, headers, body)
    }
}

fn header_lookup(headers: &str, name: &str) -> Option<String> {
    let name_l = name.to_ascii_lowercase();
    for line in headers.lines().skip(1) {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().to_ascii_lowercase() == name_l {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn parse_http_response(raw: &str) -> (u16, String, String) {
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw, ""));
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    (status, head.to_string(), body.to_string())
}

#[test]
fn http_health_unauthenticated() {
    let srv = HttpServer::spawn("test-token-health");
    let (status, _, body) = srv.request("GET", "/health", None, None);
    assert_eq!(status, 200, "health body={body}");
    let v: Value = serde_json::from_str(&body).expect("json health");
    assert_eq!(v["ok"], true);
    assert_eq!(v["service"], "locus-mcp");
    assert!(v["version"].as_str().is_some());
    assert_eq!(v["transport"], "streamable-http-lite");
    assert!(v["endpoints"]["rpc"].as_str().is_some());
}

#[test]
fn http_get_mcp_capabilities_requires_token() {
    let srv = HttpServer::spawn("caps-token-required");
    let (status, _, resp) = srv.request("GET", "/mcp", None, None);
    assert_eq!(status, 401, "expected unauthorized, got {status}: {resp}");
    assert!(
        resp.contains("unauthorized") || resp.contains("Unauthorized"),
        "{resp}"
    );
}

#[test]
fn http_get_mcp_capabilities_values_free() {
    let srv = HttpServer::spawn("caps-token-ok");
    let (status, headers, body) = srv.request("GET", "/mcp", None, Some(&srv.token));
    assert_eq!(status, 200, "{body}");
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("content-type: application/json"),
        "headers={headers}"
    );
    let v: Value = serde_json::from_str(&body).expect("capabilities json");
    assert_eq!(v["ok"], true);
    assert_eq!(v["service"], "locus-mcp");
    assert_eq!(v["transport"], "streamable-http-lite");
    assert_eq!(v["protocolVersion"], "2024-11-05");
    assert!(v["capabilities"]["tools"].is_object());
    assert_eq!(v["pin"]["pinned"], false);
    let tools = v["tools"].as_array().expect("tools names array");
    assert!(
        tools.iter().any(|t| t.as_str() == Some("locus_whoami")),
        "expected control tool names, got {tools:?}"
    );
    // Values-free: no secret-looking material, no credential fields.
    let lower = body.to_ascii_lowercase();
    assert!(!lower.contains("phm_"), "{body}");
    assert!(!lower.contains("\"credential_ref\""), "{body}");
    assert!(!lower.contains("secret-token"), "{body}");
    // Tool entries are bare names, not full schemas.
    assert!(
        tools.iter().all(|t| t.is_string()),
        "tools must be name strings only: {tools:?}"
    );
}

#[test]
fn http_accept_sse_only_returns_single_event() {
    let srv = HttpServer::spawn("sse-token");
    let mut stream = TcpStream::connect(srv.addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let body = br#"{"jsonrpc":"2.0","id":3,"method":"ping","params":{}}"#;
    let req = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nAccept: text/event-stream\r\nContent-Length: {}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
        body.len(),
        srv.token
    );
    stream.write_all(req.as_bytes()).unwrap();
    stream.write_all(body).unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    let (status, headers, resp_body) = parse_http_response(&raw);
    assert_eq!(status, 200, "{raw}");
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("content-type: text/event-stream"),
        "headers={headers}"
    );
    assert!(
        resp_body.contains("event: message") && resp_body.contains("data: "),
        "{resp_body}"
    );
    // Extract JSON after "data: "
    let data = resp_body
        .lines()
        .find_map(|l| l.strip_prefix("data: "))
        .expect("sse data line");
    let v: Value = serde_json::from_str(data).expect("jsonrpc in sse");
    assert_eq!(v["id"], 3);
    assert!(v.get("result").is_some(), "{v}");
}

#[test]
fn http_accept_not_acceptable() {
    let srv = HttpServer::spawn("accept-406");
    let mut stream = TcpStream::connect(srv.addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let body = br#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{}}"#;
    let req = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nAccept: text/plain\r\nContent-Length: {}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
        body.len(),
        srv.token
    );
    stream.write_all(req.as_bytes()).unwrap();
    stream.write_all(body).unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    let (status, _, resp_body) = parse_http_response(&raw);
    assert_eq!(status, 406, "{raw}");
    assert!(resp_body.contains("not_acceptable"), "{resp_body}");
}

#[test]
fn http_content_type_reject_non_json() {
    let srv = HttpServer::spawn("ct-415");
    let mut stream = TcpStream::connect(srv.addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let body = br#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{}}"#;
    let req = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
        body.len(),
        srv.token
    );
    stream.write_all(req.as_bytes()).unwrap();
    stream.write_all(body).unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    let (status, _, resp_body) = parse_http_response(&raw);
    assert_eq!(status, 415, "{raw}");
    assert!(resp_body.contains("unsupported_media_type"), "{resp_body}");
}

#[test]
fn http_token_reject_without_header() {
    let srv = HttpServer::spawn("secret-token-xyz");
    let rpc = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "ping",
        "params": {}
    });
    let body = serde_json::to_vec(&rpc).unwrap();
    let (status, _, resp) = srv.request("POST", "/mcp", Some(&body), None);
    assert_eq!(status, 401, "expected unauthorized, got {status}: {resp}");
    assert!(
        resp.contains("unauthorized") || resp.contains("Unauthorized"),
        "{resp}"
    );
}

#[test]
fn http_token_reject_wrong_token() {
    let srv = HttpServer::spawn("correct-token");
    let rpc = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "ping",
        "params": {}
    });
    let body = serde_json::to_vec(&rpc).unwrap();
    let (status, _, resp) = srv.request("POST", "/mcp", Some(&body), Some("wrong-token"));
    assert_eq!(status, 401, "{resp}");
}

#[test]
fn http_jsonrpc_ping_with_token() {
    let srv = HttpServer::spawn("good-token-42");
    let rpc = json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "ping",
        "params": {}
    });
    let body = serde_json::to_vec(&rpc).unwrap();
    let (status, _, resp) = srv.request("POST", "/mcp", Some(&body), Some(&srv.token));
    assert_eq!(status, 200, "{resp}");
    let v: Value = serde_json::from_str(&resp).expect("jsonrpc");
    assert_eq!(v["id"], 7);
    assert!(v.get("result").is_some(), "{v}");
    assert!(v.get("error").is_none(), "{v}");
}

#[test]
fn http_jsonrpc_initialize_and_tools_list() {
    let srv = HttpServer::spawn("init-token");
    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "http-test", "version": "0" }
        }
    });
    let body = serde_json::to_vec(&init).unwrap();
    let (status, headers, resp) = srv.request("POST", "/mcp", Some(&body), Some(&srv.token));
    assert_eq!(status, 200, "{resp}");
    let v: Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(v["result"]["serverInfo"]["name"], "locus-mcp");
    let session_id =
        header_lookup(&headers, "Mcp-Session-Id").expect("initialize must mint Mcp-Session-Id");
    assert_eq!(session_id.len(), 32, "opaque hex session id: {session_id}");

    let list = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    });
    let body = serde_json::to_vec(&list).unwrap();
    let (status, headers, resp) = srv.request_with_headers(
        "POST",
        "/mcp",
        Some(&body),
        Some(&srv.token),
        &[("Mcp-Session-Id", &session_id)],
    );
    assert_eq!(status, 200, "{resp}");
    assert_eq!(
        header_lookup(&headers, "Mcp-Session-Id").as_deref(),
        Some(session_id.as_str())
    );
    let v: Value = serde_json::from_str(&resp).unwrap();
    let tools = v["result"]["tools"].as_array().expect("tools");
    assert!(
        tools.iter().any(|t| t["name"] == "locus_whoami"),
        "expected control tools, got {tools:?}"
    );
}

#[test]
fn http_mcp_session_id_mint_reuse_reject() {
    let srv = HttpServer::spawn("session-id-token");
    let ping = serde_json::to_vec(&json!({
        "jsonrpc": "2.0", "id": 1, "method": "ping", "params": {}
    }))
    .unwrap();

    // First POST without session → mint.
    let (status, headers, body) = srv.request("POST", "/mcp", Some(&ping), Some(&srv.token));
    assert_eq!(status, 200, "{body}");
    let sid = header_lookup(&headers, "Mcp-Session-Id").expect("minted session");
    assert!(
        sid.chars().all(|c| c.is_ascii_hexdigit()) && sid.len() == 32,
        "sid={sid}"
    );

    // Reuse same id.
    let (status, headers, body) = srv.request_with_headers(
        "POST",
        "/mcp",
        Some(&ping),
        Some(&srv.token),
        &[("Mcp-Session-Id", &sid)],
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        header_lookup(&headers, "Mcp-Session-Id").as_deref(),
        Some(sid.as_str())
    );

    // Unknown id → 404 fail closed.
    let (status, _, body) = srv.request_with_headers(
        "POST",
        "/mcp",
        Some(&ping),
        Some(&srv.token),
        &[("Mcp-Session-Id", "ffffffffffffffffffffffffffffffff")],
    );
    assert_eq!(status, 404, "{body}");
    assert!(body.contains("unknown_session"), "{body}");

    // Empty id → 400.
    let (status, _, body) = srv.request_with_headers(
        "POST",
        "/mcp",
        Some(&ping),
        Some(&srv.token),
        &[("Mcp-Session-Id", "")],
    );
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("invalid_session"), "{body}");
}

#[test]
fn http_mcp_session_delete_and_capabilities_advertise() {
    let srv = HttpServer::spawn("session-delete-token");
    let (status, _, body) = srv.request("GET", "/mcp", None, Some(&srv.token));
    assert_eq!(status, 200, "{body}");
    let caps: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        caps["streamable"]["session"]["header"], "Mcp-Session-Id",
        "{caps}"
    );
    assert!(
        caps["streamable"]["session"]["ttl_seconds"]
            .as_u64()
            .unwrap_or(0)
            > 0
    );

    let ping = serde_json::to_vec(&json!({
        "jsonrpc": "2.0", "id": 1, "method": "ping", "params": {}
    }))
    .unwrap();
    let (status, headers, _) = srv.request("POST", "/mcp", Some(&ping), Some(&srv.token));
    assert_eq!(status, 200);
    let sid = header_lookup(&headers, "Mcp-Session-Id").unwrap();

    // DELETE terminates.
    let (status, _, body) = srv.request_with_headers(
        "DELETE",
        "/mcp",
        None,
        Some(&srv.token),
        &[("Mcp-Session-Id", &sid)],
    );
    assert_eq!(status, 204, "{body}");

    // Reuse after delete → 404.
    let (status, _, body) = srv.request_with_headers(
        "POST",
        "/mcp",
        Some(&ping),
        Some(&srv.token),
        &[("Mcp-Session-Id", &sid)],
    );
    assert_eq!(status, 404, "{body}");
    assert!(body.contains("unknown_session"), "{body}");
}

#[test]
fn direct_http_with_pin_but_no_executor_capability_cannot_discover_or_call_provider() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let binding = Binding::from_body(BindingBody {
        id: "bnd_http_direct".into(),
        alias: "http-direct".into(),
        tenant: "http-direct".into(),
        principal: None,
        description: None,
        policy: Policy::default(),
        providers: vec![ProviderBinding {
            provider: "github".into(),
            account: "http-direct".into(),
            credential_ref: "phm:HTTP_DIRECT_GITHUB".into(),
            scope: Scope::default(),
            upstream: None,
        }],
    });
    store.save_binding(&binding).unwrap();
    store
        .pin(
            "http-direct",
            dir.path(),
            Some("local-control".into()),
            false,
        )
        .unwrap();
    let server = HttpServer::spawn_with_home(
        "direct-no-executor",
        dir.path(),
        &[(locus_core::EXECUTOR_CAPABILITY_ENV, "")],
    );

    let list = serde_json::to_vec(&json!({
        "jsonrpc": "2.0", "id": 21, "method": "tools/list", "params": {}
    }))
    .unwrap();
    let (status, _, response) = server.request("POST", "/mcp", Some(&list), Some(&server.token));
    assert_eq!(status, 200, "{response}");
    let listed: Value = serde_json::from_str(&response).unwrap();
    if let Some(tools) = listed["result"]["tools"].as_array() {
        assert!(!tools.iter().any(|tool| tool["name"] == "github.scope"));
    } else {
        assert!(
            listed.get("error").is_some(),
            "unexpected tools/list: {listed}"
        );
    }

    let call = serde_json::to_vec(&json!({
        "jsonrpc": "2.0", "id": 22, "method": "tools/call",
        "params": {"name": "github.scope", "arguments": {}}
    }))
    .unwrap();
    let (status, _, response) = server.request("POST", "/mcp", Some(&call), Some(&server.token));
    assert_eq!(status, 200, "{response}");
    let called: Value = serde_json::from_str(&response).unwrap();
    assert!(
        called.get("error").is_some() || called["result"]["isError"] == true,
        "provider call unexpectedly succeeded without executor authority: {called}"
    );
}

#[test]
fn http_x_locus_token_header_accepted() {
    let srv = HttpServer::spawn("header-token");
    let mut stream = TcpStream::connect(srv.addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let body = br#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{}}"#;
    let req = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nX-Locus-Token: {}\r\nConnection: close\r\n\r\n",
        body.len(),
        srv.token
    );
    stream.write_all(req.as_bytes()).unwrap();
    stream.write_all(body).unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    let (status, _, resp_body) = parse_http_response(&raw);
    assert_eq!(status, 200, "{raw}");
    let v: Value = serde_json::from_str(&resp_body).unwrap();
    assert!(v.get("result").is_some());
}

#[test]
fn http_require_approval_precedes_worker_and_credential_startup() {
    if Command::new("python3").arg("--version").output().is_err() {
        return;
    }

    let dir = tempdir().unwrap();
    let marker = dir.path().join("http-worker-token.txt");
    let marker_arg = marker.display().to_string();
    let store = Store::open(dir.path()).unwrap();
    let binding = Binding::from_body(BindingBody {
        id: "bnd_http_hostile".into(),
        alias: "http-hostile".into(),
        tenant: "http-hostile-test".into(),
        principal: None,
        description: None,
        policy: Policy {
            require_approval: vec!["github.delete_repo".into()],
            ..Policy::default()
        },
        providers: vec![ProviderBinding {
            provider: "github".into(),
            account: "http-hostile-gh".into(),
            credential_ref: "env:HTTP_HOSTILE_WORKER_TOKEN".into(),
            scope: Scope::default(),
            upstream: Some(
                UpstreamSpec::new("python3")
                    .with_args([
                        "-u",
                        "-c",
                        r#"import os, pathlib, sys, time
pathlib.Path(sys.argv[1]).write_text(os.environ.get("GH_TOKEN", "missing"))
time.sleep(30)
"#,
                        marker_arg.as_str(),
                    ])
                    .resolve_secrets(true),
            ),
        }],
    });
    store.save_binding(&binding).unwrap();
    store
        .pin(
            "http-hostile",
            dir.path(),
            Some("local-control".into()),
            false,
        )
        .unwrap();

    let server = HttpServer::spawn_with_home(
        "http-hostile-auth",
        dir.path(),
        &[("HTTP_HOSTILE_WORKER_TOKEN", "http-worker-canary-token")],
    );
    let call = json!({
        "jsonrpc": "2.0",
        "id": 9,
        "method": "tools/call",
        "params": {
            "name": "github.delete_repo",
            "arguments": { "owner": "acme", "repo": "critical" }
        }
    });
    let body = serde_json::to_vec(&call).unwrap();
    let (status, _, response) = server.request("POST", "/mcp", Some(&body), Some(&server.token));
    assert_eq!(status, 200, "{response}");
    assert!(response.contains("requires_approval"), "{response}");
    thread::sleep(Duration::from_millis(100));
    assert!(
        !marker.exists(),
        "HTTP worker observed a credential before approval: {}",
        std::fs::read_to_string(marker).unwrap_or_default()
    );
    assert_eq!(store.pending_approvals().unwrap().len(), 1);
}

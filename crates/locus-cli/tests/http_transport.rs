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
        headers
            .to_ascii_lowercase()
            .contains("x-locus-streamable: sse-single"),
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

/// Count SSE `event: message` frames and parse each `data:` JSON payload.
fn parse_sse_message_events(body: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let mut expect_data = false;
    for line in body.lines() {
        if line.starts_with("event:") {
            expect_data = line.trim_start_matches("event:").trim() == "message";
            continue;
        }
        if expect_data {
            if let Some(data) = line.strip_prefix("data: ") {
                if let Ok(v) = serde_json::from_str::<Value>(data) {
                    out.push(v);
                }
                expect_data = false;
            } else if line.is_empty() {
                expect_data = false;
            }
        }
    }
    // Fallback: any data: line (some frames omit event:).
    if out.is_empty() {
        for line in body.lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                if let Ok(v) = serde_json::from_str::<Value>(data) {
                    out.push(v);
                }
            }
        }
    }
    out
}

#[test]
fn http_tools_call_large_body_streams_multi_sse() {
    let dir = tempdir().unwrap();
    let home = dir.path().join("locus-home");
    std::fs::create_dir_all(&home).unwrap();
    // Low thresholds so control-tool results cross multi + chunk boundaries.
    let srv = HttpServer::spawn_with_home(
        "sse-multi-token",
        &home,
        &[
            ("LOCUS_MCP_SSE_MULTI_BYTES", "64"),
            ("LOCUS_MCP_SSE_CHUNK_BYTES", "32"),
        ],
    );

    // tools/list schemas are large enough with a low multi threshold.
    let body = br#"{"jsonrpc":"2.0","id":42,"method":"tools/list","params":{}}"#;
    let mut stream = TcpStream::connect(srv.addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
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
        headers
            .to_ascii_lowercase()
            .contains("x-locus-streamable: sse-multi"),
        "expected multi SSE header, headers={headers}"
    );

    let events = parse_sse_message_events(&resp_body);
    assert!(
        events.len() >= 2,
        "expected progress + final, got {} events: {resp_body}",
        events.len()
    );

    // First event: progress notification (no id).
    let progress = &events[0];
    assert_eq!(progress["method"], "notifications/message");
    assert!(progress.get("id").is_none(), "{progress}");
    assert_eq!(
        progress["params"]["data"]["kind"], "locus.sse.progress",
        "{progress}"
    );

    // Final event: complete JSON-RPC response with original id.
    let final_msg = events.last().expect("final");
    assert_eq!(final_msg["id"], 42, "{final_msg}");
    assert!(final_msg.get("result").is_some(), "{final_msg}");
    assert!(final_msg.get("error").is_none(), "{final_msg}");
    let tools = final_msg["result"]["tools"]
        .as_array()
        .expect("tools array");
    assert!(
        tools.iter().any(|t| t["name"] == "locus_whoami"),
        "final result must include control tools: {tools:?}"
    );
}

#[test]
fn http_tools_call_sse_progress_and_chunks() {
    let dir = tempdir().unwrap();
    let home = dir.path().join("locus-home");
    std::fs::create_dir_all(&home).unwrap();
    let srv = HttpServer::spawn_with_home(
        "sse-chunk-token",
        &home,
        &[
            ("LOCUS_MCP_SSE_MULTI_BYTES", "48"),
            ("LOCUS_MCP_SSE_CHUNK_BYTES", "24"),
        ],
    );

    let body = br#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"locus_list_bindings","arguments":{}}}"#;
    let mut stream = TcpStream::connect(srv.addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
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
            .contains("x-locus-streamable: sse-multi")
            || headers
                .to_ascii_lowercase()
                .contains("x-locus-streamable: sse-single"),
        "headers={headers}"
    );
    let events = parse_sse_message_events(&resp_body);
    assert!(!events.is_empty(), "{resp_body}");
    let final_msg = events.last().unwrap();
    assert_eq!(final_msg["id"], 9, "{final_msg}");
    assert!(
        final_msg.get("result").is_some() || final_msg.get("error").is_some(),
        "{final_msg}"
    );
    if events.len() > 1 {
        assert_eq!(events[0]["method"], "notifications/message");
        assert_eq!(
            events[0]["params"]["data"]["kind"], "locus.sse.progress",
            "{}",
            events[0]
        );
    }
}

#[test]
fn http_large_tools_call_upgrades_to_sse_when_accept_lists_both() {
    let dir = tempdir().unwrap();
    let home = dir.path().join("locus-home");
    std::fs::create_dir_all(&home).unwrap();
    let srv = HttpServer::spawn_with_home(
        "sse-upgrade-token",
        &home,
        &[("LOCUS_MCP_SSE_MULTI_BYTES", "64")],
    );

    // Accept lists both JSON and SSE — small ping stays JSON; large tools/list upgrades.
    let ping = br#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{}}"#;
    let mut stream = TcpStream::connect(srv.addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let req = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
        ping.len(),
        srv.token
    );
    stream.write_all(req.as_bytes()).unwrap();
    stream.write_all(ping).unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    let (status, headers, body) = parse_http_response(&raw);
    assert_eq!(status, 200, "{raw}");
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("content-type: application/json"),
        "small response should stay JSON when Accept lists both: {headers}"
    );
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["id"], 1);

    let list = br#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#;
    let mut stream = TcpStream::connect(srv.addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let req = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
        list.len(),
        srv.token
    );
    stream.write_all(req.as_bytes()).unwrap();
    stream.write_all(list).unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    let (status, headers, resp_body) = parse_http_response(&raw);
    assert_eq!(status, 200, "{raw}");
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("content-type: text/event-stream"),
        "large body should upgrade to SSE when Accept includes event-stream: {headers}"
    );
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("x-locus-streamable: sse-multi"),
        "headers={headers}"
    );
    let events = parse_sse_message_events(&resp_body);
    assert!(events.len() >= 2, "{resp_body}");
    assert_eq!(events.last().unwrap()["id"], 2);
}

#[test]
fn http_mcp_sse_requires_token() {
    let srv = HttpServer::spawn("sse-session-auth");
    let (status, _, body) = srv.request("GET", "/mcp/sse?once=1", None, None);
    assert_eq!(status, 401, "{body}");
    assert!(body.contains("unauthorized"), "{body}");
}

#[test]
fn http_mcp_sse_once_session_tick() {
    let srv = HttpServer::spawn("sse-session-ok");
    let mut stream = TcpStream::connect(srv.addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let req = format!(
        "GET /mcp/sse?once=1 HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: text/event-stream\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
        srv.token
    );
    stream.write_all(req.as_bytes()).unwrap();
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
        headers
            .to_ascii_lowercase()
            .contains("x-locus-streamable: sse-session"),
        "headers={headers}"
    );
    let events = parse_sse_message_events(&resp_body);
    assert_eq!(events.len(), 1, "once=1 should emit one tick: {resp_body}");
    let tick = &events[0];
    assert_eq!(tick["method"], "notifications/message");
    let data = &tick["params"]["data"];
    assert_eq!(data["kind"], "locus.session_tick");
    assert!(data.get("session_ok").is_some(), "{tick}");
    assert!(data.get("doctor_verdict").is_some(), "{tick}");
    assert!(data.get("safe_next").is_some(), "{tick}");
    assert_eq!(data["pinned"], false); // unpinned test home
                                       // Values-free: no secret material.
    let lower = resp_body.to_ascii_lowercase();
    assert!(!lower.contains("phm_"), "{resp_body}");
    assert!(!lower.contains("secret"), "{resp_body}");
}

#[test]
fn http_get_mcp_advertises_session_sse() {
    let srv = HttpServer::spawn("caps-sse-endpoint");
    let (status, _, body) = srv.request("GET", "/mcp", None, Some(&srv.token));
    assert_eq!(status, 200, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["endpoints"]["session_sse"], "GET /mcp/sse");
    assert_eq!(v["streamable"]["sse"], "multi-message-for-large-tools-call");
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

    // Non-initialize POST without session header → served statelessly, no mint
    // (arbitrary POSTs must not consume session capacity).
    let (status, headers, body) = srv.request("POST", "/mcp", Some(&ping), Some(&srv.token));
    assert_eq!(status, 200, "{body}");
    assert!(
        header_lookup(&headers, "Mcp-Session-Id").is_none(),
        "ping must not mint a session: {headers}"
    );

    // initialize without session header → mint.
    let init = serde_json::to_vec(&json!({
        "jsonrpc": "2.0", "id": 2, "method": "initialize", "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "session-test", "version": "0" }
        }
    }))
    .unwrap();
    let (status, headers, body) = srv.request("POST", "/mcp", Some(&init), Some(&srv.token));
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
    // Sessions are minted on initialize only.
    let init = serde_json::to_vec(&json!({
        "jsonrpc": "2.0", "id": 2, "method": "initialize", "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "delete-test", "version": "0" }
        }
    }))
    .unwrap();
    let (status, headers, _) = srv.request("POST", "/mcp", Some(&init), Some(&srv.token));
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

#[test]
fn http_body_too_large_413_pre_auth() {
    let srv = HttpServer::spawn("cap-413-token");
    let mut stream = TcpStream::connect(srv.addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    // Declare a huge body but never send it — server must answer 413 without
    // allocating the buffer, and without requiring auth (pre-auth OOM guard).
    let req = "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: 999999999\r\nConnection: close\r\n\r\n";
    stream.write_all(req.as_bytes()).unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    let (status, _, body) = parse_http_response(&raw);
    assert_eq!(status, 413, "{raw}");
    assert!(body.contains("payload_too_large"), "{body}");
}

#[test]
fn http_headers_too_large_431_pre_auth() {
    let srv = HttpServer::spawn("cap-431-token");
    let mut stream = TcpStream::connect(srv.addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut req = String::from("GET /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\n");
    for i in 0..200 {
        req.push_str(&format!("X-Flood-{i}: v\r\n"));
    }
    req.push_str("Connection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    let (status, _, body) = parse_http_response(&raw);
    assert_eq!(status, 431, "{raw}");
    assert!(body.contains("request_header_fields_too_large"), "{body}");
}

#[test]
fn http_bad_json_body_400_and_no_session_mint() {
    let srv = HttpServer::spawn("bad-json-token");
    let garbage = b"this is not json {".to_vec();
    let (status, headers, body) = srv.request("POST", "/mcp", Some(&garbage), Some(&srv.token));
    assert_eq!(status, 400, "{body}");
    assert!(
        body.contains("-32700") || body.contains("parse error"),
        "{body}"
    );
    assert!(
        header_lookup(&headers, "Mcp-Session-Id").is_none(),
        "garbage must not mint a session: {headers}"
    );
}

// ─── MCP session pin-anchoring over HTTP (per Mcp-Session-Id, file-backed) ──

/// Two sample bindings (different tenants + binding ids) for pin-swap tests.
fn anchor_test_bindings(store: &Store) {
    for (id, alias, tenant, account, phm) in [
        (
            "bnd_acme",
            "acme",
            "acme-corp",
            "acme-gh",
            "phm:GH_TOKEN_ACME",
        ),
        (
            "bnd_beta",
            "beta",
            "beta-corp",
            "beta-gh",
            "phm:GH_TOKEN_BETA",
        ),
    ] {
        let binding = Binding::from_body(BindingBody {
            id: id.into(),
            alias: alias.into(),
            tenant: tenant.into(),
            principal: None,
            description: None,
            policy: Policy::default(),
            providers: vec![ProviderBinding {
                provider: "github".into(),
                account: account.into(),
                credential_ref: phm.into(),
                scope: Scope::default(),
                upstream: None,
            }],
        });
        store.save_binding(&binding).unwrap();
    }
}

fn rpc_initialize(id: i64) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "jsonrpc": "2.0", "id": id, "method": "initialize", "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "anchor-test", "version": "0" }
        }
    }))
    .unwrap()
}

fn rpc_tools_call(id: i64, tool: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": { "name": tool, "arguments": {} }
    }))
    .unwrap()
}

fn tool_result_text(body: &str) -> (String, bool) {
    let v: Value = serde_json::from_str(body).expect("jsonrpc body");
    let result = v.get("result").unwrap_or(&Value::Null);
    let is_error = result
        .get("isError")
        .and_then(|e| e.as_bool())
        .unwrap_or(false);
    let text = result["content"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string();
    (text, is_error)
}

/// Full pin-swap flow over HTTP: per-session anchoring at initialize; a
/// cross-alias re-pin refuses on anchored ids (pin_changed outranks the staled
/// executor grant); GET /mcp reports anchor_ok=false; the anchor is enforced
/// from disk across a server restart; POST initialize WITH the id adopts the
/// new pin (stable hub session id); a never-re-initialized sibling id keeps
/// refusing; verify_session gates per-session; DELETE clears.
#[test]
fn http_anchor_pin_swap_resume_and_adoption() {
    let dir = tempdir().unwrap();
    let home = dir.path().join("locus-home");
    std::fs::create_dir_all(&home).unwrap();
    let store = Store::open(&home).unwrap();
    anchor_test_bindings(&store);
    store
        .pin("acme", dir.path(), Some("http-anchor".into()), false)
        .unwrap();

    let srv1 = HttpServer::spawn_with_home("anchor-swap-token", &home, &[]);

    // Mint two sessions while acme is pinned — both anchor to acme.
    let (status, headers, _) =
        srv1.request("POST", "/mcp", Some(&rpc_initialize(1)), Some(&srv1.token));
    assert_eq!(status, 200);
    let sid1 = header_lookup(&headers, "Mcp-Session-Id").expect("sid1");
    let (status, headers, _) =
        srv1.request("POST", "/mcp", Some(&rpc_initialize(2)), Some(&srv1.token));
    assert_eq!(status, 200);
    let sid2 = header_lookup(&headers, "Mcp-Session-Id").expect("sid2");

    // Healthy anchored call.
    let (status, _, body) = srv1.request_with_headers(
        "POST",
        "/mcp",
        Some(&rpc_tools_call(3, "github.scope")),
        Some(&srv1.token),
        &[("Mcp-Session-Id", &sid1)],
    );
    assert_eq!(status, 200, "{body}");
    let (text, is_err) = tool_result_text(&body);
    assert!(!is_err, "pre-swap call failed: {text}");
    assert!(text.contains("acme-corp"), "{text}");

    // Disk record carries the anchor and stays values-free.
    let rec_path = home.join("http-sessions").join(format!("{sid1}.json"));
    let raw = std::fs::read_to_string(&rec_path).unwrap();
    assert!(
        raw.contains("\"anchor\""),
        "disk record must persist anchor: {raw}"
    );
    let lower = raw.to_ascii_lowercase();
    for banned in ["phm:", "credential", "token", "secret", "api_key"] {
        assert!(
            !lower.contains(banned),
            "disk record leaked `{banned}`: {raw}"
        );
    }

    // Cross-alias re-pin in another terminal.
    store.leave().unwrap();
    store
        .pin("beta", dir.path(), Some("http-anchor".into()), false)
        .unwrap();

    // Anchored session refuses with pin_changed (not runtime_unhealthy).
    let (status, _, body) = srv1.request_with_headers(
        "POST",
        "/mcp",
        Some(&rpc_tools_call(4, "github.scope")),
        Some(&srv1.token),
        &[("Mcp-Session-Id", &sid1)],
    );
    assert_eq!(status, 200, "{body}");
    let (text, is_err) = tool_result_text(&body);
    assert!(is_err, "{text}");
    let refusal: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(refusal["error"], "pin_changed", "{refusal}");
    assert_eq!(refusal["anchored"]["alias"], "acme", "{refusal}");
    assert_eq!(refusal["current"]["alias"], "beta", "{refusal}");
    assert_eq!(
        refusal["safe_next"]["action"], "reinitialize_client",
        "{refusal}"
    );

    // GET /mcp with the mismatched id reports anchor_ok=false (values-free).
    let (status, _, body) = srv1.request_with_headers(
        "GET",
        "/mcp",
        None,
        Some(&srv1.token),
        &[("Mcp-Session-Id", &sid1)],
    );
    assert_eq!(status, 200, "{body}");
    let caps: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(caps["anchor_ok"], false, "{caps}");
    assert_eq!(caps["anchor"]["alias"], "acme", "{caps}");

    // Server restart (fresh executor grant for beta): the anchor is enforced
    // from the file-backed record — the resumed session still refuses.
    drop(srv1);
    let srv2 = HttpServer::spawn_with_home("anchor-swap-token", &home, &[]);
    let (status, _, body) = srv2.request_with_headers(
        "POST",
        "/mcp",
        Some(&rpc_tools_call(5, "github.scope")),
        Some(&srv2.token),
        &[("Mcp-Session-Id", &sid1)],
    );
    assert_eq!(status, 200, "{body}");
    let (text, is_err) = tool_result_text(&body);
    assert!(is_err, "{text}");
    assert!(
        text.contains("pin_changed"),
        "anchor must survive restart: {text}"
    );

    // POST initialize WITH the existing id adopts the new pin (audited) and
    // keeps the hub-stable session id working.
    let (status, headers, _) = srv2.request_with_headers(
        "POST",
        "/mcp",
        Some(&rpc_initialize(6)),
        Some(&srv2.token),
        &[("Mcp-Session-Id", &sid1)],
    );
    assert_eq!(status, 200);
    assert_eq!(
        header_lookup(&headers, "Mcp-Session-Id").as_deref(),
        Some(sid1.as_str()),
        "initialize with header must keep the session id"
    );
    let (status, _, body) = srv2.request_with_headers(
        "POST",
        "/mcp",
        Some(&rpc_tools_call(7, "github.scope")),
        Some(&srv2.token),
        &[("Mcp-Session-Id", &sid1)],
    );
    assert_eq!(status, 200, "{body}");
    let (text, is_err) = tool_result_text(&body);
    assert!(!is_err, "adopted session must operate as beta: {text}");
    assert!(text.contains("beta-corp"), "{text}");
    let events = store.read_audit_events().unwrap();
    assert!(
        events.iter().any(|e| e.op == "mcp.anchor_reset"),
        "expected mcp.anchor_reset audit"
    );

    // The never-re-initialized sibling still refuses after the restart…
    let (status, _, body) = srv2.request_with_headers(
        "POST",
        "/mcp",
        Some(&rpc_tools_call(8, "github.scope")),
        Some(&srv2.token),
        &[("Mcp-Session-Id", &sid2)],
    );
    assert_eq!(status, 200, "{body}");
    let (text, is_err) = tool_result_text(&body);
    assert!(is_err && text.contains("pin_changed"), "{text}");

    // …and its verify_session gates per-session (hub contract), while
    // isError stays false.
    let (status, _, body) = srv2.request_with_headers(
        "POST",
        "/mcp",
        Some(&rpc_tools_call(9, "locus_verify_session")),
        Some(&srv2.token),
        &[("Mcp-Session-Id", &sid2)],
    );
    assert_eq!(status, 200, "{body}");
    let (text, is_err) = tool_result_text(&body);
    assert!(!is_err, "verify_session pack still returns: {text}");
    let pack: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(pack["session_ok"], false, "{pack}");
    assert_eq!(pack["mcp_anchor_mismatch"], true, "{pack}");
    assert_eq!(pack["safe_next"]["action"], "reinitialize_client", "{pack}");
    assert_eq!(pack["mcp_anchor"]["anchored_alias"], "acme", "{pack}");
    assert_eq!(pack["mcp_anchor"]["current_alias"], "beta", "{pack}");
    assert_eq!(pack["mcp_anchor"]["match"], false, "{pack}");

    // A fresh initialize (no header) mints a session anchored to beta.
    let (status, headers, _) =
        srv2.request("POST", "/mcp", Some(&rpc_initialize(10)), Some(&srv2.token));
    assert_eq!(status, 200);
    let sid3 = header_lookup(&headers, "Mcp-Session-Id").expect("sid3");
    assert_ne!(sid3, sid1);
    let (status, _, body) = srv2.request_with_headers(
        "POST",
        "/mcp",
        Some(&rpc_tools_call(11, "github.scope")),
        Some(&srv2.token),
        &[("Mcp-Session-Id", &sid3)],
    );
    assert_eq!(status, 200, "{body}");
    let (text, is_err) = tool_result_text(&body);
    assert!(!is_err && text.contains("beta-corp"), "{text}");

    // DELETE clears the anchored session (memory + disk).
    let (status, _, _) = srv2.request_with_headers(
        "DELETE",
        "/mcp",
        None,
        Some(&srv2.token),
        &[("Mcp-Session-Id", &sid2)],
    );
    assert_eq!(status, 204);
    assert!(!home
        .join("http-sessions")
        .join(format!("{sid2}.json"))
        .is_file());

    // The refusal never froze the (healthy, beta) store pin.
    let active = store.active_session().unwrap().expect("beta pin");
    assert_eq!(active.binding_alias, "beta");
    assert!(
        !active.frozen,
        "anchor refusals must not freeze active.json"
    );
}

/// Legacy v1 disk records (predating the anchor field) resume fine and adopt
/// an anchor at the next healthy observation.
#[test]
fn http_legacy_session_record_without_anchor_adopts_once() {
    let dir = tempdir().unwrap();
    let home = dir.path().join("locus-home");
    std::fs::create_dir_all(&home).unwrap();
    let store = Store::open(&home).unwrap();
    anchor_test_bindings(&store);
    store
        .pin("acme", dir.path(), Some("http-legacy".into()), false)
        .unwrap();

    // Hand-written v1 record without the anchor field.
    let sid = "abcdefabcdefabcdefabcdefabcdef01";
    let sessions = home.join("http-sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    std::fs::write(
        sessions.join(format!("{sid}.json")),
        format!("{{\"v\":1,\"id\":\"{sid}\",\"created_at_unix\":{now},\"last_seen_unix\":{now}}}"),
    )
    .unwrap();

    let srv = HttpServer::spawn_with_home("legacy-anchor-token", &home, &[]);
    let (status, _, body) = srv.request_with_headers(
        "POST",
        "/mcp",
        Some(&rpc_tools_call(1, "github.scope")),
        Some(&srv.token),
        &[("Mcp-Session-Id", sid)],
    );
    assert_eq!(status, 200, "{body}");
    let (text, is_err) = tool_result_text(&body);
    assert!(!is_err, "legacy session must resume + operate: {text}");
    let raw = std::fs::read_to_string(sessions.join(format!("{sid}.json"))).unwrap();
    assert!(
        raw.contains("\"anchor\""),
        "legacy record must adopt an anchor on first healthy observation: {raw}"
    );
}

/// Sessionless POSTs share the process-level anchor: omitting the
/// Mcp-Session-Id header must NOT bypass pin-swap protection. A cross-alias
/// re-pin refuses stateless provider tools/call with pin_changed until a
/// fresh sessionless initialize adopts the new pin.
#[test]
fn http_sessionless_provider_calls_are_pin_swap_anchored() {
    let dir = tempdir().unwrap();
    let home = dir.path().join("locus-home");
    std::fs::create_dir_all(&home).unwrap();
    let store = Store::open(&home).unwrap();
    anchor_test_bindings(&store);
    store
        .pin(
            "acme",
            dir.path(),
            Some("http-stateless-anchor".into()),
            false,
        )
        .unwrap();

    let srv = HttpServer::spawn_with_home("stateless-anchor-token", &home, &[]);

    // First sessionless call establishes the process anchor on acme.
    let (status, _, body) = srv.request(
        "POST",
        "/mcp",
        Some(&rpc_tools_call(1, "github.scope")),
        Some(&srv.token),
    );
    assert_eq!(status, 200, "{body}");
    let (text, is_err) = tool_result_text(&body);
    assert!(!is_err, "pre-swap stateless call must work: {text}");
    assert!(text.contains("acme-corp"), "{text}");

    // Cross-alias re-pin in another terminal.
    store.leave().unwrap();
    store
        .pin(
            "beta",
            dir.path(),
            Some("http-stateless-anchor".into()),
            false,
        )
        .unwrap();

    // Stateless provider call must fail closed with pin_changed — omitting
    // the header is not an anchor bypass.
    let (status, _, body) = srv.request(
        "POST",
        "/mcp",
        Some(&rpc_tools_call(2, "github.scope")),
        Some(&srv.token),
    );
    assert_eq!(status, 200, "{body}");
    let (text, is_err) = tool_result_text(&body);
    assert!(is_err, "stateless call after pin swap must refuse: {text}");
    let refusal: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(refusal["error"], "pin_changed", "{refusal}");
    assert_eq!(refusal["anchored"]["alias"], "acme", "{refusal}");
    assert_eq!(refusal["current"]["alias"], "beta", "{refusal}");

    // A fresh sessionless initialize is the adoption path for the process
    // anchor (mints a session id the stateless client is free to ignore):
    // afterwards the stateless call is no longer anchor-refused — the
    // remaining refusal is the staled executor grant (authority-plane fact,
    // cleared by a server restart), never `pin_changed`.
    let (status, _, _) = srv.request("POST", "/mcp", Some(&rpc_initialize(3)), Some(&srv.token));
    assert_eq!(status, 200);
    let (status, _, body) = srv.request(
        "POST",
        "/mcp",
        Some(&rpc_tools_call(4, "github.scope")),
        Some(&srv.token),
    );
    assert_eq!(status, 200, "{body}");
    let (text, _) = tool_result_text(&body);
    assert!(
        !text.contains("pin_changed"),
        "sessionless initialize must adopt / clear the process anchor: {text}"
    );

    // Restart (fresh executor grant for beta): stateless calls operate as
    // beta again.
    drop(srv);
    let srv2 = HttpServer::spawn_with_home("stateless-anchor-token", &home, &[]);
    let (status, _, body) = srv2.request(
        "POST",
        "/mcp",
        Some(&rpc_tools_call(5, "github.scope")),
        Some(&srv2.token),
    );
    assert_eq!(status, 200, "{body}");
    let (text, is_err) = tool_result_text(&body);
    assert!(!is_err, "post-restart stateless call must work: {text}");
    assert!(text.contains("beta-corp"), "{text}");

    // The refusal never froze the healthy beta store pin.
    let active = store.active_session().unwrap().expect("beta pin");
    assert_eq!(active.binding_alias, "beta");
    assert!(!active.frozen);
}

/// Sessionless POST tools/call keeps today's stateless behavior by default and
/// turns strict (400) under LOCUS_MCP_HTTP_REQUIRE_SESSION=1 — control tools
/// stay callable sessionless either way.
#[test]
fn http_sessionless_provider_calls_default_and_require_session_env() {
    let dir = tempdir().unwrap();
    let home = dir.path().join("locus-home");
    std::fs::create_dir_all(&home).unwrap();
    let store = Store::open(&home).unwrap();
    anchor_test_bindings(&store);
    store
        .pin("acme", dir.path(), Some("http-stateless".into()), false)
        .unwrap();

    // Default: sessionless provider tools/call served statelessly (existing CI flows).
    let srv = HttpServer::spawn_with_home("stateless-token", &home, &[]);
    let (status, headers, body) = srv.request(
        "POST",
        "/mcp",
        Some(&rpc_tools_call(1, "github.scope")),
        Some(&srv.token),
    );
    assert_eq!(status, 200, "{body}");
    assert!(header_lookup(&headers, "Mcp-Session-Id").is_none());
    let (text, is_err) = tool_result_text(&body);
    assert!(
        !is_err,
        "default sessionless call must keep working: {text}"
    );
    drop(srv);

    // Strict: provider tools/call requires a session; control tools do not.
    let srv = HttpServer::spawn_with_home(
        "strict-token",
        &home,
        &[("LOCUS_MCP_HTTP_REQUIRE_SESSION", "1")],
    );
    let (status, _, body) = srv.request(
        "POST",
        "/mcp",
        Some(&rpc_tools_call(2, "github.scope")),
        Some(&srv.token),
    );
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("session_required"), "{body}");

    let (status, _, body) = srv.request(
        "POST",
        "/mcp",
        Some(&rpc_tools_call(3, "locus_whoami")),
        Some(&srv.token),
    );
    assert_eq!(status, 200, "control tools stay sessionless: {body}");

    // With a session, provider calls work under strict mode.
    let (status, headers, _) =
        srv.request("POST", "/mcp", Some(&rpc_initialize(4)), Some(&srv.token));
    assert_eq!(status, 200);
    let sid = header_lookup(&headers, "Mcp-Session-Id").expect("sid");
    let (status, _, body) = srv.request_with_headers(
        "POST",
        "/mcp",
        Some(&rpc_tools_call(5, "github.scope")),
        Some(&srv.token),
        &[("Mcp-Session-Id", &sid)],
    );
    assert_eq!(status, 200, "{body}");
    let (text, is_err) = tool_result_text(&body);
    assert!(!is_err, "{text}");
}

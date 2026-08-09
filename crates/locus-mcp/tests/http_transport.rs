//! HTTP transport: health probe + token auth reject/accept + JSON-RPC POST /mcp.

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

        let port = free_port();
        let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let mut child = Command::new(mcp_bin())
            .arg("--http")
            .arg(addr.to_string())
            .env("LOCUS_HOME", &home_owned)
            .env("LOCUS_MCP_HTTP_TOKEN", token)
            .env("LOCUS_MCP_AUTO_PIN", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn locus-mcp --http");

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
    let (status, _, resp) = srv.request("POST", "/mcp", Some(&body), Some(&srv.token));
    assert_eq!(status, 200, "{resp}");
    let v: Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(v["result"]["serverInfo"]["name"], "locus-mcp");

    let list = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    });
    let body = serde_json::to_vec(&list).unwrap();
    let (status, _, resp) = srv.request("POST", "/mcp", Some(&body), Some(&srv.token));
    assert_eq!(status, 200, "{resp}");
    let v: Value = serde_json::from_str(&resp).unwrap();
    let tools = v["result"]["tools"].as_array().expect("tools");
    assert!(
        tools.iter().any(|t| t["name"] == "locus_whoami"),
        "expected control tools, got {tools:?}"
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

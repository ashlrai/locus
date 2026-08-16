//! Multi-tenant HTTP transport: per-request grant routing, tenant isolation,
//! fail-closed token/session handling, revoke/TTL lifecycle.
//!
//! Lives in its own test binary (not http_transport.rs) so a process-wide
//! `LOCUS_CONTROL_CAPABILITY` can be pinned BEFORE any store/broker work —
//! multi-tenant servers validate every tenant session via the operator
//! control capability, which must be shared between this process (minting)
//! and the spawned server. Scratch LOCUS_HOME only; never ~/.locus.

use locus_core::{Binding, BindingBody, Policy, ProviderBinding, Scope, Store};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};
use tempfile::tempdir;

/// Pin one control capability for the whole test process (env-derived brokers
/// stay consistent) and share it with every spawned server.
fn control_cap() -> &'static str {
    static CAP: OnceLock<String> = OnceLock::new();
    CAP.get_or_init(|| {
        if let Ok(v) = std::env::var("LOCUS_CONTROL_CAPABILITY") {
            if !v.trim().is_empty() {
                return v;
            }
        }
        // 32 bytes lowercase hex — the shape decode_capability requires.
        let mut state: u64 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64
            ^ u64::from(std::process::id());
        let cap: String = (0..64)
            .map(|_| {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                char::from_digit(((state >> 32) % 16) as u32, 16).unwrap()
            })
            .collect();
        std::env::set_var("LOCUS_CONTROL_CAPABILITY", &cap);
        cap
    })
}

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

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("ephemeral bind");
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

struct MtServer {
    child: Child,
    addr: SocketAddr,
    token: String,
}

impl MtServer {
    fn spawn(home: &std::path::Path, server_token: &str) -> Self {
        let cap = control_cap().to_string();
        let port = free_port();
        let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let mut child = Command::new(mcp_bin())
            .arg("--http")
            .arg(addr.to_string())
            .arg("--multi-tenant")
            .env("LOCUS_HOME", home)
            .env("LOCUS_MCP_HTTP_TOKEN", server_token)
            .env("LOCUS_MCP_AUTO_PIN", "0")
            .env("LOCUS_CONTROL_CAPABILITY", cap)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn locus-mcp --http --multi-tenant");

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if Instant::now() > deadline {
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() {
                    let _ = e.read_to_string(&mut err);
                }
                let _ = child.kill();
                panic!("MT server did not become ready: {err}");
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
            token: server_token.to_string(),
        }
    }

    /// One HTTP request. `tenant_token` rides X-Locus-Tenant-Token; `session`
    /// rides Mcp-Session-Id.
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
        tenant_token: Option<&str>,
        session: Option<&str>,
    ) -> (u16, String, String) {
        let mut stream = TcpStream::connect(self.addr).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let body = body.unwrap_or(b"");
        let mut req = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\nConnection: close\r\nAuthorization: Bearer {}\r\n",
            body.len(),
            self.token
        );
        if !body.is_empty() {
            req.push_str("Content-Type: application/json\r\n");
        }
        if let Some(t) = tenant_token {
            req.push_str(&format!("X-Locus-Tenant-Token: {t}\r\n"));
        }
        if let Some(id) = session {
            req.push_str(&format!("Mcp-Session-Id: {id}\r\n"));
        }
        req.push_str("\r\n");
        stream.write_all(req.as_bytes()).unwrap();
        if !body.is_empty() {
            stream.write_all(body).unwrap();
        }
        stream.flush().unwrap();
        let mut raw = String::new();
        stream.read_to_string(&mut raw).unwrap();
        parse_http_response(&raw)
    }
}

impl Drop for MtServer {
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

/// Two bindings on DIFFERENT providers so catalog exclusivity is observable.
fn seed_bindings(store: &Store) {
    for (id, alias, tenant, provider, account, phm) in [
        (
            "bnd_acme",
            "acme",
            "acme-corp",
            "github",
            "acme-gh",
            "phm:GH_TOKEN_ACME",
        ),
        (
            "bnd_beta",
            "beta",
            "beta-corp",
            "supabase",
            "beta-sb",
            "phm:SB_TOKEN_BETA",
        ),
        (
            "bnd_gamma",
            "gamma",
            "gamma-corp",
            "vercel",
            "gamma-vc",
            "phm:VC_TOKEN_GAMMA",
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
                provider: provider.into(),
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
            "clientInfo": { "name": "mt-test", "version": "0" }
        }
    }))
    .unwrap()
}

fn rpc_tools_list(id: i64) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "jsonrpc": "2.0", "id": id, "method": "tools/list", "params": {}
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

fn tool_names(body: &str) -> Vec<String> {
    let v: Value = serde_json::from_str(body).expect("jsonrpc body");
    v["result"]["tools"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|t| t["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
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

struct MtWorld {
    _dir: tempfile::TempDir,
    home: PathBuf,
    store: Store,
    token_a: String,
    token_b: String,
    grant_a: locus_core::McpGrant,
    grant_b: locus_core::McpGrant,
    srv: MtServer,
}

/// Shared setup: scratch home, acme+beta grants, one MT server.
fn mt_world(server_token: &str) -> MtWorld {
    let _ = control_cap();
    let dir = tempdir().unwrap();
    let home = dir.path().join("locus-home");
    std::fs::create_dir_all(&home).unwrap();
    let store = Store::open(&home).unwrap();
    seed_bindings(&store);
    let (_sa, grant_a, token_a) = store
        .create_mcp_grant("acme", dir.path(), None, Some("job-a".into()), false)
        .unwrap();
    let (_sb, grant_b, token_b) = store
        .create_mcp_grant("beta", dir.path(), None, Some("job-b".into()), false)
        .unwrap();
    let srv = MtServer::spawn(&home, server_token);
    MtWorld {
        _dir: dir,
        home,
        store,
        token_a,
        token_b,
        grant_a,
        grant_b,
        srv,
    }
}

fn initialize_session(w: &MtWorld, token: &str, id: i64) -> String {
    let (status, headers, body) =
        w.srv
            .request("POST", "/mcp", Some(&rpc_initialize(id)), Some(token), None);
    assert_eq!(status, 200, "initialize failed: {body}");
    header_lookup(&headers, "Mcp-Session-Id").expect("session id minted")
}

#[test]
fn mt_cors_preflight_allows_tenant_token_header() {
    let w = mt_world("mt-cors-token");
    // Browsers strip Authorization and custom headers on preflight, so send a
    // raw credential-less OPTIONS — it must succeed without any token.
    let mut stream = TcpStream::connect(w.srv.addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream
        .write_all(
            b"OPTIONS /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: http://localhost:5173\r\n\
              Access-Control-Request-Method: POST\r\n\
              Access-Control-Request-Headers: x-locus-tenant-token\r\n\
              Connection: close\r\n\r\n",
        )
        .unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    let (status, headers, _body) = parse_http_response(&raw);
    assert_eq!(status, 204, "preflight: {headers}");
    let allow = header_lookup(&headers, "Access-Control-Allow-Headers")
        .expect("Access-Control-Allow-Headers present");
    let allow_l = allow.to_ascii_lowercase();
    // Browser clients must be able to send the MT bearer header preflight-clean.
    for required in ["x-locus-tenant-token", "authorization", "mcp-session-id"] {
        assert!(
            allow_l.contains(required),
            "Allow-Headers missing `{required}`: {allow}"
        );
    }
}

#[test]
fn mt_two_tenants_isolated_catalogs_identities_and_cross_tenant_403() {
    let w = mt_world("mt-iso-token");

    let sid_a = initialize_session(&w, &w.token_a, 1);
    let sid_b = initialize_session(&w, &w.token_b, 2);
    assert_ne!(sid_a, sid_b);

    // Catalogs are exclusive per binding: acme sees github.*, never supabase.*.
    let (status, _, body) = w.srv.request(
        "POST",
        "/mcp",
        Some(&rpc_tools_list(3)),
        Some(&w.token_a),
        Some(&sid_a),
    );
    assert_eq!(status, 200, "{body}");
    let names_a = tool_names(&body);
    assert!(
        names_a.iter().any(|n| n == "github.scope"),
        "acme catalog missing github tools: {names_a:?}"
    );
    assert!(
        !names_a.iter().any(|n| n.starts_with("supabase.")),
        "acme catalog leaked beta provider: {names_a:?}"
    );

    let (status, _, body) = w.srv.request(
        "POST",
        "/mcp",
        Some(&rpc_tools_list(4)),
        Some(&w.token_b),
        Some(&sid_b),
    );
    assert_eq!(status, 200, "{body}");
    let names_b = tool_names(&body);
    assert!(
        names_b.iter().any(|n| n == "supabase.scope"),
        "beta catalog missing supabase tools: {names_b:?}"
    );
    assert!(
        !names_b.iter().any(|n| n.starts_with("github.")),
        "beta catalog leaked acme provider: {names_b:?}"
    );

    // Identities answer per grant, with grant_id attached.
    let (status, _, body) = w.srv.request(
        "POST",
        "/mcp",
        Some(&rpc_tools_call(5, "locus_whoami")),
        Some(&w.token_a),
        Some(&sid_a),
    );
    assert_eq!(status, 200);
    let (text, is_err) = tool_result_text(&body);
    assert!(!is_err, "{text}");
    let who: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(who["binding_alias"], "acme");
    assert_eq!(who["grant_id"], json!(w.grant_a.grant_id));

    let (_, _, body) = w.srv.request(
        "POST",
        "/mcp",
        Some(&rpc_tools_call(6, "locus_whoami")),
        Some(&w.token_b),
        Some(&sid_b),
    );
    let (text, _) = tool_result_text(&body);
    let who: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(who["binding_alias"], "beta");

    // Provider call works and is audited with grant + http session ids.
    let (status, _, body) = w.srv.request(
        "POST",
        "/mcp",
        Some(&rpc_tools_call(7, "github.scope")),
        Some(&w.token_a),
        Some(&sid_a),
    );
    assert_eq!(status, 200);
    let (text, is_err) = tool_result_text(&body);
    assert!(!is_err, "provider call refused: {text}");

    // Cross-tenant session use: A's session id with B's token → 403.
    let (status, _, body) = w.srv.request(
        "POST",
        "/mcp",
        Some(&rpc_tools_list(8)),
        Some(&w.token_b),
        Some(&sid_a),
    );
    assert_eq!(status, 403, "cross-tenant session must 403: {body}");
    assert!(body.contains("tenant_mismatch"), "{body}");

    // locus_request_pin inside a tenant → structured refusal.
    let (_, _, body) = w.srv.request(
        "POST",
        "/mcp",
        Some(
            &serde_json::to_vec(&json!({
                "jsonrpc": "2.0", "id": 9, "method": "tools/call",
                "params": { "name": "locus_request_pin", "arguments": {"alias": "beta"} }
            }))
            .unwrap(),
        ),
        Some(&w.token_a),
        Some(&sid_a),
    );
    let (text, is_err) = tool_result_text(&body);
    assert!(is_err);
    assert!(text.contains("tenant_fixed_by_grant"), "{text}");

    // Audit: per-call rows carry grant_id + http_session_id; token never logged.
    let audit_raw = std::fs::read_to_string(w.home.join("audit").join("events.jsonl")).unwrap();
    assert!(
        audit_raw.contains(&w.grant_a.grant_id),
        "audit missing grant id"
    );
    assert!(audit_raw.contains("mcp.tenant_session_bound"));
    assert!(audit_raw.contains(&sid_a), "audit missing http session id");
    assert!(
        !audit_raw.contains("lmt_"),
        "bearer token leaked into audit"
    );
}

#[test]
fn mt_tenantless_stateless_and_bad_tokens_fail_closed() {
    let w = mt_world("mt-fail-token");

    // No tenant token → uniform 401 invalid_grant (even for initialize).
    let (status, _, body) = w
        .srv
        .request("POST", "/mcp", Some(&rpc_initialize(1)), None, None);
    assert_eq!(status, 401, "{body}");
    assert!(body.contains("invalid_grant"), "{body}");

    // Garbage and wrong-secret tokens → uniform 401.
    for bad in [
        "not-a-token".to_string(),
        format!("lmt_{}.{}", "0".repeat(16), "0".repeat(64)),
        format!("lmt_{}.{}", &w.grant_a.grant_id, "0".repeat(64)),
    ] {
        let (status, _, body) =
            w.srv
                .request("POST", "/mcp", Some(&rpc_initialize(2)), Some(&bad), None);
        assert_eq!(status, 401, "token {bad} must 401: {body}");
        assert!(body.contains("invalid_grant"), "{body}");
    }

    // Valid token but stateless non-initialize POST → session_required.
    let (status, _, body) = w.srv.request(
        "POST",
        "/mcp",
        Some(&rpc_tools_call(3, "github.scope")),
        Some(&w.token_a),
        None,
    );
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("session_required"), "{body}");

    // GET /mcp without tenant token → 401 too (no tenantless surfaces).
    let (status, _, body) = w.srv.request("GET", "/mcp", None, None, None);
    assert_eq!(status, 401, "{body}");

    // Unknown session id with a valid token → 404 fail closed.
    let (status, _, _) = w.srv.request(
        "POST",
        "/mcp",
        Some(&rpc_tools_list(4)),
        Some(&w.token_a),
        Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
    );
    assert_eq!(status, 404);
}

#[test]
fn mt_capabilities_and_sse_are_single_tenant_and_values_free() {
    let w = mt_world("mt-caps-token");
    let sid_a = initialize_session(&w, &w.token_a, 1);

    let (status, _, body) = w
        .srv
        .request("GET", "/mcp", None, Some(&w.token_a), Some(&sid_a));
    assert_eq!(status, 200, "{body}");
    let caps: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(caps["mode"], "multi_tenant");
    assert_eq!(caps["grant_id"], json!(w.grant_a.grant_id));
    assert_eq!(caps["pin"]["binding_alias"], "acme");
    assert_eq!(caps["anchor_ok"], true);
    // Single-tenant only: no other tenant's alias/provider; values-free.
    assert!(!body.contains("beta"), "cross-tenant leak: {body}");
    assert!(!body.contains("supabase"), "cross-tenant leak: {body}");
    assert!(!body.contains("lmt_"), "token leak: {body}");
    assert!(!body.contains("phm:"), "credential leak: {body}");

    // SSE tick is computed from THIS grant only.
    let mut stream = TcpStream::connect(w.srv.addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let req = format!(
        "GET /mcp/sse?once=1 HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: text/event-stream\r\nAuthorization: Bearer {}\r\nX-Locus-Tenant-Token: {}\r\nConnection: close\r\n\r\n",
        w.srv.token, w.token_a
    );
    stream.write_all(req.as_bytes()).unwrap();
    let mut raw = String::new();
    let _ = stream.read_to_string(&mut raw);
    assert!(raw.contains("locus.session_tick"), "{raw}");
    assert!(raw.contains(&w.grant_a.grant_id), "{raw}");
    assert!(raw.contains("\"session_ok\":true"), "{raw}");
    assert!(!raw.contains("beta"), "cross-tenant leak in tick: {raw}");

    // SSE without the tenant token → 401.
    let (status, _, _) = w.srv.request("GET", "/mcp/sse?once=1", None, None, None);
    assert_eq!(status, 401);
}

#[test]
fn mt_revoke_mid_flight_expiry_and_delete_lifecycle() {
    let w = mt_world("mt-lifecycle-token");
    let sid_a = initialize_session(&w, &w.token_a, 1);
    let sid_b = initialize_session(&w, &w.token_b, 2);

    // Healthy call for A.
    let (_, _, body) = w.srv.request(
        "POST",
        "/mcp",
        Some(&rpc_tools_call(3, "github.scope")),
        Some(&w.token_a),
        Some(&sid_a),
    );
    let (text, is_err) = tool_result_text(&body);
    assert!(!is_err, "{text}");

    // Revoke A (operator CLI path) → immediate 401 for A, B unaffected.
    w.store.revoke_mcp_grant(&w.grant_a.grant_id).unwrap();
    let (status, _, body) = w.srv.request(
        "POST",
        "/mcp",
        Some(&rpc_tools_call(4, "github.scope")),
        Some(&w.token_a),
        Some(&sid_a),
    );
    assert_eq!(status, 401, "revoked grant must 401: {body}");
    assert!(body.contains("invalid_grant"), "{body}");

    let (status, _, body) = w.srv.request(
        "POST",
        "/mcp",
        Some(&rpc_tools_call(5, "supabase.scope")),
        Some(&w.token_b),
        Some(&sid_b),
    );
    assert_eq!(status, 200, "{body}");
    let (text, is_err) = tool_result_text(&body);
    assert!(!is_err, "B must keep working after A's revoke: {text}");

    // Expire B by rewriting its grant record (MAC does not cover expiry —
    // the 0600 grant file is operator-trusted storage).
    let grant_path = w
        .home
        .join("mcp-grants")
        .join(format!("{}.json", w.grant_b.grant_id));
    let mut g: Value =
        serde_json::from_str(&std::fs::read_to_string(&grant_path).unwrap()).unwrap();
    g["expires_at"] = json!("2000-01-01T00:00:00Z");
    std::fs::write(&grant_path, serde_json::to_vec(&g).unwrap()).unwrap();
    let (status, _, body) = w.srv.request(
        "POST",
        "/mcp",
        Some(&rpc_tools_call(6, "supabase.scope")),
        Some(&w.token_b),
        Some(&sid_b),
    );
    assert_eq!(status, 401, "{body}");
    assert!(body.contains("grant_expired"), "{body}");
    assert!(
        body.contains("locus mcp mint"),
        "remint hint missing: {body}"
    );

    // Restore B and DELETE its session explicitly.
    g["expires_at"] = json!(w.grant_b.expires_at.to_rfc3339());
    std::fs::write(&grant_path, serde_json::to_vec(&g).unwrap()).unwrap();
    // Session was swept when the grant read expired → re-initialize.
    let sid_b2 = initialize_session(&w, &w.token_b, 7);
    let (status, _, _) = w
        .srv
        .request("DELETE", "/mcp", None, Some(&w.token_b), Some(&sid_b2));
    assert_eq!(status, 204);
    let (status, _, _) = w
        .srv
        .request("DELETE", "/mcp", None, Some(&w.token_b), Some(&sid_b2));
    assert_eq!(status, 404, "deleted session must be gone");
}

#[test]
fn mt_global_pin_and_leave_have_zero_effect_on_tenants() {
    let w = mt_world("mt-global-token");
    let sid_a = initialize_session(&w, &w.token_a, 1);

    // Operator pins gamma globally mid-flight.
    w.store
        .pin("gamma", w.home.parent().unwrap(), None, false)
        .unwrap();

    let (_, _, body) = w.srv.request(
        "POST",
        "/mcp",
        Some(&rpc_tools_call(2, "locus_whoami")),
        Some(&w.token_a),
        Some(&sid_a),
    );
    let (text, is_err) = tool_result_text(&body);
    assert!(!is_err, "{text}");
    let who: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        who["binding_alias"], "acme",
        "tenant identity must ignore the global pin: {text}"
    );

    // Catalog still acme's provider — not gamma's vercel.
    let (_, _, body) = w.srv.request(
        "POST",
        "/mcp",
        Some(&rpc_tools_list(3)),
        Some(&w.token_a),
        Some(&sid_a),
    );
    let names = tool_names(&body);
    assert!(names.iter().any(|n| n == "github.scope"), "{names:?}");
    assert!(
        !names.iter().any(|n| n.starts_with("vercel.")),
        "global pin leaked into tenant catalog: {names:?}"
    );

    // Operator leaves globally — tenant session keeps working.
    w.store.leave().unwrap();
    let (status, _, body) = w.srv.request(
        "POST",
        "/mcp",
        Some(&rpc_tools_call(4, "github.scope")),
        Some(&w.token_a),
        Some(&sid_a),
    );
    assert_eq!(status, 200);
    let (text, is_err) = tool_result_text(&body);
    assert!(!is_err, "tenant must survive global leave: {text}");
}

#[test]
fn stdio_with_multi_tenant_flag_fails_closed() {
    let dir = tempdir().unwrap();
    let out = Command::new(mcp_bin())
        .arg("--multi-tenant")
        .env("LOCUS_HOME", dir.path())
        .env_remove("LOCUS_MCP_HTTP")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .expect("run locus-mcp --multi-tenant");
    assert!(
        !out.status.success(),
        "stdio + --multi-tenant must be a startup error"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("--http"), "{err}");
}

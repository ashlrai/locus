//! Swarm broker black-box acceptance: authenticated grants select graph nodes,
//! catalogs narrow even within one tenant, and every call still meets the
//! existing session/policy gates. All stores and server homes are disposable.

use locus_core::{Binding, BindingBody, McpGrant, Policy, ProviderBinding, Scope, Store};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};
use tempfile::{tempdir, TempDir};

fn control_cap() -> &'static str {
    static CAP: OnceLock<String> = OnceLock::new();
    CAP.get_or_init(|| {
        // Synthetic operator capability, shared only with these scratch stores.
        let cap = "35".repeat(32);
        std::env::set_var("LOCUS_CONTROL_CAPABILITY", &cap);
        cap
    })
}

struct Server {
    child: Child,
    addr: SocketAddr,
}

impl Server {
    fn spawn(home: &Path, graph: &Path) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let mut child = Command::new(env!("CARGO_BIN_EXE_locus-mcp"))
            .args(["--http", &addr.to_string(), "--multi-tenant", "--swarm"])
            .arg(graph)
            .env("HOME", home.parent().unwrap())
            .env("LOCUS_HOME", home)
            .env("LOCUS_CONTROL_CAPABILITY", control_cap())
            .env("LOCUS_MCP_HTTP_TOKEN", "synthetic-swarm-server-token")
            .env("LOCUS_MCP_AUTO_PIN", "0")
            .env_remove("LOCUS_EXECUTOR_CAPABILITY")
            .env_remove("LOCUS_MCP_SESSION_DIR")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn swarm broker");
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                panic!("swarm broker exited before readiness: {status}");
            }
            if TcpStream::connect(addr).is_ok() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("swarm broker did not become ready");
            }
            thread::sleep(Duration::from_millis(20));
        }
        Self { child, addr }
    }

    fn request(
        &self,
        method: &str,
        body: Option<&Value>,
        token: Option<&str>,
        session: Option<&str>,
        extra_headers: &[(&str, &str)],
    ) -> (u16, String, Value) {
        let mut stream = TcpStream::connect(self.addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let body = body
            .map(|v| serde_json::to_vec(v).unwrap())
            .unwrap_or_default();
        let mut request = format!(
            "{method} /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nAccept: application/json\r\nAuthorization: Bearer synthetic-swarm-server-token\r\nContent-Length: {}\r\n",
            body.len()
        );
        if let Some(token) = token {
            request.push_str(&format!("X-Locus-Tenant-Token: {token}\r\n"));
        }
        if let Some(session) = session {
            request.push_str(&format!("Mcp-Session-Id: {session}\r\n"));
        }
        for (name, value) in extra_headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str("\r\n");
        stream.write_all(request.as_bytes()).unwrap();
        stream.write_all(&body).unwrap();
        stream.flush().unwrap();
        let mut raw = String::new();
        stream.read_to_string(&mut raw).unwrap();
        let (headers, body) = raw.split_once("\r\n\r\n").expect("HTTP response");
        let status = headers
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        (
            status,
            headers.to_string(),
            serde_json::from_str(body).expect("JSON response"),
        )
    }

    fn initialize(&self, token: &str) -> String {
        let (status, headers, response) = self.request(
            "POST",
            Some(&rpc(
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05", "capabilities": {},
                    "clientInfo": {"name":"swarm-test", "version":"1"}
                }),
            )),
            Some(token),
            None,
            &[],
        );
        assert_eq!(status, 200);
        assert!(
            response.get("result").is_some(),
            "initialize result required"
        );
        headers
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case("Mcp-Session-Id")
                    .then(|| value.trim().to_owned())
            })
            .expect("MCP session id")
    }

    fn rpc(&self, token: &str, session: &str, method: &str, params: Value) -> Value {
        let (status, _, body) = self.request(
            "POST",
            Some(&rpc(method, params)),
            Some(token),
            Some(session),
            &[],
        );
        assert_eq!(status, 200);
        body
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn rpc(method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0", "id":1, "method":method, "params":params})
}

fn tools(body: &Value) -> Vec<&str> {
    body["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect()
}

fn refused(body: &Value) -> bool {
    body.get("error").is_some() || body["result"]["isError"] == true
}

struct World {
    // Server must drop before the directory that owns its live broker/session files.
    srv: Server,
    store: Store,
    graph: PathBuf,
    grant_a: McpGrant,
    grant_b: McpGrant,
    token_a: String,
    token_b: String,
    token_unknown: String,
    _dir: TempDir,
}

fn world() -> World {
    let _ = control_cap();
    let dir = tempdir().unwrap();
    let home = dir.path().join("locus-home");
    let store = Store::open(&home).unwrap();
    store
        .save_binding(&Binding::from_body(BindingBody {
            id: "bnd_swarm".into(),
            alias: "swarm".into(),
            tenant: "swarm-test".into(),
            principal: None,
            description: None,
            policy: Policy::default(),
            providers: vec![ProviderBinding {
                provider: "github".into(),
                account: "synthetic-account".into(),
                credential_ref: "phm:SYNTHETIC_SWARM_TOKEN".into(),
                scope: Scope::default(),
                upstream: None,
            }],
        }))
        .unwrap();
    let (_, grant_a, token_a) = store
        .create_mcp_grant("swarm", dir.path(), None, Some("agent-a".into()), false)
        .unwrap();
    let (_, grant_b, token_b) = store
        .create_mcp_grant("swarm", dir.path(), None, Some("agent-b".into()), false)
        .unwrap();
    let (_, _, token_unknown) = store
        .create_mcp_grant(
            "swarm",
            dir.path(),
            None,
            Some("unknown-agent".into()),
            false,
        )
        .unwrap();
    let graph = dir.path().join("swarm.json");
    std::fs::write(&graph, serde_json::to_vec(&json!({"version":1, "nodes":[
        {"id":"graph-node-a", "grant_id":grant_a.grant_id, "tools":["locus_whoami", "github.scope", "github.nonexistent"]},
        {"id":"graph-node-b", "grant_id":grant_b.grant_id, "tools":["locus_heartbeat"]}
    ]})).unwrap()).unwrap();
    let srv = Server::spawn(&home, &graph);
    World {
        srv,
        store,
        graph,
        grant_a,
        grant_b,
        token_a,
        token_b,
        token_unknown,
        _dir: dir,
    }
}

#[test]
fn same_tenant_agents_receive_disjoint_exact_catalogs_and_successful_granted_calls() {
    let w = world();
    let a = w.srv.initialize(&w.token_a);
    let b = w.srv.initialize(&w.token_b);
    let list_a = w.srv.rpc(&w.token_a, &a, "tools/list", json!({}));
    let mut names_a = tools(&list_a);
    names_a.sort_unstable();
    assert_eq!(names_a, ["github.scope", "locus_whoami"]);
    let list_b = w.srv.rpc(&w.token_b, &b, "tools/list", json!({}));
    assert_eq!(tools(&list_b), ["locus_heartbeat"]);
    for name in ["locus_whoami", "github.scope"] {
        let called = w.srv.rpc(
            &w.token_a,
            &a,
            "tools/call",
            json!({"name":name,"arguments":{}}),
        );
        assert!(!refused(&called), "granted call must pass existing gates");
    }
    let (_, _, caps) = w.srv.request("GET", None, Some(&w.token_b), None, &[]);
    assert_eq!(caps["tools"], json!(["locus_heartbeat"]));
}

#[test]
fn unknown_agent_has_zero_tools_even_with_a_valid_grant_and_global_pin() {
    let w = world();
    w.store.pin("swarm", w._dir.path(), None, false).unwrap();
    let session = w.srv.initialize(&w.token_unknown);
    let list = w
        .srv
        .rpc(&w.token_unknown, &session, "tools/list", json!({}));
    assert!(tools(&list).is_empty());
    let (_, _, caps) = w
        .srv
        .request("GET", None, Some(&w.token_unknown), None, &[]);
    assert_eq!(caps["tools"], json!([]));
    for name in ["locus_whoami", "locus_request_pin", "github.scope"] {
        assert!(refused(&w.srv.rpc(
            &w.token_unknown,
            &session,
            "tools/call",
            json!({"name":name})
        )));
    }
}

#[test]
fn forged_agent_selectors_and_cross_agent_sessions_never_widen_the_grant() {
    let w = world();
    let a = w.srv.initialize(&w.token_a);
    let b = w.srv.initialize(&w.token_b);
    for name in ["locus_heartbeat", "locus_request_pin", "github.nonexistent"] {
        let params =
            json!({"name":name,"agent_id":"graph-node-b","arguments":{},"node_id":"graph-node-b"});
        let (status, _, body) = w.srv.request(
            "POST",
            Some(&rpc("tools/call", params)),
            Some(&w.token_a),
            Some(&a),
            &[("X-Locus-Agent-Id", "graph-node-b")],
        );
        assert_eq!(status, 200);
        assert!(refused(&body), "self-asserted node cannot authorize a call");
    }
    let (status, _, _) = w.srv.request(
        "POST",
        Some(&rpc("tools/list", json!({}))),
        Some(&w.token_a),
        Some(&b),
        &[],
    );
    assert_eq!(status, 403);
    let list = w.srv.rpc(
        &w.token_a,
        &a,
        "tools/list",
        json!({"agent_id":"missing-node"}),
    );
    assert!(
        tools(&list).contains(&"github.scope"),
        "identity comes from the authenticated graph mapping"
    );
}

#[test]
fn broker_audits_catalogs_and_call_decisions_without_tokens_or_arguments() {
    let w = world();
    let a = w.srv.initialize(&w.token_a);
    let unknown = w.srv.initialize(&w.token_unknown);
    w.srv.rpc(&w.token_a, &a, "tools/list", json!({}));
    w.srv
        .rpc(&w.token_unknown, &unknown, "tools/list", json!({}));
    w.srv.rpc(
        &w.token_a,
        &a,
        "tools/call",
        json!({"name":"locus_whoami", "arguments":{}}),
    );
    let marker = "SYNTHETIC_ARGUMENT_MUST_NOT_BE_AUDITED";
    w.srv.rpc(
        &w.token_a,
        &a,
        "tools/call",
        json!({"name":"locus_heartbeat", "arguments":{"private":marker}}),
    );
    let events = w.store.read_audit_events().unwrap();
    let lists: Vec<_> = events
        .iter()
        .filter(|event| event.op == "mcp.swarm.tools_list")
        .collect();
    assert!(lists.iter().any(|event| event
        .detail
        .as_ref()
        .is_some_and(|v| v["agent_id"] == "graph-node-a" && v["tool_count"] == 2)));
    assert!(lists.iter().any(|event| event
        .detail
        .as_ref()
        .is_some_and(|v| v["agent_id"].is_null() && v["tool_count"] == 0)));
    let calls: Vec<_> = events
        .iter()
        .filter(|event| event.op == "mcp.swarm.tool_call")
        .collect();
    for allowed in [true, false] {
        assert!(calls.iter().any(|event| event
            .detail
            .as_ref()
            .is_some_and(|v| v["grant_id"] == w.grant_a.grant_id
                && v["agent_id"] == "graph-node-a"
                && v["allowed"] == allowed)));
    }
    let raw = std::fs::read_to_string(w.store.audit_path()).unwrap();
    for forbidden in [&w.token_a, &w.token_b, &w.token_unknown, marker] {
        assert!(
            !raw.contains(forbidden),
            "audit must contain no bearer or argument values"
        );
    }
}

#[test]
fn revocation_expiry_and_binding_drift_close_live_agent_sessions() {
    let w = world();
    let a = w.srv.initialize(&w.token_a);
    let b = w.srv.initialize(&w.token_b);
    w.store.revoke_mcp_grant(&w.grant_a.grant_id).unwrap();
    let (status, _, _) = w.srv.request(
        "POST",
        Some(&rpc("tools/call", json!({"name":"github.scope"}))),
        Some(&w.token_a),
        Some(&a),
        &[],
    );
    assert_eq!(status, 401);
    let grant_path = w
        .store
        .home()
        .join("mcp-grants")
        .join(format!("{}.json", w.grant_b.grant_id));
    let mut grant: Value = serde_json::from_slice(&std::fs::read(&grant_path).unwrap()).unwrap();
    grant["expires_at"] = json!("2000-01-01T00:00:00Z");
    std::fs::write(&grant_path, serde_json::to_vec(&grant).unwrap()).unwrap();
    let (status, _, _) = w.srv.request(
        "POST",
        Some(&rpc("tools/list", json!({}))),
        Some(&w.token_b),
        Some(&b),
        &[],
    );
    assert_eq!(status, 401);

    let w = world();
    let a = w.srv.initialize(&w.token_a);
    let mut binding = w.store.load_binding("swarm").unwrap();
    binding.providers[0].scope.orgs = vec!["changed-scope".into()];
    w.store.save_binding(&binding).unwrap();
    let list = w.srv.rpc(&w.token_a, &a, "tools/list", json!({}));
    assert!(
        tools(&list).is_empty(),
        "binding drift removes even control tools in broker mode"
    );
    assert!(refused(&w.srv.rpc(
        &w.token_a,
        &a,
        "tools/call",
        json!({"name":"locus_whoami"})
    )));
}

#[test]
fn audit_failure_blocks_catalog_disclosure_and_execution_without_control_fallback() {
    let w = world();
    let a = w.srv.initialize(&w.token_a);
    std::fs::remove_file(w.store.audit_path()).unwrap();
    std::fs::create_dir(w.store.audit_path()).unwrap();
    let list = w.srv.rpc(&w.token_a, &a, "tools/list", json!({}));
    assert!(
        list.get("error").is_some(),
        "audit write must succeed before catalog disclosure"
    );
    let call = w
        .srv
        .rpc(&w.token_a, &a, "tools/call", json!({"name":"github.scope"}));
    assert!(refused(&call), "audit write failure must block execution");
    let (_, _, caps) = w.srv.request("GET", None, Some(&w.token_a), None, &[]);
    assert_eq!(
        caps["tools"],
        json!([]),
        "error fallback must not restore control tools"
    );
}

#[test]
fn broker_exposes_no_alternate_resources_or_prompts_and_uses_an_immutable_snapshot() {
    let w = world();
    let a = w.srv.initialize(&w.token_a);
    assert_eq!(
        w.srv.rpc(&w.token_a, &a, "resources/list", json!({}))["result"]["resources"],
        json!([])
    );
    assert_eq!(
        w.srv.rpc(&w.token_a, &a, "prompts/list", json!({}))["result"]["prompts"],
        json!([])
    );
    assert!(refused(&w.srv.rpc(
        &w.token_a,
        &a,
        "resources/read",
        json!({"uri":"locus://session"})
    )));
    assert!(refused(&w.srv.rpc(
        &w.token_a,
        &a,
        "prompts/get",
        json!({"name":"locus_context"})
    )));
    std::fs::write(
        &w.graph,
        serde_json::to_vec(&json!({"version":1,"nodes":[{
            "id":"graph-node-a","grant_id":w.grant_a.grant_id,"tools":["locus_heartbeat"]
        }]}))
        .unwrap(),
    )
    .unwrap();
    let list = w.srv.rpc(&w.token_a, &a, "tools/list", json!({}));
    assert!(!tools(&list).contains(&"locus_heartbeat"));
    assert!(refused(&w.srv.rpc(
        &w.token_a,
        &a,
        "tools/call",
        json!({"name":"locus_heartbeat"})
    )));
}

#[test]
fn missing_bearers_and_tampered_session_seals_cannot_use_graph_grants() {
    let w = world();
    for token in [None, Some("invalid-synthetic-grant")] {
        let (status, _, _) = w.srv.request(
            "POST",
            Some(&rpc("initialize", json!({}))),
            token,
            None,
            &[],
        );
        assert_eq!(status, 401);
    }
    let a = w.srv.initialize(&w.token_a);
    let backing = w
        .store
        .load_session_by_id_resolved(&w.grant_a.session_id)
        .unwrap()
        .unwrap();
    let mut session: Value =
        serde_json::from_slice(&std::fs::read(&backing.path).unwrap()).unwrap();
    session["seal"] = json!("tampered");
    std::fs::write(&backing.path, serde_json::to_vec(&session).unwrap()).unwrap();
    let list = w.srv.rpc(&w.token_a, &a, "tools/list", json!({}));
    assert!(
        list.get("error").is_some() || tools(&list).is_empty(),
        "a valid graph mapping never repairs an invalid seal"
    );
    assert!(refused(&w.srv.rpc(
        &w.token_a,
        &a,
        "tools/call",
        json!({"name":"github.scope"})
    )));
}

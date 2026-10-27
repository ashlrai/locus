//! Black-box acceptance for the local, authenticated peer relay. Every store,
//! manifest, and child HOME is disposable; fixtures never resolve credentials.

use locus_core::{Binding, BindingBody, McpGrant, Policy, ProviderBinding, Scope, Store};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};
use tempfile::{tempdir, TempDir};

fn server_token() -> &'static str {
    concat!("synthetic", "-peer-server-token")
}

fn control_cap() -> &'static str {
    static CAP: OnceLock<String> = OnceLock::new();
    CAP.get_or_init(|| {
        let cap = "46".repeat(32);
        std::env::set_var("LOCUS_CONTROL_CAPABILITY", &cap);
        cap
    })
}

struct Server {
    child: Child,
    addr: SocketAddr,
}

impl Server {
    fn spawn(home: &Path, manifest: &Path) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let mut child = Command::new(env!("CARGO_BIN_EXE_locus-mcp"))
            .args(["--http", &addr.to_string(), "--multi-tenant", "--peers"])
            .arg(manifest)
            .env("HOME", home.parent().unwrap())
            .env("LOCUS_HOME", home)
            .env("LOCUS_CONTROL_CAPABILITY", control_cap())
            .env("LOCUS_MCP_HTTP_TOKEN", server_token())
            .env("LOCUS_MCP_AUTO_PIN", "0")
            .env_remove("LOCUS_EXECUTOR_CAPABILITY")
            .env_remove("LOCUS_MCP_SESSION_DIR")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn peer server");
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                panic!("peer server exited before readiness: {status}");
            }
            if TcpStream::connect(addr).is_ok() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("peer server did not become ready");
            }
            thread::sleep(Duration::from_millis(20));
        }
        Self { child, addr }
    }

    fn request(
        &self,
        method: &str,
        body: Option<&Value>,
        token: &str,
        session: Option<&str>,
    ) -> (u16, String, Value) {
        let mut stream = TcpStream::connect(self.addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let body = body
            .map(|v| serde_json::to_vec(v).unwrap())
            .unwrap_or_default();
        let server_token = server_token();
        let mut request = format!(
            "{method} /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nAccept: application/json\r\nAuthorization: Bearer {server_token}\r\nX-Locus-Tenant-Token: {token}\r\nContent-Length: {}\r\n",
            body.len()
        );
        if let Some(session) = session {
            request.push_str(&format!("Mcp-Session-Id: {session}\r\n"));
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
        let body = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(body).expect("JSON response")
        };
        (status, headers.to_owned(), body)
    }

    fn initialize(&self, token: &str, metadata: Option<Value>) -> Client {
        let mut params = json!({
            "protocolVersion":"2024-11-05", "capabilities":{},
            "clientInfo":{"name":"peer-test", "version":"1"}
        });
        if let Some(metadata) = metadata {
            params["_meta"] = json!({"locus_peer":metadata});
        }
        let (status, headers, response) =
            self.request("POST", Some(&rpc("initialize", params)), token, None);
        assert_eq!(status, 200);
        assert!(response.get("result").is_some(), "initialize must succeed");
        let session = headers
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case("Mcp-Session-Id")
                    .then(|| value.trim().to_owned())
            })
            .expect("MCP session header");
        let peer = response["result"]["_meta"]["locus_peer"].clone();
        assert!(peer["id"].is_string(), "initialize must register a peer");
        Client { session, peer }
    }

    fn call(&self, token: &str, client: &Client, name: &str, args: Value) -> Value {
        let (status, _, response) = self.request(
            "POST",
            Some(&rpc("tools/call", json!({"name":name,"arguments":args}))),
            token,
            Some(&client.session),
        );
        assert_eq!(status, 200);
        response
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Client {
    session: String,
    peer: Value,
}

fn rpc(method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})
}

fn refused(response: &Value) -> bool {
    response.get("error").is_some() || response["result"]["isError"] == true
}

fn content(response: &Value) -> Value {
    assert!(!refused(response), "peer operation must succeed");
    serde_json::from_str(
        response["result"]["content"][0]["text"]
            .as_str()
            .expect("text tool content"),
    )
    .expect("JSON tool content")
}

struct World {
    server: Server,
    store: Store,
    grant_a: McpGrant,
    token_a: String,
    token_b: String,
    token_unknown: String,
    token_other: String,
    _dir: TempDir,
}

fn world(pid_b: u32) -> World {
    world_with_policy(pid_b, Policy::default())
}

fn world_with_policy(pid_b: u32, policy: Policy) -> World {
    let _ = control_cap();
    let dir = tempdir().unwrap();
    let home = dir.path().join("locus-home");
    let store = Store::open(&home).unwrap();
    for alias in ["peers", "other"] {
        store
            .save_binding(&Binding::from_body(BindingBody {
                id: format!("bnd_{alias}"),
                alias: alias.into(),
                tenant: format!("tenant-{alias}"),
                principal: None,
                description: None,
                policy: policy.clone(),
                providers: vec![ProviderBinding {
                    provider: "github".into(),
                    account: "synthetic-peer-account".into(),
                    credential_ref: "env:SYNTHETIC_PEER_TOKEN".into(),
                    scope: Scope::default(),
                    upstream: None,
                }],
            }))
            .unwrap();
    }
    let (_, grant_a, token_a) = store
        .create_mcp_grant("peers", dir.path(), None, Some("agent-a".into()), false)
        .unwrap();
    let (_, grant_b, token_b) = store
        .create_mcp_grant("peers", dir.path(), None, Some("agent-b".into()), false)
        .unwrap();
    let (_, _, token_unknown) = store
        .create_mcp_grant("peers", dir.path(), None, Some("unknown".into()), false)
        .unwrap();
    let (_, _, token_other) = store
        .create_mcp_grant("other", dir.path(), None, Some("other".into()), false)
        .unwrap();
    let manifest = dir.path().join("engines.json");
    std::fs::write(&manifest, serde_json::to_vec(&json!({"engines":[
        {"grant_id":grant_a.grant_id,"engine_id":"codex-a","task":"review","cwd":"/tmp/project","pid":std::process::id()},
        {"grant_id":grant_b.grant_id,"engine_id":"codex-b","task":"implement","cwd":"/tmp/project","pid":pid_b}
    ]})).unwrap()).unwrap();
    let server = Server::spawn(&home, &manifest);
    World {
        server,
        store,
        grant_a,
        token_a,
        token_b,
        token_unknown,
        token_other,
        _dir: dir,
    }
}

fn unknown_metadata() -> Value {
    json!({"engine_id":"codex-a","task":"observe","cwd":"/tmp/project","pid":std::process::id()})
}

#[test]
fn direct_and_group_messages_have_attested_senders_and_private_inboxes() {
    let w = world(std::process::id());
    let a = w.server.initialize(&w.token_a, None);
    let b = w.server.initialize(&w.token_b, None);
    let unknown = w
        .server
        .initialize(&w.token_unknown, Some(unknown_metadata()));
    let listed = content(&w.server.call(&w.token_a, &a, "locus.peers.list", json!({})));
    assert_eq!(listed["self"]["id"], a.peer["id"]);
    assert_eq!(listed["peers"].as_array().unwrap().len(), 3);
    assert_eq!(a.peer["trusted"], true);
    assert_eq!(unknown.peer["trusted"], false);
    assert!(refused(&w.server.call(
        &w.token_unknown,
        &unknown,
        "locus.peers.send",
        json!({"recipient":a.peer["id"],"body":"unknown engines are read-only"}),
    )));

    let sent = content(&w.server.call(
        &w.token_a,
        &a,
        "locus.peers.send",
        json!({"recipient":b.peer["id"],"body":"Please review the parser."}),
    ));
    let inbox = content(
        &w.server
            .call(&w.token_b, &b, "locus.peers.inbox", json!({})),
    );
    let messages = inbox["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["sender"], a.peer);
    assert_eq!(messages[0]["body"], "Please review the parser.");
    assert!(
        !serde_json::to_string(&inbox).unwrap().contains(&a.session),
        "public peer data must not expose a session capability"
    );
    let after = messages[0]["id"].clone();
    assert!(content(
        &w.server
            .call(&w.token_b, &b, "locus.peers.inbox", json!({"after":after}))
    )["messages"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(content(
        &w.server
            .call(&w.token_unknown, &unknown, "locus.peers.inbox", json!({}))
    )["messages"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(sent.is_object());

    let created = content(&w.server.call(
        &w.token_a,
        &a,
        "locus.peers.thread",
        json!({"members":[b.peer["id"],unknown.peer["id"]]}),
    ));
    let id = created["thread"]["id"].clone();
    assert!(id.is_string());
    assert!(!refused(&w.server.call(
        &w.token_b,
        &b,
        "locus.peers.send",
        json!({"thread":id,"body":"Review complete."})
    )));
    let view = content(&w.server.call(
        &w.token_unknown,
        &unknown,
        "locus.peers.thread",
        json!({"id":id}),
    ));
    assert_eq!(view["messages"][0]["sender"], b.peer);
    assert_eq!(view["messages"][0]["body"], "Review complete.");
    assert!(refused(&w.server.call(
        &w.token_unknown,
        &unknown,
        "locus.peers.send",
        json!({"thread":id,"body":"Cannot impersonate an approved engine."})
    )));
    assert!(refused(&w.server.call(
        &w.token_unknown,
        &unknown,
        "locus.peers.thread",
        json!({"members":[a.peer["id"]]})
    )));
}

#[test]
fn forged_senders_secret_shapes_oversize_and_nontext_bodies_never_deliver() {
    let w = world(std::process::id());
    let a = w.server.initialize(&w.token_a, None);
    let b = w.server.initialize(&w.token_b, None);
    for args in [
        json!({"recipient":b.peer["id"],"body":"hello","sender":b.peer}),
        json!({"recipient":b.peer["id"],"body":format!("Bearer {}", "x".repeat(40))}),
        json!({"recipient":b.peer["id"],"body":format!("ghp_{}", "A".repeat(36))}),
        json!({"recipient":b.peer["id"],"body":"x".repeat(8193)}),
        json!({"recipient":b.peer["id"],"body":{"text":"hello"}}),
        json!({"recipient":b.peer["id"],"body":"bad\u{0000}text"}),
    ] {
        assert!(
            refused(&w.server.call(&w.token_a, &a, "locus.peers.send", args)),
            "unsafe message must be refused"
        );
    }
    let inbox = content(
        &w.server
            .call(&w.token_b, &b, "locus.peers.inbox", json!({})),
    );
    assert!(inbox["messages"].as_array().unwrap().is_empty());
}

#[test]
fn tenant_partition_and_thread_membership_are_enforced() {
    let w = world(std::process::id());
    let a = w.server.initialize(&w.token_a, None);
    let b = w.server.initialize(&w.token_b, None);
    let unknown = w
        .server
        .initialize(&w.token_unknown, Some(unknown_metadata()));
    let other = w
        .server
        .initialize(&w.token_other, Some(unknown_metadata()));
    let listed = content(
        &w.server
            .call(&w.token_other, &other, "locus.peers.list", json!({})),
    );
    assert_eq!(listed["peers"].as_array().unwrap().len(), 1);
    assert!(refused(&w.server.call(
        &w.token_a,
        &a,
        "locus.peers.send",
        json!({"recipient":other.peer["id"],"body":"wrong tenant"})
    )));
    assert!(refused(&w.server.call(
        &w.token_a,
        &a,
        "locus.peers.thread",
        json!({"members":[other.peer["id"]]})
    )));
    let created = content(&w.server.call(
        &w.token_a,
        &a,
        "locus.peers.thread",
        json!({"members":[b.peer["id"]]}),
    ));
    let id = created["thread"]["id"].clone();
    assert!(refused(&w.server.call(
        &w.token_unknown,
        &unknown,
        "locus.peers.thread",
        json!({"id":id})
    )));
    assert!(refused(&w.server.call(
        &w.token_other,
        &other,
        "locus.peers.thread",
        json!({"id":id})
    )));
    let (status, _, _) = w.server.request(
        "POST",
        Some(&rpc(
            "tools/call",
            json!({"name":"locus.peers.list","arguments":{}}),
        )),
        &w.token_other,
        Some(&a.session),
    );
    assert_eq!(status, 403, "another grant cannot borrow an MCP session");
}

#[test]
fn deleted_session_is_pruned_and_reinitialization_does_not_inherit_its_inbox() {
    let w = world(std::process::id());
    let a = w.server.initialize(&w.token_a, None);
    let b = w.server.initialize(&w.token_b, None);
    assert!(!refused(&w.server.call(
        &w.token_a,
        &a,
        "locus.peers.send",
        json!({"recipient":b.peer["id"],"body":"old message"})
    )));
    let (status, _, _) = w
        .server
        .request("DELETE", None, &w.token_b, Some(&b.session));
    assert_eq!(status, 204);
    let listed = content(&w.server.call(&w.token_a, &a, "locus.peers.list", json!({})));
    assert_eq!(listed["peers"].as_array().unwrap().len(), 1);
    assert!(refused(&w.server.call(
        &w.token_a,
        &a,
        "locus.peers.send",
        json!({"recipient":b.peer["id"],"body":"stale recipient"})
    )));
    let replacement = w.server.initialize(&w.token_b, None);
    assert_ne!(replacement.peer["id"], b.peer["id"]);
    assert!(content(
        &w.server
            .call(&w.token_b, &replacement, "locus.peers.inbox", json!({}))
    )["messages"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn revoked_grant_cannot_send_or_read_peers() {
    let w = world(std::process::id());
    let a = w.server.initialize(&w.token_a, None);
    w.store.revoke_mcp_grant(&w.grant_a.grant_id).unwrap();
    let (status, _, _) = w.server.request(
        "POST",
        Some(&rpc(
            "tools/call",
            json!({"name":"locus.peers.list","arguments":{}}),
        )),
        &w.token_a,
        Some(&a.session),
    );
    assert_eq!(status, 401);
}

#[test]
fn initialize_metadata_cannot_replace_operator_attested_identity() {
    let w = world(std::process::id());
    let a = w.server.initialize(
        &w.token_a,
        Some(json!({"engine_id":"forged-engine","task":"forged-task","cwd":"/tmp/forged","pid":std::process::id()})),
    );
    assert_eq!(a.peer["engine_id"], "codex-a");
    assert_eq!(a.peer["task"], "review");
    assert_eq!(a.peer["cwd"], "/tmp/project");
    assert_eq!(a.peer["trusted"], true);
    let (status, _, response) = w.server.request(
        "POST",
        Some(&rpc("initialize", json!({"_meta":{"locus_peer":{"engine_id":"another-forgery","task":"forged","cwd":"/tmp","pid":std::process::id()}}}))),
        &w.token_a,
        Some(&a.session),
    );
    assert_eq!(status, 200);
    assert_eq!(response["result"]["_meta"]["locus_peer"], a.peer);
    let (_, _, catalog) = w.server.request(
        "POST",
        Some(&rpc("tools/list", json!({}))),
        &w.token_a,
        Some(&a.session),
    );
    let tools = catalog["result"]["tools"].as_array().unwrap();
    for name in [
        "locus.peers.list",
        "locus.peers.send",
        "locus.peers.inbox",
        "locus.peers.thread",
    ] {
        assert!(
            tools.iter().any(|tool| tool["name"] == name),
            "peer tool must be discoverable"
        );
    }
}

#[test]
fn peer_operations_preserve_binding_deny_and_approval_gates() {
    let policy = Policy {
        rules: vec![locus_core::PolicyRule::new("locus.peers.send", "deny")],
        require_approval: vec!["locus.peers.thread".into()],
        ..Policy::default()
    };
    let w = world_with_policy(std::process::id(), policy);
    let a = w.server.initialize(&w.token_a, None);
    let b = w.server.initialize(&w.token_b, None);
    assert!(!refused(&w.server.call(
        &w.token_a,
        &a,
        "locus.peers.list",
        json!({})
    )));
    assert!(refused(&w.server.call(
        &w.token_a,
        &a,
        "locus.peers.send",
        json!({"recipient":b.peer["id"],"body":"denied by binding policy"})
    )));
    assert!(refused(&w.server.call(
        &w.token_a,
        &a,
        "locus.peers.thread",
        json!({"members":[b.peer["id"]]})
    )));
    assert!(content(
        &w.server
            .call(&w.token_b, &b, "locus.peers.inbox", json!({}))
    )["messages"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn session_action_budget_blocks_additional_peer_delivery() {
    let w = world(std::process::id());
    w.store
        .save_config(&locus_core::config::LocusConfig {
            budget: locus_core::config::BudgetConfig {
                max_actions: Some(1),
                max_minutes: None,
            },
            ..Default::default()
        })
        .unwrap();
    let a = w.server.initialize(&w.token_a, None);
    let b = w.server.initialize(&w.token_b, None);
    assert!(!refused(&w.server.call(
        &w.token_a,
        &a,
        "locus.peers.send",
        json!({"recipient":b.peer["id"],"body":"within budget"})
    )));
    assert!(refused(&w.server.call(
        &w.token_a,
        &a,
        "locus.peers.send",
        json!({"recipient":b.peer["id"],"body":"over budget"})
    )));
    let inbox = content(
        &w.server
            .call(&w.token_b, &b, "locus.peers.inbox", json!({})),
    );
    assert_eq!(inbox["messages"].as_array().unwrap().len(), 1);
    assert_eq!(inbox["messages"][0]["body"], "within budget");
}

#[cfg(unix)]
#[test]
fn dead_pid_is_pruned_from_live_http_sessions() {
    let mut process = Command::new("sleep")
        .arg("30")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let w = world(process.id());
    let a = w.server.initialize(&w.token_a, None);
    let b = w.server.initialize(&w.token_b, None);
    process.kill().unwrap();
    process.wait().unwrap();
    let listed = content(&w.server.call(&w.token_a, &a, "locus.peers.list", json!({})));
    assert_eq!(listed["peers"].as_array().unwrap().len(), 1);
    assert!(refused(&w.server.call(
        &w.token_a,
        &a,
        "locus.peers.send",
        json!({"recipient":b.peer["id"],"body":"dead recipient"})
    )));
}

#[test]
fn peer_mode_refuses_non_ipv4_localhost_even_with_remote_override() {
    let dir = tempdir().unwrap();
    for address in ["0.0.0.0:0", "127.0.0.2:0", "[::1]:0", "192.0.2.1:0"] {
        let output = Command::new(env!("CARGO_BIN_EXE_locus-mcp"))
            .args(["--http", address, "--multi-tenant", "--peers"])
            .arg(dir.path().join("absent-manifest.json"))
            .env("HOME", dir.path())
            .env("LOCUS_HOME", dir.path().join("locus"))
            .env("LOCUS_MCP_HTTP_ALLOW_REMOTE", "1")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("127.0.0.1"),
            "bind must be refused before opening the manifest"
        );
    }
}

//! Integration: spawn locus-mcp, exercise MCP handshake + tools over stdio.
//!
//! Covers both Content-Length framing (MCP standard) and NDJSON.

use locus_core::{Binding, BindingBody, Policy, ProviderBinding, Scope, Store, UpstreamSpec};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Duration;
use tempfile::tempdir;

fn mcp_bin() -> PathBuf {
    // cargo sets CARGO_BIN_EXE_<name> for integration tests in the same package
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_locus-mcp") {
        return PathBuf::from(p);
    }
    // Fallback: workspace target
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // crates
    p.pop(); // workspace root
    p.push("target");
    p.push("debug");
    p.push("locus-mcp");
    p
}

struct McpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    framing: Framing,
    next_id: i64,
}

#[derive(Clone, Copy)]
enum Framing {
    ContentLength,
    Ndjson,
}

impl McpClient {
    fn spawn(locus_home: &std::path::Path, framing: Framing) -> Self {
        Self::spawn_opts(locus_home, framing, None, &[])
    }

    /// Spawn with optional working directory and extra env pairs.
    fn spawn_opts(
        locus_home: &std::path::Path,
        framing: Framing,
        cwd: Option<&std::path::Path>,
        extra_env: &[(&str, &str)],
    ) -> Self {
        let mut cmd = Command::new(mcp_bin());
        cmd.env("LOCUS_HOME", locus_home)
            // Default off so tests without workspace defaults stay unpinned.
            .env("LOCUS_MCP_AUTO_PIN", "0")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        let mut child = cmd
            .spawn()
            .expect("spawn locus-mcp — run `cargo build -p locus-mcp` first");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        Self {
            child,
            stdin,
            stdout,
            framing,
            next_id: 1,
        }
    }

    fn write_msg(&mut self, msg: &Value) {
        let body = serde_json::to_vec(msg).unwrap();
        match self.framing {
            Framing::ContentLength => {
                write!(self.stdin, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
                self.stdin.write_all(&body).unwrap();
            }
            Framing::Ndjson => {
                self.stdin.write_all(&body).unwrap();
                self.stdin.write_all(b"\n").unwrap();
            }
        }
        self.stdin.flush().unwrap();
    }

    fn read_msg(&mut self) -> Value {
        match self.framing {
            Framing::ContentLength => {
                let mut content_length = None;
                loop {
                    let mut line = String::new();
                    let n = self.stdout.read_line(&mut line).expect("read header");
                    assert!(n > 0, "EOF before Content-Length response");
                    let lower = line.trim().to_ascii_lowercase();
                    if lower.starts_with("content-length:") {
                        content_length = Some(
                            lower
                                .trim_start_matches("content-length:")
                                .trim()
                                .parse::<usize>()
                                .expect("Content-Length"),
                        );
                    }
                    if line.trim().is_empty() {
                        break;
                    }
                }
                let len = content_length.expect("missing Content-Length on response");
                let mut buf = vec![0u8; len];
                self.stdout.read_exact(&mut buf).expect("read body");
                serde_json::from_slice(&buf).expect("json body")
            }
            Framing::Ndjson => {
                let mut line = String::new();
                let n = self.stdout.read_line(&mut line).expect("read line");
                assert!(n > 0, "EOF before NDJSON response");
                serde_json::from_str(line.trim()).expect("ndjson")
            }
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.write_msg(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }));
        let resp = self.read_msg();
        assert_eq!(resp.get("id"), Some(&json!(id)), "id mismatch: {resp}");
        resp
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.write_msg(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }));
        // no response expected
    }

    fn tool_text(resp: &Value) -> (String, bool) {
        let result = resp.get("result").expect("result");
        let is_error = result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let text = result
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .and_then(|c| c.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        (text, is_error)
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn sample_bindings(store: &Store) {
    let acme = Binding::from_body(BindingBody {
        id: "bnd_acme".into(),
        alias: "acme".into(),
        tenant: "acme-corp".into(),
        principal: None,
        description: None,
        policy: Policy {
            require_approval: vec!["*.delete*".into(), "vercel.deploy.prod".into()],
            max_ttl: Some("1h".into()),
            ..Policy::default()
        },
        providers: vec![
            ProviderBinding {
                provider: "github".into(),
                account: "acme-gh".into(),
                credential_ref: "phm:GH_TOKEN_ACME".into(),
                scope: Scope {
                    orgs: vec!["acme-corp".into()],
                    ..Scope::default()
                },
                upstream: None,
            },
            ProviderBinding {
                provider: "vercel".into(),
                account: "acme-vc".into(),
                credential_ref: "phm:VERCEL_TOKEN_ACME".into(),
                scope: Scope {
                    team_id: Some("team_acme".into()),
                    projects: vec!["acme-web".into()],
                    ..Scope::default()
                },
                upstream: None,
            },
            ProviderBinding {
                provider: "supabase".into(),
                account: "acme-db".into(),
                credential_ref: "phm:SUPABASE_ACME".into(),
                scope: Scope {
                    project_ref: Some("proj_acme".into()),
                    read_only: Some(true),
                    ..Scope::default()
                },
                upstream: None,
            },
        ],
    });
    store.save_binding(&acme).unwrap();
}

fn handshake(client: &mut McpClient) -> Value {
    let init = client.request(
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "locus-test", "version": "0.0.1" }
        }),
    );
    assert!(init.get("result").is_some(), "initialize failed: {init}");
    let result = init.get("result").unwrap().clone();
    assert_eq!(
        result
            .get("serverInfo")
            .and_then(|s| s.get("name"))
            .and_then(|n| n.as_str()),
        Some("locus-mcp")
    );
    // notifications/initialized must not hang / break subsequent requests
    client.notify("notifications/initialized", json!({}));
    // tiny pause so notification is fully consumed before next write
    std::thread::sleep(Duration::from_millis(20));
    result
}

fn parse_resource_text(resp: &Value) -> String {
    resp["result"]["contents"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|c| c.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string()
}

#[test]
fn unpinned_tools_list_only_control_tools_ndjson() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    sample_bindings(&store);
    // no pin

    let mut client = McpClient::spawn(dir.path(), Framing::Ndjson);
    let init = handshake(&mut client);

    // initialize: agent instructions + resource/prompt capabilities
    let instructions = init["instructions"].as_str().unwrap_or("");
    assert!(
        instructions.to_lowercase().contains("cannot pin") || instructions.contains("locus_whoami"),
        "initialize instructions should teach agents about pin: {instructions}"
    );
    assert!(
        init["capabilities"]["resources"].is_object(),
        "initialize must advertise resources capability"
    );
    assert!(
        init["capabilities"]["prompts"].is_object()
            || init["capabilities"].get("prompts").is_some(),
        "initialize must advertise prompts capability: {init}"
    );

    let list = client.request("tools/list", json!({}));
    let tools = list["result"]["tools"].as_array().expect("tools array");
    assert!(!tools.is_empty());
    // locus_whoami always first
    assert_eq!(
        tools[0]["name"].as_str(),
        Some("locus_whoami"),
        "locus_whoami must be first tool"
    );
    for t in tools {
        let name = t["name"].as_str().unwrap();
        assert!(
            name.starts_with("locus_"),
            "unpinned tools/list must be control-only, got {name}"
        );
        let desc = t["description"].as_str().unwrap_or("");
        assert!(
            desc.starts_with("[locus:unpinned]"),
            "unpinned tool {name} description must start with [locus:unpinned], got: {desc}"
        );
    }
    assert!(tools.iter().any(|t| t["name"] == "locus_whoami"));
    assert!(tools.iter().any(|t| t["name"] == "locus_safe_next"));
    assert!(tools.iter().any(|t| t["name"] == "locus_request_pin"));
    assert!(tools.iter().any(|t| t["name"] == "locus_heartbeat"));
    assert!(tools.iter().any(|t| t["name"] == "locus_enter_hint"));
    // provider tools must be absent
    assert!(!tools.iter().any(|t| t["name"] == "github.scope"));
    assert!(!tools.iter().any(|t| t["name"] == "vercel.scope"));
    // listChanged advertised so clients re-fetch after auto-pin / pin change
    assert_eq!(
        init["capabilities"]["tools"]["listChanged"], true,
        "tools.listChanged should be true"
    );
}

#[test]
fn content_length_initialize_tools_list_and_call_with_freeze_deny() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    sample_bindings(&store);
    store
        .pin("acme", dir.path(), Some("mcp-test".into()), false)
        .unwrap();

    let mut client = McpClient::spawn(dir.path(), Framing::ContentLength);
    handshake(&mut client);

    // tools/list after initialize includes provider tools for pin
    let list = client.request("tools/list", json!({}));
    let tools = list["result"]["tools"].as_array().expect("tools");
    assert_eq!(
        tools[0]["name"].as_str(),
        Some("locus_whoami"),
        "locus_whoami must be first when pinned"
    );
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"locus_whoami"));
    assert!(names.contains(&"locus_heartbeat"));
    assert!(names.contains(&"locus_enter_hint"));
    assert!(names.contains(&"github.scope"));
    assert!(names.contains(&"github.check_repo"));
    assert!(names.contains(&"vercel.scope"));
    assert!(names.contains(&"supabase.scope"));
    // Every description tagged with active pin alias
    for t in tools {
        let name = t["name"].as_str().unwrap_or("?");
        let desc = t["description"].as_str().unwrap_or("");
        assert!(
            desc.starts_with("[locus:acme]"),
            "pinned tool {name} must start with [locus:acme], got: {desc}"
        );
    }

    // github.scope end-to-end
    let gh = client.request(
        "tools/call",
        json!({
            "name": "github.scope",
            "arguments": {}
        }),
    );
    let (text, is_err) = McpClient::tool_text(&gh);
    assert!(!is_err, "github.scope error: {text}");
    let body: Value = serde_json::from_str(&text).expect("github body json");
    assert!(body.get("credential_ref").is_none());
    assert_eq!(body["credential"]["present"], true);
    assert_eq!(body["credential"]["source"], "phantom");
    assert!(!text.contains("GH_TOKEN_ACME"));
    assert_eq!(body["tenant"], "acme-corp");
    assert_eq!(body["binding"], "acme");

    // vercel.scope end-to-end
    let vc = client.request(
        "tools/call",
        json!({
            "name": "vercel.scope",
            "arguments": {}
        }),
    );
    let (text, is_err) = McpClient::tool_text(&vc);
    assert!(!is_err, "vercel.scope error: {text}");
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["team_id"], "team_acme");
    assert!(body.get("credential_ref").is_none());
    assert_eq!(body["credential"]["source"], "phantom");
    assert!(!text.contains("VERCEL_TOKEN_ACME"));

    // Agent-facing identity views must not reveal any credential_ref value.
    let whoami = client.request(
        "tools/call",
        json!({ "name": "locus_whoami", "arguments": {} }),
    );
    let (whoami_text, whoami_err) = McpClient::tool_text(&whoami);
    assert!(!whoami_err, "locus_whoami failed: {whoami_text}");
    for canary in [
        "GH_TOKEN_ACME",
        "VERCEL_TOKEN_ACME",
        "SUPABASE_ACME",
        "credential_ref",
    ] {
        assert!(
            !whoami_text.contains(canary),
            "locus_whoami leaked {canary}: {whoami_text}"
        );
    }

    // freeze deny — model supplies wrong team_id
    let denied = client.request(
        "tools/call",
        json!({
            "name": "vercel.scope",
            "arguments": { "team_id": "team_evil" }
        }),
    );
    let (text, is_err) = McpClient::tool_text(&denied);
    assert!(is_err, "expected freeze deny isError=true, got: {text}");
    assert!(
        text.contains("scope freeze") || text.contains("team_evil"),
        "unexpected freeze message: {text}"
    );

    // supabase freeze deny too
    let denied2 = client.request(
        "tools/call",
        json!({
            "name": "supabase.scope",
            "arguments": { "project_ref": "proj_evil" }
        }),
    );
    let (text, is_err) = McpClient::tool_text(&denied2);
    assert!(is_err, "expected supabase freeze deny: {text}");
    assert!(text.contains("scope freeze") || text.contains("proj_evil"));

    // require_approval path writes pending approval for locus approve
    let del = client.request(
        "tools/call",
        json!({
            "name": "supabase.table.delete",
            "arguments": { "table": "users" }
        }),
    );
    let (text, is_err) = McpClient::tool_text(&del);
    assert!(is_err);
    assert!(text.contains("requires_approval") || text.contains("approval"));
    assert!(
        text.contains("appr_"),
        "expected approval_id in response: {text}"
    );
    assert!(
        text.contains("local_advisory"),
        "missing trust label: {text}"
    );
    assert!(
        text.contains("authoritative_path_enabled"),
        "missing authority state: {text}"
    );

    let pending = store.pending_approvals().unwrap();
    assert!(
        !pending.is_empty(),
        "expected pending approval record under approvals/"
    );
    assert!(pending.iter().any(|r| {
        r.tool == "supabase.table.delete" && r.binding == "acme" && r.id.starts_with("appr_")
    }));

    // A local assertion remains advisory and cannot unlock provider execution.
    let id = pending
        .iter()
        .find(|r| r.tool == "supabase.table.delete")
        .unwrap()
        .id
        .clone();
    store.grant_approval(&id, None, "e2e").unwrap();
    let del2 = client.request(
        "tools/call",
        json!({
            "name": "supabase.table.delete",
            "arguments": { "table": "users" }
        }),
    );
    let (text2, is_err2) = McpClient::tool_text(&del2);
    assert!(is_err2, "local advisory must remain blocked: {text2}");
    assert!(text2.contains("local_advisory"));
    assert!(text2.contains("authoritative_path_enabled"));
}

#[test]
fn ndjson_provider_call_not_pinned() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    sample_bindings(&store);

    let mut client = McpClient::spawn(dir.path(), Framing::Ndjson);
    handshake(&mut client);

    let resp = client.request(
        "tools/call",
        json!({
            "name": "github.scope",
            "arguments": {}
        }),
    );
    let (text, is_err) = McpClient::tool_text(&resp);
    assert!(is_err);
    assert!(text.contains("not_pinned") || text.contains("pin"));
}

/// Mock upstream MCP script (python3 NDJSON) for auto-spawn tests.
fn mock_upstream_script() -> &'static str {
    r#"
import sys, json
def send(o):
    sys.stdout.write(json.dumps(o)+"\n"); sys.stdout.flush()
for line in sys.stdin:
    line=line.strip()
    if not line: continue
    msg=json.loads(line)
    mid=msg.get("id")
    method=msg.get("method","")
    if mid is None: continue
    if method=="initialize":
        send({"jsonrpc":"2.0","id":mid,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"mock","version":"0"}}})
    elif method=="tools/list":
        send({"jsonrpc":"2.0","id":mid,"result":{"tools":[
            {"name":"ping","description":"upstream ping","inputSchema":{"type":"object"}},
            {"name":"echo","description":"echo","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}}}
        ]}})
    elif method=="tools/call":
        name=msg.get("params",{}).get("name","")
        args=msg.get("params",{}).get("arguments",{})
        if name=="ping":
            send({"jsonrpc":"2.0","id":mid,"result":{"content":[{"type":"text","text":"pong"}],"isError":False}})
        elif name=="echo":
            send({"jsonrpc":"2.0","id":mid,"result":{"content":[{"type":"text","text":args.get("text","")}],"isError":False}})
        else:
            send({"jsonrpc":"2.0","id":mid,"error":{"code":-32601,"message":name}})
    else:
        send({"jsonrpc":"2.0","id":mid,"error":{"code":-32601,"message":method}})
"#
}

#[test]
fn pinned_upstream_auto_spawn_list_and_call() {
    // Requires python3 for mock upstream MCP
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }

    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();

    let binding = Binding::from_body(BindingBody {
        id: "bnd_up".into(),
        alias: "up".into(),
        tenant: "acme-corp".into(),
        principal: None,
        description: None,
        policy: Policy::default(),
        providers: vec![
            ProviderBinding {
                provider: "github".into(),
                account: "acme-gh".into(),
                credential_ref: "phm:GH_TOKEN_ACME".into(),
                scope: Scope {
                    orgs: vec!["acme-corp".into()],
                    ..Scope::default()
                },
                upstream: Some(UpstreamSpec::new("python3").with_args([
                    "-u",
                    "-c",
                    mock_upstream_script(),
                ])),
            },
            ProviderBinding {
                provider: "supabase".into(),
                account: "acme-db".into(),
                credential_ref: "phm:SUPABASE_ACME".into(),
                scope: Scope {
                    project_ref: Some("proj_acme".into()),
                    ..Scope::default()
                },
                upstream: None,
            },
        ],
    });
    store.save_binding(&binding).unwrap();
    store
        .pin("up", dir.path(), Some("mcp-upstream".into()), false)
        .unwrap();

    let mut client = McpClient::spawn(dir.path(), Framing::Ndjson);
    handshake(&mut client);

    let list = client.request("tools/list", json!({}));
    let tools = list["result"]["tools"].as_array().expect("tools");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();

    // Control + synthetic + namespaced upstream
    assert!(names.contains(&"locus_whoami"));
    assert!(names.contains(&"github.scope")); // synthetic kept
    assert!(names.contains(&"github.ping")); // upstream
    assert!(names.contains(&"github.echo"));
    assert!(names.contains(&"supabase.scope")); // synthetic only (no upstream)
    assert!(!names.contains(&"supabase.ping"));

    // Upstream call
    let ping = client.request(
        "tools/call",
        json!({ "name": "github.ping", "arguments": {} }),
    );
    let (text, is_err) = McpClient::tool_text(&ping);
    assert!(!is_err, "github.ping failed: {text}");
    assert!(text.contains("pong"), "expected pong in {text}");

    let echo = client.request(
        "tools/call",
        json!({
            "name": "github.echo",
            "arguments": { "text": "via-locus-mcp" }
        }),
    );
    let (text, is_err) = McpClient::tool_text(&echo);
    assert!(!is_err, "github.echo failed: {text}");
    assert!(text.contains("via-locus-mcp"), "echo body: {text}");

    // Synthetic still works alongside upstream
    let scope = client.request(
        "tools/call",
        json!({ "name": "github.scope", "arguments": {} }),
    );
    let (text, is_err) = McpClient::tool_text(&scope);
    assert!(!is_err, "github.scope failed: {text}");
    assert!(text.contains("acme"));
    assert!(!text.contains("GH_TOKEN_ACME"));
}

#[test]
fn locus_heartbeat_and_enter_hint() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    sample_bindings(&store);

    // Unpinned heartbeat
    let mut client = McpClient::spawn(dir.path(), Framing::Ndjson);
    handshake(&mut client);

    let hb = client.request(
        "tools/call",
        json!({ "name": "locus_heartbeat", "arguments": {} }),
    );
    let (text, is_err) = McpClient::tool_text(&hb);
    assert!(is_err, "unpinned heartbeat should flag unhealthy: {text}");
    let body: Value = serde_json::from_str(&text).expect("heartbeat json");
    assert_eq!(body["pinned"], false);
    assert_eq!(body["ok"], false);
    assert!(body.get("runtime").is_some());
    assert!(body.get("issues").is_some());

    let hint = client.request(
        "tools/call",
        json!({
            "name": "locus_enter_hint",
            "arguments": { "alias": "acme" }
        }),
    );
    let (text, is_err) = McpClient::tool_text(&hint);
    assert!(!is_err, "enter_hint error: {text}");
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["agents_cannot_pin"], true);
    assert_eq!(body["command"], "locus enter acme");
    assert_eq!(body["pin_command"], "locus pin acme");
    assert_eq!(body["binding_exists"], true);

    // Pinned healthy heartbeat
    store
        .pin("acme", dir.path(), Some("mcp-hb".into()), false)
        .unwrap();
    // New client so it sees the pin (store is shared via LOCUS_HOME)
    drop(client);
    let mut client = McpClient::spawn(dir.path(), Framing::Ndjson);
    handshake(&mut client);

    let hb2 = client.request(
        "tools/call",
        json!({ "name": "locus_heartbeat", "arguments": {} }),
    );
    let (text, is_err) = McpClient::tool_text(&hb2);
    assert!(!is_err, "healthy heartbeat should not error: {text}");
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(body["pinned"], true);
    assert_eq!(body["seal_ok"], true);
    assert_eq!(body["frozen"], false);
    assert_eq!(body["binding"], "acme");
}

#[test]
fn frozen_session_tools_list_control_only_and_call_errors() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    sample_bindings(&store);
    store
        .pin("acme", dir.path(), Some("mcp-freeze".into()), false)
        .unwrap();

    // Mutate binding under pin → drift freeze
    let mut b = store.load_binding("acme").unwrap();
    b.providers[0].scope.orgs = vec!["mutated-org".into()];
    store.save_binding(&b).unwrap();
    let drift = store.check_drift_and_freeze().unwrap();
    assert!(drift.frozen || !drift.ok, "expected drift after mutation");

    let mut client = McpClient::spawn(dir.path(), Framing::Ndjson);
    handshake(&mut client);

    // tools/list must not expose provider tools when frozen/unhealthy
    let list = client.request("tools/list", json!({}));
    let tools = list["result"]["tools"].as_array().expect("tools");
    for t in tools {
        let name = t["name"].as_str().unwrap();
        assert!(
            name.starts_with("locus_"),
            "frozen tools/list must be control-only, got {name}"
        );
    }
    assert!(tools.iter().any(|t| t["name"] == "locus_heartbeat"));

    // Provider tools/call must fail closed with clear error
    let call = client.request(
        "tools/call",
        json!({ "name": "github.scope", "arguments": {} }),
    );
    let (text, is_err) = McpClient::tool_text(&call);
    assert!(is_err, "expected frozen/unhealthy error: {text}");
    assert!(
        text.contains("session_frozen")
            || text.contains("runtime_unhealthy")
            || text.contains("frozen")
            || text.contains("re-pin"),
        "unexpected error body: {text}"
    );

    // Heartbeat reports freeze / issues
    let hb = client.request(
        "tools/call",
        json!({ "name": "locus_heartbeat", "arguments": {} }),
    );
    let (text, is_err) = McpClient::tool_text(&hb);
    assert!(is_err);
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["ok"], false);
    assert!(
        body["frozen"].as_bool().unwrap_or(false)
            || body["issues"]
                .as_array()
                .map(|a| !a.is_empty())
                .unwrap_or(false)
    );
}

#[test]
fn github_check_repo_and_vercel_env_freeze_over_mcp() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    // Binding with repo allowlist + vercel env freeze
    let acme = Binding::from_body(BindingBody {
        id: "bnd_acme".into(),
        alias: "acme".into(),
        tenant: "acme-corp".into(),
        principal: None,
        description: None,
        policy: Policy::default(),
        providers: vec![
            ProviderBinding {
                provider: "github".into(),
                account: "acme-gh".into(),
                credential_ref: "phm:GH_TOKEN_ACME".into(),
                scope: Scope {
                    orgs: vec!["acme-corp".into()],
                    repos: vec!["acme-corp/web".into()],
                    ..Scope::default()
                },
                upstream: None,
            },
            ProviderBinding {
                provider: "vercel".into(),
                account: "acme-vc".into(),
                credential_ref: "phm:VERCEL_TOKEN_ACME".into(),
                scope: Scope {
                    team_id: Some("team_acme".into()),
                    env: vec!["preview".into()],
                    ..Scope::default()
                },
                upstream: None,
            },
            ProviderBinding {
                provider: "supabase".into(),
                account: "acme-db".into(),
                credential_ref: "phm:SUPABASE_ACME".into(),
                scope: Scope {
                    project_ref: Some("proj_acme".into()),
                    ..Scope::default()
                },
                upstream: None,
            },
        ],
    });
    store.save_binding(&acme).unwrap();
    store
        .pin("acme", dir.path(), Some("mcp-freeze-tools".into()), false)
        .unwrap();

    let mut client = McpClient::spawn(dir.path(), Framing::Ndjson);
    handshake(&mut client);

    let ok = client.request(
        "tools/call",
        json!({
            "name": "github.check_repo",
            "arguments": { "full_name": "acme-corp/web" }
        }),
    );
    let (text, is_err) = McpClient::tool_text(&ok);
    assert!(!is_err, "check_repo allow: {text}");
    assert!(text.contains("\"allowed\":true") || text.contains("acme-corp/web"));

    let deny = client.request(
        "tools/call",
        json!({
            "name": "github.check_repo",
            "arguments": { "full_name": "evil/other" }
        }),
    );
    let (text, is_err) = McpClient::tool_text(&deny);
    assert!(is_err, "expected org/repo deny: {text}");
    assert!(text.contains("scope freeze") || text.contains("refusing"));

    let env_deny = client.request(
        "tools/call",
        json!({
            "name": "vercel.scope",
            "arguments": { "env": "production" }
        }),
    );
    let (text, is_err) = McpClient::tool_text(&env_deny);
    assert!(is_err, "expected vercel env freeze: {text}");
    assert!(text.contains("scope freeze") || text.contains("production"));

    // supabase project_ref freeze still holds
    let sb = client.request(
        "tools/call",
        json!({
            "name": "supabase.scope",
            "arguments": { "project_ref": "proj_evil" }
        }),
    );
    let (text, is_err) = McpClient::tool_text(&sb);
    assert!(is_err);
    assert!(text.contains("scope freeze") || text.contains("proj_evil"));
}

// ─── Resources + prompts (AI-native surface) ────────────────────────────────

#[test]
fn resources_list_and_read_session_doctor_bindings_unpinned() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    sample_bindings(&store);

    let mut client = McpClient::spawn(dir.path(), Framing::Ndjson);
    handshake(&mut client);

    let list = client.request("resources/list", json!({}));
    let resources = list["result"]["resources"]
        .as_array()
        .expect("resources array");
    let uris: Vec<&str> = resources.iter().filter_map(|r| r["uri"].as_str()).collect();
    assert!(uris.contains(&"locus://session"), "uris={uris:?}");
    assert!(uris.contains(&"locus://doctor"), "uris={uris:?}");
    assert!(uris.contains(&"locus://bindings"), "uris={uris:?}");
    for r in resources {
        assert_eq!(r["mimeType"], "application/json");
        assert!(r["description"].as_str().is_some());
    }

    // session — unpinned
    let sess = client.request("resources/read", json!({ "uri": "locus://session" }));
    assert!(sess.get("result").is_some(), "session read failed: {sess}");
    let text = parse_resource_text(&sess);
    let body: Value = serde_json::from_str(&text).expect("session json");
    assert_eq!(body["pinned"], false);

    // doctor lite — structured report, no secrets
    let doc = client.request("resources/read", json!({ "uri": "locus://doctor" }));
    assert!(doc.get("result").is_some(), "doctor read failed: {doc}");
    let text = parse_resource_text(&doc);
    let body: Value = serde_json::from_str(&text).expect("doctor json");
    assert!(body.get("verdict").is_some() || body.get("runtime").is_some());
    assert!(body.get("ok").is_some() || body.get("runtime").is_some());
    let dumped = text.to_lowercase();
    assert!(
        !dumped.contains("sk-") && !dumped.contains("ghp_"),
        "doctor must not leak secret-like material"
    );

    // bindings list
    let binds = client.request("resources/read", json!({ "uri": "locus://bindings" }));
    let text = parse_resource_text(&binds);
    let body: Value = serde_json::from_str(&text).expect("bindings json");
    let arr = body.as_array().expect("bindings array");
    assert!(
        arr.iter().any(|b| b["alias"] == "acme"),
        "expected acme binding: {body}"
    );
    assert!(
        arr.iter().all(|b| b.get("credential_ref").is_none()),
        "summaries must not include raw credential values"
    );

    // unknown resource
    let bad = client.request("resources/read", json!({ "uri": "locus://nope" }));
    assert!(
        bad.get("error").is_some(),
        "unknown resource should error: {bad}"
    );
}

#[test]
fn resources_session_whoami_when_pinned() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    sample_bindings(&store);
    store
        .pin("acme", dir.path(), Some("mcp-res".into()), false)
        .unwrap();

    let mut client = McpClient::spawn(dir.path(), Framing::ContentLength);
    handshake(&mut client);

    let sess = client.request("resources/read", json!({ "uri": "locus://session" }));
    let text = parse_resource_text(&sess);
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["binding_alias"], "acme");
    assert_eq!(body["tenant"], "acme-corp");
    assert_eq!(body["seal_ok"], true);
    assert!(body.get("providers").is_some());
    // Credential source/presence only — locator names are absent.
    for p in body["providers"].as_array().unwrap() {
        assert!(p.get("credential_ref").is_none());
        assert_eq!(p["credential"]["present"], true);
        assert_eq!(p["credential"]["source"], "phantom");
    }
    for canary in ["GH_TOKEN_ACME", "VERCEL_TOKEN_ACME", "SUPABASE_ACME"] {
        assert!(!text.contains(canary), "session resource leaked {canary}");
    }

    let doc = client.request("resources/read", json!({ "uri": "locus://doctor" }));
    let text = parse_resource_text(&doc);
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["pinned"], "acme");
    // Doctor lite may WARN on unresolved phm: (Phantom not probed in MCP resource).
    // Assert pin/runtime health rather than full SAFE verdict.
    assert!(
        body["runtime"]["ok"].as_bool().unwrap_or(false)
            || body["runtime"]["pinned"].as_bool().unwrap_or(false),
        "doctor resource should report pinned runtime: {body}"
    );
    assert!(body.get("verdict").is_some());
}

#[test]
fn prompts_list_and_get_locus_context() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    sample_bindings(&store);

    // Unpinned context
    let mut client = McpClient::spawn(dir.path(), Framing::Ndjson);
    handshake(&mut client);

    let list = client.request("prompts/list", json!({}));
    let prompts = list["result"]["prompts"].as_array().expect("prompts");
    assert!(
        prompts.iter().any(|p| p["name"] == "locus_context"),
        "expected locus_context: {list}"
    );

    let got = client.request("prompts/get", json!({ "name": "locus_context" }));
    assert!(got.get("result").is_some(), "prompts/get failed: {got}");
    let messages = got["result"]["messages"].as_array().expect("messages");
    assert!(!messages.is_empty());
    let text = messages[0]["content"]["text"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert!(
        text.to_lowercase().contains("cannot pin"),
        "context must say agents cannot pin: {text}"
    );
    assert!(
        text.contains("pinned**: false")
            || text.contains("pinned: false")
            || text.contains("No sealed session")
            || text.contains("**pinned**: false"),
        "unpinned context should state unpinned: {text}"
    );
    assert!(
        text.contains("locus://session") || text.contains("locus_whoami"),
        "context should point at session resource/tool"
    );

    // Unknown prompt
    let bad = client.request("prompts/get", json!({ "name": "nope" }));
    assert!(bad.get("error").is_some(), "unknown prompt: {bad}");

    // Pinned context includes tenant + frozen scopes
    store
        .pin("acme", dir.path(), Some("mcp-prompt".into()), false)
        .unwrap();
    drop(client);
    let mut client = McpClient::spawn(dir.path(), Framing::Ndjson);
    handshake(&mut client);
    let got = client.request("prompts/get", json!({ "name": "locus_context" }));
    let text = got["result"]["messages"][0]["content"]["text"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert!(
        text.contains("acme"),
        "pinned context missing alias: {text}"
    );
    assert!(
        text.contains("acme-corp"),
        "pinned context missing tenant: {text}"
    );
    assert!(
        text.contains("project_ref") || text.contains("proj_acme") || text.contains("github"),
        "pinned context should mention frozen scopes/providers: {text}"
    );
    assert!(
        text.to_lowercase().contains("cannot pin"),
        "still cannot pin when pinned: {text}"
    );
}

#[test]
fn malformed_workspace_blocks_auto_pin_and_surfaces_unsafe_prompt() {
    let dir = tempdir().unwrap();
    let home = dir.path().join("home");
    let project = dir.path().join("project");
    let store = Store::open(&home).unwrap();
    sample_bindings(&store);
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join(".locus.toml"), "allowed_bindings = [").unwrap();

    let mut client = McpClient::spawn_opts(
        &home,
        Framing::ContentLength,
        Some(&project),
        &[("LOCUS_MCP_AUTO_PIN", "1")],
    );
    handshake(&mut client);
    let listed = client.request("tools/list", json!({}));
    let names: Vec<_> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert!(names.iter().all(|name| name.starts_with("locus_")));
    assert!(!names.iter().any(|name| name.starts_with("github.")));

    let prompt = client.request("prompts/get", json!({ "name": "locus_context" }));
    let text = prompt["result"]["messages"][0]["content"]["text"]
        .as_str()
        .unwrap();
    assert!(text.contains("status**: `unsafe`"));
    assert!(text.contains("do not use provider tools"));
}

#[test]
fn mcp_auto_pin_from_workspace_default_binding() {
    let dir = tempdir().unwrap();
    let home = dir.path().join("locus-home");
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let store = Store::open(&home).unwrap();
    sample_bindings(&store);

    // Workspace with default_binding enables preferred MCP auto-pin
    std::fs::write(
        project.join(".locus.toml"),
        r#"
version = 1
default_binding = "acme"
allowed_bindings = ["acme"]
"#,
    )
    .unwrap();

    // Explicit enable + cwd = project so auto-pin finds .locus.toml
    let mut client = McpClient::spawn_opts(
        &home,
        Framing::Ndjson,
        Some(&project),
        &[("LOCUS_MCP_AUTO_PIN", "1")],
    );
    handshake(&mut client);

    // After initialize auto-pin, tools/list should include provider tools
    let list = client.request("tools/list", json!({}));
    let tools = list["result"]["tools"].as_array().expect("tools");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(
        names.contains(&"github.scope"),
        "auto-pin should expose provider tools, got {names:?}"
    );
    assert_eq!(tools[0]["name"], "locus_whoami");
    for t in tools {
        let desc = t["description"].as_str().unwrap_or("");
        assert!(
            desc.starts_with("[locus:acme]"),
            "auto-pinned descriptions: {desc}"
        );
    }

    let who = client.request(
        "tools/call",
        json!({ "name": "locus_whoami", "arguments": {} }),
    );
    let (text, is_err) = McpClient::tool_text(&who);
    assert!(!is_err, "whoami after auto-pin: {text}");
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["binding_alias"], "acme");
    assert_eq!(body["tenant"], "acme-corp");

    // Audit trail: session.auto_pin
    let events = store.read_audit_events().unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.op == "session.auto_pin" && e.binding == "acme"),
        "expected session.auto_pin audit, events={events:?}"
    );
}

#[test]
fn mcp_auto_pin_default_on_with_default_binding_without_explicit_env() {
    // Preferred default: .locus.toml with default_binding ⇒ auto-pin on MCP start
    // even without LOCUS_MCP_AUTO_PIN=1 (policy treats default_binding as enable).
    let dir = tempdir().unwrap();
    let home = dir.path().join("locus-home");
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let store = Store::open(&home).unwrap();
    sample_bindings(&store);
    std::fs::write(
        project.join(".locus.toml"),
        r#"
version = 1
default_binding = "acme"
require_pin = true
allowed_bindings = ["acme"]
"#,
    )
    .unwrap();

    // Do NOT set LOCUS_MCP_AUTO_PIN=1 — rely on workspace default_binding / require_pin.
    // spawn_opts defaults LOCUS_MCP_AUTO_PIN=0 which explicitly disables; override by unsetting
    // via empty and use a custom spawn that omits the kill-switch.
    let mut child = Command::new(mcp_bin())
        .env("LOCUS_HOME", &home)
        .env_remove("LOCUS_MCP_AUTO_PIN")
        .env_remove("LOCUS_AUTO_PIN")
        .current_dir(&project)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    let stdin = child.stdin.take().unwrap();
    let stdout = BufReader::new(child.stdout.take().unwrap());
    let mut client = McpClient {
        child,
        stdin,
        stdout,
        framing: Framing::Ndjson,
        next_id: 1,
    };
    handshake(&mut client);

    let list = client.request("tools/list", json!({}));
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(
        names.contains(&"github.scope"),
        "default_binding + require_pin should auto-pin without LOCUS_MCP_AUTO_PIN: {names:?}"
    );
    let _ = store; // keep home alive semantics
}

#[test]
fn mcp_auto_pin_respects_allowlist() {
    let dir = tempdir().unwrap();
    let home = dir.path().join("locus-home");
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let store = Store::open(&home).unwrap();
    sample_bindings(&store);

    // default_binding blocked by allowlist — pin_auto must not force
    std::fs::write(
        project.join(".locus.toml"),
        r#"
version = 1
default_binding = "acme"
allowed_bindings = ["other"]
require_pin = true
"#,
    )
    .unwrap();

    let mut client = McpClient::spawn_opts(
        &home,
        Framing::Ndjson,
        Some(&project),
        &[("LOCUS_MCP_AUTO_PIN", "1")],
    );
    handshake(&mut client);

    let list = client.request("tools/list", json!({}));
    let tools = list["result"]["tools"].as_array().unwrap();
    for t in tools {
        let name = t["name"].as_str().unwrap();
        assert!(
            name.starts_with("locus_"),
            "allowlist block must leave session unpinned, got {name}"
        );
    }
    assert!(store.active_session().unwrap().is_none());
}

#[test]
fn mcp_auto_pin_disabled_stays_unpinned() {
    let dir = tempdir().unwrap();
    let home = dir.path().join("locus-home");
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let store = Store::open(&home).unwrap();
    sample_bindings(&store);
    std::fs::write(
        project.join(".locus.toml"),
        r#"
version = 1
default_binding = "acme"
"#,
    )
    .unwrap();

    // Explicit kill-switch
    let mut client = McpClient::spawn_opts(
        &home,
        Framing::Ndjson,
        Some(&project),
        &[("LOCUS_MCP_AUTO_PIN", "0")],
    );
    handshake(&mut client);

    let list = client.request("tools/list", json!({}));
    let tools = list["result"]["tools"].as_array().unwrap();
    for t in tools {
        assert!(t["name"].as_str().unwrap().starts_with("locus_"));
    }
    assert!(store.active_session().unwrap().is_none());
}

#[test]
fn mcp_auto_pin_via_clients_auto_pin_cwd_config() {
    let dir = tempdir().unwrap();
    let home = dir.path().join("locus-home");
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let store = Store::open(&home).unwrap();
    sample_bindings(&store);
    std::fs::write(
        project.join(".locus.toml"),
        r#"
version = 1
default_binding = "acme"
allowed_bindings = ["acme"]
"#,
    )
    .unwrap();
    std::fs::write(
        home.join("config.toml"),
        r#"
[clients]
auto_pin = "cwd"
"#,
    )
    .unwrap();

    // No LOCUS_MCP_AUTO_PIN kill-switch — clients.auto_pin=cwd enables policy.
    let mut child = Command::new(mcp_bin())
        .env("LOCUS_HOME", &home)
        .env_remove("LOCUS_MCP_AUTO_PIN")
        .current_dir(&project)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    let stdin = child.stdin.take().unwrap();
    let stdout = BufReader::new(child.stdout.take().unwrap());
    let mut client = McpClient {
        child,
        stdin,
        stdout,
        framing: Framing::Ndjson,
        next_id: 1,
    };
    handshake(&mut client);
    let list = client.request("tools/list", json!({}));
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(
        names.contains(&"github.scope"),
        "clients.auto_pin=cwd should enable auto-pin: {names:?}"
    );
    let _ = store;
}

#[test]
fn locus_safe_next_unpinned_and_ready() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    sample_bindings(&store);

    // Unpinned → action=enter, isError=true (not ready)
    let mut client = McpClient::spawn(dir.path(), Framing::Ndjson);
    handshake(&mut client);

    let list = client.request("tools/list", json!({}));
    let tools = list["result"]["tools"].as_array().unwrap();
    assert!(
        tools.iter().any(|t| t["name"] == "locus_safe_next"),
        "locus_safe_next must appear in control catalog"
    );
    let safe_desc = tools
        .iter()
        .find(|t| t["name"] == "locus_safe_next")
        .and_then(|t| t["description"].as_str())
        .unwrap_or("");
    assert!(
        safe_desc.contains("enter") || safe_desc.contains("approve"),
        "description should mention next actions: {safe_desc}"
    );

    let call = client.request(
        "tools/call",
        json!({ "name": "locus_safe_next", "arguments": {} }),
    );
    let (text, is_err) = McpClient::tool_text(&call);
    assert!(is_err, "unpinned safe_next should isError: {text}");
    let body: Value = serde_json::from_str(&text).expect("safe_next json");
    assert_eq!(body["action"], "enter");
    assert_eq!(body["ready"], false);
    assert!(
        body["command"]
            .as_str()
            .unwrap_or("")
            .contains("locus enter")
            || body["command"].as_str().unwrap_or("").contains("locus pin"),
        "command for human pin: {body}"
    );
    assert!(
        body["message"]
            .as_str()
            .unwrap_or("")
            .contains("cannot pin")
            || body["message"]
                .as_str()
                .unwrap_or("")
                .contains("Not pinned"),
        "message should explain gate: {body}"
    );
    // Never leak secret-looking material
    let raw = text.to_lowercase();
    assert!(!raw.contains("sk-"));
    assert!(!raw.contains("ghp_"));
    assert!(!raw.contains("phm_"));

    // Pin → action=ready, isError=false
    store
        .pin("acme", dir.path(), Some("mcp-safe-next".into()), false)
        .unwrap();
    let call2 = client.request(
        "tools/call",
        json!({ "name": "locus_safe_next", "arguments": {} }),
    );
    let (text2, is_err2) = McpClient::tool_text(&call2);
    assert!(!is_err2, "pinned safe_next should be ready: {text2}");
    let body2: Value = serde_json::from_str(&text2).unwrap();
    assert_eq!(body2["action"], "ready");
    assert_eq!(body2["ready"], true);
    assert_eq!(body2["binding"], "acme");
    assert_eq!(body2["tenant"], "acme-corp");
    assert!(body2.get("command").is_none() || body2["command"].is_null());
}

#[test]
fn resources_and_prompts_reflect_auto_pin() {
    // After MCP auto-pin, resources/prompts must show the new pin (not stale unpinned).
    let dir = tempdir().unwrap();
    let home = dir.path().join("locus-home");
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let store = Store::open(&home).unwrap();
    sample_bindings(&store);
    std::fs::write(
        project.join(".locus.toml"),
        r#"
version = 1
default_binding = "acme"
allowed_bindings = ["acme"]
require_pin = true
"#,
    )
    .unwrap();

    let mut client = McpClient::spawn_opts(
        &home,
        Framing::Ndjson,
        Some(&project),
        &[("LOCUS_MCP_AUTO_PIN", "1")],
    );
    let init = handshake(&mut client);
    let instructions = init["instructions"].as_str().unwrap_or("");
    assert!(
        instructions.contains("acme") || instructions.contains("Active pin"),
        "initialize.instructions should include pin after auto-pin: {instructions}"
    );
    assert!(
        instructions.contains("locus_safe_next") || instructions.contains("locus_whoami"),
        "instructions should point at compliance tools: {instructions}"
    );

    // resources/list descriptions tagged with live pin
    let rlist = client.request("resources/list", json!({}));
    let resources = rlist["result"]["resources"].as_array().unwrap();
    for r in resources {
        let desc = r["description"].as_str().unwrap_or("");
        assert!(
            desc.contains("[locus:acme]"),
            "resource description after auto-pin: {desc}"
        );
    }

    // resources/read locus://session is pinned
    let sess = client.request("resources/read", json!({ "uri": "locus://session" }));
    let text = parse_resource_text(&sess);
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["binding_alias"], "acme");
    assert_eq!(body["tenant"], "acme-corp");

    // prompts/list + get reflect pin
    let plist = client.request("prompts/list", json!({}));
    let pdesc = plist["result"]["prompts"][0]["description"]
        .as_str()
        .unwrap_or("");
    assert!(
        pdesc.contains("[locus:acme]"),
        "prompt list description after auto-pin: {pdesc}"
    );

    let pget = client.request("prompts/get", json!({ "name": "locus_context" }));
    let ptext = pget["result"]["messages"][0]["content"]["text"]
        .as_str()
        .unwrap_or("");
    assert!(
        ptext.contains("acme") && ptext.contains("acme-corp"),
        "locus_context after auto-pin: {ptext}"
    );
    assert!(
        ptext.to_lowercase().contains("cannot pin"),
        "still cannot pin: {ptext}"
    );
    let _ = store;
}

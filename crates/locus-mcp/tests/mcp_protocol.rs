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
        if !extra_env
            .iter()
            .any(|(key, _)| *key == locus_core::EXECUTOR_CAPABILITY_ENV)
        {
            if let Ok(store) = Store::open(locus_home) {
                if let Ok(Some(session)) = store.active_session() {
                    if let Ok(capability) = store.grant_executor_capability(&session) {
                        cmd.env(locus_core::EXECUTOR_CAPABILITY_ENV, capability);
                    }
                }
            }
        }
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

/// Second sample binding (different tenant + binding id) for pin-swap tests.
fn beta_binding(store: &Store) {
    let beta = Binding::from_body(BindingBody {
        id: "bnd_beta".into(),
        alias: "beta".into(),
        tenant: "beta-corp".into(),
        principal: None,
        description: None,
        policy: Policy::default(),
        providers: vec![ProviderBinding {
            provider: "github".into(),
            account: "beta-gh".into(),
            credential_ref: "phm:GH_TOKEN_BETA".into(),
            scope: Scope {
                orgs: vec!["beta-corp".into()],
                ..Scope::default()
            },
            upstream: None,
        }],
    });
    store.save_binding(&beta).unwrap();
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
fn direct_stdio_with_pin_but_no_executor_capability_exposes_no_provider_tools() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    sample_bindings(&store);
    store
        .pin("acme", dir.path(), Some("local-control".into()), false)
        .unwrap();

    let mut client = McpClient::spawn_opts(
        dir.path(),
        Framing::Ndjson,
        None,
        &[(locus_core::EXECUTOR_CAPABILITY_ENV, "")],
    );
    handshake(&mut client);
    let list = client.request("tools/list", json!({}));
    if let Some(tools) = list["result"]["tools"].as_array() {
        assert!(tools.iter().all(|tool| tool["name"]
            .as_str()
            .is_some_and(|name| name.starts_with("locus_"))));
    } else {
        assert!(
            list.get("error").is_some(),
            "unexpected tools/list response: {list}"
        );
    }

    let call = client.request(
        "tools/call",
        json!({"name": "github.scope", "arguments": {}}),
    );
    assert!(
        call.get("error").is_some() || call["result"]["isError"] == true,
        "provider call unexpectedly succeeded without executor authority: {call}"
    );
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
import sys, json, os, pathlib
if len(sys.argv) > 1:
    pathlib.Path(sys.argv[1]).write_text(os.environ.get("GH_TOKEN", "missing"))
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

#[cfg(target_os = "macos")]
fn sandbox_scope_fixture_script() -> &'static str {
    r#"#!/bin/sh
printf '%s|%s|%s|%s|%s|%s\n' \
  "${LOCUS_WORKER_PROVIDER:-missing}" \
  "${LOCUS_WORKER_SANDBOXED:-missing}" \
  "${LOCUS_WORKER_SANDBOX_BACKEND:-missing}" \
  "${GH_TOKEN:-missing}" \
  "${SUPABASE_ACCESS_TOKEN:-missing}" \
  "${LOCUS_PROVIDERS:-missing}" > worker-env.txt
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"sandbox-scope-fixture","version":"1"}}}'
      ;;
    *'"method":"tools/list"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"ping","description":"sandbox scope probe","inputSchema":{"type":"object"}}]}}'
      ;;
    *'"method":"tools/call"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"sandbox scope probe ok"}],"isError":false}}'
      ;;
  esac
done
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
    let discovery_marker = dir.path().join("discovery-worker-token.txt");
    std::env::set_var("LOCUS_DISCOVERY_GITHUB_TOKEN", "discovery-github-canary");

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
                credential_ref: "env:LOCUS_DISCOVERY_GITHUB_TOKEN".into(),
                scope: Scope {
                    orgs: vec!["acme-corp".into()],
                    ..Scope::default()
                },
                upstream: Some(
                    UpstreamSpec::new("python3")
                        .with_args([
                            "-u",
                            "-c",
                            mock_upstream_script(),
                            discovery_marker.to_str().unwrap(),
                        ])
                        .resolve_secrets(true),
                ),
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

    // Discovery is side-effect free: control + synthetic only until one
    // authorized upstream call starts the provider worker.
    assert!(names.contains(&"locus_whoami"));
    assert!(names.contains(&"github.scope")); // synthetic kept
    assert!(!names.contains(&"github.ping"));
    assert!(!names.contains(&"github.echo"));
    assert!(names.contains(&"supabase.scope")); // synthetic only (no upstream)
    assert!(!names.contains(&"supabase.ping"));
    assert!(
        !discovery_marker.exists(),
        "tools/list started a credential-bearing provider child"
    );

    // Upstream call
    let ping = client.request(
        "tools/call",
        json!({ "name": "github.ping", "arguments": {} }),
    );
    let (text, is_err) = McpClient::tool_text(&ping);
    assert!(!is_err, "github.ping failed: {text}");
    assert!(text.contains("pong"), "expected pong in {text}");
    assert_eq!(
        std::fs::read_to_string(&discovery_marker).unwrap(),
        "discovery-github-canary"
    );

    let list = client.request("tools/list", json!({}));
    let tools = list["result"]["tools"].as_array().expect("tools");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"github.ping"));
    assert!(names.contains(&"github.echo"));

    let echo = client.request(
        "tools/call",
        json!({
            "name": "github.echo",
            "arguments": { "text": "via-locus-mcp" }
        }),
    );
    std::env::remove_var("LOCUS_DISCOVERY_GITHUB_TOKEN");
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
fn require_approval_blocks_before_hostile_worker_sees_resolved_token() {
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }

    let dir = tempdir().unwrap();
    let marker = dir.path().join("worker-startup-token.txt");
    let marker_arg = marker.display().to_string();
    let store = Store::open(dir.path()).unwrap();
    let binding = Binding::from_body(BindingBody {
        id: "bnd_hostile".into(),
        alias: "hostile".into(),
        tenant: "hostile-test".into(),
        principal: None,
        description: None,
        policy: Policy {
            require_approval: vec!["github.delete_repo".into()],
            ..Policy::default()
        },
        providers: vec![ProviderBinding {
            provider: "github".into(),
            account: "hostile-gh".into(),
            credential_ref: "env:HOSTILE_WORKER_TOKEN".into(),
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
        .pin("hostile", dir.path(), Some("local-control".into()), false)
        .unwrap();

    let mut client = McpClient::spawn_opts(
        dir.path(),
        Framing::Ndjson,
        None,
        &[("HOSTILE_WORKER_TOKEN", "worker-canary-token")],
    );
    handshake(&mut client);
    let response = client.request(
        "tools/call",
        json!({
            "name": "github.delete_repo",
            "arguments": { "owner": "acme", "repo": "critical" }
        }),
    );
    let (text, is_error) = McpClient::tool_text(&response);
    assert!(
        is_error,
        "require_approval call unexpectedly succeeded: {text}"
    );
    assert!(
        text.contains("requires_approval"),
        "unexpected block: {text}"
    );
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        !marker.exists(),
        "worker started and observed credential before approval: {}",
        std::fs::read_to_string(&marker).unwrap_or_default()
    );
    assert_eq!(store.pending_approvals().unwrap().len(), 1);
}

#[cfg(target_os = "macos")]
#[test]
fn broker_session_gates_sandbox_start_and_scopes_each_provider_environment() {
    use std::os::unix::fs::PermissionsExt;

    assert!(
        std::path::Path::new("/usr/bin/sandbox-exec").is_file(),
        "native Seatbelt backend is required for this integration regression"
    );

    let dir = tempdir().unwrap();
    let home = dir.path().join("locus-home");
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let fixture = project.join("sandbox-scope-fixture.sh");
    std::fs::write(&fixture, sandbox_scope_fixture_script()).unwrap();
    std::fs::set_permissions(&fixture, std::fs::Permissions::from_mode(0o700)).unwrap();

    let store = Store::open(&home).unwrap();
    let upstream = || {
        Some(
            UpstreamSpec::new(fixture.display().to_string())
                .resolve_secrets(true)
                .sandbox(true),
        )
    };
    let binding = Binding::from_body(BindingBody {
        id: "bnd_sandbox_scope".into(),
        alias: "sandbox-scope".into(),
        tenant: "sandbox-scope".into(),
        principal: None,
        description: None,
        policy: Policy {
            require_approval: vec!["github.delete_repo".into()],
            ..Policy::default()
        },
        providers: vec![
            ProviderBinding {
                provider: "github".into(),
                account: "sandbox-gh".into(),
                credential_ref: "env:SANDBOX_GITHUB_TOKEN".into(),
                scope: Scope {
                    orgs: vec!["sandbox-org".into()],
                    ..Scope::default()
                },
                upstream: upstream(),
            },
            ProviderBinding {
                provider: "supabase".into(),
                account: "sandbox-db".into(),
                credential_ref: "env:SANDBOX_SUPABASE_TOKEN".into(),
                scope: Scope {
                    project_ref: Some("sandbox-project".into()),
                    read_only: Some(true),
                    ..Scope::default()
                },
                upstream: upstream(),
            },
        ],
    });
    store.save_binding(&binding).unwrap();
    let session = store
        .pin(
            "sandbox-scope",
            &project,
            Some("broker-sandbox-regression".into()),
            false,
        )
        .unwrap();
    assert_eq!(session.seal_version, 3);
    assert!(
        session.authority_anchor.is_some(),
        "session must be backed by the live authority broker"
    );

    let github_marker =
        std::path::Path::new(&session.worker_home).join("slots/github/worker-env.txt");
    let supabase_marker =
        std::path::Path::new(&session.worker_home).join("slots/supabase/worker-env.txt");
    let mut client = McpClient::spawn_opts(
        &home,
        Framing::Ndjson,
        Some(&project),
        &[
            ("SANDBOX_GITHUB_TOKEN", "github-sandbox-canary"),
            ("SANDBOX_SUPABASE_TOKEN", "supabase-sandbox-canary"),
        ],
    );
    handshake(&mut client);

    let discovery = client.request("tools/list", json!({}));
    assert!(
        discovery.get("result").is_some(),
        "tools/list failed: {discovery}"
    );
    assert!(!github_marker.exists() && !supabase_marker.exists());

    let blocked = client.request(
        "tools/call",
        json!({
            "name": "github.delete_repo",
            "arguments": { "owner": "sandbox-org", "repo": "critical" }
        }),
    );
    let (blocked_text, blocked_error) = McpClient::tool_text(&blocked);
    assert!(blocked_error && blocked_text.contains("requires_approval"));
    assert!(
        !github_marker.exists() && !supabase_marker.exists(),
        "approval-blocked call started a sandbox worker"
    );

    let github = client.request(
        "tools/call",
        json!({ "name": "github.ping", "arguments": {} }),
    );
    let (github_text, github_error) = McpClient::tool_text(&github);
    assert!(!github_error, "sandboxed GitHub call failed: {github_text}");
    assert_eq!(
        std::fs::read_to_string(&github_marker).unwrap().trim(),
        "github|1|sandbox-exec|github-sandbox-canary|missing|github"
    );
    assert!(
        !supabase_marker.exists(),
        "GitHub authorization started the Supabase worker"
    );

    let supabase = client.request(
        "tools/call",
        json!({ "name": "supabase.ping", "arguments": {} }),
    );
    let (supabase_text, supabase_error) = McpClient::tool_text(&supabase);
    assert!(
        !supabase_error,
        "sandboxed Supabase call failed: {supabase_text}"
    );
    assert_eq!(
        std::fs::read_to_string(&supabase_marker).unwrap().trim(),
        "supabase|1|sandbox-exec|missing|supabase-sandbox-canary|supabase"
    );
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
fn mcp_workspace_default_cannot_grant_auto_pin_authority() {
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

    // Workspace labels are hints only; an MCP process cannot mint authority.
    let list = client.request("tools/list", json!({}));
    let tools = list["result"]["tools"].as_array().expect("tools");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.iter().all(|name| name.starts_with("locus_")));
    assert_eq!(tools[0]["name"], "locus_whoami");
    for t in tools {
        let desc = t["description"].as_str().unwrap_or("");
        assert!(
            desc.starts_with("[locus:unpinned]"),
            "untrusted workspace label changed description authority: {desc}"
        );
    }
    assert!(store.active_session().unwrap().is_none());
    let events = store.read_audit_events().unwrap();
    assert!(!events.iter().any(|event| event.op == "session.auto_pin"));
    // The refusal is honest and audited: the advisory workspace binding is
    // recorded, and the reason states operator delegation is required.
    let denied = events
        .iter()
        .find(|event| event.op == "session.auto_pin_denied")
        .expect("advisory auto-pin probe must audit session.auto_pin_denied");
    assert_eq!(denied.binding, "acme");
    let reason = denied
        .detail
        .as_ref()
        .and_then(|d| d["reason"].as_str())
        .unwrap_or("");
    assert!(
        reason.contains("auto-pin requires operator delegation"),
        "denial reason must be the honest operator-delegation error: {reason}"
    );
    assert_eq!(
        denied
            .detail
            .as_ref()
            .and_then(|d| d["advisory_binding"].as_str()),
        Some("acme")
    );
}

#[test]
fn mcp_default_binding_without_capability_stays_unpinned() {
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
    assert!(names.iter().all(|name| name.starts_with("locus_")));
    assert!(store.active_session().unwrap().is_none());
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
    // Kill switch skips the advisory probe entirely — not even a denial audit.
    let events = store.read_audit_events().unwrap();
    assert!(!events
        .iter()
        .any(|event| event.op.starts_with("session.auto_pin")));
}

#[test]
fn mcp_client_auto_pin_config_cannot_grant_authority() {
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
    assert!(names.iter().all(|name| name.starts_with("locus_")));
    assert!(store.active_session().unwrap().is_none());
    // clients.auto_pin=cwd enables only the advisory probe: honest denial audit,
    // never a session.auto_pin grant.
    let events = store.read_audit_events().unwrap();
    assert!(!events.iter().any(|event| event.op == "session.auto_pin"));
    assert!(events
        .iter()
        .any(|event| event.op == "session.auto_pin_denied"));
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

    // A process started before the pin has no generation-bound executor grant.
    store
        .pin("acme", dir.path(), Some("mcp-safe-next".into()), false)
        .unwrap();
    let call2 = client.request(
        "tools/call",
        json!({ "name": "locus_safe_next", "arguments": {} }),
    );
    let (text2, is_err2) = McpClient::tool_text(&call2);
    assert!(is_err2, "pre-pin process upgraded authority: {text2}");
    assert!(text2.contains("executor_authority_unavailable"));

    drop(client);
    let mut client = McpClient::spawn(dir.path(), Framing::Ndjson);
    handshake(&mut client);
    let call2 = client.request(
        "tools/call",
        json!({ "name": "locus_safe_next", "arguments": {} }),
    );
    let (text2, is_err2) = McpClient::tool_text(&call2);
    assert!(
        !is_err2,
        "supervised post-pin safe_next should be ready: {text2}"
    );
    let body2: Value = serde_json::from_str(&text2).unwrap();
    assert_eq!(body2["action"], "ready");
    assert_eq!(body2["ready"], true);
    assert_eq!(body2["binding"], "acme");
    assert_eq!(body2["tenant"], "acme-corp");
    assert!(body2.get("command").is_none() || body2["command"].is_null());
}

#[test]
fn resources_and_prompts_do_not_treat_auto_pin_labels_as_authority() {
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
    assert!(instructions.contains("Currently unpinned"));
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
            desc.contains("[locus:unpinned]"),
            "resource description accepted auto-pin label: {desc}"
        );
    }

    // resources/read locus://session remains unpinned.
    let sess = client.request("resources/read", json!({ "uri": "locus://session" }));
    let text = parse_resource_text(&sess);
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["pinned"], false);

    // prompts/list + get reflect pin
    let plist = client.request("prompts/list", json!({}));
    let pdesc = plist["result"]["prompts"][0]["description"]
        .as_str()
        .unwrap_or("");
    assert!(
        pdesc.contains("[locus:unpinned]"),
        "prompt list accepted auto-pin label: {pdesc}"
    );

    let pget = client.request("prompts/get", json!({ "name": "locus_context" }));
    let ptext = pget["result"]["messages"][0]["content"]["text"]
        .as_str()
        .unwrap_or("");
    assert!(
        ptext.to_lowercase().contains("unpinned"),
        "locus_context accepted auto-pin label: {ptext}"
    );
    assert!(
        ptext.to_lowercase().contains("cannot pin"),
        "still cannot pin: {ptext}"
    );
    assert!(store.active_session().unwrap().is_none());
}

/// Dispatch-level regression net for the always-false session_ok bug: a healthy
/// pinned fixture with deterministic external facts (fake `phantom` shim on
/// PATH listing every phm: ref) must verify with session_ok=true over
/// tools/call — same pack as `locus verify session --json`.
#[cfg(unix)]
#[test]
fn locus_verify_session_session_ok_true_on_healthy_pin() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    sample_bindings(&store);
    store
        .pin("acme", dir.path(), Some("mcp-verify".into()), false)
        .unwrap();

    // Fake phantom shim: --version ok + list prints every phm: name the
    // sample bindings reference, so unresolved_phm is deterministically empty.
    let shim_dir = dir.path().join("shim-bin");
    std::fs::create_dir_all(&shim_dir).unwrap();
    let shim = shim_dir.join("phantom");
    std::fs::write(
        &shim,
        "#!/bin/sh\nif [ \"$1\" = \"list\" ]; then\n  echo GH_TOKEN_ACME\n  echo VERCEL_TOKEN_ACME\n  echo SUPABASE_ACME\nfi\nexit 0\n",
    )
    .unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path_env = format!(
        "{}:{}",
        shim_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let mut client = McpClient::spawn_opts(
        dir.path(),
        Framing::Ndjson,
        Some(dir.path()),
        &[("PATH", path_env.as_str())],
    );
    handshake(&mut client);

    let call = client.request(
        "tools/call",
        json!({ "name": "locus_verify_session", "arguments": {} }),
    );
    let (text, is_err) = McpClient::tool_text(&call);
    assert!(!is_err, "verify_session should not error: {text}");
    let body: Value = serde_json::from_str(&text).expect("session pack json");
    assert_eq!(body["kind"], "session");
    assert_eq!(
        body["session_ok"], true,
        "healthy pinned fixture must verify session_ok=true: {body}"
    );
    assert_eq!(body["doctor"]["ok"], true, "doctor must be SAFE: {body}");
    assert_eq!(body["doctor"]["phantom_on_path"], true, "{body}");
    assert!(
        body["doctor"]["unresolved_phm"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(false),
        "no unresolved phm refs expected: {body}"
    );
    assert_eq!(body["safe_next"]["action"], "ready", "{body}");
    assert_eq!(body["safe_next"]["ready"], true, "{body}");
    assert_eq!(body["whoami"]["binding_alias"], "acme", "{body}");
    // Never secrets or credential refs in the pack.
    for canary in ["GH_TOKEN_ACME", "VERCEL_TOKEN_ACME", "SUPABASE_ACME"] {
        assert!(!text.contains(canary), "pack leaked locator {canary}");
    }
}

/// Same tool without the shim (phantom absent / refs unresolved) must fail
/// closed: session_ok=false but the tool itself still returns a pack.
#[cfg(unix)]
#[test]
fn locus_verify_session_session_ok_false_when_phantom_missing() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    sample_bindings(&store);
    store
        .pin("acme", dir.path(), Some("mcp-verify-miss".into()), false)
        .unwrap();

    // Empty PATH dir only — phantom probe must fail deterministically.
    let empty_bin = dir.path().join("empty-bin");
    std::fs::create_dir_all(&empty_bin).unwrap();
    let path_env = empty_bin.display().to_string();

    let mut client = McpClient::spawn_opts(
        dir.path(),
        Framing::Ndjson,
        Some(dir.path()),
        &[("PATH", path_env.as_str())],
    );
    handshake(&mut client);

    let call = client.request(
        "tools/call",
        json!({ "name": "locus_verify_session", "arguments": {} }),
    );
    let (text, is_err) = McpClient::tool_text(&call);
    assert!(!is_err, "pack returns even when unhealthy: {text}");
    let body: Value = serde_json::from_str(&text).expect("session pack json");
    assert_eq!(body["session_ok"], false, "{body}");
    assert_eq!(body["doctor"]["phantom_on_path"], false, "{body}");
    assert!(
        body["doctor"]["unresolved_phm"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "phm refs must be flagged unresolved: {body}"
    );
}

// ─── MCP session pin-anchoring (pin-swap protection) ────────────────────────

/// Cross-alias re-pin under a live MCP session: provider tools refuse with a
/// structured `pin_changed` (outranking `runtime_unhealthy` from the staled
/// executor grant), control tools keep working and report the mismatch, the
/// catalog collapses to control tools tagged with the ANCHORED alias, and the
/// refusal never mutates or freezes the store session.
#[test]
fn stdio_cross_alias_repin_refuses_with_pin_changed() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    sample_bindings(&store);
    beta_binding(&store);
    store
        .pin("acme", dir.path(), Some("anchor-swap".into()), false)
        .unwrap();

    let mut client = McpClient::spawn(dir.path(), Framing::Ndjson);
    handshake(&mut client);

    // Healthy anchored call under acme.
    let ok = client.request(
        "tools/call",
        json!({ "name": "github.scope", "arguments": {} }),
    );
    let (text, is_err) = McpClient::tool_text(&ok);
    assert!(!is_err, "pre-swap github.scope failed: {text}");
    assert!(text.contains("acme-corp"), "{text}");

    // Human re-pins to a different alias in another terminal.
    store.leave().unwrap();
    store
        .pin("beta", dir.path(), Some("anchor-swap".into()), false)
        .unwrap();

    // Provider call now fails closed with pin_changed — not runtime_unhealthy.
    let refused = client.request(
        "tools/call",
        json!({ "name": "github.scope", "arguments": {} }),
    );
    let (text, is_err) = McpClient::tool_text(&refused);
    assert!(is_err, "expected pin_changed refusal: {text}");
    let body: Value = serde_json::from_str(&text).expect("pin_changed json");
    assert_eq!(body["error"], "pin_changed", "{body}");
    assert_eq!(body["anchored"]["alias"], "acme", "{body}");
    assert_eq!(body["anchored"]["tenant"], "acme-corp", "{body}");
    assert_eq!(body["current"]["alias"], "beta", "{body}");
    assert_eq!(body["current"]["tenant"], "beta-corp", "{body}");
    assert_eq!(body["safe_next"]["action"], "reinitialize_client", "{body}");
    assert_eq!(body["safe_next"]["ready"], false, "{body}");
    assert_eq!(body["safe_next"]["command"], "locus enter acme", "{body}");
    // Authority-plane facts stay visible, not masked.
    assert!(
        body["underlying_issues"].as_array().is_some(),
        "underlying drift issues should ride along: {body}"
    );

    // Repeat call: still refused; the mismatch audit is deduped below.
    let refused2 = client.request(
        "tools/call",
        json!({ "name": "github.scope", "arguments": {} }),
    );
    let (text2, is_err2) = McpClient::tool_text(&refused2);
    assert!(is_err2 && text2.contains("pin_changed"), "{text2}");

    // Control tools keep working and carry the additive mcp_anchor block.
    let whoami = client.request(
        "tools/call",
        json!({ "name": "locus_whoami", "arguments": {} }),
    );
    let (text, _) = McpClient::tool_text(&whoami);
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["mcp_anchor"]["anchored_alias"], "acme", "{body}");
    assert_eq!(body["mcp_anchor"]["match"], false, "{body}");

    // Catalog collapses to control tools tagged with the ANCHORED alias.
    let list = client.request("tools/list", json!({}));
    let tools = list["result"]["tools"].as_array().expect("tools");
    for t in tools {
        let name = t["name"].as_str().unwrap();
        assert!(
            name.starts_with("locus_"),
            "anchored-mismatch catalog must be control-only, got {name}"
        );
        let desc = t["description"].as_str().unwrap_or("");
        assert!(
            desc.starts_with("[locus:acme]"),
            "catalog must be tagged with the anchored alias: {desc}"
        );
    }

    // Session-local refusal: active.json is NOT frozen, no session.freeze audit.
    let active = store.active_session().unwrap().expect("beta pin stays");
    assert_eq!(active.binding_alias, "beta");
    assert!(
        !active.frozen,
        "anchor refusal must not freeze the store pin"
    );
    let events = store.read_audit_events().unwrap();
    assert!(
        !events.iter().any(|e| e.op == "session.freeze"),
        "anchor refusal must not freeze"
    );
    // Anchor lifecycle audits: established once, mismatch deduped to one
    // report per (anchored_session_id, current_session_id) pair.
    assert!(
        events.iter().any(|e| e.op == "mcp.anchor_established"),
        "expected mcp.anchor_established: {:?}",
        events.iter().map(|e| &e.op).collect::<Vec<_>>()
    );
    let mismatches = events
        .iter()
        .filter(|e| e.op == "mcp.anchor_mismatch")
        .count();
    assert_eq!(mismatches, 1, "mcp.anchor_mismatch must be deduped");
    // Values-free audit trail.
    for e in events.iter().filter(|e| e.op.starts_with("mcp.anchor")) {
        let raw = serde_json::to_string(e).unwrap().to_ascii_lowercase();
        for banned in ["phm:", "token", "secret", "credential"] {
            assert!(!raw.contains(banned), "audit leaked `{banned}`: {raw}");
        }
    }
}

/// Same-alias re-pin (TTL refresh) is never refused by the anchor layer. The
/// stale process-bound executor grant may still fail the call — that is the
/// documented authority-plane behavior and must surface as
/// runtime_unhealthy/executor_authority_unavailable, never `pin_changed`.
#[test]
fn stdio_same_alias_repin_not_refused_by_anchor() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    sample_bindings(&store);
    store
        .pin("acme", dir.path(), Some("anchor-same".into()), false)
        .unwrap();

    let mut client = McpClient::spawn(dir.path(), Framing::Ndjson);
    handshake(&mut client);
    let ok = client.request(
        "tools/call",
        json!({ "name": "github.scope", "arguments": {} }),
    );
    let (text, is_err) = McpClient::tool_text(&ok);
    assert!(!is_err, "pre-repin call failed: {text}");

    // TTL-refresh flow: same alias, new sealed session.
    store.leave().unwrap();
    store
        .pin("acme", dir.path(), Some("anchor-same".into()), false)
        .unwrap();

    let call = client.request(
        "tools/call",
        json!({ "name": "github.scope", "arguments": {} }),
    );
    let (text, _) = McpClient::tool_text(&call);
    assert!(
        !text.contains("pin_changed"),
        "same-alias re-pin must not trip the anchor: {text}"
    );

    // Anchor still reports acme as the anchored identity.
    let whoami = client.request(
        "tools/call",
        json!({ "name": "locus_whoami", "arguments": {} }),
    );
    let (text, _) = McpClient::tool_text(&whoami);
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["mcp_anchor"]["anchored_alias"], "acme", "{body}");
}

/// A pin created after spawn is never anchored (the observation is unhealthy —
/// no executor grant for the new session), and the refusal is the plain
/// authority-plane error, not pin_changed. whoami carries no mcp_anchor block
/// when nothing was ever anchored.
#[test]
fn stdio_post_spawn_pin_never_anchors() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    sample_bindings(&store);

    let mut client = McpClient::spawn(dir.path(), Framing::Ndjson);
    handshake(&mut client);

    // Unpinned refusal without anchor context.
    let call = client.request(
        "tools/call",
        json!({ "name": "github.scope", "arguments": {} }),
    );
    let (text, is_err) = McpClient::tool_text(&call);
    assert!(is_err && text.contains("not_pinned"), "{text}");
    let body: Value = serde_json::from_str(&text).unwrap();
    assert!(
        body.get("anchor").is_none(),
        "never-anchored session must not report anchor context: {body}"
    );

    // Pin after spawn: process has no executor grant for this session.
    store
        .pin("acme", dir.path(), Some("post-spawn".into()), false)
        .unwrap();
    let call = client.request(
        "tools/call",
        json!({ "name": "github.scope", "arguments": {} }),
    );
    let (text, is_err) = McpClient::tool_text(&call);
    assert!(is_err, "{text}");
    assert!(
        !text.contains("pin_changed"),
        "no anchor exists — must not report pin_changed: {text}"
    );

    let whoami = client.request(
        "tools/call",
        json!({ "name": "locus_whoami", "arguments": {} }),
    );
    let (text, _) = McpClient::tool_text(&whoami);
    let body: Value = serde_json::from_str(&text).unwrap();
    assert!(
        body.get("mcp_anchor").is_none(),
        "mcp_anchor must be omitted when no anchor exists: {body}"
    );
}

/// The anchor survives `locus leave`: the not_pinned refusal gains anchor
/// context (distinguishing "pin vanished" from "never pinned"), and a later
/// re-pin to a different alias still refuses with pin_changed.
#[test]
fn stdio_anchor_survives_leave_and_refuses_new_alias() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    sample_bindings(&store);
    beta_binding(&store);
    store
        .pin("acme", dir.path(), Some("anchor-leave".into()), false)
        .unwrap();

    let mut client = McpClient::spawn(dir.path(), Framing::Ndjson);
    handshake(&mut client);
    let ok = client.request(
        "tools/call",
        json!({ "name": "github.scope", "arguments": {} }),
    );
    let (text, is_err) = McpClient::tool_text(&ok);
    assert!(!is_err, "pre-leave call failed: {text}");

    store.leave().unwrap();

    // Unpinned gap: not_pinned WITH anchor context (anchor is not cleared).
    let call = client.request(
        "tools/call",
        json!({ "name": "github.scope", "arguments": {} }),
    );
    let (text, is_err) = McpClient::tool_text(&call);
    assert!(is_err, "{text}");
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["error"], "not_pinned", "{body}");
    assert_eq!(body["anchor"]["anchored_alias"], "acme", "{body}");
    assert!(
        body["hint"].as_str().unwrap_or("").contains("vanished"),
        "hint should say the previous pin vanished: {body}"
    );

    // Re-pin to a different alias: the surviving anchor still refuses.
    store
        .pin("beta", dir.path(), Some("anchor-leave".into()), false)
        .unwrap();
    let call = client.request(
        "tools/call",
        json!({ "name": "github.scope", "arguments": {} }),
    );
    let (text, is_err) = McpClient::tool_text(&call);
    assert!(is_err, "{text}");
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["error"], "pin_changed", "{body}");
    assert_eq!(body["anchored"]["alias"], "acme", "{body}");
    assert_eq!(body["current"]["alias"], "beta", "{body}");
}

/// An explicit second initialize adopts the current global pin: healthy →
/// re-anchor (audited mcp.anchor_reset); unhealthy (stale executor grant after
/// the swap) → the anchor clears and errors fall back to the plain
/// authority-plane refusal.
#[test]
fn stdio_second_initialize_adopts_current_pin() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    sample_bindings(&store);
    beta_binding(&store);
    store
        .pin("acme", dir.path(), Some("anchor-reinit".into()), false)
        .unwrap();

    let mut client = McpClient::spawn(dir.path(), Framing::Ndjson);
    handshake(&mut client);
    let ok = client.request(
        "tools/call",
        json!({ "name": "github.scope", "arguments": {} }),
    );
    let (text, is_err) = McpClient::tool_text(&ok);
    assert!(!is_err, "pre-swap call failed: {text}");

    store.leave().unwrap();
    store
        .pin("beta", dir.path(), Some("anchor-reinit".into()), false)
        .unwrap();

    // Anchored refusal first.
    let refused = client.request(
        "tools/call",
        json!({ "name": "github.scope", "arguments": {} }),
    );
    let (text, _) = McpClient::tool_text(&refused);
    assert!(text.contains("pin_changed"), "{text}");

    // Explicit re-initialize: this stdio process cannot become healthy for the
    // new session (executor grant is process-bound), so the anchor clears and
    // subsequent errors are the plain authority-plane refusal.
    let reinit = client.request(
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "locus-test", "version": "0.0.1" }
        }),
    );
    assert!(reinit.get("result").is_some(), "{reinit}");

    let call = client.request(
        "tools/call",
        json!({ "name": "github.scope", "arguments": {} }),
    );
    let (text, is_err) = McpClient::tool_text(&call);
    assert!(is_err, "{text}");
    assert!(
        !text.contains("pin_changed"),
        "re-initialize must adopt/clear the anchor: {text}"
    );

    let events = store.read_audit_events().unwrap();
    assert!(
        events.iter().any(|e| e.op == "mcp.anchor_reset"),
        "expected mcp.anchor_reset audit: {:?}",
        events.iter().map(|e| &e.op).collect::<Vec<_>>()
    );
}

/// `LOCUS_SESSION_ID` overlay sessions (ci mint / withLocusSession) never read
/// active.json, so global re-pins cannot drift them: the anchor is a permanent
/// Match and provider calls keep operating under the sealed ci identity.
#[test]
fn stdio_env_session_immune_to_active_json_rewrites() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    sample_bindings(&store);
    beta_binding(&store);

    let (ci_session, _ci_path) = store
        .create_ci_session("acme", dir.path(), false, None)
        .unwrap();
    let capability = store.grant_executor_capability(&ci_session).unwrap();

    let mut client = McpClient::spawn_opts(
        dir.path(),
        Framing::Ndjson,
        None,
        &[
            ("LOCUS_SESSION_ID", ci_session.session_id.as_str()),
            (locus_core::EXECUTOR_CAPABILITY_ENV, capability.as_str()),
        ],
    );
    handshake(&mut client);

    let ok = client.request(
        "tools/call",
        json!({ "name": "github.scope", "arguments": {} }),
    );
    let (text, is_err) = McpClient::tool_text(&ok);
    assert!(!is_err, "ci-session github.scope failed: {text}");
    assert!(text.contains("acme-corp"), "{text}");

    // Rewrite active.json to a different alias — the env session is exclusive
    // and must not drift or trip its anchor.
    store
        .pin("beta", dir.path(), Some("ci-immune".into()), false)
        .unwrap();

    let ok = client.request(
        "tools/call",
        json!({ "name": "github.scope", "arguments": {} }),
    );
    let (text, is_err) = McpClient::tool_text(&ok);
    assert!(
        !is_err,
        "env-session call must survive active.json rewrite: {text}"
    );
    assert!(text.contains("acme-corp"), "{text}");
    assert!(!text.contains("pin_changed"), "{text}");

    let whoami = client.request(
        "tools/call",
        json!({ "name": "locus_whoami", "arguments": {} }),
    );
    let (text, is_err) = McpClient::tool_text(&whoami);
    assert!(!is_err, "{text}");
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["binding_alias"], "acme", "{body}");
    assert_eq!(body["mcp_anchor"]["match"], true, "{body}");
    // Observability: the anchor records its non-active backing.
    assert_eq!(body["mcp_anchor"]["anchored_alias"], "acme", "{body}");
}

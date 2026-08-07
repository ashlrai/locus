//! Integration: spawn locus-mcp, exercise MCP handshake + tools over stdio.
//!
//! Covers both Content-Length framing (MCP standard) and NDJSON.

use locus_core::{Binding, BindingBody, Policy, ProviderBinding, Scope, Store};
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
        let mut child = Command::new(mcp_bin())
            .env("LOCUS_HOME", locus_home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
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
            },
        ],
    });
    store.save_binding(&acme).unwrap();
}

fn handshake(client: &mut McpClient) {
    let init = client.request(
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "locus-test", "version": "0.0.1" }
        }),
    );
    assert!(init.get("result").is_some(), "initialize failed: {init}");
    let result = init.get("result").unwrap();
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
}

#[test]
fn unpinned_tools_list_only_control_tools_ndjson() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    sample_bindings(&store);
    // no pin

    let mut client = McpClient::spawn(dir.path(), Framing::Ndjson);
    handshake(&mut client);

    let list = client.request("tools/list", json!({}));
    let tools = list["result"]["tools"].as_array().expect("tools array");
    assert!(!tools.is_empty());
    for t in tools {
        let name = t["name"].as_str().unwrap();
        assert!(
            name.starts_with("locus_"),
            "unpinned tools/list must be control-only, got {name}"
        );
    }
    assert!(tools.iter().any(|t| t["name"] == "locus_whoami"));
    assert!(tools.iter().any(|t| t["name"] == "locus_request_pin"));
    // provider tools must be absent
    assert!(!tools.iter().any(|t| t["name"] == "github.scope"));
    assert!(!tools.iter().any(|t| t["name"] == "vercel.scope"));
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
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"locus_whoami"));
    assert!(names.contains(&"github.scope"));
    assert!(names.contains(&"vercel.scope"));
    assert!(names.contains(&"supabase.scope"));

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
    assert_eq!(body["credential_ref"], "phm:GH_TOKEN_ACME");
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
    assert_eq!(body["credential_ref"], "phm:VERCEL_TOKEN_ACME");

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

    let pending = store.pending_approvals().unwrap();
    assert!(
        !pending.is_empty(),
        "expected pending approval record under approvals/"
    );
    assert!(pending.iter().any(|r| {
        r.tool == "supabase.table.delete" && r.binding == "acme" && r.id.starts_with("appr_")
    }));

    // Grant then re-call with same args succeeds
    let id = pending
        .iter()
        .find(|r| r.tool == "supabase.table.delete")
        .unwrap()
        .id
        .clone();
    store.grant_approval(&id, None).unwrap();
    let del2 = client.request(
        "tools/call",
        json!({
            "name": "supabase.table.delete",
            "arguments": { "table": "users" }
        }),
    );
    let (text2, is_err2) = McpClient::tool_text(&del2);
    assert!(!is_err2, "expected allow after grant: {text2}");
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

//! Integration: MCP lifecycle conformance over stdio (version negotiation,
//! non-object messages) and honest messages when the plain client entry
//! (`locus-mcp` with no executor capability) cannot observe a live pin.

use locus_core::{Binding, BindingBody, Policy, ProviderBinding, Scope, Store};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
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

/// NDJSON client. The executor capability is always cleared, as for the
/// plain entry `locus setup` writes into client configs.
struct Client {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Client {
    fn spawn(home: &Path) -> Self {
        let mut child = Command::new(mcp_bin())
            .env("LOCUS_HOME", home)
            .env("LOCUS_MCP_AUTO_PIN", "0")
            .env(locus_core::EXECUTOR_CAPABILITY_ENV, "")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn locus-mcp");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            stdout,
        }
    }

    fn send_raw(&mut self, line: &str) {
        self.stdin.write_all(line.as_bytes()).unwrap();
        self.stdin.write_all(b"\n").unwrap();
        self.stdin.flush().unwrap();
    }

    fn read(&mut self) -> Value {
        let mut line = String::new();
        let n = self.stdout.read_line(&mut line).expect("read line");
        assert!(n > 0, "EOF before response");
        serde_json::from_str(line.trim()).expect("stdout carries only JSON-RPC")
    }

    fn request(&mut self, id: i64, method: &str, params: Value) -> Value {
        self.send_raw(
            &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string(),
        );
        let resp = self.read();
        assert_eq!(resp["id"], json!(id), "id mismatch: {resp}");
        resp
    }

    fn initialize(&mut self, protocol_version: Option<&str>) -> Value {
        let mut params = json!({
            "capabilities": {},
            "clientInfo": {"name": "lifecycle-test", "version": "0"}
        });
        if let Some(v) = protocol_version {
            params["protocolVersion"] = json!(v);
        }
        let resp = self.request(1, "initialize", params);
        self.send_raw(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        resp["result"].clone()
    }

    fn tool(&mut self, id: i64, name: &str) -> (Value, bool) {
        let resp = self.request(id, "tools/call", json!({"name": name, "arguments": {}}));
        let result = &resp["result"];
        let text = result["content"][0]["text"].as_str().unwrap_or("{}");
        (
            serde_json::from_str(text).expect("tool text is JSON"),
            result["isError"].as_bool().unwrap_or(false),
        )
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn pinned_home() -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store
        .save_binding(&Binding::from_body(BindingBody {
            id: "bnd_acme".into(),
            alias: "acme".into(),
            tenant: "acme-corp".into(),
            principal: None,
            description: None,
            policy: Policy::default(),
            providers: vec![ProviderBinding {
                provider: "github".into(),
                account: "acme-gh".into(),
                credential_ref: "phm:GH_TOKEN_ACME".into(),
                scope: Scope {
                    orgs: vec!["acme-corp".into()],
                    ..Scope::default()
                },
                upstream: None,
            }],
        }))
        .unwrap();
    store
        .pin("acme", dir.path(), Some("cli".into()), false)
        .unwrap();
    dir
}

#[test]
fn initialize_negotiates_protocol_version() {
    let dir = tempdir().unwrap();
    for (requested, expected) in [
        (Some("2024-11-05"), "2024-11-05"),
        (Some("2025-03-26"), "2025-03-26"),
        (Some("2025-06-18"), "2025-06-18"),
        // Newer or unknown revision: answer our latest; the client decides.
        (Some("2099-01-01"), "2025-06-18"),
        (None, "2025-06-18"),
    ] {
        let mut c = Client::spawn(dir.path());
        let result = c.initialize(requested);
        assert_eq!(
            result["protocolVersion"], expected,
            "requested {requested:?}: {result}"
        );
        // The session keeps working after negotiation.
        let pong = c.request(2, "ping", json!({}));
        assert_eq!(pong["result"], json!({}));
    }
}

#[test]
fn batch_and_non_object_messages_get_an_error_instead_of_silence() {
    let dir = tempdir().unwrap();
    let mut c = Client::spawn(dir.path());
    c.initialize(Some("2025-06-18"));

    c.send_raw(r#"[{"jsonrpc":"2.0","id":5,"method":"ping"}]"#);
    let resp = c.read();
    assert_eq!(resp["id"], Value::Null, "{resp}");
    assert_eq!(resp["error"]["code"], -32600, "{resp}");
    assert!(
        resp["error"]["message"].as_str().unwrap().contains("batch"),
        "{resp}"
    );

    c.send_raw("42");
    let resp = c.read();
    assert_eq!(resp["error"]["code"], -32600, "{resp}");

    // A client response (no method) is not answered; the next request is.
    c.send_raw(r#"{"jsonrpc":"2.0","id":99,"result":{}}"#);
    let pong = c.request(6, "ping", json!({}));
    assert_eq!(pong["result"], json!({}));
}

#[test]
fn plain_entry_with_live_pin_says_pin_is_unobservable_not_unpinned() {
    let dir = pinned_home();
    let mut c = Client::spawn(dir.path());
    let init = c.initialize(Some("2025-06-18"));
    let instructions = init["instructions"].as_str().unwrap();
    assert!(
        instructions.contains("Pin not observable") && instructions.contains("--supervised"),
        "{instructions}"
    );
    assert!(
        !instructions.contains("Currently unpinned"),
        "a live pin must not be described as unpinned: {instructions}"
    );

    // Provider call: actionable hint, never a nonexistent `locus heartbeat` CLI.
    let (body, is_err) = c.tool(2, "github.scope");
    assert!(is_err);
    assert_eq!(body["error"], "runtime_unhealthy", "{body}");
    let hint = body["hint"].as_str().unwrap();
    assert!(hint.contains("locus mcp --supervised"), "{hint}");
    assert!(!hint.contains("locus heartbeat"), "{hint}");

    // safe_next names the real fix instead of looping on `locus doctor`.
    let (body, is_err) = c.tool(3, "locus_safe_next");
    assert!(is_err);
    assert_eq!(body["action"], "launch_supervised", "{body}");
    assert_eq!(body["command"], "locus mcp --supervised", "{body}");
    assert!(body["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i == "executor_authority_unavailable"));

    // locus_status no longer reads healthy when no provider call can succeed.
    let (body, is_err) = c.tool(4, "locus_status");
    assert!(is_err, "unusable pin must be flagged: {body}");
    assert_eq!(body["pinned"], true);
    assert_eq!(body["ok"], false, "{body}");
    assert!(body["hint"].as_str().unwrap().contains("--supervised"));

    let (body, _) = c.tool(5, "locus_heartbeat");
    assert!(body["hint"].as_str().unwrap().contains("--supervised"));

    // Resource + prompt agree.
    let res = c.request(6, "resources/read", json!({"uri": "locus://session"}));
    let text = res["result"]["contents"][0]["text"].as_str().unwrap();
    let session: Value = serde_json::from_str(text).unwrap();
    assert_eq!(
        session["status"], "executor_authority_unavailable",
        "{session}"
    );
    assert_eq!(session["pinned"], Value::Null);

    let prompt = c.request(7, "prompts/get", json!({"name": "locus_context"}));
    let text = prompt["result"]["messages"][0]["content"]["text"]
        .as_str()
        .unwrap();
    assert!(text.contains("not observable"), "{text}");
    assert!(!text.contains("**pinned**: false"), "{text}");
}

#[test]
fn unpinned_plain_entry_still_says_unpinned() {
    let dir = tempdir().unwrap();
    let mut c = Client::spawn(dir.path());
    let init = c.initialize(Some("2025-06-18"));
    let instructions = init["instructions"].as_str().unwrap();
    assert!(
        instructions.contains("Currently unpinned"),
        "{instructions}"
    );
    let (body, _) = c.tool(2, "locus_status");
    assert_eq!(body["pinned"], false);
}

/// `locus-mcp --version` / `-V` print the version on stdout and exit 0
/// (previously "unknown argument: --version", exit 1).
#[test]
fn version_flag_prints_version_and_exits_zero() {
    for flag in ["--version", "-V"] {
        let out = Command::new(mcp_bin())
            .arg(flag)
            .stdin(Stdio::null())
            .output()
            .expect("run locus-mcp");
        assert!(out.status.success(), "{flag}: {out:?}");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            format!("locus-mcp {}", env!("CARGO_PKG_VERSION")),
            "{flag}"
        );
    }
}

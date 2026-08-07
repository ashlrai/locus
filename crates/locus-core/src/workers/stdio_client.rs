//! Minimal MCP client over a child process's stdio.
//!
//! Supports NDJSON (one JSON object per line) which most local MCP servers
//! accept; also tries Content-Length when reading if the first response is
//! framed that way.

use crate::error::{LocusError, Result};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// Live connection to one upstream MCP server process.
pub struct McpStdioClient {
    stdin: Mutex<ChildStdin>,
    stdout: Mutex<BufReader<ChildStdout>>,
    next_id: AtomicU64,
    /// Cached tools from last tools/list: name → schema meta (optional).
    tools: Mutex<Vec<UpstreamTool>>,
    initialized: Mutex<bool>,
}

#[derive(Debug, Clone)]
pub struct UpstreamTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

impl McpStdioClient {
    /// Take ownership of child's stdin/stdout after spawn.
    pub fn from_child(child: &mut Child) -> Result<Self> {
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| LocusError::msg("child missing stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| LocusError::msg("child missing stdout"))?;
        Ok(Self {
            stdin: Mutex::new(stdin),
            stdout: Mutex::new(BufReader::new(stdout)),
            next_id: AtomicU64::new(1),
            tools: Mutex::new(Vec::new()),
            initialized: Mutex::new(false),
        })
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    /// Write NDJSON request.
    fn write_request(&self, msg: &Value) -> Result<()> {
        let mut stdin = self
            .stdin
            .lock()
            .map_err(|_| LocusError::msg("stdin lock poisoned"))?;
        let line = serde_json::to_string(msg)?;
        writeln!(stdin, "{line}").map_err(|e| LocusError::msg(format!("write mcp: {e}")))?;
        stdin
            .flush()
            .map_err(|e| LocusError::msg(format!("flush mcp: {e}")))?;
        Ok(())
    }

    /// Read one JSON-RPC response (skip notifications without id).
    fn read_response(&self, expect_id: u64) -> Result<Value> {
        let mut stdout = self
            .stdout
            .lock()
            .map_err(|_| LocusError::msg("stdout lock poisoned"))?;

        // Bound reads so a hung child can't block forever in tests
        // (platform-dependent; we rely on short mock servers).
        let deadline = std::time::Instant::now() + Duration::from_secs(10);

        loop {
            if std::time::Instant::now() > deadline {
                return Err(LocusError::msg("mcp client read timeout"));
            }

            let mut first = String::new();
            let n = stdout
                .read_line(&mut first)
                .map_err(|e| LocusError::msg(format!("read mcp: {e}")))?;
            if n == 0 {
                return Err(LocusError::msg("mcp child EOF"));
            }
            let trimmed = first.trim();
            if trimmed.is_empty() {
                continue;
            }

            let msg: Value = if trimmed.to_ascii_lowercase().starts_with("content-length:") {
                // Content-Length framing
                let mut content_length = None;
                let mut line = first;
                loop {
                    let lower = line.trim().to_ascii_lowercase();
                    if lower.starts_with("content-length:") {
                        content_length = Some(
                            lower
                                .trim_start_matches("content-length:")
                                .trim()
                                .parse::<usize>()
                                .map_err(|e| LocusError::msg(e.to_string()))?,
                        );
                    }
                    if line.trim().is_empty() {
                        break;
                    }
                    line.clear();
                    let n = stdout
                        .read_line(&mut line)
                        .map_err(|e| LocusError::msg(format!("read header: {e}")))?;
                    if n == 0 {
                        return Err(LocusError::msg("mcp EOF in headers"));
                    }
                }
                let len = content_length
                    .ok_or_else(|| LocusError::msg("Content-Length missing from child"))?;
                let mut buf = vec![0u8; len];
                stdout
                    .read_exact(&mut buf)
                    .map_err(|e| LocusError::msg(format!("read body: {e}")))?;
                serde_json::from_slice(&buf)?
            } else {
                serde_json::from_str(trimmed)?
            };

            // Skip notifications (no id)
            if msg.get("id").is_none() {
                continue;
            }
            let id = msg
                .get("id")
                .and_then(|v| v.as_u64().or_else(|| v.as_i64().map(|i| i as u64)))
                .unwrap_or(0);
            if id != expect_id {
                // Unexpected id — keep reading (or error if strict)
                continue;
            }
            if let Some(err) = msg.get("error") {
                return Err(LocusError::msg(format!("upstream mcp error: {err}")));
            }
            return Ok(msg
                .get("result")
                .cloned()
                .unwrap_or(Value::Null));
        }
    }

    fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id();
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.write_request(&msg)?;
        self.read_response(id)
    }

    /// Initialize handshake + tools/list cache.
    pub fn handshake(&self) -> Result<Vec<UpstreamTool>> {
        let mut initialized = self
            .initialized
            .lock()
            .map_err(|_| LocusError::msg("init lock poisoned"))?;
        if !*initialized {
            let _ = self.request(
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "locus-worker", "version": env!("CARGO_PKG_VERSION") }
                }),
            )?;
            // notifications/initialized (no response expected)
            let _ = self.write_request(&json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized",
                "params": {}
            }));
            *initialized = true;
        }
        drop(initialized);

        let result = self.request("tools/list", json!({}))?;
        let tools_arr = result
            .get("tools")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default();
        let mut tools = Vec::new();
        for t in tools_arr {
            let name = t
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            if name.is_empty() {
                continue;
            }
            tools.push(UpstreamTool {
                name,
                description: t
                    .get("description")
                    .and_then(|d| d.as_str())
                    .unwrap_or("")
                    .to_string(),
                input_schema: t
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or(json!({"type": "object"})),
            });
        }
        *self
            .tools
            .lock()
            .map_err(|_| LocusError::msg("tools lock poisoned"))? = tools.clone();
        Ok(tools)
    }

    pub fn list_tools_cached(&self) -> Result<Vec<UpstreamTool>> {
        let guard = self
            .tools
            .lock()
            .map_err(|_| LocusError::msg("tools lock poisoned"))?;
        if guard.is_empty() {
            drop(guard);
            return self.handshake();
        }
        Ok(guard.clone())
    }

    pub fn call_tool(&self, name: &str, arguments: &Value) -> Result<Value> {
        // Ensure handshake once
        {
            let init = self
                .initialized
                .lock()
                .map_err(|_| LocusError::msg("init lock poisoned"))?;
            if !*init {
                drop(init);
                let _ = self.handshake()?;
            }
        }
        self.request(
            "tools/call",
            json!({
                "name": name,
                "arguments": arguments,
            }),
        )
    }
}

pub fn client_key(session_id: &str, provider: &str) -> String {
    format!("{session_id}:{provider}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    /// Tiny NDJSON MCP mock via python (or skip if no python).
    fn mock_server_cmd() -> Option<Command> {
        let script = r#"
import sys, json
def send(o):
    sys.stdout.write(json.dumps(o) + "\n")
    sys.stdout.flush()
for line in sys.stdin:
    line=line.strip()
    if not line: continue
    msg=json.loads(line)
    mid=msg.get("id")
    method=msg.get("method","")
    if mid is None:
        continue
    if method=="initialize":
        send({"jsonrpc":"2.0","id":mid,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"mock","version":"0"}}})
    elif method=="tools/list":
        send({"jsonrpc":"2.0","id":mid,"result":{"tools":[{"name":"echo","description":"echo","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}}}]}})
    elif method=="tools/call":
        args=msg.get("params",{}).get("arguments",{})
        send({"jsonrpc":"2.0","id":mid,"result":{"content":[{"type":"text","text":args.get("text","")}],"isError":False}})
    else:
        send({"jsonrpc":"2.0","id":mid,"error":{"code":-32601,"message":method}})
"#;
        let mut cmd = Command::new("python3");
        cmd.arg("-u")
            .arg("-c")
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        Some(cmd)
    }

    #[test]
    fn handshake_list_and_call() {
        let Some(mut cmd) = mock_server_cmd() else {
            return;
        };
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(_) => return, // no python3
        };
        let client = McpStdioClient::from_child(&mut child).unwrap();
        let tools = client.handshake().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");
        let result = client
            .call_tool("echo", &json!({"text": "hello-locus"}))
            .unwrap();
        let text = result
            .pointer("/content/0/text")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(text, "hello-locus");
        let _ = child.kill();
    }
}

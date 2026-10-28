//! Minimal MCP client over a child process's stdio.
//!
//! Supports NDJSON (one JSON object per line) which most local MCP servers
//! accept; also tries Content-Length when reading if the first response is
//! framed that way.

use crate::error::{LocusError, Result};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

// Bound both framing and blocking I/O; checking time before read_line alone
// cannot interrupt a child that never emits a newline (or stops reading stdin).
const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
const MAX_HEADER_BYTES: usize = 16 * 1024;

struct DeadlinePipe<T> {
    pipe: T,
    deadline: Instant,
}

#[cfg(unix)]
impl<T: std::os::fd::AsRawFd> DeadlinePipe<T> {
    fn new(pipe: T) -> std::io::Result<Self> {
        let fd = pipe.as_raw_fd();
        // SAFETY: fd is borrowed from a live owned pipe. Preserve existing flags.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags == -1 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) == -1 {
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(Self {
            pipe,
            deadline: Instant::now(),
        })
    }

    fn wait(&self, events: libc::c_short) -> std::io::Result<()> {
        loop {
            let remaining = self
                .deadline
                .checked_duration_since(Instant::now())
                .filter(|duration| !duration.is_zero())
                .ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::TimedOut, "mcp I/O timeout")
                })?;
            let mut pollfd = libc::pollfd {
                fd: self.pipe.as_raw_fd(),
                events,
                revents: 0,
            };
            let millis = remaining
                .as_millis()
                .saturating_add(1)
                .min(i32::MAX as u128) as i32;
            // SAFETY: pollfd points to one initialized descriptor for this call.
            let ready = unsafe { libc::poll(&mut pollfd, 1, millis) };
            if ready > 0 {
                return Ok(());
            }
            if ready < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        }
    }
}

#[cfg(unix)]
impl<T: Read + std::os::fd::AsRawFd> Read for DeadlinePipe<T> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            self.wait(libc::POLLIN)?;
            match self.pipe.read(buf) {
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    continue
                }
                result => return result,
            }
        }
    }
}

#[cfg(unix)]
impl<T: Write + std::os::fd::AsRawFd> Write for DeadlinePipe<T> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        loop {
            self.wait(libc::POLLOUT)?;
            match self.pipe.write(buf) {
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    continue
                }
                result => return result,
            }
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.pipe.flush()
    }
}

// Unsupported platforms refuse connection rather than pretend blocking reads
// have a timeout. Keep the generic I/O shape available for cross-compilation.
#[cfg(not(unix))]
impl<T> DeadlinePipe<T> {
    fn new(_pipe: T) -> std::io::Result<Self> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "bounded MCP stdio requires Unix",
        ))
    }
}
#[cfg(not(unix))]
impl<T: Read> Read for DeadlinePipe<T> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.pipe.read(buf)
    }
}
#[cfg(not(unix))]
impl<T: Write> Write for DeadlinePipe<T> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.pipe.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.pipe.flush()
    }
}

fn bounded_line(reader: &mut impl BufRead, limit: usize) -> Result<String> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if line.is_empty() {
                return Err(LocusError::msg("mcp child EOF"));
            }
            break;
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let count = newline.map_or(available.len(), |index| index + 1);
        if count > limit.saturating_sub(line.len()) {
            return Err(LocusError::msg("mcp frame exceeds size limit"));
        }
        line.extend_from_slice(&available[..count]);
        reader.consume(count);
        if newline.is_some() {
            break;
        }
    }
    String::from_utf8(line).map_err(|_| LocusError::msg("mcp frame is not UTF-8"))
}

/// Live connection to one upstream MCP server process.
pub struct McpStdioClient {
    stdin: Mutex<Option<DeadlinePipe<ChildStdin>>>,
    stdout: Mutex<BufReader<DeadlinePipe<ChildStdout>>>,
    exchange: Mutex<()>,
    failed: AtomicBool,
    timeout: Duration,
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
            stdin: Mutex::new(Some(DeadlinePipe::new(stdin)?)),
            stdout: Mutex::new(BufReader::new(DeadlinePipe::new(stdout)?)),
            exchange: Mutex::new(()),
            failed: AtomicBool::new(false),
            timeout: Duration::from_secs(10),
            next_id: AtomicU64::new(1),
            tools: Mutex::new(Vec::new()),
            initialized: Mutex::new(false),
        })
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    /// Write NDJSON request.
    fn write_request(&self, msg: &Value, deadline: Instant) -> Result<()> {
        let mut stdin = self
            .stdin
            .lock()
            .map_err(|_| LocusError::msg("stdin lock poisoned"))?;
        let stdin = stdin
            .as_mut()
            .ok_or_else(|| LocusError::msg("mcp stdin closed"))?;
        stdin.deadline = deadline;
        let line = serde_json::to_string(msg)?;
        if line.len() >= MAX_FRAME_BYTES {
            return Err(LocusError::msg("mcp request exceeds size limit"));
        }
        writeln!(stdin, "{line}").map_err(|e| LocusError::msg(format!("write mcp: {e}")))?;
        stdin
            .flush()
            .map_err(|e| LocusError::msg(format!("flush mcp: {e}")))?;
        Ok(())
    }

    /// Read one JSON-RPC response (skip notifications without id).
    fn read_response(&self, expect_id: u64, deadline: Instant) -> Result<Value> {
        let mut stdout = self
            .stdout
            .lock()
            .map_err(|_| LocusError::msg("stdout lock poisoned"))?;

        stdout.get_mut().deadline = deadline;

        loop {
            if std::time::Instant::now() > deadline {
                return Err(LocusError::msg("mcp client read timeout"));
            }

            let first = bounded_line(&mut *stdout, MAX_FRAME_BYTES)?;
            let trimmed = first.trim();
            if trimmed.is_empty() {
                continue;
            }

            let msg: Value = if trimmed.to_ascii_lowercase().starts_with("content-length:") {
                // Content-Length framing
                let mut content_length = None;
                let mut header_bytes = first.len();
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
                    line =
                        bounded_line(&mut *stdout, MAX_HEADER_BYTES.saturating_sub(header_bytes))?;
                    header_bytes += line.len();
                }
                let len = content_length
                    .ok_or_else(|| LocusError::msg("Content-Length missing from child"))?;
                if len > MAX_FRAME_BYTES {
                    return Err(LocusError::msg("mcp frame exceeds size limit"));
                }
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
            return Ok(msg);
        }
    }

    fn request(&self, method: &str, params: Value) -> Result<Value> {
        // Serialize the complete exchange: readers must never consume another
        // request's response and discard it as an unexpected ID.
        let _exchange = self
            .exchange
            .lock()
            .map_err(|_| LocusError::msg("mcp exchange lock poisoned"))?;
        if self.failed.load(Ordering::SeqCst) {
            return Err(LocusError::msg("mcp connection failed; restart required"));
        }
        let deadline = Instant::now() + self.timeout;
        let id = self.next_id();
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let result = self
            .write_request(&msg, deadline)
            .and_then(|()| self.read_response(id, deadline));
        if result.is_err() {
            // A partial frame cannot safely be reused by a later call.
            self.close_stdin();
        }
        let response = result?;
        if let Some(error) = response.get("error") {
            return Err(LocusError::msg(format!("upstream mcp error: {error}")));
        }
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }

    /// A transport failure requires a fresh connection; calls are never replayed.
    pub fn is_healthy(&self) -> bool {
        !self.failed.load(Ordering::SeqCst)
    }

    /// Signal EOF before process termination, including after a transport error.
    pub fn close_stdin(&self) {
        self.failed.store(true, Ordering::SeqCst);
        self.stdin
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
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
            self.write_request(
                &json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/initialized",
                    "params": {}
                }),
                Instant::now() + self.timeout,
            )?;
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
    fn mock_server_cmd() -> Command {
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
        cmd
    }

    #[test]
    fn handshake_list_and_call() {
        let mut cmd = mock_server_cmd();
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
        std::thread::scope(|scope| {
            for i in 0..8 {
                let client = &client;
                scope.spawn(move || {
                    let response = client
                        .call_tool("echo", &json!({"text": i.to_string()}))
                        .unwrap();
                    assert_eq!(response["content"][0]["text"], i.to_string());
                });
            }
        });
        let _ = child.kill();
        let _ = child.wait();
    }

    #[cfg(unix)]
    struct ReapChild(Child);

    #[cfg(unix)]
    impl Drop for ReapChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[cfg(unix)]
    fn inert_server(script: &str) -> (ReapChild, McpStdioClient) {
        let mut child = ReapChild(
            Command::new("/bin/sh")
                .args(["-c", script])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let mut client = McpStdioClient::from_child(&mut child.0).unwrap();
        client.timeout = Duration::from_secs(2);
        (child, client)
    }

    #[test]
    #[cfg(unix)]
    fn silent_partial_line_and_partial_body_are_bounded() {
        for script in [
            "exec sleep 5",
            "printf '{'; exec sleep 5",
            "printf 'Content-Length: 99\\r\\n\\r\\n{'; exec sleep 5",
        ] {
            let (_child, mut client) = inert_server(script);
            client.timeout = Duration::from_millis(150);
            let started = Instant::now();
            let error = client.request("ping", json!({})).unwrap_err();
            assert!(error.to_string().contains("timeout"), "{error}");
            assert!(started.elapsed() < Duration::from_secs(2));
            assert!(
                client.stdin.lock().unwrap().is_none(),
                "failure must deliver EOF"
            );
            assert!(client
                .request("ping", json!({}))
                .unwrap_err()
                .to_string()
                .contains("restart required"));
        }
    }

    #[test]
    #[cfg(unix)]
    fn child_that_never_reads_cannot_block_request_write() {
        let (_child, mut client) = inert_server("exec sleep 5");
        client.timeout = Duration::from_millis(150);
        let started = Instant::now();
        let error = client
            .request("ping", json!({"payload": "x".repeat(1024 * 1024)}))
            .unwrap_err();
        assert!(error.to_string().contains("timeout"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    #[cfg(unix)]
    fn oversized_content_length_is_rejected_before_allocation() {
        let (_child, client) =
            inert_server("printf 'Content-Length: 99999999\\r\\n\\r\\n'; exec sleep 5");
        let error = client.request("ping", json!({})).unwrap_err();
        assert!(error.to_string().contains("size limit"), "{error}");
    }

    #[test]
    fn newline_free_frame_has_a_size_bound() {
        let mut reader = std::io::Cursor::new(vec![b'x'; 17]);
        let error = bounded_line(&mut reader, 16).unwrap_err();
        assert!(error.to_string().contains("size limit"), "{error}");
    }

    #[test]
    #[cfg(unix)]
    fn framed_response_and_rpc_error_preserve_connection() {
        let error =
            json!({"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"unknown method"}})
                .to_string();
        let success = json!({"jsonrpc":"2.0","id":2,"result":{"ok":true}}).to_string();
        let script = format!(
            "printf '%s' 'Content-Length: {}\r\n\r\n{}Content-Length: {}\r\n\r\n{}'; exec sleep 5",
            error.len(),
            error,
            success.len(),
            success
        );
        let (_child, client) = inert_server(&script);
        let error = client.request("unknown", json!({})).unwrap_err();
        assert!(error.to_string().contains("upstream mcp error"), "{error}");
        assert_eq!(
            client.request("ping", json!({})).unwrap(),
            json!({"ok":true})
        );
    }

    #[test]
    #[cfg(unix)]
    fn close_stdin_delivers_eof_to_child() {
        let (mut child, client) = inert_server("cat >/dev/null");
        client.close_stdin();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(Instant::now() < deadline, "child did not receive EOF");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

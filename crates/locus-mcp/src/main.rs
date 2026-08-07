//! locus-mcp — stdio MCP multiplexor hard-scoped to the active Locus pin.
//!
//! Agents see only control tools when unpinned, and control + provider tools
//! when pinned. Agents cannot pin themselves (`locus_request_pin` only).
//!
//! Protocol: JSON-RPC 2.0 over stdio.
//! Framing: **Content-Length** (MCP standard; Claude Code / Cursor) and
//! **NDJSON** (newline-delimited JSON) for simple clients/tests. Responses
//! use the same framing as the request that triggered them.

use anyhow::{Context, Result};
use locus_core::{
    call_tool_gated, control_tools, tools_for_binding, AdapterTool, ApprovalGate, Store, VERSION,
};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

/// Wire framing chosen per message so mixed clients stay happy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Framing {
    /// `Content-Length: N\r\n\r\n{body}` (MCP stdio transport).
    ContentLength,
    /// Single JSON object terminated by `\n`.
    Ndjson,
}

fn main() {
    // MCP servers must not pollute stdout with logs
    if let Err(e) = run() {
        eprintln!("locus-mcp error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut reader = stdin.lock();

    loop {
        let Some((msg, framing)) = read_message(&mut reader)? else {
            break; // EOF
        };

        // Notifications have no id — handle then continue without a response.
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(json!({}));

        if id.is_none() {
            handle_notification(method, &params);
            continue;
        }

        let result = match method {
            "initialize" => Ok(handle_initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => handle_tools_list(),
            "tools/call" => handle_tools_call(&params),
            "resources/list" => Ok(json!({ "resources": [] })),
            "prompts/list" => Ok(json!({ "prompts": [] })),
            other => Err(rpc_error(-32601, format!("method not found: {other}"))),
        };

        let response = match result {
            Ok(r) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": r,
            }),
            Err(err) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": err,
            }),
        };
        write_message(&mut stdout, &response, framing)?;
    }
    Ok(())
}

/// Read one MCP message. Supports Content-Length headers and NDJSON lines.
fn read_message<R: BufRead>(reader: &mut R) -> Result<Option<(Value, Framing)>> {
    let mut first = String::new();
    let n = reader.read_line(&mut first)?;
    if n == 0 {
        return Ok(None);
    }
    let trimmed = first.trim();
    if trimmed.is_empty() {
        // Skip blank lines (common between framed messages)
        return read_message(reader);
    }

    // Content-Length framed: headers until blank line, then body bytes.
    if trimmed.to_ascii_lowercase().starts_with("content-length:")
        || trimmed.to_ascii_lowercase().starts_with("content-type:")
    {
        let mut content_length: Option<usize> = None;
        let mut line = first;
        loop {
            let lower = line.trim().to_ascii_lowercase();
            if lower.starts_with("content-length:") {
                let v = lower
                    .trim_start_matches("content-length:")
                    .trim()
                    .parse::<usize>()
                    .context("Content-Length parse")?;
                content_length = Some(v);
            }
            // blank line ends headers
            if line.trim().is_empty() {
                break;
            }
            line.clear();
            let n = reader.read_line(&mut line)?;
            if n == 0 {
                return Ok(None);
            }
        }
        let len = content_length.context("Content-Length header missing")?;
        let mut buf = vec![0u8; len];
        reader
            .read_exact(&mut buf)
            .context("read Content-Length body")?;
        let msg: Value = serde_json::from_slice(&buf).context("json body")?;
        return Ok(Some((msg, Framing::ContentLength)));
    }

    // NDJSON: first non-empty line is the JSON object.
    match serde_json::from_str::<Value>(trimmed) {
        Ok(msg) => Ok(Some((msg, Framing::Ndjson))),
        Err(e) => {
            eprintln!("locus-mcp: bad json: {e}");
            // Skip garbage line; keep serving
            read_message(reader)
        }
    }
}

fn write_message<W: Write>(out: &mut W, msg: &Value, framing: Framing) -> Result<()> {
    let body = serde_json::to_vec(msg)?;
    match framing {
        Framing::ContentLength => {
            // MCP stdio uses CRLF headers + raw body (no trailing newline required).
            write!(out, "Content-Length: {}\r\n\r\n", body.len())?;
            out.write_all(&body)?;
        }
        Framing::Ndjson => {
            out.write_all(&body)?;
            out.write_all(b"\n")?;
        }
    }
    out.flush()?;
    Ok(())
}

/// Notifications never receive a JSON-RPC response.
fn handle_notification(method: &str, _params: &Value) {
    match method {
        "notifications/initialized" | "initialized" => {
            // Client finished initialize handshake — ready for tools/list etc.
            // No server-side state required in phase 1.
        }
        "notifications/cancelled" => {}
        _ => {
            // Ignore unknown notifications (forward-compatible).
        }
    }
}

fn rpc_error(code: i64, message: String) -> Value {
    json!({ "code": code, "message": message })
}

fn handle_initialize(_params: &Value) -> Value {
    json!({
        "protocolVersion": "2024-11-05",
        "capabilities": {
            "tools": { "listChanged": false }
        },
        "serverInfo": {
            "name": "locus-mcp",
            "version": VERSION
        },
        "instructions": "Locus identity plane. Tools are hard-scoped to the active pin. Call locus_whoami first. Agents cannot pin — ask the human to run `locus pin <alias>` or use locus_request_pin."
    })
}

fn store() -> Result<Store> {
    Store::open_default().context("open locus store")
}

fn active_binding() -> Result<Option<(locus_core::Session, locus_core::Binding)>> {
    let s = store()?;
    match s.active_session()? {
        None => Ok(None),
        Some(session) => {
            let key = s.seal_key()?;
            session.verify(&key)?;
            let binding = s.load_binding(&session.binding_alias)?;
            Ok(Some((session, binding)))
        }
    }
}

/// Unpinned ⇒ only locus_* control tools. Pinned ⇒ control + provider tools.
fn handle_tools_list() -> std::result::Result<Value, Value> {
    let pinned = active_binding().map_err(|e| rpc_error(-32000, e.to_string()))?;
    let mut tools: Vec<AdapterTool> = control_tools(pinned.is_some());
    if let Some((_, ref binding)) = pinned {
        tools.extend(tools_for_binding(binding));
    }
    // INV: unpinned must not expose provider tools
    if pinned.is_none() {
        debug_assert!(tools.iter().all(|t| t.name.starts_with("locus_")));
    }
    let list: Vec<Value> = tools
        .into_iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "inputSchema": t.input_schema,
            })
        })
        .collect();
    Ok(json!({ "tools": list }))
}

fn handle_tools_call(params: &Value) -> std::result::Result<Value, Value> {
    let name = params
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| rpc_error(-32602, "missing tool name".into()))?;
    let args = params.get("arguments").cloned().unwrap_or(json!({}));

    // Control tools
    if name.starts_with("locus_") {
        return call_control(name, &args);
    }

    // Provider tools require pin
    let s = store().map_err(|e| rpc_error(-32000, e.to_string()))?;
    let pinned = active_binding().map_err(|e| rpc_error(-32000, e.to_string()))?;
    let Some((session, binding)) = pinned else {
        return Ok(tool_text(
            json!({
                "error": "not_pinned",
                "hint": "Human must run: locus pin <alias>. Agents: call locus_request_pin."
            }),
            true,
        ));
    };

    let gate = ApprovalGate {
        store: &s,
        session_id: &session.session_id,
    };
    match call_tool_gated(&binding, name, &args, Some(gate)) {
        Ok(r) => {
            // Audit require_approval blocks (record already on disk under approvals/)
            if !r.ok {
                if let Some(err) = r.content.get("error").and_then(|v| v.as_str()) {
                    if err == "requires_approval" {
                        let _ = s.audit(
                            "mcp.require_approval",
                            &binding.alias,
                            Some(json!({
                                "tool": name,
                                "status": "pending",
                                "approval_id": r.content.get("approval_id"),
                                "args_digest": r.content.get("args_digest"),
                                "detail": r.content.get("detail"),
                            })),
                        );
                    }
                }
            }
            Ok(tool_text(r.content, !r.ok))
        }
        Err(e) => {
            // Scope freeze denials surface as errors from adapters
            let msg = e.to_string();
            if msg.contains("scope freeze") {
                let _ = s.audit(
                    "mcp.scope_freeze",
                    &binding.alias,
                    Some(json!({ "tool": name, "error": msg, "args": args })),
                );
            }
            Ok(tool_text(json!({ "error": msg }), true))
        }
    }
}

fn call_control(name: &str, args: &Value) -> std::result::Result<Value, Value> {
    let s = store().map_err(|e| rpc_error(-32000, e.to_string()))?;
    match name {
        "locus_whoami" => match s.whoami() {
            Ok(w) => Ok(tool_text(
                serde_json::to_value(w).unwrap_or(json!({})),
                false,
            )),
            Err(e) => Ok(tool_text(
                json!({
                    "pinned": false,
                    "error": e.to_string(),
                    "hint": "Run `locus pin <alias>` in this workspace."
                }),
                true,
            )),
        },
        "locus_status" => {
            let active = s
                .active_session()
                .map_err(|e| rpc_error(-32000, e.to_string()))?;
            match active {
                None => Ok(tool_text(
                    json!({ "pinned": false, "status": "unpinned" }),
                    false,
                )),
                Some(session) => {
                    let key = s.seal_key().map_err(|e| rpc_error(-32000, e.to_string()))?;
                    let seal_ok = session.verify(&key).is_ok();
                    Ok(tool_text(
                        json!({
                            "pinned": true,
                            "binding": session.binding_alias,
                            "tenant": session.tenant,
                            "session_id": session.session_id,
                            "seal_ok": seal_ok,
                            "expired": session.is_expired(),
                        }),
                        false,
                    ))
                }
            }
        }
        "locus_list_bindings" => {
            let list = s
                .list_bindings()
                .map_err(|e| rpc_error(-32000, e.to_string()))?;
            Ok(tool_text(
                serde_json::to_value(list).unwrap_or(json!([])),
                false,
            ))
        }
        "locus_request_pin" => {
            let alias = args.get("alias").and_then(|a| a.as_str()).unwrap_or("");
            if alias.is_empty() {
                return Ok(tool_text(json!({ "error": "alias required" }), true));
            }
            // Verify binding exists
            let exists = s.load_binding(alias).is_ok();
            let _ = s.audit("mcp.request_pin", alias, Some(json!({ "exists": exists })));
            Ok(tool_text(
                json!({
                    "requested": alias,
                    "binding_exists": exists,
                    "message": format!(
                        "Pin request recorded for `{alias}`. Agents cannot pin themselves. \
                         Human: run `locus pin {alias}` in the terminal, then continue."
                    ),
                    "command": format!("locus pin {alias}")
                }),
                false,
            ))
        }
        "locus_providers" => match s.whoami() {
            Ok(w) => Ok(tool_text(json!({ "providers": w.providers }), false)),
            Err(e) => Ok(tool_text(json!({ "error": e.to_string() }), true)),
        },
        other => Ok(tool_text(
            json!({ "error": format!("unknown control tool: {other}") }),
            true,
        )),
    }
}

fn tool_text(value: Value, is_error: bool) -> Value {
    // Compact JSON only — pretty-print embeds raw newlines that break NDJSON stdio.
    let text = match &value {
        Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_else(|_| other.to_string()),
    };
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error
    })
}

//! MCP stdio child-process backend with JSON-RPC fan-out.
//!
//! Builds a `std::process::Command` with isolated env, optionally spawns the
//! child, handshakes MCP, and routes `tools/call` to the upstream server.
//!
//! When sandbox is enabled (`LOCUS_WORKER_SANDBOX=1`, [`McpStdioConfig::sandbox`],
//! or binding `upstream.sandbox`), spawn requires a supported OS isolation
//! backend. Missing backends and unresolved executables fail before spawn.

use super::sandbox::{
    resolve_sandbox_spawn, sandbox_enabled, ENV_WORKER_SANDBOXED, ENV_WORKER_SANDBOX_BACKEND,
};
use super::stdio_client::{client_key, McpStdioClient};
use super::{WorkerBackend, WorkerKey, WorkerSlot, WorkerState, WorkerToolResult};
use crate::binding::{Binding, ProviderBinding};
use crate::error::{LocusError, Result};
use crate::isolation::build_isolated_env_opts;
use crate::session::Session;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

fn slot_client_key(slot: &WorkerSlot) -> String {
    if slot.key.binding_alias.is_empty() {
        client_key(&slot.key.session_id, &slot.key.provider)
    } else {
        format!(
            "{}:{}:{}",
            slot.key.session_id, slot.key.binding_alias, slot.key.provider
        )
    }
}

/// Configuration for an MCP stdio worker spawn.
#[derive(Debug, Clone, Default)]
pub struct McpStdioConfig {
    /// Executable (e.g. `npx`, path to upstream MCP binary).
    pub command: String,
    pub args: Vec<String>,
    /// When false (default), `ensure` only prepares the slot + work dir.
    pub spawn: bool,
    /// Resolve credentials into provider-standard child env keys when spawning.
    pub resolve_secrets: bool,
    /// Deprecated compatibility field. Arbitrary env is never forwarded; use
    /// binding scope metadata or provider credential resolution instead.
    pub extra_env: BTreeMap<String, String>,
    /// Require OS-backed sandbox isolation. PATH/markers are diagnostics only.
    /// Also enabled when `LOCUS_WORKER_SANDBOX=1` regardless of this flag.
    pub sandbox: bool,
}

/// Stdio MCP backend with live JSON-RPC clients.
pub struct McpStdioBackend {
    config: McpStdioConfig,
    /// session:provider → Child process
    children: Mutex<BTreeMap<WorkerKey, Child>>,
    /// Live MCP clients (stdin/stdout taken from children)
    clients: Mutex<BTreeMap<String, McpStdioClient>>,
}

impl McpStdioBackend {
    pub fn new(config: McpStdioConfig) -> Self {
        Self {
            config,
            children: Mutex::new(BTreeMap::new()),
            clients: Mutex::new(BTreeMap::new()),
        }
    }

    /// Build a `Command` ready to spawn with isolated env.
    ///
    /// When sandbox is on: resolve the protected executable, require an OS
    /// backend, and install a private temp root before returning the command.
    pub fn build_command(
        &self,
        session: &Session,
        binding: &Binding,
        provider: &ProviderBinding,
        work_dir: &Path,
    ) -> Result<Command> {
        let iso = build_isolated_env_opts(session, binding, self.config.resolve_secrets);
        let sandboxed = sandbox_enabled(self.config.sandbox);

        let (program, args, sandbox_backend) = if sandboxed {
            let spawn = resolve_sandbox_spawn(
                &self.config.command,
                &self.config.args,
                work_dir,
                Path::new(&session.worker_home),
            )?;
            (spawn.program, spawn.args, Some((spawn.backend, spawn.path)))
        } else {
            (self.config.command.clone(), self.config.args.clone(), None)
        };

        let mut cmd = Command::new(&program);
        cmd.args(&args)
            .current_dir(work_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear();

        for (k, v) in &iso.vars {
            cmd.env(k, v);
        }
        cmd.env("LOCUS_WORKER_PROVIDER", &provider.provider);
        cmd.env("LOCUS_WORKER_ACCOUNT", &provider.account);
        cmd.env("LOCUS_WORKER_DIR", work_dir);

        if let Some((backend, restricted_path)) = sandbox_backend {
            let temp_root = Path::new(&session.worker_home).join("tmp");
            std::fs::create_dir_all(&temp_root)?;
            // These diagnostics are set only after real OS isolation resolved.
            cmd.env("PATH", restricted_path);
            cmd.env("TMPDIR", &temp_root);
            cmd.env("TMP", &temp_root);
            cmd.env("TEMP", &temp_root);
            cmd.env(ENV_WORKER_SANDBOXED, "1");
            cmd.env(ENV_WORKER_SANDBOX_BACKEND, backend.as_str());
        }

        Ok(cmd)
    }

    /// Whether this backend will apply sandbox on spawn (config or env).
    pub fn sandbox_active(&self) -> bool {
        sandbox_enabled(self.config.sandbox)
    }

    /// List tools from a live upstream client (handshake if needed).
    pub fn list_upstream_tools(
        &self,
        session_id: &str,
        provider: &str,
    ) -> Result<Vec<super::UpstreamTool>> {
        self.list_upstream_tools_for(session_id, None, provider)
    }

    /// List tools; `binding_alias` disambiguates namespaced multi-bind clients.
    pub fn list_upstream_tools_for(
        &self,
        session_id: &str,
        binding_alias: Option<&str>,
        provider: &str,
    ) -> Result<Vec<super::UpstreamTool>> {
        let clients = self
            .clients
            .lock()
            .map_err(|_| LocusError::msg("clients lock poisoned"))?;
        let ck = match binding_alias {
            Some(a) if !a.is_empty() => format!("{session_id}:{a}:{provider}"),
            _ => client_key(session_id, provider),
        };
        let client = clients.get(&ck).or_else(|| {
            // Fallback: single-client backends (one slot per backend instance)
            if clients.len() == 1 {
                clients.values().next()
            } else {
                None
            }
        });
        let client = client.ok_or_else(|| LocusError::msg("no live mcp client for provider"))?;
        client.list_tools_cached()
    }

    /// Convenience: tool names only.
    pub fn upstream_tools(&self, session_id: &str, provider: &str) -> Result<Vec<String>> {
        Ok(self
            .list_upstream_tools(session_id, provider)?
            .into_iter()
            .map(|t| t.name)
            .collect())
    }
}

impl WorkerBackend for McpStdioBackend {
    fn name(&self) -> &'static str {
        "mcp_stdio"
    }

    fn ensure(
        &self,
        session: &Session,
        binding: &Binding,
        provider: &ProviderBinding,
        work_dir: &Path,
    ) -> Result<WorkerSlot> {
        std::fs::create_dir_all(work_dir)?;
        let key = if session.is_namespaced() {
            WorkerKey::namespaced(
                &session.session_id,
                &binding.alias,
                provider.provider.to_ascii_lowercase(),
            )
        } else {
            WorkerKey::new(&session.session_id, provider.provider.to_ascii_lowercase())
        };

        let mut pid = None;
        let mut state = WorkerState::Ready;

        if self.config.spawn {
            if self.config.command.is_empty() {
                return Err(LocusError::msg(
                    "mcp_stdio spawn requested but command is empty",
                ));
            }
            let mut cmd = self.build_command(session, binding, provider, work_dir)?;
            match cmd.spawn() {
                Ok(mut child) => {
                    pid = Some(child.id());
                    // Handshake before storing as Running
                    let client = McpStdioClient::from_child(&mut child)?;
                    match client.handshake() {
                        Ok(_tools) => {
                            state = WorkerState::Running;
                            // Disambiguate client map when namespaced multi-bind
                            let ck = if session.is_namespaced() {
                                format!(
                                    "{}:{}:{}",
                                    session.session_id, binding.alias, provider.provider
                                )
                            } else {
                                client_key(&session.session_id, &provider.provider)
                            };
                            self.clients
                                .lock()
                                .map_err(|_| LocusError::msg("clients lock poisoned"))?
                                .insert(ck, client);
                            self.children
                                .lock()
                                .map_err(|_| LocusError::msg("children lock poisoned"))?
                                .insert(key.clone(), child);
                        }
                        Err(e) => {
                            let _ = child.kill();
                            let _ = child.wait();
                            return Err(LocusError::msg(format!(
                                "mcp handshake failed for {}: {e}",
                                provider.provider
                            )));
                        }
                    }
                }
                Err(e) => {
                    return Err(LocusError::msg(format!(
                        "failed to spawn mcp_stdio worker `{}`: {e}",
                        self.config.command
                    )));
                }
            }
        }

        Ok(WorkerSlot {
            key,
            binding_id: binding.id.clone(),
            binding_alias: binding.alias.clone(),
            account: provider.account.clone(),
            credential: crate::credential::credential_metadata(&provider.credential_ref),
            state,
            work_dir: work_dir.to_path_buf(),
            backend: "mcp_stdio".into(),
            pid,
        })
    }

    fn teardown(&self, slot: &WorkerSlot) -> Result<()> {
        let ck = slot_client_key(slot);
        let _ = self
            .clients
            .lock()
            .map_err(|_| LocusError::msg("clients lock poisoned"))?
            .remove(&ck);
        // Also try exclusive-form key for legacy slots
        let _ = self
            .clients
            .lock()
            .ok()
            .and_then(|mut g| g.remove(&client_key(&slot.key.session_id, &slot.key.provider)));
        let mut guard = self
            .children
            .lock()
            .map_err(|_| LocusError::msg("worker children lock poisoned"))?;
        if let Some(mut child) = guard.remove(&slot.key) {
            let _ = child.kill();
            let _ = child.wait();
        }
        Ok(())
    }

    fn call_tool(
        &self,
        slot: &WorkerSlot,
        _binding: &Binding,
        tool: &str,
        args: &Value,
    ) -> Result<WorkerToolResult> {
        let ck = slot_client_key(slot);
        let clients = self
            .clients
            .lock()
            .map_err(|_| LocusError::msg("clients lock poisoned"))?;

        let client = clients
            .get(&ck)
            .or_else(|| clients.get(&client_key(&slot.key.session_id, &slot.key.provider)));
        let Some(client) = client else {
            return Ok(WorkerToolResult {
                ok: false,
                content: json!({
                    "error": "mcp_stdio_not_connected",
                    "detail": "No live MCP client — spawn=true required, or child exited",
                    "tool": tool,
                    "provider": slot.key.provider,
                    "backend": "mcp_stdio",
                }),
                provider: slot.key.provider.clone(),
            });
        };

        // Strip provider prefix if tools were namespaced as provider.tool
        let upstream_name = tool
            .strip_prefix(&format!("{}.", slot.key.provider))
            .unwrap_or(tool);

        match client.call_tool(upstream_name, args) {
            Ok(result) => {
                let is_error = result
                    .get("isError")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                Ok(WorkerToolResult {
                    ok: !is_error,
                    content: result,
                    provider: slot.key.provider.clone(),
                })
            }
            Err(e) => Ok(WorkerToolResult {
                ok: false,
                content: json!({
                    "error": "upstream_call_failed",
                    "detail": e.to_string(),
                    "tool": tool,
                    "upstream_tool": upstream_name,
                    "provider": slot.key.provider,
                }),
                provider: slot.key.provider.clone(),
            }),
        }
    }
}

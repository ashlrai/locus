//! MCP stdio child-process backend (scaffold).
//!
//! Builds a `std::process::Command` with:
//! - isolated env from `build_isolated_env_opts` (scrubbed ambient identity)
//! - private work dir under the session worker home
//! - no requirement that the upstream binary exists when `spawn = false`
//!
//! Real JSON-RPC handshake / tools/list fan-out is intentionally deferred.

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

/// Configuration for an MCP stdio worker spawn.
#[derive(Debug, Clone, Default)]
pub struct McpStdioConfig {
    /// Executable (e.g. `npx`, path to upstream MCP binary).
    pub command: String,
    pub args: Vec<String>,
    /// When false (default), `ensure` only prepares the slot + work dir.
    pub spawn: bool,
    /// Extra env layered after isolation (non-secret config only preferred).
    pub extra_env: BTreeMap<String, String>,
}

/// Stdio MCP backend. Child handles are held only when `spawn = true`.
pub struct McpStdioBackend {
    config: McpStdioConfig,
    /// session_id:provider → Child (only when spawned).
    children: Mutex<BTreeMap<WorkerKey, Child>>,
}

impl McpStdioBackend {
    pub fn new(config: McpStdioConfig) -> Self {
        Self {
            config,
            children: Mutex::new(BTreeMap::new()),
        }
    }

    /// Build a `Command` ready to spawn with isolated env.
    ///
    /// Does not start the process. Callers (or `ensure` with `spawn=true`) own lifecycle.
    pub fn build_command(
        &self,
        session: &Session,
        binding: &Binding,
        provider: &ProviderBinding,
        work_dir: &Path,
    ) -> Command {
        let iso = build_isolated_env_opts(session, binding, false);

        let mut cmd = Command::new(&self.config.command);
        cmd.args(&self.config.args)
            .current_dir(work_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear();

        for (k, v) in &iso.vars {
            cmd.env(k, v);
        }
        // Provider-specific markers (non-secret)
        cmd.env("LOCUS_WORKER_PROVIDER", &provider.provider);
        cmd.env("LOCUS_WORKER_ACCOUNT", &provider.account);
        cmd.env("LOCUS_WORKER_CREDENTIAL_REF", &provider.credential_ref);
        cmd.env("LOCUS_WORKER_DIR", work_dir);

        for (k, v) in &self.config.extra_env {
            cmd.env(k, v);
        }

        cmd
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
        let key = WorkerKey::new(&session.session_id, provider.provider.to_ascii_lowercase());

        let mut pid = None;
        let mut state = WorkerState::Ready;

        if self.config.spawn {
            if self.config.command.is_empty() {
                return Err(LocusError::msg(
                    "mcp_stdio spawn requested but command is empty",
                ));
            }
            let mut cmd = self.build_command(session, binding, provider, work_dir);
            match cmd.spawn() {
                Ok(child) => {
                    pid = Some(child.id());
                    state = WorkerState::Running;
                    let mut guard = self
                        .children
                        .lock()
                        .map_err(|_| LocusError::msg("worker children lock poisoned"))?;
                    guard.insert(key.clone(), child);
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
            credential_ref: provider.credential_ref.clone(),
            state,
            work_dir: work_dir.to_path_buf(),
            backend: "mcp_stdio".into(),
            pid,
        })
    }

    fn teardown(&self, slot: &WorkerSlot) -> Result<()> {
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
        // Scaffold: no JSON-RPC yet. Report clearly so callers don't pretend success.
        Ok(WorkerToolResult {
            ok: false,
            content: json!({
                "error": "mcp_stdio_not_connected",
                "detail": "Phase 2 scaffold: stdio JSON-RPC fan-out not implemented yet",
                "tool": tool,
                "args": args,
                "provider": slot.key.provider,
                "pid": slot.pid,
                "backend": "mcp_stdio",
            }),
            provider: slot.key.provider.clone(),
        })
    }
}

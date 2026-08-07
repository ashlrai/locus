//! Synthetic worker backend — in-process adapter tools (no child process).
//!
//! This is the Phase 1 / Phase 2 scaffolding default: tools are served by
//! `crate::adapters` without spawning upstream MCP servers.

use super::{WorkerBackend, WorkerKey, WorkerSlot, WorkerState, WorkerToolResult};
use crate::adapters;
use crate::binding::{Binding, ProviderBinding};
use crate::error::Result;
use crate::session::Session;
use serde_json::Value;
use std::path::Path;

/// Serves adapter tools in-process.
#[derive(Debug, Default, Clone, Copy)]
pub struct SyntheticBackend;

impl WorkerBackend for SyntheticBackend {
    fn name(&self) -> &'static str {
        "synthetic"
    }

    fn ensure(
        &self,
        session: &Session,
        binding: &Binding,
        provider: &ProviderBinding,
        work_dir: &Path,
    ) -> Result<WorkerSlot> {
        std::fs::create_dir_all(work_dir)?;
        Ok(WorkerSlot {
            key: WorkerKey::new(&session.session_id, provider.provider.to_ascii_lowercase()),
            binding_id: binding.id.clone(),
            binding_alias: binding.alias.clone(),
            account: provider.account.clone(),
            credential_ref: provider.credential_ref.clone(),
            state: WorkerState::Ready,
            work_dir: work_dir.to_path_buf(),
            backend: "synthetic".into(),
            pid: None,
        })
    }

    fn teardown(&self, _slot: &WorkerSlot) -> Result<()> {
        // Nothing to kill — pure in-process.
        Ok(())
    }

    fn call_tool(
        &self,
        slot: &WorkerSlot,
        binding: &Binding,
        tool: &str,
        args: &Value,
    ) -> Result<WorkerToolResult> {
        let result = adapters::call_tool(binding, tool, args)?;
        Ok(WorkerToolResult {
            ok: result.ok,
            content: result.content,
            provider: slot.key.provider.clone(),
        })
    }
}

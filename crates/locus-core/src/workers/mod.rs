//! Worker slots — isolated process handles per Binding × Provider.
//!
//! - [`SyntheticBackend`] serves in-process adapter tools (default).
//! - [`McpStdioBackend`] spawns an upstream MCP child, handshakes, and
//!   fans out `tools/call` over stdio JSON-RPC.
//! - [`CompositeWorkerManager`] routes per provider from binding `upstream`.

mod composite;
mod mcp_stdio;
mod sandbox;
mod stdio_client;
mod synthetic;

use crate::binding::{Binding, ProviderBinding};
use crate::error::{LocusError, Result};
use crate::session::Session;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub use composite::{
    idle_timeout_from_env, mcp_config_from_upstream, namespace_upstream_tool,
    provider_from_tool_name, strip_provider_prefix, CompositeWorkerManager, ENV_WORKER_IDLE_SECS,
};
pub use mcp_stdio::{McpStdioBackend, McpStdioConfig};
pub use sandbox::{
    restricted_worker_path, sandbox_enabled, sandbox_from_env, ENV_WORKER_SANDBOX,
    ENV_WORKER_SANDBOXED, ENV_WORKER_SANDBOX_BACKEND,
};
pub use stdio_client::{McpStdioClient, UpstreamTool};
pub use synthetic::SyntheticBackend;

/// Stable key for a worker slot: session (+ optional binding alias) + provider.
///
/// `binding_alias` is empty in exclusive mode. Namespaced multi-bind fills it so
/// two bindings that both declare `github` do not collide.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WorkerKey {
    pub session_id: String,
    /// Empty string for exclusive pins; binding alias when namespaced.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub binding_alias: String,
    pub provider: String,
}

impl WorkerKey {
    pub fn new(session_id: impl Into<String>, provider: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            binding_alias: String::new(),
            provider: provider.into(),
        }
    }

    pub fn namespaced(
        session_id: impl Into<String>,
        binding_alias: impl Into<String>,
        provider: impl Into<String>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            binding_alias: binding_alias.into(),
            provider: provider.into(),
        }
    }

    /// Key for a binding: namespaced when `binding_alias` is non-empty and
    /// differs from a pure exclusive slot, or when caller opts in.
    pub fn for_binding(
        session_id: impl Into<String>,
        binding_alias: Option<&str>,
        provider: impl Into<String>,
    ) -> Self {
        match binding_alias {
            Some(a) if !a.is_empty() => Self::namespaced(session_id, a, provider),
            _ => Self::new(session_id, provider),
        }
    }
}

/// Lifecycle state of a worker slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerState {
    /// Slot reserved but not yet ready.
    Pending,
    /// Backend ready to accept tool calls.
    Ready,
    /// Child process running (stdio backend only).
    Running,
    /// Teardown requested / completed.
    Stopped,
    /// Last operation failed; may be retryable.
    Failed,
}

/// One Binding × Provider worker handle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerSlot {
    pub key: WorkerKey,
    pub binding_id: String,
    pub binding_alias: String,
    pub account: String,
    pub credential: crate::credential::CredentialMetadata,
    pub state: WorkerState,
    /// Private work dir under session worker_home.
    pub work_dir: PathBuf,
    /// Backend kind label for diagnostics.
    pub backend: String,
    /// Optional child PID when a real process is running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

/// Result of a tool call routed through a worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerToolResult {
    pub ok: bool,
    pub content: Value,
    pub provider: String,
}

/// Backend that owns how tools are served for a provider.
pub trait WorkerBackend: Send + Sync {
    fn name(&self) -> &'static str;

    /// Ensure a slot is ready for the given provider binding.
    fn ensure(
        &self,
        session: &Session,
        binding: &Binding,
        provider: &ProviderBinding,
        work_dir: &std::path::Path,
    ) -> Result<WorkerSlot>;

    /// Tear down a slot (kill child if any). Best-effort.
    fn teardown(&self, slot: &WorkerSlot) -> Result<()>;

    /// Route a tool call. Synthetic backends call adapters; stdio will JSON-RPC.
    fn call_tool(
        &self,
        slot: &WorkerSlot,
        binding: &Binding,
        tool: &str,
        args: &Value,
    ) -> Result<WorkerToolResult>;
}

/// Manages worker slots for a session (or process-wide for exclusive pin).
pub trait WorkerManager: Send + Sync {
    /// Ensure a slot exists and is ready for `provider`.
    fn ensure(
        &mut self,
        session: &Session,
        binding: &Binding,
        provider: &str,
    ) -> Result<WorkerSlot>;

    /// Ensure slots for every provider on the binding.
    fn ensure_all(&mut self, session: &Session, binding: &Binding) -> Result<Vec<WorkerSlot>>;

    /// Tear down one provider slot.
    fn teardown(&mut self, key: &WorkerKey) -> Result<()>;

    /// Tear down every slot for a session.
    fn teardown_session(&mut self, session_id: &str) -> Result<()>;

    /// Snapshot of active slots.
    fn list(&self) -> Vec<WorkerSlot>;

    /// Lookup a ready slot.
    fn get(&self, key: &WorkerKey) -> Option<&WorkerSlot>;
}

/// In-memory manager used by locus-mcp / tests.
pub struct InMemoryWorkerManager {
    backend: Box<dyn WorkerBackend>,
    slots: BTreeMap<WorkerKey, WorkerSlot>,
}

impl InMemoryWorkerManager {
    pub fn new(backend: Box<dyn WorkerBackend>) -> Self {
        Self {
            backend,
            slots: BTreeMap::new(),
        }
    }

    /// Convenience: synthetic (in-process adapter) backend.
    pub fn synthetic() -> Self {
        Self::new(Box::new(SyntheticBackend))
    }

    pub fn backend_name(&self) -> &'static str {
        self.backend.name()
    }
}

impl WorkerManager for InMemoryWorkerManager {
    fn ensure(
        &mut self,
        session: &Session,
        binding: &Binding,
        provider: &str,
    ) -> Result<WorkerSlot> {
        let key = WorkerKey::new(&session.session_id, provider.to_ascii_lowercase());
        if let Some(existing) = self.slots.get(&key) {
            if matches!(
                existing.state,
                WorkerState::Ready | WorkerState::Running | WorkerState::Pending
            ) {
                return Ok(existing.clone());
            }
        }

        let pb = binding.provider(provider).ok_or_else(|| {
            LocusError::msg(format!(
                "provider '{provider}' not present on binding `{}`",
                binding.alias
            ))
        })?;

        let work_dir = PathBuf::from(&session.worker_home)
            .join("slots")
            .join(provider.to_ascii_lowercase());
        std::fs::create_dir_all(&work_dir)?;

        let slot = self.backend.ensure(session, binding, pb, &work_dir)?;
        self.slots.insert(key.clone(), slot.clone());
        Ok(slot)
    }

    fn ensure_all(&mut self, session: &Session, binding: &Binding) -> Result<Vec<WorkerSlot>> {
        let mut out = Vec::with_capacity(binding.providers.len());
        for p in &binding.providers {
            out.push(self.ensure(session, binding, &p.provider)?);
        }
        Ok(out)
    }

    fn teardown(&mut self, key: &WorkerKey) -> Result<()> {
        if let Some(slot) = self.slots.remove(key) {
            self.backend.teardown(&slot)?;
        }
        Ok(())
    }

    fn teardown_session(&mut self, session_id: &str) -> Result<()> {
        let keys: Vec<WorkerKey> = self
            .slots
            .keys()
            .filter(|k| k.session_id == session_id)
            .cloned()
            .collect();
        for k in keys {
            self.teardown(&k)?;
        }
        Ok(())
    }

    fn list(&self) -> Vec<WorkerSlot> {
        self.slots.values().cloned().collect()
    }

    fn get(&self, key: &WorkerKey) -> Option<&WorkerSlot> {
        self.slots.get(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::{BindingBody, Policy, Scope};
    use crate::seal::SealKey;
    use crate::session::PinSource;
    use chrono::Duration;
    use std::process::Command;
    use tempfile::tempdir;

    fn sample_binding() -> Binding {
        Binding::from_body(BindingBody {
            id: "bnd_acme".into(),
            alias: "acme".into(),
            tenant: "acme-corp".into(),
            principal: None,
            description: None,
            policy: Policy::default(),
            providers: vec![
                ProviderBinding {
                    provider: "supabase".into(),
                    account: "acme".into(),
                    credential_ref: "phm:SUPABASE_ACME".into(),
                    scope: Scope {
                        project_ref: Some("proj_acme".into()),
                        ..Scope::default()
                    },
                    upstream: None,
                },
                ProviderBinding {
                    provider: "github".into(),
                    account: "acme-gh".into(),
                    credential_ref: "phm:GH_ACME".into(),
                    scope: Scope::default(),
                    upstream: None,
                },
            ],
        })
    }

    fn sample_session(worker_home: &str) -> Session {
        let key = SealKey::generate();
        Session::new(
            "bnd_acme",
            "acme",
            "acme-corp",
            None,
            PinSource::Explicit,
            Some("test".into()),
            Duration::hours(1),
            worker_home.into(),
            &key,
        )
    }

    #[test]
    fn manager_ensure_and_teardown() {
        let dir = tempdir().unwrap();
        let worker_home = dir.path().join("worker");
        std::fs::create_dir_all(&worker_home).unwrap();
        let session = sample_session(&worker_home.display().to_string());
        let binding = sample_binding();

        let mut mgr = InMemoryWorkerManager::synthetic();
        assert_eq!(mgr.backend_name(), "synthetic");
        assert!(mgr.list().is_empty());

        let slot = mgr.ensure(&session, &binding, "supabase").unwrap();
        assert_eq!(slot.key.provider, "supabase");
        assert_eq!(slot.state, WorkerState::Ready);
        assert_eq!(slot.backend, "synthetic");
        assert!(slot.work_dir.exists());
        assert_eq!(mgr.list().len(), 1);

        // Idempotent ensure
        let slot2 = mgr.ensure(&session, &binding, "supabase").unwrap();
        assert_eq!(slot2.key, slot.key);
        assert_eq!(mgr.list().len(), 1);

        let all = mgr.ensure_all(&session, &binding).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(mgr.list().len(), 2);

        mgr.teardown(&WorkerKey::new(&session.session_id, "supabase"))
            .unwrap();
        assert_eq!(mgr.list().len(), 1);

        mgr.teardown_session(&session.session_id).unwrap();
        assert!(mgr.list().is_empty());
    }

    #[test]
    fn ensure_unknown_provider_fails() {
        let dir = tempdir().unwrap();
        let session = sample_session(&dir.path().display().to_string());
        let binding = sample_binding();
        let mut mgr = InMemoryWorkerManager::synthetic();
        let err = mgr.ensure(&session, &binding, "stripe").unwrap_err();
        assert!(err.to_string().contains("not present"));
    }

    #[test]
    fn synthetic_call_tool() {
        let dir = tempdir().unwrap();
        let worker_home = dir.path().join("worker");
        std::fs::create_dir_all(&worker_home).unwrap();
        let session = sample_session(&worker_home.display().to_string());
        let binding = sample_binding();
        let mut mgr = InMemoryWorkerManager::synthetic();
        let slot = mgr.ensure(&session, &binding, "supabase").unwrap();
        let r = SyntheticBackend
            .call_tool(&slot, &binding, "supabase.scope", &serde_json::json!({}))
            .unwrap();
        assert!(r.ok);
        assert_eq!(r.provider, "supabase");
    }

    #[test]
    fn mcp_stdio_builds_command_without_spawn() {
        let dir = tempdir().unwrap();
        let worker_home = dir.path().join("worker");
        std::fs::create_dir_all(&worker_home).unwrap();
        let session = sample_session(&worker_home.display().to_string());
        let binding = sample_binding();
        let pb = binding.provider("github").unwrap();
        let work_dir = worker_home.join("slots").join("github");
        std::fs::create_dir_all(&work_dir).unwrap();

        std::env::set_var("MCP_UNLISTED_SECRET_CANARY", "ambient-mcp-secret");
        let backend = McpStdioBackend::new(McpStdioConfig {
            command: "false".into(),
            args: vec![],
            spawn: false,
            resolve_secrets: false,
            extra_env: BTreeMap::from([(
                "ARBITRARY_EXTRA_SECRET".into(),
                "configured-mcp-secret".into(),
            )]),
            sandbox: false,
        });
        let slot = backend.ensure(&session, &binding, pb, &work_dir).unwrap();
        assert_eq!(slot.backend, "mcp_stdio");
        assert_eq!(slot.state, WorkerState::Ready);
        assert!(slot.pid.is_none());
        let command = backend.build_command(&session, &binding, pb, &work_dir);
        let env = command
            .get_envs()
            .filter_map(|(key, value)| value.map(|value| (key, value)))
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.to_string_lossy().into_owned(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        assert!(!env.keys().any(|key| key.contains("CREDENTIAL_REF")));
        assert!(!env.values().any(|value| value.contains("GH_ACME")));
        assert!(!env.contains_key("MCP_UNLISTED_SECRET_CANARY"));
        assert!(!env.contains_key("ARBITRARY_EXTRA_SECRET"));
        assert!(!env.values().any(|value| value == "ambient-mcp-secret"));
        assert!(!env.values().any(|value| value == "configured-mcp-secret"));
        let slot_json = serde_json::to_string(&slot).unwrap();
        assert!(!slot_json.contains("GH_ACME"));
        assert!(!slot_json.contains("credential_ref"));
        backend.teardown(&slot).unwrap();
        std::env::remove_var("MCP_UNLISTED_SECRET_CANARY");
    }

    #[test]
    fn mcp_stdio_sandbox_restricts_path_marker() {
        let dir = tempdir().unwrap();
        let worker_home = dir.path().join("worker");
        std::fs::create_dir_all(&worker_home).unwrap();
        let session = sample_session(&worker_home.display().to_string());
        let binding = sample_binding();
        let pb = binding.provider("github").unwrap();
        let work_dir = worker_home.join("slots").join("github");
        std::fs::create_dir_all(&work_dir).unwrap();

        let backend = McpStdioBackend::new(McpStdioConfig {
            command: "false".into(),
            args: vec![],
            spawn: false,
            resolve_secrets: false,
            extra_env: BTreeMap::new(),
            sandbox: true,
        });
        assert!(backend.sandbox_active());

        let command = backend.build_command(&session, &binding, pb, &work_dir);
        let env = command
            .get_envs()
            .filter_map(|(key, value)| value.map(|value| (key, value)))
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.to_string_lossy().into_owned(),
                )
            })
            .collect::<BTreeMap<_, _>>();

        let restricted = crate::workers::restricted_worker_path();
        let path = env.get("PATH").expect("sandboxed worker must set PATH");
        assert_eq!(
            path, &restricted,
            "PATH must equal restricted_worker_path()"
        );
        assert!(path.contains("/usr/bin"));
        assert!(path.contains("/bin"));

        assert_eq!(
            env.get(crate::workers::ENV_WORKER_SANDBOXED)
                .map(String::as_str),
            Some("1"),
            "LOCUS_WORKER_SANDBOXED marker required"
        );
        let backend_tag = env
            .get(crate::workers::ENV_WORKER_SANDBOX_BACKEND)
            .map(String::as_str)
            .expect("LOCUS_WORKER_SANDBOX_BACKEND required");
        assert!(
            backend_tag == "path" || backend_tag == "sandbox-exec",
            "unexpected sandbox backend: {backend_tag}"
        );
    }

    #[test]
    fn mcp_stdio_spawn_mock_and_call() {
        // Requires python3 for mock server
        if Command::new("python3").arg("--version").output().is_err() {
            return;
        }
        let dir = tempdir().unwrap();
        let worker_home = dir.path().join("worker");
        std::fs::create_dir_all(&worker_home).unwrap();
        let session = sample_session(&worker_home.display().to_string());
        let binding = sample_binding();
        let pb = binding.provider("github").unwrap();
        let work_dir = worker_home.join("slots").join("github");
        std::fs::create_dir_all(&work_dir).unwrap();

        let script = r#"
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
        send({"jsonrpc":"2.0","id":mid,"result":{"tools":[{"name":"ping","description":"p","inputSchema":{"type":"object"}}]}})
    elif method=="tools/call":
        send({"jsonrpc":"2.0","id":mid,"result":{"content":[{"type":"text","text":"pong"}],"isError":False}})
    else:
        send({"jsonrpc":"2.0","id":mid,"error":{"code":-32601,"message":method}})
"#;
        let backend = McpStdioBackend::new(McpStdioConfig {
            command: "python3".into(),
            args: vec!["-u".into(), "-c".into(), script.into()],
            spawn: true,
            resolve_secrets: false,
            extra_env: BTreeMap::new(),
            sandbox: false,
        });
        let slot = backend.ensure(&session, &binding, pb, &work_dir).unwrap();
        assert_eq!(slot.state, WorkerState::Running);
        assert!(slot.pid.is_some());
        let names = backend
            .upstream_tools(&session.session_id, "github")
            .unwrap();
        assert!(names.iter().any(|n| n == "ping"));
        let r = backend
            .call_tool(&slot, &binding, "ping", &serde_json::json!({}))
            .unwrap();
        assert!(r.ok);
        backend.teardown(&slot).unwrap();
    }
}

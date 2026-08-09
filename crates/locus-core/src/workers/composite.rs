//! Composite worker manager — routes per provider to synthetic or MCP stdio.
//!
//! Providers with [`crate::binding::UpstreamSpec`] get an [`McpStdioBackend`]
//! with `spawn=true`. All others use [`SyntheticBackend`].

use super::mcp_stdio::{McpStdioBackend, McpStdioConfig};
use super::stdio_client::UpstreamTool;
use super::synthetic::SyntheticBackend;
use super::{WorkerBackend, WorkerKey, WorkerManager, WorkerSlot, WorkerState, WorkerToolResult};
use crate::adapters::{self, AdapterTool};
use crate::binding::{Binding, UpstreamSpec};
use crate::error::{LocusError, Result};
use crate::session::Session;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Env var: idle seconds before upstream workers are torn down (0 / unset = never).
pub const ENV_WORKER_IDLE_SECS: &str = "LOCUS_WORKER_IDLE_SECS";

/// Build MCP config from a binding's upstream spec (always spawn).
pub fn mcp_config_from_upstream(spec: &UpstreamSpec) -> McpStdioConfig {
    McpStdioConfig {
        command: spec.command.clone(),
        args: spec.args.clone(),
        spawn: true,
        resolve_secrets: spec.resolve_secrets,
        extra_env: BTreeMap::new(),
    }
}

/// Parse idle timeout from `LOCUS_WORKER_IDLE_SECS` (seconds). `None` = disabled.
pub fn idle_timeout_from_env() -> Option<Duration> {
    let Ok(raw) = std::env::var(ENV_WORKER_IDLE_SECS) else {
        return None;
    };
    let raw = raw.trim();
    if raw.is_empty() || raw == "0" {
        return None;
    }
    raw.parse::<u64>()
        .ok()
        .filter(|&s| s > 0)
        .map(Duration::from_secs)
}

/// Namespace an upstream tool as `provider.toolname` (matches synthetic style).
pub fn namespace_upstream_tool(provider: &str, tool_name: &str) -> String {
    format!("{}.{}", provider.to_ascii_lowercase(), tool_name)
}

/// Strip `provider.` prefix for upstream fan-out (case-insensitive provider).
pub fn strip_provider_prefix(provider: &str, tool: &str) -> String {
    let prefix = format!("{}.", provider.to_ascii_lowercase());
    let lower = tool.to_ascii_lowercase();
    if lower.starts_with(&prefix) {
        tool[prefix.len()..].to_string()
    } else {
        tool.to_string()
    }
}

/// First path segment before `.` — used as provider key for routing.
pub fn provider_from_tool_name(tool: &str) -> Option<&str> {
    tool.split('.').next().filter(|s| !s.is_empty())
}

/// Per-provider routing: synthetic adapters and/or auto-spawned MCP children.
///
/// **Reuse:** `ensure` returns an existing Ready/Running/Pending slot for the
/// same `WorkerKey` without respawning. Upstream MCP children stay live across
/// `tools/list` and `tools/call` for the session until teardown or idle reap.
///
/// **Idle timeout (optional):** set `LOCUS_WORKER_IDLE_SECS` or call
/// [`Self::with_idle_timeout`] / [`Self::reap_idle`]. Touches on ensure + call.
pub struct CompositeWorkerManager {
    synthetic: SyntheticBackend,
    /// Live MCP backends for providers that declared `upstream`.
    mcp: BTreeMap<WorkerKey, McpStdioBackend>,
    slots: BTreeMap<WorkerKey, WorkerSlot>,
    /// Last use time per slot (for idle reap). Not serialized.
    last_used: BTreeMap<WorkerKey, Instant>,
    /// When set, `ensure*` / `call_tool` paths may reap idle workers first.
    idle_timeout: Option<Duration>,
}

impl Default for CompositeWorkerManager {
    fn default() -> Self {
        Self::new()
    }
}

impl CompositeWorkerManager {
    pub fn new() -> Self {
        Self {
            synthetic: SyntheticBackend,
            mcp: BTreeMap::new(),
            slots: BTreeMap::new(),
            last_used: BTreeMap::new(),
            idle_timeout: idle_timeout_from_env(),
        }
    }

    /// Construct with an explicit idle timeout (overrides env for this instance).
    pub fn with_idle_timeout(timeout: Option<Duration>) -> Self {
        let mut m = Self::new();
        m.idle_timeout = timeout;
        m
    }

    /// Configure idle timeout after construction.
    pub fn set_idle_timeout(&mut self, timeout: Option<Duration>) {
        self.idle_timeout = timeout;
    }

    pub fn idle_timeout(&self) -> Option<Duration> {
        self.idle_timeout
    }

    fn touch(&mut self, key: &WorkerKey) {
        self.last_used.insert(key.clone(), Instant::now());
    }

    /// Tear down slots whose last use exceeds `timeout` (or configured idle).
    ///
    /// Returns the number of slots reaped. Synthetic-only slots are cheap but
    /// still dropped so the pool stays accurate.
    pub fn reap_idle(&mut self, timeout: Option<Duration>) -> Result<usize> {
        let Some(limit) = timeout.or(self.idle_timeout) else {
            return Ok(0);
        };
        if limit.is_zero() {
            return Ok(0);
        }
        let now = Instant::now();
        let stale: Vec<WorkerKey> = self
            .slots
            .keys()
            .filter(|k| {
                self.last_used
                    .get(k)
                    .map(|t| now.duration_since(*t) >= limit)
                    // Never used / missing timestamp → treat as idle past limit.
                    .unwrap_or(true)
            })
            .cloned()
            .collect();
        let n = stale.len();
        for k in stale {
            self.teardown(&k)?;
        }
        Ok(n)
    }

    /// Reap using configured idle timeout (no-op when unset).
    pub fn reap_idle_configured(&mut self) -> Result<usize> {
        self.reap_idle(self.idle_timeout)
    }

    /// Tear down any slots not belonging to `session_id` (pin switch).
    pub fn focus_session(&mut self, session_id: &str) -> Result<()> {
        let stale: Vec<WorkerKey> = self
            .slots
            .keys()
            .filter(|k| k.session_id != session_id)
            .cloned()
            .collect();
        for k in stale {
            self.teardown(&k)?;
        }
        Ok(())
    }

    /// Ensure all providers for the pin; drop workers from other sessions.
    pub fn ensure_binding(
        &mut self,
        session: &Session,
        binding: &Binding,
    ) -> Result<Vec<WorkerSlot>> {
        let _ = self.reap_idle_configured()?;
        self.focus_session(&session.session_id)?;
        self.ensure_all(session, binding)
    }

    /// Whether the provider uses MCP stdio (has live or declared upstream).
    pub fn is_upstream_provider(&self, binding: &Binding, provider: &str) -> bool {
        binding.provider(provider).is_some_and(|p| p.has_upstream())
    }

    fn worker_key(session: &Session, binding: &Binding, provider: &str) -> WorkerKey {
        // Always key by binding alias when namespaced (or when multiple bindings
        // share the session) so provider slots never collide.
        if session.is_namespaced() {
            WorkerKey::namespaced(
                &session.session_id,
                &binding.alias,
                provider.to_ascii_lowercase(),
            )
        } else {
            WorkerKey::new(&session.session_id, provider.to_ascii_lowercase())
        }
    }

    /// Cached upstream tool definitions (provider-level names not alias-prefixed).
    pub fn list_upstream_tools(
        &self,
        session: &Session,
        binding: &Binding,
        provider: &str,
    ) -> Result<Vec<UpstreamTool>> {
        let key = Self::worker_key(session, binding, provider);
        let backend = self
            .mcp
            .get(&key)
            .ok_or_else(|| LocusError::msg(format!("no mcp worker for provider '{provider}'")))?;
        backend.list_upstream_tools_for(
            &session.session_id,
            if session.is_namespaced() {
                Some(binding.alias.as_str())
            } else {
                None
            },
            provider,
        )
    }

    /// Synthetic adapter tools + namespaced upstream tools for the binding.
    ///
    /// Call [`Self::ensure_binding`] first so upstream children are live.
    /// Upstream list failures are soft (empty merge) so synthetic tools still work.
    pub fn tools_for_pin(&self, session: &Session, binding: &Binding) -> Vec<AdapterTool> {
        let mut tools = adapters::tools_for_binding(binding);
        let synthetic_names: std::collections::BTreeSet<String> =
            tools.iter().map(|t| t.name.clone()).collect();

        for p in &binding.providers {
            if !p.has_upstream() {
                continue;
            }
            let Ok(upstream) = self.list_upstream_tools(session, binding, &p.provider) else {
                continue;
            };
            let prov = p.provider.to_ascii_lowercase();
            for t in upstream {
                let name = namespace_upstream_tool(&prov, &t.name);
                if synthetic_names.contains(&name) {
                    // Prefer synthetic identity/scope tools on name collision.
                    continue;
                }
                tools.push(AdapterTool {
                    name,
                    description: if t.description.is_empty() {
                        format!(
                            "Upstream MCP tool `{}` via {} worker (binding `{}`).",
                            t.name, p.provider, binding.alias
                        )
                    } else {
                        t.description
                    },
                    input_schema: t.input_schema,
                    provider: p.provider.clone(),
                    destructive: false,
                });
            }
        }
        tools
    }

    /// Tools for every binding in the session. Exclusive: single binding tools.
    /// Namespaced: each tool name prefixed with `alias__`.
    pub fn tools_for_session(
        &self,
        session: &Session,
        bindings: &[(String, Binding)],
    ) -> Vec<AdapterTool> {
        if !session.is_namespaced() {
            if let Some((_, b)) = bindings.first() {
                return self.tools_for_pin(session, b);
            }
            return Vec::new();
        }
        let mut out = Vec::new();
        for (alias, binding) in bindings {
            for mut t in self.tools_for_pin(session, binding) {
                t.name = crate::session::namespace_tool(alias, &t.name);
                t.description = format!("[{alias}] {}", t.description);
                out.push(t);
            }
        }
        out
    }

    /// Ensure workers for every binding in a (possibly namespaced) session.
    pub fn ensure_session(
        &mut self,
        session: &Session,
        bindings: &[(String, Binding)],
    ) -> Result<Vec<WorkerSlot>> {
        let _ = self.reap_idle_configured()?;
        self.focus_session(&session.session_id)?;
        let mut out = Vec::new();
        for (_, binding) in bindings {
            out.extend(self.ensure_all(session, binding)?);
        }
        Ok(out)
    }

    /// Route a tool call: upstream MCP when the tool is not a synthetic adapter
    /// tool and the provider has an upstream worker; otherwise synthetic.
    ///
    /// Touches the slot's last-used time for idle pool reuse accounting.
    pub fn call_tool(
        &mut self,
        session: &Session,
        binding: &Binding,
        tool: &str,
        args: &Value,
    ) -> Result<WorkerToolResult> {
        let Some(provider) = provider_from_tool_name(tool) else {
            return Err(LocusError::msg(format!(
                "cannot parse provider from tool `{tool}`"
            )));
        };

        let key = Self::worker_key(session, binding, provider);
        let slot = self
            .slots
            .get(&key)
            .ok_or_else(|| {
                LocusError::msg(format!(
                    "no worker slot for provider '{provider}' — call ensure first"
                ))
            })?
            .clone();
        self.touch(&key);

        // Prefer synthetic when the tool is owned by an in-process adapter.
        let synthetic_names: Vec<String> = adapters::tools_for_binding(binding)
            .into_iter()
            .filter(|t| t.provider.eq_ignore_ascii_case(provider))
            .map(|t| t.name)
            .collect();

        if synthetic_names.iter().any(|n| n == tool) {
            return self.synthetic.call_tool(&slot, binding, tool, args);
        }

        if let Some(backend) = self.mcp.get(&key) {
            let upstream_name = strip_provider_prefix(provider, tool);
            return backend.call_tool(&slot, binding, &upstream_name, args);
        }

        // No upstream — fall through to synthetic (may still error unknown tool).
        self.synthetic.call_tool(&slot, binding, tool, args)
    }
}

impl WorkerManager for CompositeWorkerManager {
    fn ensure(
        &mut self,
        session: &Session,
        binding: &Binding,
        provider: &str,
    ) -> Result<WorkerSlot> {
        let key = Self::worker_key(session, binding, provider);
        if let Some(existing) = self.slots.get(&key) {
            if matches!(
                existing.state,
                WorkerState::Ready | WorkerState::Running | WorkerState::Pending
            ) {
                // Reuse live worker — do not respawn upstream children.
                let out = existing.clone();
                self.touch(&key);
                return Ok(out);
            }
            // Stopped / Failed — tear down before recreating.
            let _ = self.teardown(&key);
        }

        let pb = binding.provider(provider).ok_or_else(|| {
            LocusError::msg(format!(
                "provider '{provider}' not present on binding `{}`",
                binding.alias
            ))
        })?;

        let work_dir = if session.is_namespaced() {
            PathBuf::from(&session.worker_home)
                .join("slots")
                .join(&binding.alias)
                .join(provider.to_ascii_lowercase())
        } else {
            PathBuf::from(&session.worker_home)
                .join("slots")
                .join(provider.to_ascii_lowercase())
        };
        std::fs::create_dir_all(&work_dir)?;

        let slot = if let Some(spec) = pb.upstream.as_ref().filter(|u| !u.command.is_empty()) {
            let backend = McpStdioBackend::new(mcp_config_from_upstream(spec));
            let slot = backend.ensure(session, binding, pb, &work_dir)?;
            self.mcp.insert(key.clone(), backend);
            slot
        } else {
            self.synthetic.ensure(session, binding, pb, &work_dir)?
        };

        self.slots.insert(key.clone(), slot.clone());
        self.touch(&key);
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
        self.last_used.remove(key);
        if let Some(slot) = self.slots.remove(key) {
            if let Some(backend) = self.mcp.remove(key) {
                backend.teardown(&slot)?;
            } else {
                self.synthetic.teardown(&slot)?;
            }
        } else if let Some(backend) = self.mcp.remove(key) {
            // Slot already gone — best-effort kill any leftover child map entry.
            let _ = backend;
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

impl Drop for CompositeWorkerManager {
    fn drop(&mut self) {
        let keys: Vec<WorkerKey> = self.slots.keys().cloned().collect();
        for k in keys {
            let _ = self.teardown(&k);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::{BindingBody, Policy, ProviderBinding, Scope};
    use crate::seal::SealKey;
    use crate::session::PinSource;
    use chrono::Duration as ChronoDuration;
    use std::process::Command;
    use std::time::Duration;
    use tempfile::tempdir;

    fn mock_script() -> &'static str {
        r#"
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
        send({"jsonrpc":"2.0","id":mid,"result":{"tools":[
            {"name":"ping","description":"p","inputSchema":{"type":"object"}},
            {"name":"echo","description":"echo text","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}}}
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

    fn binding_mixed(with_upstream: bool) -> Binding {
        let mut gh = ProviderBinding {
            provider: "github".into(),
            account: "acme-gh".into(),
            credential_ref: "phm:GH_ACME".into(),
            scope: Scope {
                orgs: vec!["acme-corp".into()],
                ..Scope::default()
            },
            upstream: None,
        };
        if with_upstream {
            gh.upstream = Some(UpstreamSpec::new("python3").with_args(["-u", "-c", mock_script()]));
        }
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
                gh,
            ],
        })
    }

    fn session_at(worker_home: &str) -> Session {
        let key = SealKey::generate();
        Session::new(
            "bnd_acme",
            "acme",
            "acme-corp",
            None,
            PinSource::Explicit,
            Some("test".into()),
            ChronoDuration::hours(1),
            worker_home.into(),
            &key,
        )
    }

    #[test]
    fn namespace_helpers() {
        assert_eq!(namespace_upstream_tool("GitHub", "ping"), "github.ping");
        assert_eq!(strip_provider_prefix("github", "github.ping"), "ping");
        assert_eq!(strip_provider_prefix("github", "ping"), "ping");
        assert_eq!(provider_from_tool_name("github.ping"), Some("github"));
        assert_eq!(
            provider_from_tool_name("supabase.table.delete"),
            Some("supabase")
        );
    }

    #[test]
    fn composite_synthetic_only() {
        let dir = tempdir().unwrap();
        let worker_home = dir.path().join("worker");
        std::fs::create_dir_all(&worker_home).unwrap();
        let session = session_at(&worker_home.display().to_string());
        let binding = binding_mixed(false);

        let mut mgr = CompositeWorkerManager::new();
        let slots = mgr.ensure_binding(&session, &binding).unwrap();
        assert_eq!(slots.len(), 2);
        assert!(slots.iter().all(|s| s.backend == "synthetic"));

        let tools = mgr.tools_for_pin(&session, &binding);
        assert!(tools.iter().any(|t| t.name == "supabase.scope"));
        assert!(tools.iter().any(|t| t.name == "github.scope"));
        assert!(!tools.iter().any(|t| t.name == "github.ping"));

        let r = mgr
            .call_tool(&session, &binding, "supabase.scope", &serde_json::json!({}))
            .unwrap();
        assert!(r.ok);
    }

    #[test]
    fn composite_upstream_spawn_list_and_call() {
        if Command::new("python3").arg("--version").output().is_err() {
            return;
        }
        let dir = tempdir().unwrap();
        let worker_home = dir.path().join("worker");
        std::fs::create_dir_all(&worker_home).unwrap();
        let session = session_at(&worker_home.display().to_string());
        let binding = binding_mixed(true);

        let mut mgr = CompositeWorkerManager::new();
        let slots = mgr.ensure_binding(&session, &binding).unwrap();
        assert_eq!(slots.len(), 2);

        let gh = slots.iter().find(|s| s.key.provider == "github").unwrap();
        assert_eq!(gh.backend, "mcp_stdio");
        assert_eq!(gh.state, WorkerState::Running);
        assert!(gh.pid.is_some());

        let sb = slots.iter().find(|s| s.key.provider == "supabase").unwrap();
        assert_eq!(sb.backend, "synthetic");

        let tools = mgr.tools_for_pin(&session, &binding);
        assert!(tools.iter().any(|t| t.name == "supabase.scope"));
        assert!(tools.iter().any(|t| t.name == "github.scope")); // synthetic kept
        assert!(tools.iter().any(|t| t.name == "github.ping"));
        assert!(tools.iter().any(|t| t.name == "github.echo"));

        let ping = mgr
            .call_tool(&session, &binding, "github.ping", &serde_json::json!({}))
            .unwrap();
        assert!(ping.ok, "{ping:?}");
        let text = ping
            .content
            .pointer("/content/0/text")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(text, "pong");

        let echo = mgr
            .call_tool(
                &session,
                &binding,
                "github.echo",
                &serde_json::json!({"text": "hello-upstream"}),
            )
            .unwrap();
        assert!(echo.ok);
        let text = echo
            .content
            .pointer("/content/0/text")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(text, "hello-upstream");

        // Synthetic still works alongside upstream
        let scope = mgr
            .call_tool(&session, &binding, "github.scope", &serde_json::json!({}))
            .unwrap();
        assert!(scope.ok);

        mgr.teardown_session(&session.session_id).unwrap();
        assert!(mgr.list().is_empty());
    }

    #[test]
    fn focus_session_tears_down_other() {
        if Command::new("python3").arg("--version").output().is_err() {
            return;
        }
        let dir = tempdir().unwrap();
        let wh1 = dir.path().join("w1");
        let wh2 = dir.path().join("w2");
        std::fs::create_dir_all(&wh1).unwrap();
        std::fs::create_dir_all(&wh2).unwrap();
        let s1 = session_at(&wh1.display().to_string());
        let mut s2 = session_at(&wh2.display().to_string());
        // Force distinct session ids
        s2.session_id = format!("{}-other", s1.session_id);
        let binding = binding_mixed(true);

        let mut mgr = CompositeWorkerManager::new();
        mgr.ensure_binding(&s1, &binding).unwrap();
        assert_eq!(mgr.list().len(), 2);
        mgr.ensure_binding(&s2, &binding).unwrap();
        // Old session torn down
        assert!(mgr
            .list()
            .iter()
            .all(|sl| sl.key.session_id == s2.session_id));
        assert_eq!(mgr.list().len(), 2);
    }

    #[test]
    fn mcp_config_from_upstream_spawns() {
        let spec = UpstreamSpec::new("npx")
            .with_args(["-y", "@pkg"])
            .resolve_secrets(true);
        let cfg = mcp_config_from_upstream(&spec);
        assert!(cfg.spawn);
        assert!(cfg.resolve_secrets);
        assert_eq!(cfg.command, "npx");
        assert_eq!(cfg.args, vec!["-y", "@pkg"]);
    }

    #[test]
    fn ensure_reuses_same_slot_without_respawn() {
        let dir = tempdir().unwrap();
        let worker_home = dir.path().join("worker");
        std::fs::create_dir_all(&worker_home).unwrap();
        let session = session_at(&worker_home.display().to_string());
        let binding = binding_mixed(false);

        let mut mgr = CompositeWorkerManager::new();
        let a = mgr.ensure(&session, &binding, "supabase").unwrap();
        let b = mgr.ensure(&session, &binding, "supabase").unwrap();
        assert_eq!(a.key, b.key);
        assert_eq!(mgr.list().len(), 1);
        // Second ensure is reuse — same work_dir
        assert_eq!(a.work_dir, b.work_dir);
    }

    #[test]
    fn reap_idle_tears_down_after_timeout() {
        let dir = tempdir().unwrap();
        let worker_home = dir.path().join("worker");
        std::fs::create_dir_all(&worker_home).unwrap();
        let session = session_at(&worker_home.display().to_string());
        let binding = binding_mixed(false);

        let mut mgr = CompositeWorkerManager::with_idle_timeout(Some(Duration::from_millis(30)));
        mgr.ensure_binding(&session, &binding).unwrap();
        assert_eq!(mgr.list().len(), 2);
        // Immediate reap: last_used is fresh → keep
        assert_eq!(mgr.reap_idle(Some(Duration::from_secs(60))).unwrap(), 0);
        assert_eq!(mgr.list().len(), 2);
        std::thread::sleep(Duration::from_millis(40));
        let n = mgr.reap_idle(Some(Duration::from_millis(20))).unwrap();
        assert_eq!(n, 2);
        assert!(mgr.list().is_empty());
    }

    #[test]
    fn namespaced_tools_prefix_alias() {
        let dir = tempdir().unwrap();
        let wh = dir.path().join("worker");
        std::fs::create_dir_all(&wh).unwrap();
        let mut session = session_at(&wh.display().to_string());
        session.mode = crate::session::SessionMode::Namespaced;
        session.namespaces = vec!["personal".into()];
        session.namespace_fps = vec!["fp".into()];

        let acme = binding_mixed(false);
        let personal = Binding::from_body(BindingBody {
            id: "bnd_personal".into(),
            alias: "personal".into(),
            tenant: "personal".into(),
            principal: None,
            description: None,
            policy: Policy::default(),
            providers: vec![ProviderBinding {
                provider: "github".into(),
                account: "me".into(),
                credential_ref: "phm:GH_ME".into(),
                scope: Scope::default(),
                upstream: None,
            }],
        });

        let mut mgr = CompositeWorkerManager::new();
        let bindings = vec![("acme".into(), acme), ("personal".into(), personal)];
        // Primary binding_alias on session is acme
        session.binding_alias = "acme".into();
        mgr.ensure_session(&session, &bindings).unwrap();
        let tools = mgr.tools_for_session(&session, &bindings);
        assert!(tools.iter().any(|t| t.name == "acme__supabase.scope"));
        assert!(tools.iter().any(|t| t.name == "acme__github.scope"));
        assert!(tools.iter().any(|t| t.name == "personal__github.scope"));
        // Unprefixed tools must not appear in namespaced mode
        assert!(!tools.iter().any(|t| t.name == "github.scope"));
    }
}

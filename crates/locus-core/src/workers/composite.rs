//! Composite worker manager — routes per provider to synthetic or MCP stdio.
//!
//! Providers with [`crate::binding::UpstreamSpec`] get an [`McpStdioBackend`]
//! with `spawn=true`. All others use [`SyntheticBackend`].

use super::mcp_stdio::{McpStdioBackend, McpStdioConfig};
use super::sandbox::{
    sandbox_enabled_with_env, sandbox_from_env, sandbox_no_network_enabled_with_env,
    sandbox_no_network_from_env,
};
use super::stdio_client::UpstreamTool;
use super::synthetic::SyntheticBackend;
use super::{WorkerBackend, WorkerKey, WorkerManager, WorkerSlot, WorkerState, WorkerToolResult};
use crate::adapters::{self, AdapterTool};
use crate::binding::{Binding, UpstreamSpec};
use crate::error::{LocusError, Result};
use crate::session::Session;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Env var: idle seconds before upstream workers are torn down (0 / unset = never).
pub const ENV_WORKER_IDLE_SECS: &str = "LOCUS_WORKER_IDLE_SECS";

/// Build MCP config from a binding's upstream spec (always spawn).
///
/// Expands built-in `recipe` fields when present. Fails if the recipe is
/// unknown or neither recipe nor command is usable.
///
/// Sandbox is on when `upstream.sandbox = true` **or** `LOCUS_WORKER_SANDBOX=1`.
/// Network deny is on when `upstream.sandbox_no_network = true` **or**
/// `LOCUS_WORKER_SANDBOX_NO_NETWORK=1` (default remains network allowed for MCP).
pub fn mcp_config_from_upstream(spec: &UpstreamSpec) -> Result<McpStdioConfig> {
    mcp_config_from_upstream_with_env(spec, sandbox_from_env(), sandbox_no_network_from_env())
}

fn mcp_config_from_upstream_with_env(
    spec: &UpstreamSpec,
    env_sandbox: bool,
    env_no_network: bool,
) -> Result<McpStdioConfig> {
    let expanded = spec.expand()?;
    let sandbox_incompatibility = expanded
        .recipe
        .as_deref()
        .map(crate::recipes::get_recipe)
        .transpose()?
        .and_then(|recipe| recipe.sandbox_incompatibility());
    Ok(McpStdioConfig {
        command: expanded.command,
        args: expanded.args,
        spawn: true,
        resolve_secrets: expanded.resolve_secrets,
        extra_env: BTreeMap::new(),
        sandbox: sandbox_enabled_with_env(expanded.sandbox.unwrap_or(false), env_sandbox),
        sandbox_no_network: sandbox_no_network_enabled_with_env(
            expanded.sandbox_no_network,
            env_no_network,
        ),
        sandbox_incompatibility,
    })
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
    /// Multi-session mode (multi-tenant server): many sealed sessions are
    /// live concurrently in ONE manager, so `ensure*` must NOT focus-teardown
    /// slots belonging to other sessions (that is single-tenant pin-switch
    /// semantics — it would kill sibling tenants' credential-bearing workers
    /// on every call). Lifecycle is instead driven by explicit per-grant
    /// teardown (DELETE /mcp, dead-grant sweeps, TTL reconcile) + idle reap.
    multi_session: bool,
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
            multi_session: false,
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

    /// Enable multi-session mode (multi-tenant server): `ensure*` keeps other
    /// sessions' workers alive instead of focus-tearing them down. Set once at
    /// startup, before the manager serves requests.
    pub fn set_multi_session(&mut self, multi: bool) {
        self.multi_session = multi;
    }

    pub fn multi_session(&self) -> bool {
        self.multi_session
    }

    /// Single-tenant pin switch: drop workers from other sessions. No-op in
    /// multi-session mode where concurrent sessions are the normal state.
    fn focus_session_unless_multi(&mut self, session_id: &str) -> Result<()> {
        if self.multi_session {
            return Ok(());
        }
        self.focus_session(session_id)
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
        self.focus_session_unless_multi(&session.session_id)?;
        self.ensure_all(session, binding)
    }

    /// Ensure only the provider addressed by one already-authorized tool call.
    /// This prevents an allow decision for one provider from resolving every
    /// other provider credential in the binding.
    pub fn ensure_provider(
        &mut self,
        session: &Session,
        binding: &Binding,
        provider: &str,
    ) -> Result<WorkerSlot> {
        let _ = self.reap_idle_configured()?;
        self.focus_session_unless_multi(&session.session_id)?;
        self.ensure(session, binding, provider)
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
            let Ok(contract) = p.verified_community_adapter() else {
                continue;
            };
            if p.community_frozen_values().is_err() {
                continue;
            }
            let Ok(upstream) = self.list_upstream_tools(session, binding, &p.provider) else {
                continue;
            };
            let prov = p.provider.to_ascii_lowercase();
            for t in upstream {
                let name = namespace_upstream_tool(&prov, &t.name);
                if contract.is_some_and(|manifest| !manifest.entry.tools.contains(&name)) {
                    continue;
                }
                if synthetic_names.contains(&name) {
                    // Prefer synthetic identity/scope tools on name collision.
                    continue;
                }
                let destructive = contract
                    .is_some_and(|manifest| manifest.entry.destructive_tools.contains(&name));
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
                    destructive,
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
        self.focus_session_unless_multi(&session.session_id)?;
        let before = self.slots.keys().cloned().collect::<BTreeSet<_>>();
        let mut out = Vec::new();
        for (_, binding) in bindings {
            match self.ensure_all(session, binding) {
                Ok(slots) => out.extend(slots),
                Err(error) => {
                    self.rollback_new_workers(&before)?;
                    return Err(error);
                }
            }
        }
        Ok(out)
    }

    fn rollback_new_workers(&mut self, before: &BTreeSet<WorkerKey>) -> Result<()> {
        let created = self
            .slots
            .keys()
            .filter(|key| !before.contains(*key))
            .cloned()
            .collect::<Vec<_>>();
        let mut failures = Vec::new();
        for key in created {
            if let Err(error) = self.teardown(&key) {
                failures.push(error.to_string());
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(LocusError::msg(format!(
                "worker startup rollback failed: {}",
                failures.join("; ")
            )))
        }
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

        let pb = binding
            .provider(provider)
            .ok_or_else(|| LocusError::msg("upstream provider absent from binding"))?;
        let scoped_args = pb.community_tool_args(&binding.policy, tool, args)?;
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
            return backend.call_tool(&slot, binding, &upstream_name, &scoped_args);
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
        if let Some(pb) = binding.provider(provider) {
            let community = pb.verified_community_adapter()?.is_some();
            pb.community_frozen_values()?;
            if community
                && pb
                    .upstream
                    .as_ref()
                    .map(|upstream| upstream.expand())
                    .transpose()?
                    .is_some_and(|upstream| upstream.resolve_secrets)
                && (crate::credential::inject_keys_for_binding_provider(pb)?.is_empty()
                    || crate::credential::resolve(&crate::credential::CredentialRef::validate(
                        &pb.credential_ref,
                    )?)?
                    .is_empty())
            {
                return Err(LocusError::msg(
                    "community worker requires a supported available credential mapping",
                ));
            }
        }
        let key = Self::worker_key(session, binding, provider);
        if let Some(existing) = self.slots.get(&key) {
            if matches!(
                existing.state,
                WorkerState::Ready | WorkerState::Running | WorkerState::Pending
            ) {
                // A cached community worker must retain the currently authorized source key.
                if let (Some(backend), Some(pb)) = (self.mcp.get(&key), binding.provider(provider))
                {
                    backend.validate_community_worker_contract(existing, binding, pb)?;
                }
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

        let slot = if let Some(spec) = pb.upstream.as_ref().filter(|u| u.is_declared()) {
            let cfg = mcp_config_from_upstream(spec)?;
            let backend = McpStdioBackend::new(cfg);
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
        let before = self.slots.keys().cloned().collect::<BTreeSet<_>>();
        let mut out = Vec::with_capacity(binding.providers.len());
        for p in &binding.providers {
            match self.ensure(session, binding, &p.provider) {
                Ok(slot) => out.push(slot),
                Err(error) => {
                    self.rollback_new_workers(&before)?;
                    return Err(error);
                }
            }
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
import sys, json, os, pathlib
if len(sys.argv) > 1:
    pathlib.Path(sys.argv[1]).write_text(str(os.getpid()) + "|" + os.environ.get("GH_TOKEN", "missing") + "|" + os.environ.get("SUPABASE_ACCESS_TOKEN", "missing"))
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
    fn partial_multi_provider_startup_rolls_back_earlier_credential_child() {
        if Command::new("python3").arg("--version").output().is_err() {
            return;
        }
        let dir = tempdir().unwrap();
        let worker_home = dir.path().join("worker");
        let marker = dir.path().join("first-provider.txt");
        std::fs::create_dir_all(&worker_home).unwrap();
        let session = session_at(&worker_home.display().to_string());
        std::env::set_var("LOCUS_ROLLBACK_GITHUB", "github-rollback-canary");
        std::env::set_var("LOCUS_ROLLBACK_SUPABASE", "supabase-rollback-canary");
        let binding = Binding::from_body(BindingBody {
            id: "bnd_rollback".into(),
            alias: "rollback".into(),
            tenant: "rollback".into(),
            principal: None,
            description: None,
            policy: Policy::default(),
            providers: vec![
                ProviderBinding {
                    provider: "github".into(),
                    account: "rollback-gh".into(),
                    credential_ref: "env:LOCUS_ROLLBACK_GITHUB".into(),
                    scope: Scope::default(),
                    upstream: Some(
                        UpstreamSpec::new("python3")
                            .with_args(["-u", "-c", mock_script(), marker.to_str().unwrap()])
                            .resolve_secrets(true),
                    ),
                },
                ProviderBinding {
                    provider: "supabase".into(),
                    account: "rollback-db".into(),
                    credential_ref: "env:LOCUS_ROLLBACK_SUPABASE".into(),
                    scope: Scope::default(),
                    upstream: Some(
                        UpstreamSpec::new(
                            dir.path().join("missing-upstream").display().to_string(),
                        )
                        .resolve_secrets(true),
                    ),
                },
            ],
        });

        let mut mgr = CompositeWorkerManager::new();
        let error = mgr.ensure_binding(&session, &binding).unwrap_err();
        assert!(error.to_string().contains("failed to spawn"));
        assert!(
            mgr.list().is_empty(),
            "partial startup retained worker slots"
        );
        let marker_body = std::fs::read_to_string(&marker).unwrap();
        let mut fields = marker_body.split('|');
        let _pid = fields.next().unwrap().to_string();
        assert_eq!(fields.next(), Some("github-rollback-canary"));
        assert_eq!(fields.next(), Some("missing"));
        #[cfg(unix)]
        {
            let mut stopped = false;
            for _ in 0..20 {
                if !Command::new("kill")
                    .args(["-0", &_pid])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .unwrap()
                    .success()
                {
                    stopped = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            assert!(
                stopped,
                "earlier credential-bearing child {_pid} survived rollback"
            );
        }
        std::env::remove_var("LOCUS_ROLLBACK_GITHUB");
        std::env::remove_var("LOCUS_ROLLBACK_SUPABASE");
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
    fn multi_session_ensure_keeps_other_sessions_workers() {
        // Multi-tenant server: tenant A's tools/call must never tear down
        // tenant B's live workers (each grant has its own session_id but
        // shares the singleton manager). Synthetic-only — no child spawn.
        let dir = tempdir().unwrap();
        let wh1 = dir.path().join("w1");
        let wh2 = dir.path().join("w2");
        std::fs::create_dir_all(&wh1).unwrap();
        std::fs::create_dir_all(&wh2).unwrap();
        let s1 = session_at(&wh1.display().to_string());
        let mut s2 = session_at(&wh2.display().to_string());
        s2.session_id = format!("{}-other", s1.session_id);
        let binding = binding_mixed(false);

        let mut mgr = CompositeWorkerManager::new();
        mgr.set_multi_session(true);
        mgr.ensure_binding(&s1, &binding).unwrap();
        assert_eq!(mgr.list().len(), 2);
        // Alternating tenants: neither ensure_binding nor ensure_provider
        // evicts the sibling session's slots.
        mgr.ensure_binding(&s2, &binding).unwrap();
        assert_eq!(mgr.list().len(), 4, "both sessions' slots stay live");
        mgr.ensure_provider(&s1, &binding, "github").unwrap();
        assert_eq!(mgr.list().len(), 4, "ensure_provider must not evict");
        assert!(mgr
            .list()
            .iter()
            .any(|sl| sl.key.session_id == s1.session_id));
        assert!(mgr
            .list()
            .iter()
            .any(|sl| sl.key.session_id == s2.session_id));
        // Explicit per-session teardown still works (grant death path).
        mgr.teardown_session(&s1.session_id).unwrap();
        assert!(mgr
            .list()
            .iter()
            .all(|sl| sl.key.session_id == s2.session_id));
    }

    #[test]
    fn mcp_config_from_upstream_spawns() {
        let spec = UpstreamSpec::new("npx")
            .with_args(["-y", "@pkg"])
            .resolve_secrets(true);
        let cfg = mcp_config_from_upstream(&spec).unwrap();
        assert!(cfg.spawn);
        assert!(cfg.resolve_secrets);
        assert_eq!(cfg.command, "npx");
        assert_eq!(cfg.args, vec!["-y", "@pkg"]);
        // sandbox may be true if LOCUS_WORKER_SANDBOX is set in the process env
        assert_eq!(cfg.sandbox, crate::workers::sandbox_from_env());
    }

    #[test]
    fn mcp_config_sandbox_from_spec_or_env() {
        // Spec flag forces sandbox on regardless of env.
        let spec = UpstreamSpec::new("npx").sandbox(true);
        let cfg = mcp_config_from_upstream_with_env(&spec, false, false).unwrap();
        assert!(cfg.sandbox);
        assert!(!cfg.sandbox_no_network);

        // Env-only path is injected so parallel tests do not mutate process state.
        let cfg2 = mcp_config_from_upstream_with_env(
            &UpstreamSpec::new("false-cmd-for-test"),
            true,
            false,
        )
        .unwrap();
        assert!(cfg2.sandbox);
        let cfg3 =
            mcp_config_from_upstream_with_env(&UpstreamSpec::new("npx"), false, false).unwrap();
        assert!(!cfg3.sandbox);
    }

    #[test]
    fn mcp_config_no_network_from_spec_or_env() {
        let base = UpstreamSpec::new("npx").sandbox(true);
        let cfg = mcp_config_from_upstream_with_env(&base, false, false).unwrap();
        assert!(!cfg.sandbox_no_network);

        let cfg_spec =
            mcp_config_from_upstream_with_env(&base.clone().sandbox_no_network(true), false, false)
                .unwrap();
        assert!(cfg_spec.sandbox_no_network);

        let cfg_env = mcp_config_from_upstream_with_env(&base, false, true).unwrap();
        assert!(cfg_env.sandbox_no_network);
    }

    #[test]
    fn mcp_config_from_recipe_expands() {
        let spec = UpstreamSpec::from_recipe("everything-mcp");
        let cfg = mcp_config_from_upstream(&spec).unwrap();
        assert_eq!(cfg.command, "npx");
        assert!(cfg.args.iter().any(|a| a.contains("server-everything")));
        assert!(cfg.spawn);
    }

    /// Top-adapter recipes expand through the composite config path with the
    /// sandbox / secret defaults operators get from pure-recipe bindings.
    /// Does not spawn real npm packages (offline-safe).
    #[test]
    fn top_adapter_recipes_expand_sandbox_defaults_for_composite() {
        // Sandbox-compatible stdio recipes — pure expand adopts both defaults.
        for (id, package_needle) in [
            ("github-mcp", "@modelcontextprotocol/server-github"),
            ("supabase-mcp", "@supabase/mcp-server-supabase"),
        ] {
            let cfg =
                mcp_config_from_upstream_with_env(&UpstreamSpec::from_recipe(id), false, false)
                    .unwrap_or_else(|e| panic!("{id} expand failed: {e}"));
            assert_eq!(cfg.command, "npx", "{id}");
            assert!(
                cfg.args.iter().any(|a| a.contains(package_needle)),
                "{id} args missing well-known package {package_needle}: {:?}",
                cfg.args
            );
            assert!(cfg.spawn, "{id}");
            assert!(
                cfg.resolve_secrets,
                "{id} pure-recipe must default resolve_secrets"
            );
            assert!(cfg.sandbox, "{id} pure-recipe must default sandbox");
            assert!(
                !cfg.sandbox_no_network,
                "{id} must default to network allowed for MCP"
            );
            assert!(
                cfg.sandbox_incompatibility.is_none(),
                "{id} must be sandbox-compatible"
            );
        }

        // Vercel remote bridge is intentionally unavailable until explicit
        // sandbox = false; OAuth defaults leave resolve_secrets off.
        assert!(
            mcp_config_from_upstream_with_env(
                &UpstreamSpec::from_recipe("vercel-mcp"),
                false,
                false
            )
            .is_err(),
            "vercel-mcp must fail closed without sandbox = false"
        );
        let vercel = mcp_config_from_upstream_with_env(
            &UpstreamSpec::from_recipe("vercel-mcp").sandbox(false),
            false,
            false,
        )
        .expect("vercel-mcp with sandbox=false");
        assert_eq!(vercel.command, "npx");
        assert_eq!(
            vercel.args,
            vec![
                "-y".to_string(),
                "mcp-remote".to_string(),
                "https://mcp.vercel.com".to_string()
            ]
        );
        assert!(vercel.spawn);
        assert!(
            !vercel.resolve_secrets,
            "vercel OAuth bridge must not default resolve_secrets"
        );
        assert!(!vercel.sandbox);
        assert!(vercel.sandbox_incompatibility.is_some());
    }

    /// Recipe-based upstream keeps the exclusive synthetic catalog (scope /
    /// freeze tools) and never falls through to ambient provider tools.
    #[test]
    fn recipe_upstream_keeps_exclusive_synthetic_catalog_and_freeze() {
        if Command::new("python3").arg("--version").output().is_err() {
            return;
        }
        let dir = tempdir().unwrap();
        let worker_home = dir.path().join("worker");
        std::fs::create_dir_all(&worker_home).unwrap();
        let session = session_at(&worker_home.display().to_string());

        // github uses a recipe id for documentation/config path, but we override
        // command/args with the offline mock so tests do not fetch npm packages.
        // Command/args override must not strip synthetic freeze tools.
        let mut gh_upstream =
            UpstreamSpec::from_recipe("github-mcp").with_args(["-u", "-c", mock_script()]);
        gh_upstream.command = "python3".into();
        gh_upstream.resolve_secrets = false;
        gh_upstream.sandbox = Some(false);

        let binding = Binding::from_body(BindingBody {
            id: "bnd_acme".into(),
            alias: "acme".into(),
            tenant: "acme-corp".into(),
            principal: None,
            description: None,
            policy: Policy::default(),
            providers: vec![
                ProviderBinding {
                    provider: "github".into(),
                    account: "acme-gh".into(),
                    credential_ref: "phm:GH_ACME".into(),
                    scope: Scope {
                        orgs: vec!["acme-corp".into()],
                        ..Scope::default()
                    },
                    upstream: Some(gh_upstream),
                },
                ProviderBinding {
                    provider: "supabase".into(),
                    account: "acme".into(),
                    credential_ref: "phm:SUPABASE_ACME".into(),
                    scope: Scope {
                        project_ref: Some("proj_acme".into()),
                        ..Scope::default()
                    },
                    upstream: None, // synthetic only
                },
            ],
        });

        let mut mgr = CompositeWorkerManager::new();
        // ensure_provider starts only the addressed upstream — exclusive credential boundary.
        let gh_slot = mgr
            .ensure_provider(&session, &binding, "github")
            .expect("github recipe-shaped upstream");
        assert_eq!(gh_slot.backend, "mcp_stdio");
        assert_eq!(gh_slot.state, WorkerState::Running);
        assert!(
            mgr.list().iter().all(|s| s.key.provider == "github"),
            "ensure_provider must not start sibling providers: {:?}",
            mgr.list()
                .iter()
                .map(|s| s.key.provider.as_str())
                .collect::<Vec<_>>()
        );

        // Synthetic schemas remain exclusive to this pin; upstream tools merge under provider.
        let tools = mgr.tools_for_pin(&session, &binding);
        assert!(
            tools.iter().any(|t| t.name == "github.scope"),
            "synthetic github.scope must remain in exclusive catalog"
        );
        assert!(
            tools.iter().any(|t| t.name == "supabase.scope"),
            "synthetic supabase.scope must remain in exclusive catalog"
        );
        assert!(
            tools.iter().any(|t| t.name == "github.ping"),
            "live upstream tools merge under provider namespace"
        );
        assert!(!tools.iter().any(|t| t.name.starts_with("personal__")));
        assert!(!tools.iter().any(|t| t.name.contains("evil")));

        // Start supabase synthetic only; freeze remains exclusive to this pin.
        let sb = mgr
            .ensure_provider(&session, &binding, "supabase")
            .expect("supabase synthetic");
        assert_eq!(sb.backend, "synthetic");

        let scope_ok = mgr
            .call_tool(&session, &binding, "github.scope", &serde_json::json!({}))
            .expect("synthetic github.scope alongside upstream");
        assert!(scope_ok.ok);
        let scope_body = serde_json::to_string(&scope_ok.content).unwrap();
        assert!(
            scope_body.contains("acme-gh") || scope_body.contains("acme-corp"),
            "exclusive pin identity missing from scope: {scope_body}"
        );
        assert!(!scope_body.contains("phm:GH_ACME"));

        let freeze = mgr
            .call_tool(
                &session,
                &binding,
                "supabase.scope",
                &serde_json::json!({ "project_ref": "proj_evil" }),
            )
            .expect_err("scope freeze must deny alternate project_ref");
        let msg = freeze.to_string();
        assert!(
            msg.contains("scope freeze")
                || msg.contains("proj_evil")
                || msg.contains("project_ref"),
            "unexpected freeze error: {msg}"
        );

        mgr.teardown_session(&session.session_id).unwrap();
    }

    /// When resolve_secrets is false, ambient provider secrets and parent env
    /// canaries must not appear in the MCP child environment (recipe or explicit).
    #[test]
    fn recipe_config_resolve_secrets_false_excludes_ambient_provider_secrets() {
        let dir = tempdir().unwrap();
        let worker_home = dir.path().join("worker");
        let work_dir = worker_home.join("slots/github");
        std::fs::create_dir_all(&work_dir).unwrap();
        let session = session_at(&worker_home.display().to_string());
        let binding = Binding::from_body(BindingBody {
            id: "bnd_acme".into(),
            alias: "acme".into(),
            tenant: "acme-corp".into(),
            principal: None,
            description: None,
            policy: Policy::default(),
            providers: vec![ProviderBinding {
                provider: "github".into(),
                account: "acme-gh".into(),
                credential_ref: "env:LOCUS_AMBIENT_GH_CANARY".into(),
                scope: Scope {
                    orgs: vec!["acme-corp".into()],
                    ..Scope::default()
                },
                // Non-pure recipe override keeps resolve_secrets as written
                // (pure recipe ORs default_resolve_secrets and cannot opt out alone).
                upstream: Some(UpstreamSpec {
                    recipe: Some("github-mcp".into()),
                    command: "npx".into(),
                    args: vec!["-y".into(), "@modelcontextprotocol/server-github".into()],
                    resolve_secrets: false,
                    sandbox: Some(false),
                    sandbox_no_network: false,
                    community_adapter: None,
                }),
            }],
        });
        let provider = binding.provider("github").unwrap();

        std::env::set_var("LOCUS_AMBIENT_GH_CANARY", "should-never-reach-child");
        std::env::set_var("GH_TOKEN", "ambient-gh-token-leak");
        std::env::set_var("GITHUB_PERSONAL_ACCESS_TOKEN", "ambient-pat-leak");
        std::env::set_var("SUPABASE_ACCESS_TOKEN", "ambient-supabase-leak");

        let cfg = mcp_config_from_upstream_with_env(
            binding.providers[0].upstream.as_ref().unwrap(),
            false,
            false,
        )
        .unwrap();
        assert!(!cfg.resolve_secrets);
        assert!(!cfg.sandbox);

        let backend = McpStdioBackend::new(cfg);
        let command = backend
            .build_command(&session, &binding, provider, &work_dir)
            .unwrap();
        let env: BTreeMap<String, String> = command
            .get_envs()
            .filter_map(|(k, v)| {
                v.map(|v| {
                    (
                        k.to_string_lossy().into_owned(),
                        v.to_string_lossy().into_owned(),
                    )
                })
            })
            .collect();

        assert!(!env.contains_key("GH_TOKEN"), "ambient GH_TOKEN leaked");
        assert!(
            !env.contains_key("GITHUB_PERSONAL_ACCESS_TOKEN"),
            "ambient GITHUB_PERSONAL_ACCESS_TOKEN leaked"
        );
        assert!(
            !env.contains_key("SUPABASE_ACCESS_TOKEN"),
            "ambient SUPABASE_ACCESS_TOKEN leaked"
        );
        assert!(
            !env.values().any(|v| v.contains("should-never-reach-child")),
            "credential canary leaked into child env"
        );
        assert!(
            !env.values().any(|v| {
                v.contains("ambient-gh-token-leak")
                    || v.contains("ambient-pat-leak")
                    || v.contains("ambient-supabase-leak")
            }),
            "ambient secret values leaked into child"
        );
        assert_eq!(
            env.get("LOCUS_GITHUB_CREDENTIAL_RESOLVED")
                .map(String::as_str),
            Some("0"),
            "credential must stay unresolved when resolve_secrets=false"
        );
        // Frozen identity still present (exclusive pin surface).
        assert_eq!(env.get("LOCUS_BINDING").map(String::as_str), Some("acme"));
        assert_eq!(
            env.get("LOCUS_GITHUB_ACCOUNT").map(String::as_str),
            Some("acme-gh")
        );

        std::env::remove_var("LOCUS_AMBIENT_GH_CANARY");
        std::env::remove_var("GH_TOKEN");
        std::env::remove_var("GITHUB_PERSONAL_ACCESS_TOKEN");
        std::env::remove_var("SUPABASE_ACCESS_TOKEN");
    }

    #[test]
    fn incompatible_recipes_never_become_false_sandbox_claims() {
        let dir = tempdir().unwrap();
        let worker_home = dir.path().join("locus-home/workers/sess_test");
        let work_dir = worker_home.join("slots/github");
        std::fs::create_dir_all(&work_dir).unwrap();
        let session = session_at(&worker_home.display().to_string());
        let binding = binding_mixed(false);
        let provider = binding.provider("github").unwrap();

        for id in ["github-official", "vercel-mcp"] {
            let omitted = UpstreamSpec::from_recipe(id);
            assert!(mcp_config_from_upstream(&omitted).is_err());

            let requested_sandbox = UpstreamSpec::from_recipe(id).sandbox(true);
            assert!(mcp_config_from_upstream(&requested_sandbox).is_err());

            let acknowledged = UpstreamSpec::from_recipe(id).sandbox(false);
            let mut cfg = mcp_config_from_upstream(&acknowledged).unwrap();
            assert!(!cfg.sandbox, "{id} must remain explicitly unsandboxed");
            assert!(cfg.sandbox_incompatibility.is_some());

            // This fixture tests sandbox compatibility independently of
            // credential admission; no provider credential is needed here.
            cfg.resolve_secrets = false;
            // Model a later global force without mutating process-global env.
            // The spawn layer must fail before resolving or launching the child.
            cfg.sandbox = true;
            let backend = McpStdioBackend::new(cfg);
            let err = backend
                .build_command(&session, &binding, provider, &work_dir)
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("cannot run in the worker sandbox"),
                "{id}: {err}"
            );
        }
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

#[cfg(test)]
pub(crate) mod community_runtime_tests {
    use super::*;
    use crate::adapter_registry::{
        ed25519_public_key_b64, sign_entry_material_ed25519, AdapterManifestEntry,
        LOCUS_ADAPTER_TRUST_KEYS_ENV,
    };
    use crate::binding::{BindingBody, Policy, ProviderBinding, Scope};
    use crate::marketplace::CommunityAdapterManifest;
    use crate::seal::SealKey;
    use crate::session::PinSource;
    use chrono::Duration as ChronoDuration;
    use ed25519_dalek::SigningKey;
    use serde_json::json;
    use std::ffi::OsString;

    struct Environment(Vec<(String, Option<OsString>)>);
    impl Environment {
        fn set(&mut self, key: &str, value: impl AsRef<std::ffi::OsStr>) {
            self.0.push((key.into(), std::env::var_os(key)));
            std::env::set_var(key, value);
        }
    }
    impl Drop for Environment {
        fn drop(&mut self) {
            for (key, value) in self.0.drain(..).rev() {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    fn sign(manifest: &mut CommunityAdapterManifest, signing: &SigningKey) {
        manifest.entry.signature = Some(sign_entry_material_ed25519(
            &manifest.signing_material(),
            signing,
        ));
    }

    pub(crate) fn fixture(
        provider: &str,
        marker: &std::path::Path,
        signing: &SigningKey,
    ) -> ProviderBinding {
        let script = r#"import sys,json,os,pathlib,time
marker=pathlib.Path(sys.argv[1])
with marker.open('a') as f: f.write(json.dumps({'started':True,'home':os.environ.get('HOME'),'linear': 'LINEAR_API_KEY' in os.environ, 'notion':'NOTION_API_KEY' in os.environ,'ambient':'GH_TOKEN' in os.environ,'control':'LOCUS_CONTROL_CAPABILITY' in os.environ,'locator':'LOCUS_COMMUNITY_LINEAR_FIXTURE' in os.environ,'config':os.environ.get('GH_CONFIG_DIR'),'aws':os.environ.get('AWS_CONFIG_FILE'),'tmp':os.environ.get('TMPDIR'),'binding':os.environ.get('LOCUS_WORKER_BINDING'),'tenant':os.environ.get('LOCUS_WORKER_TENANT'),'parent_binding':os.environ.get('LOCUS_BINDING')})+'\n')
def send(result,mid):
 print(json.dumps({'jsonrpc':'2.0','id':mid,'result':result}),flush=True)
for line in sys.stdin:
 msg=json.loads(line);mid=msg.get('id');method=msg.get('method')
 if mid is None:continue
 if method=='initialize':send({'protocolVersion':'2024-11-05','capabilities':{'tools':{}},'serverInfo':{'name':'inert','version':'1'}},mid)
 elif method=='tools/list':send({'tools':[{'name':n,'description':os.environ.get('LINEAR_API_KEY',os.environ.get('NOTION_API_KEY','none')),'inputSchema':{'type':'object'}} for n in ['read','delete','extra']]},mid)
 elif method=='tools/call':
  if msg['params'].get('arguments',{}).get('barrier'):
   pathlib.Path(str(marker)+'.ready').write_text('ready')
   while not pathlib.Path(str(marker)+'.release').exists():time.sleep(.005)
  with marker.open('a') as f:f.write(json.dumps({'call':msg['params']['name'],'args':msg['params'].get('arguments',{})})+'\n')
  send({'content':[{'type':'text','text':'inert-ok '+os.environ.get('LINEAR_API_KEY',os.environ.get('NOTION_API_KEY','none'))}],'isError':False},mid)
"#;
        let mut manifest = CommunityAdapterManifest {
            manifest_version: 2,
            entry: AdapterManifestEntry {
                id: provider.into(),
                name: "inert fixture".into(),
                status: "community".into(),
                synthetic: false,
                capabilities: vec![],
                frozen_selectors: vec!["workspace".into()],
                tools: vec![format!("{provider}.read"), format!("{provider}.delete")],
                destructive_tools: vec![format!("{provider}.delete")],
                description: String::new(),
                signature: None,
                signed_by: Some("community-runtime-fixture".into()),
            },
            upstream: Some(
                UpstreamSpec::new("python3")
                    .with_args(["-u", "-c", script, marker.to_str().unwrap()])
                    .resolve_secrets(true)
                    .sandbox(false),
            ),
            credential_env: Some(format!("{}_API_KEY", provider.to_ascii_uppercase())),
            publisher: "synthetic".into(),
            version: "1".into(),
        };
        sign(&mut manifest, signing);
        ProviderBinding::new(
            provider,
            "fixture-account",
            format!(
                "env:LOCUS_COMMUNITY_{}_FIXTURE",
                provider.to_ascii_uppercase()
            ),
        )
        .with_scope(Scope {
            extra: [(
                "workspace".into(),
                toml::Value::String(format!("tenant-{provider}")),
            )]
            .into(),
            ..Scope::default()
        })
        .with_community_adapter(manifest)
        .unwrap()
    }

    fn records(path: &std::path::Path) -> Vec<Value> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    /// Actual inert MCP children prove the contract at launch and at both dispatch seams.
    #[test]
    fn community_signed_runtime_contract() {
        assert!(std::process::Command::new("python3")
            .arg("--version")
            .output()
            .unwrap()
            .status
            .success());
        let dir = tempfile::tempdir().unwrap();
        let worker_home = dir.path().join("worker");
        std::fs::create_dir_all(&worker_home).unwrap();
        let mut environment = Environment(vec![]);
        let signing = SigningKey::from_bytes(&[19; 32]);
        let trust = format!(
            "community-runtime-fixture:ed25519:{}",
            ed25519_public_key_b64(&signing.verifying_key())
        );
        environment.set(LOCUS_ADAPTER_TRUST_KEYS_ENV, &trust);
        environment.set("LOCUS_HOME", dir.path());
        environment.set("LOCUS_WORKER_SANDBOX", "0");
        environment.set("LOCUS_WORKER_SANDBOX_NO_NETWORK", "0");
        environment.set("LOCUS_COMMUNITY_LINEAR_FIXTURE", "synthetic-linear-private");
        environment.set("LOCUS_COMMUNITY_NOTION_FIXTURE", "synthetic-notion-private");
        environment.set("GH_TOKEN", "synthetic-unrelated-ambient");
        environment.set(
            "LOCUS_CONTROL_CAPABILITY",
            "synthetic-control-never-delegate",
        );
        let linear_marker = dir.path().join("linear.jsonl");
        let notion_marker = dir.path().join("notion.jsonl");
        let linear = fixture("linear", &linear_marker, &signing);
        let notion = fixture("notion", &notion_marker, &signing);
        let binding = Binding::from_body(BindingBody {
            id: "bnd_acme".into(),
            alias: "acme".into(),
            tenant: "fixture".into(),
            principal: None,
            description: None,
            policy: Policy::default(),
            providers: vec![linear.clone(), notion],
        });
        let session = Session::new(
            "bnd_acme",
            "acme",
            "fixture",
            None,
            PinSource::Explicit,
            Some("fixture".into()),
            ChronoDuration::hours(1),
            worker_home.display().to_string(),
            &SealKey::generate(),
        );
        let mut manager = CompositeWorkerManager::new();
        // Invalid requests refuse before a slot exists, with no credential-bearing child.
        assert!(manager
            .call_tool(&session, &binding, "linear.extra", &json!({}))
            .is_err());
        let mut readonly = binding.clone();
        readonly.providers[0].scope.read_only = Some(true);
        assert!(manager
            .call_tool(&session, &readonly, "linear.delete", &json!({}))
            .is_err());
        assert!(manager
            .call_tool(
                &session,
                &binding,
                "linear.read",
                &json!({"nested":[{"workspace":"wrong"}]})
            )
            .is_err());
        assert!(!linear_marker.exists());
        manager.ensure_binding(&session, &binding).unwrap();
        let l = &records(&linear_marker)[0];
        let n = &records(&notion_marker)[0];
        assert_eq!(
            l["home"],
            worker_home.join("slots/linear").display().to_string()
        );
        assert_eq!(
            n["home"],
            worker_home.join("slots/notion").display().to_string()
        );
        assert_ne!(l["home"], n["home"]);
        assert_eq!(l["linear"], true);
        assert_eq!(l["notion"], false);
        assert_eq!(n["notion"], true);
        assert_eq!(n["linear"], false);
        for record in [l, n] {
            for field in ["ambient", "control", "locator"] {
                assert_eq!(record[field], false);
            }
        }
        let catalog = manager.tools_for_pin(&session, &binding);
        assert!(catalog
            .iter()
            .filter(|tool| tool.name == "linear.read" || tool.name == "notion.read")
            .all(|tool| tool.description == "[redacted]"));
        assert!(!catalog.iter().any(|tool| tool.name == "linear.extra"));
        assert!(
            catalog
                .iter()
                .find(|tool| tool.name == "linear.delete")
                .unwrap()
                .destructive
        );
        let result = manager
            .call_tool(&session, &binding, "linear.read", &json!({}))
            .unwrap();
        assert!(result.ok);
        let encoded = serde_json::to_string(&result.content).unwrap();
        assert!(!encoded.contains("synthetic-linear-private"));
        assert!(encoded.contains("[redacted]"));
        assert_eq!(
            records(&linear_marker)[1]["args"]["workspace"],
            "tenant-linear"
        );
        let count = records(&linear_marker).len();
        assert!(manager
            .call_tool(&session, &readonly, "linear.delete", &json!({}))
            .is_err());
        let mut gated = binding.clone();
        gated.policy.require_approval.push("linear.*".into());
        assert!(manager
            .call_tool(&session, &gated, "linear.read", &json!({}))
            .is_err());
        assert!(manager
            .call_tool(&session, &binding, "linear.extra", &json!({}))
            .is_err());
        assert!(manager
            .call_tool(&session, &binding, "linear.read", &json!({"workspace":9}))
            .is_err());
        assert_eq!(records(&linear_marker).len(), count);
        // The backend itself applies policy before sending, even when bypassing Composite.
        let key = CompositeWorkerManager::worker_key(&session, &binding, "linear");
        let backend = manager.mcp.get(&key).unwrap();
        let slot = manager.slots.get(&key).unwrap();
        assert!(backend
            .call_tool(slot, &binding, "extra", &json!({}))
            .is_err());
        assert!(backend
            .call_tool(slot, &readonly, "delete", &json!({}))
            .is_err());
        assert!(backend
            .call_tool(
                slot,
                &binding,
                "read",
                &json!({"nested":{"workspace":"other"}})
            )
            .is_err());
        assert_eq!(records(&linear_marker).len(), count);
        // Even a valid new signature cannot relabel an already launched child.
        // Changed executable args and credential target both require a restart.
        for change_mapping in [false, true] {
            let mut envelope = linear
                .upstream
                .as_ref()
                .unwrap()
                .community_adapter
                .as_ref()
                .unwrap()
                .as_ref()
                .clone();
            if change_mapping {
                envelope.credential_env = Some("LINEAR_TOKEN".into());
            } else {
                envelope
                    .upstream
                    .as_mut()
                    .unwrap()
                    .args
                    .push("new-contract".into());
            }
            sign(&mut envelope, &signing);
            let mut changed = binding.clone();
            changed.providers[0] = linear.clone().with_community_adapter(envelope).unwrap();
            assert!(manager
                .call_tool(&session, &changed, "linear.read", &json!({}))
                .is_err());
            assert!(manager.ensure(&session, &changed, "linear").is_err());
        }
        assert_eq!(records(&linear_marker).len(), count);
        let mut unsigned = binding.clone();
        unsigned.providers[0]
            .upstream
            .as_mut()
            .unwrap()
            .community_adapter = None;
        assert!(manager
            .call_tool(&session, &unsigned, "linear.extra", &json!({}))
            .is_err());
        assert!(manager.ensure(&session, &unsigned, "linear").is_err());
        assert_eq!(records(&linear_marker).len(), count);
        let config = mcp_config_from_upstream(linear.upstream.as_ref().unwrap()).unwrap();
        let mut altered_config = config.clone();
        altered_config.command = "inert-unsigned-command".into();
        assert!(McpStdioBackend::new(altered_config)
            .build_command(&session, &binding, &linear, &worker_home)
            .is_err());
        // Missing or unsupported credentials refuse Command construction before launch.
        let mut missing = linear.clone();
        missing.credential_ref = "env:LOCUS_COMMUNITY_MISSING_FIXTURE".into();
        assert!(McpStdioBackend::new(config.clone())
            .build_command(&session, &binding, &missing, &worker_home)
            .is_err());
        let mut unsupported = linear.clone();
        unsupported.credential_ref = "phm:UNSUPPORTED_FIXTURE".into();
        assert!(McpStdioBackend::new(config.clone())
            .build_command(&session, &binding, &unsupported, &worker_home)
            .is_err());
        for source in [
            "LOCUS_CONTROL_CAPABILITY",
            "LOCUS_EXECUTOR_CAPABILITY",
            "LOCUS_SEAL",
            "locus_control_capability",
            "Locus_Executor_Capability",
            "locus_seal",
            "LOCUS_ADAPTER_TRUST_KEYS",
            "Locus_Adapter_Trust_Keys",
            "LOCUS_REGISTRY_SIGNING_KEY",
            "locus_registry_signing_key",
        ] {
            let canary = if source.eq_ignore_ascii_case(LOCUS_ADAPTER_TRUST_KEYS_ENV) {
                // Keep the actual synthetic publisher trusted, so rejection
                // proves source exclusion rather than an invalid trust overlay.
                format!(
                    "community-runtime-fixture:ed25519:{}",
                    ed25519_public_key_b64(&signing.verifying_key())
                )
            } else {
                format!("synthetic-authority-canary-{source}")
            };
            environment.set(source, &canary);
            let mut authority_source = linear.clone();
            authority_source.credential_ref = format!("env:{source}");
            let error = McpStdioBackend::new(config.clone())
                .build_command(&session, &binding, &authority_source, &worker_home)
                .unwrap_err();
            assert!(error.to_string().contains("credential resolution failed"));
            assert!(!error.to_string().contains(&canary));
        }
        let mut bad_key = linear.clone();
        let envelope = bad_key
            .upstream
            .as_mut()
            .unwrap()
            .community_adapter
            .as_mut()
            .unwrap();
        envelope.credential_env = Some("GH_TOKEN".into());
        sign(envelope, &signing);
        assert!(McpStdioBackend::new(config.clone())
            .build_command(&session, &binding, &bad_key, &worker_home)
            .is_err());
        let mut unknown_scope = linear.clone();
        unknown_scope.scope.extra.clear();
        assert!(McpStdioBackend::new(config.clone())
            .build_command(&session, &binding, &unknown_scope, &worker_home)
            .is_err());
        assert_eq!(records(&linear_marker).len(), count);
        environment.set("LOCUS_COMMUNITY_LINEAR_FIXTURE", "synthetic-rotated-key");
        assert!(manager
            .call_tool(&session, &binding, "linear.read", &json!({}))
            .is_err());
        assert!(manager.ensure(&session, &binding, "linear").is_err());
        assert_eq!(records(&linear_marker).len(), count);
        environment.set("LOCUS_COMMUNITY_LINEAR_FIXTURE", "synthetic-linear-private");

        // A new concrete scope or operator identity cannot relabel a live child,
        // even with unchanged binding IDs, signed envelope and credential source.
        for context in ["scope", "tenant", "principal", "policy"] {
            let mut changed = binding.clone();
            match context {
                "scope" => {
                    changed.providers[0].scope.extra.insert(
                        "workspace".into(),
                        toml::Value::String("other-workspace".into()),
                    );
                }
                "tenant" => changed.tenant = "other-tenant".into(),
                "principal" => changed.principal = Some("other-principal".into()),
                "policy" => changed.policy.require_approval.push("linear.delete".into()),
                _ => unreachable!(),
            }
            assert!(
                manager
                    .call_tool(&session, &changed, "linear.read", &json!({}))
                    .is_err(),
                "changed {context} reached the cached worker"
            );
            let backend = manager.mcp.get(&key).unwrap();
            let slot = manager.slots.get(&key).unwrap();
            assert!(backend
                .call_tool(slot, &changed, "read", &json!({}))
                .is_err());
            assert!(manager.ensure(&session, &changed, "linear").is_err());
            assert_eq!(records(&linear_marker).len(), count);
        }
        let started = &records(&linear_marker)[0];
        assert_eq!(started["binding"], binding.alias);
        assert_eq!(started["tenant"], binding.tenant);
        assert_eq!(started["parent_binding"], session.binding_alias);

        // Namespaced bindings of the same provider have disjoint private config roots.
        let second_marker = dir.path().join("other-linear.jsonl");
        let second = fixture("linear", &second_marker, &signing);
        let mut other = binding.clone();
        other.id = "bnd_other".into();
        other.alias = "other".into();
        other.tenant = "other-tenant".into();
        other.providers = vec![second];
        let namespaced = session
            .clone()
            .with_mode(crate::session::SessionMode::Namespaced)
            .with_namespaces(vec!["other".into()], vec!["synthetic".into()]);
        let mut multi = CompositeWorkerManager::new();
        multi.ensure_binding(&namespaced, &binding).unwrap();
        multi.ensure_binding(&namespaced, &other).unwrap();
        let primary = records(&linear_marker).last().unwrap().clone();
        let secondary = records(&second_marker)[0].clone();
        assert_ne!(primary["home"], secondary["home"]);
        for record in [&primary, &secondary] {
            let home = record["home"].as_str().unwrap();
            assert!(record["config"].as_str().unwrap().starts_with(home));
            assert!(record["aws"].as_str().unwrap().starts_with(home));
            assert!(record["tmp"].as_str().unwrap().starts_with(home));
            assert_eq!(record["parent_binding"], "acme");
        }
        assert_eq!(primary["binding"], "acme");
        assert_eq!(secondary["binding"], "other");
        assert_eq!(secondary["tenant"], "other-tenant");
        drop(multi);
        let count = records(&linear_marker).len();
        let mut wrong_binding = binding.clone();
        wrong_binding.id = "bnd_other".into();
        wrong_binding.alias = "other".into();
        let backend = manager.mcp.get(&key).unwrap();
        let slot = manager.slots.get(&key).unwrap();
        assert!(backend
            .call_tool(slot, &wrong_binding, "read", &json!({}))
            .is_err());
        let mut wrong_account = binding.clone();
        wrong_account.providers[0].account = "other-account".into();
        assert!(backend
            .call_tool(slot, &wrong_account, "read", &json!({}))
            .is_err());
        assert_eq!(records(&linear_marker).len(), count);
        // Current trust revocation denies cached reuse and direct dispatch, without a replay.
        environment.set(LOCUS_ADAPTER_TRUST_KEYS_ENV, "");
        assert!(manager
            .call_tool(&session, &binding, "linear.read", &json!({}))
            .is_err());
        assert!(manager.ensure(&session, &binding, "linear").is_err());
        assert_eq!(records(&linear_marker).len(), count);
        drop(manager);
    }
}

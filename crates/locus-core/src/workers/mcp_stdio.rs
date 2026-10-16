//! MCP stdio child-process backend with JSON-RPC fan-out.
//!
//! Builds a `std::process::Command` with isolated env, optionally spawns the
//! child, handshakes MCP, and routes `tools/call` to the upstream server.
//!
//! When sandbox is enabled (`LOCUS_WORKER_SANDBOX=1`, [`McpStdioConfig::sandbox`],
//! or binding `upstream.sandbox`), spawn uses a platform backend (`sandbox-exec`
//! on macOS; `bwrap` or best-effort `path` on Linux). Unresolved executables and
//! unsupported platforms fail before spawn.
//!
//! Network stays **allowed** by default for MCP → provider APIs. Opt in to deny
//! with `LOCUS_WORKER_SANDBOX_NO_NETWORK=1` or [`McpStdioConfig::sandbox_no_network`].

use super::sandbox::{
    resolve_sandbox_spawn, sandbox_enabled, sandbox_no_network_enabled, ENV_WORKER_SANDBOXED,
    ENV_WORKER_SANDBOX_BACKEND, ENV_WORKER_SANDBOX_NO_NETWORK,
};
use super::stdio_client::{client_key, McpStdioClient};
use super::{WorkerBackend, WorkerKey, WorkerSlot, WorkerState, WorkerToolResult};
use crate::binding::{Binding, ProviderBinding};
use crate::error::{LocusError, Result};
use crate::isolation::build_isolated_env_for_provider_opts;
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

/// Bind a child to the publisher envelope and the operator's concrete context.
/// This contains refs and scope metadata only, never resolved credential values.
fn community_launch_digest(
    binding: &Binding,
    provider: &ProviderBinding,
    manifest: &crate::marketplace::CommunityAdapterManifest,
) -> Result<String> {
    use sha2::Digest;
    let context = serde_json::to_vec(&(
        crate::marketplace::community_manifest_digest(manifest),
        &binding.id,
        &binding.alias,
        &binding.tenant,
        &binding.principal,
        &binding.policy,
        &provider.provider,
        &provider.account,
        &provider.scope,
        &provider.credential_ref,
    ))
    .map_err(|_| LocusError::msg("community worker context cannot be encoded"))?;
    let mut digest = sha2::Sha256::new();
    digest.update(b"locus-community-worker-context-v1\0");
    digest.update(context);
    Ok(hex::encode(digest.finalize()))
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
    /// Require sandbox wrapping. Backend tag is recorded in
    /// `LOCUS_WORKER_SANDBOX_BACKEND` (`sandbox-exec` / `bwrap` / `path`).
    /// The `path` backend is best-effort only (not kernel isolation).
    /// Also enabled when `LOCUS_WORKER_SANDBOX=1` regardless of this flag.
    pub sandbox: bool,
    /// Opt-in network isolation (default false — MCP needs provider HTTPS).
    /// Also enabled when `LOCUS_WORKER_SANDBOX_NO_NETWORK=1`. Only applies when
    /// sandboxed; `path` backend fails closed if this is set.
    pub sandbox_no_network: bool,
    /// Present when recipe metadata says sandboxing would make the command
    /// unusable or would require authority the profile intentionally denies.
    pub sandbox_incompatibility: Option<String>,
}

/// Stdio MCP backend with live JSON-RPC clients.
pub struct McpStdioBackend {
    config: McpStdioConfig,
    /// session:provider → Child process
    children: Mutex<BTreeMap<WorkerKey, Child>>,
    /// Live MCP clients (stdin/stdout taken from children)
    clients: Mutex<BTreeMap<String, McpStdioClient>>,
    /// Only this community child's injected values; never credentials from other workers.
    known_secrets: Mutex<BTreeMap<String, Vec<zeroize::Zeroizing<String>>>>,
    /// Signed envelope plus concrete operator context admitted at child launch.
    community_contracts: Mutex<BTreeMap<String, String>>,
}

impl McpStdioBackend {
    pub fn new(config: McpStdioConfig) -> Self {
        Self {
            config,
            children: Mutex::new(BTreeMap::new()),
            clients: Mutex::new(BTreeMap::new()),
            known_secrets: Mutex::new(BTreeMap::new()),
            community_contracts: Mutex::new(BTreeMap::new()),
        }
    }

    /// Build a `Command` ready to spawn with isolated env.
    ///
    /// When sandbox is on: resolve the protected executable, select a platform
    /// backend, and install a private temp root before returning the command.
    pub fn build_command(
        &self,
        session: &Session,
        binding: &Binding,
        provider: &ProviderBinding,
        work_dir: &Path,
    ) -> Result<Command> {
        if let Some(manifest) = provider.verified_community_adapter()? {
            let signed = manifest
                .upstream
                .as_ref()
                .ok_or_else(|| LocusError::msg("community adapter missing signed upstream"))?
                .expand()?;
            if self.config.command != signed.command
                || self.config.args != signed.args
                || self.config.resolve_secrets != signed.resolve_secrets
                || (signed.sandbox == Some(true) && !self.config.sandbox)
                || (signed.sandbox_no_network && !self.config.sandbox_no_network)
            {
                return Err(LocusError::msg(
                    "worker configuration differs from signed community executable contract",
                ));
            }
        }
        provider.community_frozen_values()?;
        let community = provider.verified_community_adapter()?.is_some();
        let mut iso = build_isolated_env_for_provider_opts(
            session,
            binding,
            provider,
            self.config.resolve_secrets,
        );
        if self.config.resolve_secrets && !iso.secrets_failed.is_empty() {
            return Err(LocusError::msg(
                "upstream credential resolution failed; refusing worker launch",
            ));
        }
        // Community subprocesses receive a private config root for this exact slot.
        let child_home = if community {
            work_dir
        } else {
            Path::new(&session.worker_home)
        };
        if community {
            iso.vars
                .insert("HOME".into(), child_home.display().to_string());
            iso.vars
                .insert("USERPROFILE".into(), child_home.display().to_string());
            iso.vars.insert(
                "GH_CONFIG_DIR".into(),
                child_home.join("gh").display().to_string(),
            );
            iso.vars.insert(
                "AWS_CONFIG_FILE".into(),
                child_home.join("aws/config").display().to_string(),
            );
            iso.vars.insert(
                "AWS_SHARED_CREDENTIALS_FILE".into(),
                child_home.join("aws/credentials").display().to_string(),
            );
            let temp = child_home.join("tmp");
            std::fs::create_dir_all(&temp)?;
            for key in ["TMPDIR", "TMP", "TEMP"] {
                iso.vars.insert(key.into(), temp.display().to_string());
            }
        }
        let sandboxed = sandbox_enabled(self.config.sandbox);
        let no_network = sandbox_no_network_enabled(self.config.sandbox_no_network);
        if sandboxed {
            if let Some(reason) = &self.config.sandbox_incompatibility {
                return Err(LocusError::msg(reason.clone()));
            }
        }

        let (program, args, sandbox_backend) = if sandboxed {
            let spawn = resolve_sandbox_spawn(
                &self.config.command,
                &self.config.args,
                work_dir,
                child_home,
                no_network,
            )?;
            (
                spawn.program,
                spawn.args,
                Some((spawn.backend, spawn.path, spawn.no_network)),
            )
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
        if community {
            cmd.env("LOCUS_WORKER_BINDING", &binding.alias);
            cmd.env("LOCUS_WORKER_BINDING_ID", &binding.id);
            cmd.env("LOCUS_WORKER_TENANT", &binding.tenant);
        }

        if let Some((backend, restricted_path, applied_no_network)) = sandbox_backend {
            let temp_root = child_home.join("tmp");
            std::fs::create_dir_all(&temp_root)?;
            // Markers set only after backend resolution. Tag `path` is best-effort
            // PATH restriction — not equivalent to sandbox-exec or bwrap.
            cmd.env("PATH", restricted_path);
            cmd.env("TMPDIR", &temp_root);
            cmd.env("TMP", &temp_root);
            cmd.env("TEMP", &temp_root);
            cmd.env(ENV_WORKER_SANDBOXED, "1");
            cmd.env(ENV_WORKER_SANDBOX_BACKEND, backend.as_str());
            if applied_no_network {
                cmd.env(ENV_WORKER_SANDBOX_NO_NETWORK, "1");
            }
        }

        Ok(cmd)
    }

    /// A cached community worker must retain its admitted context and source key.
    pub(super) fn validate_community_worker_contract(
        &self,
        slot: &WorkerSlot,
        binding: &Binding,
        provider: &ProviderBinding,
    ) -> Result<()> {
        let manifest = provider.verified_community_adapter()?;
        let launched = self
            .community_contracts
            .lock()
            .map_err(|_| LocusError::msg("worker contract guard unavailable"))?
            .get(&slot_client_key(slot))
            .cloned();
        let Some(manifest) = manifest else {
            if launched.is_none() {
                return Ok(());
            }
            return Err(LocusError::msg(
                "community worker signed contract removed; restart required",
            ));
        };
        let digest = community_launch_digest(binding, provider, manifest)?;
        if launched.as_ref() != Some(&digest) {
            return Err(LocusError::msg(
                "community worker signed contract changed or unavailable; restart required",
            ));
        }
        if slot.account != provider.account {
            return Err(LocusError::msg(
                "community worker account differs from pinned provider",
            ));
        }
        if !self.config.resolve_secrets {
            return Ok(());
        }
        if crate::credential::inject_keys_for_binding_provider(provider)?.is_empty() {
            return Err(LocusError::msg(
                "community worker credential mapping unavailable",
            ));
        }
        let current = crate::credential::resolve(&crate::credential::CredentialRef::validate(
            &provider.credential_ref,
        )?)?;
        let secrets = self
            .known_secrets
            .lock()
            .map_err(|_| LocusError::msg("worker secret guard unavailable"))?;
        if current.is_empty()
            || !secrets.get(&slot_client_key(slot)).is_some_and(|known| {
                known
                    .iter()
                    .any(|secret| secret.as_str() == current.as_str())
            })
        {
            return Err(LocusError::msg(
                "community worker credential source changed or unavailable; restart required",
            ));
        }
        Ok(())
    }

    /// Whether this backend will apply sandbox on spawn (config or env).
    pub fn sandbox_active(&self) -> bool {
        sandbox_enabled(self.config.sandbox)
    }

    /// Whether network isolation is requested (config or env); only applied when sandboxed.
    pub fn sandbox_no_network_active(&self) -> bool {
        sandbox_no_network_enabled(self.config.sandbox_no_network)
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
        let selected = clients.get_key_value(&ck).or_else(|| {
            if clients.len() == 1 {
                clients.iter().next()
            } else {
                None
            }
        });
        let (actual_key, client) =
            selected.ok_or_else(|| LocusError::msg("no live mcp client for provider"))?;
        let known = self
            .known_secrets
            .lock()
            .map_err(|_| LocusError::msg("worker secret guard unavailable"))?
            .get(actual_key)
            .cloned()
            .ok_or_else(|| LocusError::msg("worker secret guard unavailable"))?;
        let mut tools = client.list_tools_cached()?;
        for tool in &mut tools {
            tool.name = redact_known_text(&tool.name, &known);
            tool.description = redact_known_text(&tool.description, &known);
            redact_known_value(&mut tool.input_schema, &known, 0);
        }
        Ok(tools)
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
            let contract = provider
                .verified_community_adapter()?
                .map(|manifest| community_launch_digest(binding, provider, manifest))
                .transpose()?;
            let keys = if provider.verified_community_adapter()?.is_some() {
                crate::credential::inject_keys_for_binding_provider(provider)?
            } else {
                Vec::new()
            };
            let known = cmd
                .get_envs()
                .filter_map(|(key, value)| {
                    if keys
                        .iter()
                        .any(|allowed| key == std::ffi::OsStr::new(allowed))
                    {
                        value
                            .and_then(|value| value.to_str())
                            .filter(|value| !value.is_empty())
                            .map(|value| zeroize::Zeroizing::new(value.to_string()))
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
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
                            self.known_secrets
                                .lock()
                                .map_err(|_| LocusError::msg("worker secret guard unavailable"))?
                                .insert(ck.clone(), known);
                            if let Some(contract) = contract {
                                self.community_contracts
                                    .lock()
                                    .map_err(|_| {
                                        LocusError::msg("worker contract guard unavailable")
                                    })?
                                    .insert(ck.clone(), contract);
                            }
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
                                "mcp handshake failed for {}: {}",
                                provider.provider,
                                redact_known_text(&e.to_string(), &known)
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
        self.community_contracts
            .lock()
            .map_err(|_| LocusError::msg("worker contract guard unavailable"))?
            .remove(&ck);
        self.known_secrets
            .lock()
            .map_err(|_| LocusError::msg("worker secret guard unavailable"))?
            .remove(&ck);
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
        binding: &Binding,
        tool: &str,
        args: &Value,
    ) -> Result<WorkerToolResult> {
        let provider = binding
            .provider(&slot.key.provider)
            .ok_or_else(|| LocusError::msg("upstream provider absent from binding"))?;
        let full_tool = if tool.starts_with(&format!("{}.", slot.key.provider)) {
            tool.to_string()
        } else {
            format!("{}.{}", slot.key.provider, tool)
        };
        let scoped_args = provider.community_tool_args(&binding.policy, &full_tool, args)?;
        if provider.verified_community_adapter()?.is_some()
            && (slot.binding_id != binding.id || slot.binding_alias != binding.alias)
        {
            return Err(LocusError::msg(
                "community worker slot belongs to another binding",
            ));
        }
        self.validate_community_worker_contract(slot, binding, provider)?;
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

        let known = self
            .known_secrets
            .lock()
            .map_err(|_| LocusError::msg("worker secret guard unavailable"))?
            .get(&ck)
            .cloned()
            .ok_or_else(|| LocusError::msg("worker secret guard unavailable"))?;
        let mut response = match client.call_tool(upstream_name, &scoped_args) {
            Ok(result) => {
                let is_error = result
                    .get("isError")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                Ok::<WorkerToolResult, LocusError>(WorkerToolResult {
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
        }?;
        redact_known_value(&mut response.content, &known, 0);
        Ok(response)
    }
}

fn redact_known_text(text: &str, known: &[zeroize::Zeroizing<String>]) -> String {
    known
        .iter()
        .filter(|secret| !secret.is_empty())
        .fold(text.to_string(), |text, secret| {
            text.replace(secret.as_str(), "[redacted]")
        })
}

fn redact_known_value(value: &mut Value, known: &[zeroize::Zeroizing<String>], depth: usize) {
    if known.is_empty() {
        return;
    }
    if depth > 32 {
        *value = Value::String("[redacted: output exceeds bounded depth]".into());
        return;
    }
    match value {
        Value::String(text) => *text = redact_known_text(text, known),
        Value::Array(array) => {
            for value in array {
                redact_known_value(value, known, depth + 1);
            }
        }
        Value::Object(object) => {
            let original = std::mem::take(object);
            for (key, mut value) in original {
                redact_known_value(&mut value, known, depth + 1);
                object.insert(redact_known_text(&key, known), value);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod community_concurrency_tests {
    use super::*;
    use crate::adapter_registry::{ed25519_public_key_b64, LOCUS_ADAPTER_TRUST_KEYS_ENV};
    use crate::binding::{BindingBody, Policy};
    use crate::seal::SealKey;
    use crate::session::PinSource;
    use ed25519_dalek::SigningKey;
    use std::time::{Duration, Instant};

    struct Environment(Vec<(String, Option<std::ffi::OsString>)>);
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
    struct ReleaseOnDrop(std::path::PathBuf);
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            let _ = std::fs::write(&self.0, "release");
        }
    }
    fn wait_until(mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !ready() {
            assert!(
                Instant::now() < deadline,
                "inert protocol barrier timed out"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn empty_secret_guard_preserves_deep_upstream_values() {
        let mut original = json!("synthetic-deep-secret");
        for _ in 0..40 {
            original = json!({"nested": original});
        }
        let mut unguarded = original.clone();
        redact_known_value(&mut unguarded, &[], 0);
        assert_eq!(unguarded, original);

        let known = vec![zeroize::Zeroizing::new("synthetic-deep-secret".into())];
        let mut guarded = original;
        redact_known_value(&mut guarded, &known, 0);
        let text = serde_json::to_string(&guarded).unwrap();
        assert!(!text.contains("synthetic-deep-secret"));
        assert!(text.contains("output exceeds bounded depth"));
    }

    /// The old guard lookup after request completion leaked a reflected key when
    /// teardown deleted that guard while waiting for the client's in-flight lock.
    #[test]
    fn community_inflight_redaction_survives_teardown() {
        let dir = tempfile::tempdir().unwrap();
        let mut environment = Environment(vec![]);
        let signing = SigningKey::from_bytes(&[19; 32]);
        environment.set(
            LOCUS_ADAPTER_TRUST_KEYS_ENV,
            format!(
                "community-runtime-fixture:ed25519:{}",
                ed25519_public_key_b64(&signing.verifying_key())
            ),
        );
        environment.set("LOCUS_HOME", dir.path());
        environment.set(
            "LOCUS_COMMUNITY_LINEAR_FIXTURE",
            "synthetic-inflight-private-key",
        );
        environment.set("LOCUS_WORKER_SANDBOX", "0");
        environment.set("LOCUS_WORKER_SANDBOX_NO_NETWORK", "0");
        let marker = dir.path().join("barrier.jsonl");
        let provider =
            super::super::composite::community_runtime_tests::fixture("linear", &marker, &signing);
        let binding = Binding::from_body(BindingBody {
            id: "bnd_acme".into(),
            alias: "acme".into(),
            tenant: "fixture".into(),
            principal: None,
            description: None,
            policy: Policy::default(),
            providers: vec![provider.clone()],
        });
        let session = Session::new(
            "bnd_acme",
            "acme",
            "fixture",
            None,
            PinSource::Explicit,
            None,
            chrono::Duration::hours(1),
            dir.path().join("worker").display().to_string(),
            &SealKey::generate(),
        );
        let config =
            super::super::composite::mcp_config_from_upstream(provider.upstream.as_ref().unwrap())
                .unwrap();
        let backend = McpStdioBackend::new(config);
        let slot = backend
            .ensure(&session, &binding, &provider, &dir.path().join("slot"))
            .unwrap();
        let ck = slot_client_key(&slot);
        let ready = std::path::PathBuf::from(format!("{}.ready", marker.display()));
        let release = std::path::PathBuf::from(format!("{}.release", marker.display()));
        std::thread::scope(|scope| {
            let _release_on_unwind = ReleaseOnDrop(release.clone());
            let pending = scope
                .spawn(|| backend.call_tool(&slot, &binding, "read", &json!({"barrier":true})));
            wait_until(|| ready.exists());
            let teardown = scope.spawn(|| backend.teardown(&slot));
            // This is a deterministic lifecycle barrier, not a timing guess.
            wait_until(|| !backend.known_secrets.lock().unwrap().contains_key(&ck));
            std::fs::write(&release, "release").unwrap();
            let result = pending.join().unwrap().unwrap();
            assert!(result.ok);
            let text = serde_json::to_string(&result.content).unwrap();
            assert!(!text.contains("synthetic-inflight-private-key"));
            assert!(text.contains("[redacted]"));
            teardown.join().unwrap().unwrap();
        });
    }
}

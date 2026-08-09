//! MCP stdio child-process backend with JSON-RPC fan-out.
//!
//! Builds a `std::process::Command` with isolated env, optionally spawns the
//! child, handshakes MCP, and routes `tools/call` to the upstream server.

use super::stdio_client::{client_key, McpStdioClient};
use super::upstream_boundary::apply_capability;
use super::{WorkerBackend, WorkerKey, WorkerSlot, WorkerState, WorkerToolResult};
use crate::binding::{Binding, ProviderBinding};
use crate::credential::{inject_keys_for_provider, resolve, CredentialRef};
use crate::error::{LocusError, Result};
use crate::session::Session;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use zeroize::Zeroizing;

/// Process mechanics admitted from the parent into an upstream worker.
/// Provider, cloud, package-manager, proxy, and Locus variables are excluded.
const UPSTREAM_RUNTIME_ENV_ALLOWLIST: &[&str] = &[
    "PATH",
    "TMPDIR",
    "TMP",
    "TEMP",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TZ",
    "SYSTEMROOT",
    "COMSPEC",
    "PATHEXT",
    "WINDIR",
];

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
    /// Resolve credential_refs into child env when spawning.
    pub resolve_secrets: bool,
    /// Explicit opt-in to same-user host execution without OS confinement.
    pub unsafe_host_execution: bool,
    /// Optional overrides for keys in `UPSTREAM_RUNTIME_ENV_ALLOWLIST` only.
    /// All other keys are ignored to preserve the closed worker environment.
    pub extra_env: BTreeMap<String, String>,
}

/// Stdio MCP backend with live JSON-RPC clients.
pub struct McpStdioBackend {
    config: McpStdioConfig,
    /// session:provider → Child process
    children: Mutex<BTreeMap<WorkerKey, Child>>,
    /// Live MCP clients (stdin/stdout taken from children)
    clients: Mutex<BTreeMap<String, McpStdioClient>>,
    /// Credential values injected into this child, retained only for response blocking.
    credential_values: Mutex<Vec<Zeroizing<String>>>,
}

impl McpStdioBackend {
    pub fn new(config: McpStdioConfig) -> Self {
        Self {
            config,
            children: Mutex::new(BTreeMap::new()),
            clients: Mutex::new(BTreeMap::new()),
            credential_values: Mutex::new(Vec::new()),
        }
    }

    /// Build a `Command` ready to spawn with isolated env.
    pub fn build_command(
        &self,
        session: &Session,
        binding: &Binding,
        provider: &ProviderBinding,
        work_dir: &Path,
    ) -> Command {
        self.build_command_and_credentials(session, binding, provider, work_dir)
            .0
    }

    fn build_command_and_credentials(
        &self,
        session: &Session,
        binding: &Binding,
        provider: &ProviderBinding,
        work_dir: &Path,
    ) -> (Command, Vec<Zeroizing<String>>, Option<String>) {
        let mut vars = self.build_upstream_env(session, binding, provider, work_dir);
        let mut credential_values = Vec::new();
        let mut credential_error = None;
        if self.config.resolve_secrets {
            match resolve(&CredentialRef::parse(&provider.credential_ref)) {
                Ok(value) => {
                    for key in inject_keys_for_provider(&provider.provider) {
                        vars.insert((*key).to_string(), value.to_string());
                    }
                    vars.insert(
                        format!(
                            "LOCUS_{}_CREDENTIAL_RESOLVED",
                            provider.provider.to_uppercase()
                        ),
                        "1".into(),
                    );
                    credential_values.push(value);
                }
                Err(error) => credential_error = Some(error.to_string()),
            }
        }

        let mut cmd = Command::new(&self.config.command);
        cmd.args(&self.config.args)
            .current_dir(work_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear();

        for (k, v) in &vars {
            cmd.env(k, v);
        }

        (cmd, credential_values, credential_error)
    }

    fn build_upstream_env(
        &self,
        session: &Session,
        binding: &Binding,
        provider: &ProviderBinding,
        work_dir: &Path,
    ) -> BTreeMap<String, String> {
        let mut vars = BTreeMap::new();

        for key in UPSTREAM_RUNTIME_ENV_ALLOWLIST {
            if let Some(value) = self
                .config
                .extra_env
                .get(*key)
                .cloned()
                .or_else(|| std::env::var(key).ok())
            {
                vars.insert((*key).to_string(), value);
            }
        }

        // Defense in depth for future allowlist changes: no binding env locator
        // or provider injection key may survive before the selected credential.
        for bound_provider in &binding.providers {
            if let CredentialRef::Env { var } = CredentialRef::parse(&bound_provider.credential_ref)
            {
                vars.remove(&var);
            }
            for key in inject_keys_for_provider(&bound_provider.provider) {
                vars.remove(*key);
            }
        }

        let work_dir = work_dir.display().to_string();
        vars.insert("HOME".into(), work_dir.clone());
        vars.insert("USERPROFILE".into(), work_dir.clone());
        vars.insert("PWD".into(), work_dir.clone());
        vars.insert("LOCUS_SESSION_ID".into(), session.session_id.clone());
        vars.insert("LOCUS_BINDING".into(), session.binding_alias.clone());
        vars.insert("LOCUS_BINDING_ID".into(), session.binding_id.clone());
        vars.insert("LOCUS_TENANT".into(), session.tenant.clone());
        if let Some(principal) = &session.principal {
            vars.insert("LOCUS_PRINCIPAL".into(), principal.clone());
        }
        vars.insert("LOCUS_WORKER_PROVIDER".into(), provider.provider.clone());
        vars.insert("LOCUS_WORKER_ACCOUNT".into(), provider.account.clone());
        vars.insert(
            "LOCUS_WORKER_CREDENTIAL_REF".into(),
            provider.credential_ref.clone(),
        );
        vars.insert("LOCUS_WORKER_DIR".into(), work_dir.clone());

        let prefix = format!("LOCUS_{}", provider.provider.to_uppercase());
        vars.insert(format!("{prefix}_ACCOUNT"), provider.account.clone());
        vars.insert(
            format!("{prefix}_CREDENTIAL_REF"),
            provider.credential_ref.clone(),
        );
        vars.insert(format!("{prefix}_CREDENTIAL_RESOLVED"), "0".into());
        if let Some(value) = &provider.scope.project_ref {
            vars.insert(format!("{prefix}_PROJECT_REF"), value.clone());
        }
        if let Some(value) = &provider.scope.team_id {
            vars.insert(format!("{prefix}_TEAM_ID"), value.clone());
        }
        if let Some(value) = &provider.scope.account_id {
            vars.insert(format!("{prefix}_ACCOUNT_ID"), value.clone());
        }
        if let Some(value) = provider.scope.read_only {
            vars.insert(format!("{prefix}_READ_ONLY"), value.to_string());
        }
        for (suffix, values) in [
            ("ORGS", &provider.scope.orgs),
            ("REPOS", &provider.scope.repos),
            ("PROJECTS", &provider.scope.projects),
            ("ENV", &provider.scope.env),
            ("TOOLS", &provider.scope.tools),
        ] {
            if !values.is_empty() {
                vars.insert(format!("{prefix}_{suffix}"), values.join(","));
            }
        }

        let provider_name = provider.provider.to_ascii_lowercase();
        match provider_name.as_str() {
            "github" => {
                vars.insert("GH_CONFIG_DIR".into(), format!("{}/gh", work_dir));
            }
            "aws" => {
                vars.insert("AWS_CONFIG_FILE".into(), format!("{}/aws/config", work_dir));
                vars.insert(
                    "AWS_SHARED_CREDENTIALS_FILE".into(),
                    format!("{}/aws/credentials", work_dir),
                );
                if let Some(value) = &provider.scope.account_id {
                    vars.insert("AWS_ACCOUNT_ID".into(), value.clone());
                }
            }
            "supabase" => {
                if let Some(value) = &provider.scope.project_ref {
                    vars.insert("SUPABASE_PROJECT_REF".into(), value.clone());
                    vars.insert("SUPABASE_PROJECT_ID".into(), value.clone());
                }
            }
            "vercel" => {
                if let Some(value) = &provider.scope.team_id {
                    vars.insert("VERCEL_ORG_ID".into(), value.clone());
                    vars.insert("VERCEL_TEAM_ID".into(), value.clone());
                }
                if let Some(value) = provider.scope.projects.first() {
                    vars.insert("VERCEL_PROJECT_ID".into(), value.clone());
                }
            }
            "cloudflare" => {
                if let Some(value) = &provider.scope.account_id {
                    vars.insert("CLOUDFLARE_ACCOUNT_ID".into(), value.clone());
                }
            }
            _ => {}
        }

        vars
    }

    fn text_contains_credential(&self, text: &str) -> Result<bool> {
        let values = self
            .credential_values
            .lock()
            .map_err(|_| LocusError::msg("credential redaction lock poisoned"))?;
        Ok(values
            .iter()
            .any(|secret| !secret.is_empty() && text.contains(secret.as_str())))
    }

    fn value_contains_credential(&self, value: &Value) -> Result<bool> {
        match value {
            Value::String(text) => self.text_contains_credential(text),
            Value::Array(values) => {
                for value in values {
                    if self.value_contains_credential(value)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Value::Object(values) => {
                for (key, value) in values {
                    if self.text_contains_credential(key)?
                        || self.value_contains_credential(value)?
                    {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            _ => Ok(false),
        }
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
        let tools = client.list_tools_cached()?;
        drop(clients);
        for tool in &tools {
            let value = serde_json::to_value(tool)
                .map_err(|e| LocusError::msg(format!("serialize upstream tool: {e}")))?;
            if self.value_contains_credential(&value)? {
                return Err(LocusError::msg(
                    "upstream tools/list blocked: response contained injected credential material",
                ));
            }
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
            if !self.config.unsafe_host_execution {
                return Err(LocusError::msg(
                    "upstream_host_execution_denied: same-user MCP workers can read LOCUS_HOME/daemon.key; set upstream.unsafe_host_execution=true only for unsafe development use",
                ));
            }
            if self.config.command.is_empty() {
                return Err(LocusError::msg(
                    "mcp_stdio spawn requested but command is empty",
                ));
            }
            let (mut cmd, credential_values, credential_error) =
                self.build_command_and_credentials(session, binding, provider, work_dir);
            if let Some(error) = credential_error {
                return Err(LocusError::msg(format!(
                    "upstream_credential_resolution_denied for provider `{}`: {error}",
                    provider.provider
                )));
            }
            *self
                .credential_values
                .lock()
                .map_err(|_| LocusError::msg("credential redaction lock poisoned"))? =
                credential_values;
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
                            if self.text_contains_credential(&e.to_string())? {
                                return Err(LocusError::msg(
                                    "mcp handshake failed: upstream response contained injected credential material",
                                ));
                            }
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
            credential_ref: provider.credential_ref.clone(),
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
        binding: &Binding,
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

        let provider = binding.provider(&slot.key.provider).ok_or_else(|| {
            LocusError::msg(format!(
                "provider `{}` is not present on binding `{}`",
                slot.key.provider, binding.alias
            ))
        })?;
        let enforced_args = apply_capability(provider, upstream_name, args)?;

        let upstream_result = client.call_tool(upstream_name, &enforced_args);
        drop(clients);
        match upstream_result {
            Ok(result) => {
                if self.value_contains_credential(&result)? {
                    return Ok(WorkerToolResult {
                        ok: false,
                        content: json!({
                            "error": "upstream_response_blocked",
                            "detail": "Upstream response contained injected credential material and was discarded",
                            "tool": tool,
                            "provider": slot.key.provider,
                        }),
                        provider: slot.key.provider.clone(),
                    });
                }
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
            Err(e) => {
                let detail = if self.text_contains_credential(&e.to_string())? {
                    "Upstream error contained injected credential material and was discarded"
                        .to_string()
                } else {
                    e.to_string()
                };
                Ok(WorkerToolResult {
                    ok: false,
                    content: json!({
                        "error": "upstream_call_failed",
                        "detail": detail,
                        "tool": tool,
                        "upstream_tool": upstream_name,
                        "provider": slot.key.provider,
                    }),
                    provider: slot.key.provider.clone(),
                })
            }
        }
    }
}

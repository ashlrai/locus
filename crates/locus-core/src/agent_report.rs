//! AI-agent readiness — `locus agent report|doctor|setup`.
//!
//! Hub JSON contract:
//! ```json
//! {
//!   "version": "0.1.1",
//!   "ready": true,
//!   "status": "unsafe|protected|ready",
//!   "pin": { ... },
//!   "mcp_registered": { "claude": true, "cursor": false, "codex": false },
//!   "doctor": { ... },
//!   "commands": { "enter": "locus enter …", "whoami": "locus whoami" }
//! }
//! ```
//!
//! Never includes secret values — only aliases, digests, scopes, and issue codes.

use crate::doctor::{
    build_doctor_report, DoctorExternal, DoctorPin, DoctorReport, DoctorVerdict, IssueSeverity,
};
use crate::store::Store;
use crate::workspace::find_workspace;
use crate::VERSION;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Ladder for coding-agent identity safety (hub + CLI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    /// Seal broken, invalid pin, or doctor UNSAFE — do not act.
    Unsafe,
    /// Control plane present but incomplete (no pin, MCP unwired, no bindings).
    Protected,
    /// Seal ok, pin valid, bindings exist, MCP registered — safe to act under pin.
    Ready,
}

impl AgentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unsafe => "unsafe",
            Self::Protected => "protected",
            Self::Ready => "ready",
        }
    }

    /// Process exit code: ready=0, protected=1, unsafe=2.
    pub fn exit_code(self) -> i32 {
        match self {
            Self::Ready => 0,
            Self::Protected => 1,
            Self::Unsafe => 2,
        }
    }
}

/// Which AI clients have `locus-mcp` registered.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpRegistered {
    pub claude: bool,
    pub cursor: bool,
    pub codex: bool,
}

impl McpRegistered {
    pub fn any(&self) -> bool {
        self.claude || self.cursor || self.codex
    }
}

/// Suggested next commands for humans / hubs (never secret-bearing).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentCommands {
    pub enter: String,
    pub whoami: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doctor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup: Option<String>,
}

/// Recommended MCP server names for Ashlr agent safety (identity + secrets).
pub const REQUIRED_SERVERS: &[&str] = &["locus", "phantom"];

/// Hub-facing agent readiness report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentReport {
    pub version: String,
    pub ready: bool,
    pub status: AgentStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pin: Option<DoctorPin>,
    pub mcp_registered: McpRegistered,
    pub doctor: DoctorReport,
    pub commands: AgentCommands,
    /// Short agent-plane findings (identity setup gaps).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<String>,
    /// Human next-step bullets.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next_steps: Vec<String>,
    /// Exit code for `locus agent doctor` / `report`.
    pub exit_code: i32,
    /// Same tokens as `locus status --oneline` (hub convenience).
    pub status_oneline: String,
    /// Effective LOCUS_HOME when the report was built.
    pub home: String,
    /// Value of `LOCUS_SESSION_ID` env if set (session id, never a secret).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env_session_id: Option<String>,
    /// Register **one** multiplexor named `locus` (+ companion `phantom`).
    /// Never raw supabase/vercel MCP servers with ambient credentials.
    pub required_servers: Vec<String>,
    /// MCP multiplexor command for hub registration.
    pub mcp_command: String,
}

/// Stable keys for hub consumers.
pub const AGENT_REPORT_JSON_KEYS: &[&str] = &[
    "version",
    "ready",
    "status",
    "mcp_registered",
    "doctor",
    "commands",
    "exit_code",
    "status_oneline",
    "home",
    "required_servers",
    "mcp_command",
];

/// Validate a serialized agent report has the hub contract keys.
pub fn agent_report_json_has_stable_keys(value: &serde_json::Value) -> Result<(), Vec<String>> {
    let obj = match value.as_object() {
        Some(o) => o,
        None => return Err(vec!["root is not an object".into()]),
    };
    let mut missing = Vec::new();
    for k in AGENT_REPORT_JSON_KEYS {
        if !obj.contains_key(*k) {
            missing.push((*k).to_string());
        }
    }
    if let Some(mcp) = obj.get("mcp_registered") {
        for k in ["claude", "cursor", "codex"] {
            if mcp.get(k).is_none() {
                missing.push(format!("mcp_registered.{k}"));
            }
        }
    }
    if let Some(cmds) = obj.get("commands") {
        for k in ["enter", "whoami"] {
            if cmds.get(k).is_none() {
                missing.push(format!("commands.{k}"));
            }
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(missing)
    }
}

/// Inputs beyond the doctor report (filesystem probes).
#[derive(Debug, Clone, Default)]
pub struct AgentReportOptions {
    pub mcp: McpRegistered,
    pub project_dir: Option<PathBuf>,
    pub home_ready: bool,
    pub agent_md_present: bool,
    pub workspace_present: bool,
}

/// Build the hub agent report from a doctor report + MCP probe.
pub fn agent_report_from_doctor(doctor: DoctorReport, opts: AgentReportOptions) -> AgentReport {
    let mcp = opts.mcp;
    let pin = doctor.pin.clone();

    let mut findings: Vec<String> = Vec::new();

    if !opts.home_ready || !doctor.seal_ok {
        findings.push("locus home / seal key not ready — run `locus init`".into());
    }
    if doctor.bindings == 0 {
        findings
            .push("no bindings — run `locus init --with-samples` or `locus binding add`".into());
    }
    if pin.is_none() {
        findings
            .push("not pinned — run `locus enter` or `locus pin <alias>` before agent work".into());
    } else if pin.as_ref().is_some_and(|p| !p.seal_ok) {
        findings.push("pin seal invalid — re-pin with `locus pin <alias>`".into());
    } else if pin.as_ref().is_some_and(|p| p.expired) {
        findings.push("pin expired — re-pin with `locus pin <alias>`".into());
    }
    if !mcp.any() {
        findings.push(
            "locus-mcp not registered for Claude/Cursor/Codex — run `locus agent setup --apply`"
                .into(),
        );
    }
    if !opts.workspace_present {
        findings.push(
            "no .locus.toml in project tree — optional: `locus agent setup --apply --workspace`"
                .into(),
        );
    }
    if !opts.agent_md_present {
        findings.push(
            "no .locus/AGENT.md — run `locus agent setup --apply` to write agent guidance".into(),
        );
    }
    if doctor.verdict == DoctorVerdict::Unsafe {
        findings.push(format!(
            "doctor UNSAFE: {}",
            doctor
                .findings
                .iter()
                .filter(|f| f.severity == IssueSeverity::Unsafe)
                .map(|f| f.code.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let status = compute_status(&doctor, &mcp, &pin);
    let ready = status == AgentStatus::Ready;
    let commands = build_commands(opts.project_dir.as_deref(), pin.as_ref());

    let mut next_steps: Vec<String> = Vec::new();
    if !doctor.seal_ok || doctor.bindings == 0 {
        next_steps.push("locus init --with-samples".into());
    }
    if !mcp.any() {
        next_steps.push("locus agent setup --client all --apply".into());
    }
    if pin.is_none() {
        next_steps.push(commands.enter.clone());
    }
    next_steps.push(commands.whoami.clone());
    if doctor.verdict != DoctorVerdict::Safe {
        next_steps.push("locus doctor".into());
    }
    next_steps.dedup();

    let status_oneline = match &pin {
        None => "unpinned".to_string(),
        Some(_) if doctor.runtime.frozen => "frozen".to_string(),
        Some(p) if !p.seal_ok || p.expired || !doctor.runtime.ok => "invalid".to_string(),
        Some(p) => format!("{}:{}", p.alias, p.tenant),
    };
    let env_session_id = std::env::var("LOCUS_SESSION_ID")
        .ok()
        .filter(|s| !s.is_empty());
    let home = doctor.home.clone();

    AgentReport {
        version: VERSION.to_string(),
        ready,
        status,
        pin,
        mcp_registered: mcp,
        doctor,
        commands,
        findings,
        next_steps,
        exit_code: status.exit_code(),
        status_oneline,
        home,
        env_session_id,
        required_servers: REQUIRED_SERVERS.iter().map(|s| (*s).to_string()).collect(),
        mcp_command: "locus-mcp".into(),
    }
}

/// Build agent report from store + external doctor facts + filesystem probes.
pub fn build_agent_report(
    store: &Store,
    external: DoctorExternal,
    opts: AgentReportOptions,
) -> crate::Result<AgentReport> {
    let doctor = build_doctor_report(store, external)?;
    Ok(agent_report_from_doctor(doctor, opts))
}

fn compute_status(
    doctor: &DoctorReport,
    mcp: &McpRegistered,
    pin: &Option<DoctorPin>,
) -> AgentStatus {
    if !doctor.seal_ok || doctor.verdict == DoctorVerdict::Unsafe {
        return AgentStatus::Unsafe;
    }
    if let Some(p) = pin {
        if !p.seal_ok {
            return AgentStatus::Unsafe;
        }
    }

    let pin_ok = pin.as_ref().is_some_and(|p| p.seal_ok && !p.expired);
    if pin_ok && doctor.bindings > 0 && mcp.any() {
        AgentStatus::Ready
    } else {
        AgentStatus::Protected
    }
}

fn build_commands(project_dir: Option<&Path>, pin: Option<&DoctorPin>) -> AgentCommands {
    let enter = if let Some(dir) = project_dir {
        if let Some((_, cfg)) = find_workspace(dir) {
            if let Some(ref def) = cfg.default_binding {
                format!("locus enter {def}")
            } else {
                "locus enter".into()
            }
        } else {
            "locus enter".into()
        }
    } else {
        "locus enter".into()
    };

    let pin_cmd = pin
        .map(|p| format!("locus pin {}", p.alias))
        .or_else(|| {
            project_dir.and_then(|dir| {
                find_workspace(dir)
                    .and_then(|(_, cfg)| cfg.default_binding.map(|a| format!("locus pin {a}")))
            })
        })
        .unwrap_or_else(|| "locus pin <alias>".into());

    AgentCommands {
        enter,
        whoami: "locus whoami".into(),
        pin: Some(pin_cmd),
        doctor: Some("locus agent doctor".into()),
        setup: Some("locus agent setup --client all --apply".into()),
    }
}

// ── MCP registration probe ─────────────────────────────────────────────────

/// Detect whether locus-mcp is registered for common AI clients.
///
/// - Claude: project `.mcp.json` → `mcpServers.locus`
/// - Cursor: project `.cursor/mcp.json` or `~/.cursor/mcp.json`
/// - Codex: `~/.codex/config.toml` → `[mcp_servers.locus]`
pub fn probe_mcp_registered(project_dir: &Path, user_home: Option<&Path>) -> McpRegistered {
    let claude = mcp_json_has_locus(&project_dir.join(".mcp.json"));

    let cursor_project = project_dir.join(".cursor").join("mcp.json");
    let cursor_global = user_home.map(|h| h.join(".cursor").join("mcp.json"));
    let cursor = mcp_json_has_locus(&cursor_project)
        || cursor_global
            .as_ref()
            .map(|p| mcp_json_has_locus(p))
            .unwrap_or(false);

    let codex = user_home
        .map(|h| codex_config_has_locus(&h.join(".codex").join("config.toml")))
        .unwrap_or(false);

    McpRegistered {
        claude,
        cursor,
        codex,
    }
}

fn mcp_json_has_locus(path: &Path) -> bool {
    let Ok(raw) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    v.get("mcpServers")
        .and_then(|s| s.get("locus"))
        .map(|entry| entry.get("command").is_some() || entry.is_object())
        .unwrap_or(false)
}

fn codex_config_has_locus(path: &Path) -> bool {
    let Ok(raw) = fs::read_to_string(path) else {
        return false;
    };
    if let Ok(v) = raw.parse::<toml::Value>() {
        if let Some(servers) = v.get("mcp_servers") {
            if servers.get("locus").is_some() {
                return true;
            }
        }
    }
    raw.lines().any(|l| {
        let t = l.trim();
        t == "[mcp_servers.locus]" || t.starts_with("[mcp_servers.locus.")
    })
}

/// Env map written into MCP client configs by `locus agent setup --apply`.
/// Never includes `LOCUS_NOTIFY` (banners stay opt-in).
pub fn mcp_agent_env(client: &str) -> serde_json::Map<String, serde_json::Value> {
    let mut m = serde_json::Map::new();
    m.insert(
        "LOCUS_AUTO_PIN".into(),
        serde_json::Value::String("cwd".into()),
    );
    m.insert(
        "LOCUS_CLIENT".into(),
        serde_json::Value::String(client.to_string()),
    );
    m
}

/// Short guidance for coding agents (written to `.locus/AGENT.md`).
pub fn agent_md_content() -> &'static str {
    r#"# Locus — identity plane for coding agents

Pin a Binding; every CLI command and MCP tool is hard-scoped to that tenant until a human re-pins. **Wrong account, impossible.**

## Rules (agents)

1. Call `locus_whoami` / `locus whoami` before infrastructure mutations if context is unclear.
2. Treat the active pin as authoritative — do not invent `project_ref`, `team_id`, orgs, or repos.
3. **You cannot re-pin.** Use `locus_request_pin` only; a human runs `locus pin` / `locus enter`.
4. If tools are missing or the wrong tenant: ask the human to pin. Do not claim you can switch accounts.
5. Destructive tools may block on `require_approval` — human grants via `locus approve grant <id>`.

## Human commands

```bash
locus enter                 # pin from .locus.toml / autopin
locus whoami                # confirm tenant
locus agent doctor          # identity readiness
locus leave                 # clear pin
```

## MCP env (set by `locus agent setup`)

| Var | Value | Notes |
|-----|--------|--------|
| `LOCUS_AUTO_PIN` | `cwd` | Prefer workspace `.locus.toml` / autopin when supported |
| `LOCUS_CLIENT` | claude\|cursor\|codex | Session label |
| `LOCUS_NOTIFY` | **unset** | Desktop banners stay opt-in (`locus notify on`) |

Sibling: [Phantom](https://phm.dev) answers *can this secret enter the model?* Locus answers *as whom, against which tenant?*
"#
}

/// Minimal project workspace stub (comments guide the human).
pub fn workspace_stub_toml() -> &'static str {
    r#"# Locus workspace — commit this as .locus.toml at the project root.
version = 1
# default_binding = "personal"
# allowed_bindings = ["personal"]
require_pin = true
"#
}

/// Path for agent guidance under a project.
pub fn agent_md_path(project_dir: &Path) -> PathBuf {
    project_dir.join(".locus").join("AGENT.md")
}

/// Whether project has agent guidance file.
pub fn agent_md_present(project_dir: &Path) -> bool {
    agent_md_path(project_dir).is_file()
}

/// Whether a workspace config is found from project_dir.
pub fn workspace_present(project_dir: &Path) -> bool {
    find_workspace(project_dir).is_some()
}

/// Default options probed from the filesystem for a project dir.
pub fn probe_agent_options(project_dir: &Path, user_home: Option<&Path>) -> AgentReportOptions {
    AgentReportOptions {
        mcp: probe_mcp_registered(project_dir, user_home),
        project_dir: Some(project_dir.to_path_buf()),
        home_ready: true,
        agent_md_present: agent_md_present(project_dir),
        workspace_present: workspace_present(project_dir),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::{Binding, BindingBody, Policy, ProviderBinding, Scope};
    use tempfile::tempdir;

    fn sample_binding(alias: &str, tenant: &str) -> Binding {
        Binding::from_body(BindingBody {
            id: format!("bnd_{alias}"),
            alias: alias.into(),
            tenant: tenant.into(),
            principal: None,
            description: None,
            policy: Policy::default(),
            providers: vec![ProviderBinding {
                provider: "github".into(),
                account: alias.into(),
                credential_ref: "env:GH_TOKEN".into(),
                scope: Scope::default(),
                upstream: None,
            }],
        })
    }

    fn doctor_for(store: &Store, cwd: &Path) -> DoctorReport {
        build_doctor_report(
            store,
            DoctorExternal {
                phantom_on_path: true,
                unresolved_phm: Vec::new(),
                cwd: Some(cwd.to_path_buf()),
            },
        )
        .unwrap()
    }

    #[test]
    fn agent_protected_when_unpinned_no_mcp() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp"))
            .unwrap();
        let doctor = doctor_for(&store, dir.path());
        let report = agent_report_from_doctor(
            doctor,
            AgentReportOptions {
                mcp: McpRegistered::default(),
                project_dir: Some(dir.path().to_path_buf()),
                home_ready: true,
                agent_md_present: false,
                workspace_present: false,
            },
        );
        assert_eq!(report.status, AgentStatus::Protected);
        assert!(!report.ready);
        assert_eq!(report.exit_code, 1);
        assert!(report.commands.whoami.contains("whoami"));
        let v = serde_json::to_value(&report).unwrap();
        agent_report_json_has_stable_keys(&v).expect("stable keys");
        assert!(v.get("version").is_some());
        assert!(v.get("ready").is_some());
        assert_eq!(v["status"], "protected");
        assert!(v.get("mcp_registered").is_some());
        assert!(v.get("doctor").is_some());
        assert!(v.get("commands").is_some());
        assert_eq!(report.status_oneline, "unpinned");
        assert_eq!(
            report.required_servers,
            vec!["locus".to_string(), "phantom".to_string()]
        );
        assert_eq!(report.mcp_command, "locus-mcp");
        // Never embed raw secret material
        let s = serde_json::to_string(&report).unwrap();
        assert!(!s.contains("sk-"));
        assert!(!s.contains("ghp_"));
    }

    #[test]
    fn agent_ready_when_pinned_and_mcp() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp"))
            .unwrap();
        store.pin("acme", dir.path(), None, false).unwrap();
        let doctor = doctor_for(&store, dir.path());
        assert_eq!(doctor.verdict, DoctorVerdict::Safe);
        let report = agent_report_from_doctor(
            doctor,
            AgentReportOptions {
                mcp: McpRegistered {
                    claude: true,
                    cursor: false,
                    codex: false,
                },
                project_dir: Some(dir.path().to_path_buf()),
                home_ready: true,
                agent_md_present: true,
                workspace_present: true,
            },
        );
        assert_eq!(report.status, AgentStatus::Ready);
        assert!(report.ready);
        assert_eq!(report.exit_code, 0);
        assert_eq!(report.pin.as_ref().map(|p| p.alias.as_str()), Some("acme"));
        assert!(report.mcp_registered.claude);
        assert_eq!(report.status_oneline, "acme:acme-corp");
        assert_eq!(serde_json::to_value(&report).unwrap()["status"], "ready");
    }

    #[test]
    fn agent_unsafe_on_doctor_unsafe() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp"))
            .unwrap();
        store.pin("acme", dir.path(), None, false).unwrap();
        let path = store.active_session_path();
        let raw = fs::read_to_string(&path).unwrap();
        let mut sess: crate::session::Session = serde_json::from_str(&raw).unwrap();
        sess.binding_id = "bnd_evil".into();
        fs::write(&path, serde_json::to_string(&sess).unwrap()).unwrap();

        let doctor = doctor_for(&store, dir.path());
        assert_eq!(doctor.verdict, DoctorVerdict::Unsafe);
        let report = agent_report_from_doctor(
            doctor,
            AgentReportOptions {
                mcp: McpRegistered {
                    claude: true,
                    ..Default::default()
                },
                project_dir: Some(dir.path().to_path_buf()),
                home_ready: true,
                agent_md_present: true,
                workspace_present: true,
            },
        );
        assert_eq!(report.status, AgentStatus::Unsafe);
        assert!(!report.ready);
        assert_eq!(report.exit_code, 2);
    }

    #[test]
    fn probe_mcp_json_claude_and_cursor() {
        let dir = tempdir().unwrap();
        let project = dir.path().join("proj");
        let home = dir.path().join("home");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(home.join(".cursor")).unwrap();

        fs::write(
            project.join(".mcp.json"),
            r#"{"mcpServers":{"locus":{"command":"locus-mcp","args":[],"env":{}}}}"#,
        )
        .unwrap();
        fs::write(
            home.join(".cursor").join("mcp.json"),
            r#"{"mcpServers":{"locus":{"command":"locus-mcp"}}}"#,
        )
        .unwrap();
        fs::create_dir_all(home.join(".codex")).unwrap();
        fs::write(
            home.join(".codex").join("config.toml"),
            "[mcp_servers.locus]\ncommand = \"locus-mcp\"\n",
        )
        .unwrap();

        let m = probe_mcp_registered(&project, Some(&home));
        assert!(m.claude);
        assert!(m.cursor);
        assert!(m.codex);
        assert!(m.any());
    }

    #[test]
    fn mcp_agent_env_never_sets_notify() {
        let env = mcp_agent_env("claude");
        assert_eq!(
            env.get("LOCUS_AUTO_PIN").and_then(|v| v.as_str()),
            Some("cwd")
        );
        assert_eq!(
            env.get("LOCUS_CLIENT").and_then(|v| v.as_str()),
            Some("claude")
        );
        assert!(!env.contains_key("LOCUS_NOTIFY"));
    }

    #[test]
    fn agent_md_and_workspace_stub_nonempty() {
        assert!(agent_md_content().contains("locus_request_pin"));
        assert!(workspace_stub_toml().contains("require_pin"));
        assert!(agent_md_path(Path::new("/tmp")).ends_with(".locus/AGENT.md"));
    }

    #[test]
    fn enter_command_uses_workspace_default() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join(".locus.toml"),
            r#"
version = 1
default_binding = "acme"
require_pin = true
"#,
        )
        .unwrap();
        let store = Store::open(dir.path().join("home")).unwrap();
        store
            .save_binding(&sample_binding("acme", "acme-corp"))
            .unwrap();
        let doctor = doctor_for(&store, dir.path());
        let report = agent_report_from_doctor(
            doctor,
            AgentReportOptions {
                project_dir: Some(dir.path().to_path_buf()),
                home_ready: true,
                workspace_present: true,
                ..Default::default()
            },
        );
        assert_eq!(report.commands.enter, "locus enter acme");
    }
}

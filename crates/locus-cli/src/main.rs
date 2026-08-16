//! Locus CLI — identity plane for coding agents.
//!
//! ```text
//! locus pin acme
//! locus whoami
//! locus exec -- gh pr list
//! locus leave
//! ```

mod serve;

use anyhow::{anyhow, bail, Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};
use colored::Colorize;
use locus_core::{
    adapter_trust_keys_path, add_ed25519_trust_key, agent_md_content, agent_md_path,
    agent_report_from_doctor, all_recipes, build_ci_env_map, build_doctor_report,
    build_isolated_env_opts, build_release_manifest, builtin_manifest, ci_secrets_allowed,
    default_export_filename, export_content_type, export_events, export_forensics_pack,
    filter_audit_events, find_workspace, known_providers, list_adapters,
    list_trust_keys_with_origin, load_merged_trust_keys, mcp_agent_env, migrate_legacy_phantom_ref,
    parse_ed25519_signing_key, parse_manifest, parse_release_manifest, parse_ttl, phantom_on_path,
    post_audit_webhook, probe_agent_options, probe_mcp_registered, recipe_toml_snippet,
    release_manifest_json, resolve_audit_webhook_url, resolve_passphrase, sign_release_manifest,
    suggest_for_provider, validate_name_component, verify_claim, verify_manifest_with_keys,
    verify_release_manifest_with_keys, verify_session, workspace_stub_toml, AgentStatus, Binding,
    BindingBody, CredentialRef, CredentialResolutionIssue, DoctorExternal, DoctorVerdict,
    EntryVerifyStatus, EventsExportFormat, EventsExportOptions, EventsExportSink,
    ForensicsExportOptions, IsolatedEnv, LocusError, McpRegistered, Policy, ProviderBinding, Scope,
    Session, Store, TrustKeyOrigin, WorkspaceConfig, AUDIT_WEBHOOK_URL_ENV,
    LOCUS_REGISTRY_SIGNING_KEY_ENV, VERSION,
};
use serde_json::json;
use std::env;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Parser, Debug)]
#[command(
    name = "locus",
    version = VERSION,
    about = "Identity plane for coding agents — wrong account, impossible.",
    long_about = "Pin a Binding (tenant × providers × credentials × policy).\n\
Every CLI exec and MCP tool call is hard-scoped to that pin.\n\
Sibling to Phantom Secrets: Phantom protects secrets in context;\n\
Locus protects which identity acts.\n\n\
Commands are grouped (in display order):\n  \
  Setup         init · quickstart · setup · agent · doctor · watch · workspace · hook · mcp · engagement · graph · goal · verify · upstream · adapter\n  \
  Daily use     enter · switch · pin · leave · whoami · status · exec · run · binding\n  \
  CI            ci mint · ci env · ci run\n  \
  Approvals     approve · notify\n  \
  Audit         events · forensics\n  \
  Local UI      serve · dashboard\n  \
  Maintenance   completion · topic · version\n\n\
Topic help:  locus topic <name>  or  locus help topic <name>\n  \
  Topics: dashboard · forensics · serve · goal · verify · agent · mcp · http · upstream · adapter"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Emit machine-readable JSON where applicable
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    // ───────────────────────────── Setup ─────────────────────────────
    /// Initialize ~/.locus (or LOCUS_HOME)
    #[command(next_help_heading = "Setup")]
    Init {
        /// Also write a sample personal + acme binding pair
        #[arg(long)]
        with_samples: bool,
        /// Strict posture: mint the control capability to this process env only
        /// (prints the export line for your shell profile; never writes the file)
        #[arg(long)]
        no_persist_capability: bool,
    },

    /// First 60 seconds: init samples if needed, enter workspace default, whoami + doctor
    #[command(next_help_heading = "Setup")]
    Quickstart {
        /// Strict posture: mint the control capability to this process env only
        /// (prints the export line for your shell profile; never writes the file)
        #[arg(long)]
        no_persist_capability: bool,
    },

    /// Operator control-capability posture: status / persist / unpersist
    #[command(next_help_heading = "Setup", subcommand)]
    Capability(CapabilityCmd),

    /// Register locus-mcp with an AI client config
    #[command(next_help_heading = "Setup")]
    Setup {
        /// Client: claude | cursor | codex | grok (generic: print-only paste entry)
        #[arg(long, default_value = "claude")]
        client: String,
        /// Print config JSON instead of writing
        #[arg(long)]
        print: bool,
        /// Path to locus-mcp binary (default: same dir as locus, or PATH)
        #[arg(long)]
        mcp_bin: Option<String>,
    },

    /// Health check for the local control plane
    #[command(next_help_heading = "Setup")]
    Doctor,

    /// Continuous session heartbeat for hub (verify_session each tick)
    ///
    /// Each poll runs the same pack as `locus verify session` (doctor + whoami +
    /// safe_next + session_ok), not drift alone. With `--json`, prints one NDJSON
    /// object per tick for hub consumers. Freezes the pin on binding drift.
    ///
    /// Exit: with `--require-ok`, fail closed when `session_ok` is false.
    /// With `--once` only, fail when pin was present/expected and session is not ok
    /// (unpinned still exits 0 unless `--require-ok`).
    #[command(next_help_heading = "Setup")]
    Watch {
        /// Poll interval (e.g. 5s, 30s, 1m). Default: 5s
        #[arg(long, default_value = "5s")]
        interval: String,
        /// Exit after one check
        #[arg(long)]
        once: bool,
        /// Fail closed: exit non-zero whenever session_ok is false
        #[arg(long)]
        require_ok: bool,
    },

    /// Write a .locus.toml in the current directory
    #[command(next_help_heading = "Setup")]
    Workspace {
        /// Default binding alias
        #[arg(long)]
        default: String,
        /// Allowed binding aliases (comma-separated)
        #[arg(long)]
        allow: Option<String>,
        /// Require pin before work
        #[arg(long)]
        require_pin: bool,
        /// Overwrite existing .locus.toml
        #[arg(long)]
        force: bool,
    },

    /// Shell hook snippet (prints eval-able code)
    #[command(next_help_heading = "Setup")]
    Hook {
        /// Shell: zsh | bash | fish
        shell: String,
    },

    /// Run the locus-mcp stdio server, or manage multi-tenant MCP grants
    #[command(next_help_heading = "Setup")]
    Mcp {
        #[command(subcommand)]
        cmd: Option<McpCmd>,
    },

    // ─────────────────────────── Daily use ───────────────────────────
    /// Enter a client context (pin + shell-friendly status)
    ///
    /// Same resolution as `pin`: explicit alias, else workspace default, else
    /// opt-in git-remote autopin (`[autopin]` in config.toml).
    #[command(next_help_heading = "Daily use")]
    Enter {
        /// Binding alias or id (default: workspace / autopin)
        alias: Option<String>,
        /// Allow bindings outside workspace allowlist
        #[arg(long)]
        force: bool,
        /// Client label recorded on the session (claude, cursor, cli)
        #[arg(long)]
        client: Option<String>,
        /// Auto-expire this pin after DUR (e.g. 30m, 2h; min 1m, max 24h;
        /// capped by the binding's policy.max_ttl)
        #[arg(long, value_name = "DUR")]
        ttl: Option<String>,
        /// Print `export LOCUS_*=…` lines for eval
        #[arg(long)]
        exports: bool,
    },

    /// One-shot switch: leave the active pin (if any) and enter ALIAS
    ///
    /// Same fail-closed paths and errors as `leave` + `enter`. A target that
    /// `enter` would refuse (unknown alias, outside the workspace allowlist)
    /// is refused *before* the current pin is dropped. Audits normally via
    /// the underlying leave/pin operations.
    #[command(next_help_heading = "Daily use")]
    Switch {
        /// Binding alias or id to switch to
        alias: String,
        /// Allow bindings outside workspace allowlist
        #[arg(long)]
        force: bool,
        /// Client label recorded on the session (claude, cursor, cli)
        #[arg(long)]
        client: Option<String>,
        /// Auto-expire the new pin after DUR (e.g. 30m, 2h; min 1m, max 24h;
        /// capped by the binding's policy.max_ttl)
        #[arg(long, value_name = "DUR")]
        ttl: Option<String>,
    },

    /// Pin the current session to a binding
    #[command(next_help_heading = "Daily use")]
    Pin {
        /// Binding alias or id (default: .locus.toml default_binding, then autopin)
        alias: Option<String>,
        /// Allow bindings outside workspace allowlist
        #[arg(long)]
        force: bool,
        /// Client label recorded on the session (claude, cursor, cli)
        #[arg(long)]
        client: Option<String>,
        /// Experimental namespaced multi-binding (comma-separated aliases).
        /// Tools appear as `alias__tool` in locus-mcp. e.g. `--ns personal,acme`
        #[arg(long = "ns")]
        ns: Option<String>,
        /// Auto-expire this pin after DUR (e.g. 30m, 2h; min 1m, max 24h;
        /// capped by the binding's policy.max_ttl)
        #[arg(long, value_name = "DUR")]
        ttl: Option<String>,
    },

    /// Leave the active pin (clear identity) and suggest re-enter
    #[command(next_help_heading = "Daily use")]
    Leave {
        /// Force-clear a wedged session: tears down sessions/active.json even
        /// when the seal is invalid or the supervisor/authority anchor is
        /// gone. Requires the control capability (like normal leave), verified
        /// against the live broker or the persisted operator capability;
        /// deletes session state only — never mints anything.
        #[arg(long)]
        force: bool,

        /// With --force when no verifier is reachable (authority broker gone
        /// AND no persisted operator capability, e.g. after `locus init
        /// --no-persist-capability`): explicitly acknowledge tearing down
        /// without capability verification. Never overrides a live broker's
        /// refusal or a mismatching persisted capability.
        #[arg(long, requires = "force")]
        no_verifier: bool,
    },

    /// Manage client engagements (init / close)
    #[command(next_help_heading = "Setup", subcommand)]
    Engagement(EngagementCmd),

    /// Encrypted binding-graph share (bindings + workspace templates, no secrets)
    #[command(next_help_heading = "Setup", subcommand)]
    Graph(GraphCmd),

    /// Show who you are acting as (active pin)
    #[command(next_help_heading = "Daily use")]
    Whoami,

    /// Short status line for prompts / CI
    #[command(next_help_heading = "Daily use")]
    Status {
        /// One-line machine form: `unpinned` or `alias:tenant`
        #[arg(long)]
        oneline: bool,
    },

    /// AI-native setup + identity readiness (setup / doctor / report)
    #[command(next_help_heading = "Setup", subcommand)]
    Agent(AgentCmd),

    /// Northstar goal progress (`GOALS.md` or embedded milestones)
    #[command(next_help_heading = "Setup", subcommand)]
    Goal(GoalCmd),

    /// Verification plane — score claims / session pack before acting (M5)
    ///
    /// `locus verify claim --text "…"` returns
    /// `{ claim, confidence, needs_tool, suggestion, signals, grounding? }`.
    /// `locus verify session` packs doctor + whoami + safe_next for hub.
    /// No ML — pure heuristics for hub/agent extension. See docs/verification-plane.md.
    #[command(next_help_heading = "Setup", subcommand)]
    Verify(VerifyCmd),

    /// Run a command with only the pinned binding's identity surface
    #[command(next_help_heading = "Daily use")]
    Exec {
        /// Do not resolve credentials; fail before effects if an upstream can resolve them
        #[arg(long)]
        no_resolve: bool,
        /// Fail if any credential_ref cannot be resolved
        #[arg(long)]
        strict_creds: bool,
        /// Command and args (after `--`)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        cmd: Vec<String>,
    },

    /// One-shot: run a command under a temporary pin (global pin unchanged)
    #[command(next_help_heading = "Daily use")]
    Run {
        /// Binding alias to pin for this child only
        #[arg(short = 'b', long = "binding")]
        binding: String,
        /// Also update active.json (default: temporary session only)
        #[arg(long)]
        share_pin: bool,
        /// Do not resolve credentials; fail before effects if an upstream can resolve them
        #[arg(long)]
        no_resolve: bool,
        /// Fail if any credential_ref cannot be resolved
        #[arg(long)]
        strict_creds: bool,
        /// Allow bindings outside workspace allowlist
        #[arg(long)]
        force: bool,
        /// Command and args (after `--`)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        cmd: Vec<String>,
    },

    /// Manage bindings
    #[command(next_help_heading = "Daily use", subcommand)]
    Binding(BindingCmd),

    /// Guided client onboarding — walk through adding a client binding
    #[command(next_help_heading = "Setup", subcommand)]
    Client(ClientCmd),

    /// Built-in upstream MCP recipes (command/args for common servers)
    ///
    /// Bindings may set `upstream = { recipe = "github-mcp" }` instead of
    /// hand-writing command/args. See also `docs/workers.md`.
    #[command(next_help_heading = "Setup", subcommand)]
    Upstream(UpstreamCmd),

    /// Built-in adapter registry catalog (manifest list + signature verify)
    ///
    /// Discovery surface for `adapters/manifest.toml` — not a plugin loader.
    /// See `docs/adapter-sdk.md` and `schema/adapter-manifest.schema.json`.
    #[command(next_help_heading = "Setup", subcommand)]
    Adapter(AdapterCmd),

    /// CI / ephemeral pin minting (short-lived sealed sessions)
    ///
    /// Mints sealed sessions under `sessions/ci-*.json` without touching
    /// `active.json`. Children should set `LOCUS_SESSION_ID` (exported by
    /// `ci mint` / `ci env` / `ci run`) so `require_active` and locus-mcp
    /// resolve the ephemeral pin.
    #[command(next_help_heading = "CI", subcommand)]
    Ci(CiCmd),

    // ────────────────────────── Approvals ────────────────────────────
    /// Manage advisory review labels and blocked external authorization records
    #[command(next_help_heading = "Approvals", subcommand)]
    Approve(ApproveCmd),

    /// Desktop approval banners (OFF by default — opt in explicitly)
    #[command(next_help_heading = "Approvals", subcommand)]
    Notify(NotifyCmd),

    // ──────────────────────────── Audit ──────────────────────────────
    /// Read recent local audit events (`$LOCUS_HOME/audit/events.jsonl`)
    ///
    /// Subcommand: `locus events export [--otlp] [--out file] [--sink webhook]`
    /// for fleet pulse / OTLP / optional SIEM webhook. See docs/observability.md.
    #[command(next_help_heading = "Audit")]
    Events {
        /// Max events from the end of the log
        #[arg(long, default_value_t = 50)]
        last: usize,
        /// Filter by op (exact or substring, e.g. `session.pin`, `scope_freeze`)
        #[arg(long)]
        op: Option<String>,
        /// Filter by binding alias
        #[arg(long)]
        binding: Option<String>,
        #[command(subcommand)]
        action: Option<EventsAction>,
    },

    /// Export a shareable forensics pack (no secrets)
    #[command(next_help_heading = "Audit", subcommand)]
    Forensics(ForensicsCmd),

    // ────────────────────────── Local UI ─────────────────────────────
    /// Serve the local identity dashboard + JSON API (127.0.0.1 only)
    ///
    /// Endpoints: GET /api/status|whoami|bindings|approvals|doctor|events
    /// and POST /api/approve/{id}/grant. Never returns resolved secrets.
    /// Optional auth: LOCUS_DASHBOARD_TOKEN or --token.
    #[command(next_help_heading = "Local UI")]
    Serve {
        /// TCP port (bound to 127.0.0.1 only)
        #[arg(long, default_value_t = serve::DEFAULT_PORT)]
        port: u16,
        /// Shared secret for /api/* (env: LOCUS_DASHBOARD_TOKEN)
        #[arg(long, env = "LOCUS_DASHBOARD_TOKEN")]
        token: Option<String>,
        /// Open the default browser after bind
        #[arg(long)]
        open: bool,
    },

    /// Open the local identity dashboard (serve + browser)
    ///
    /// Shortcut for `locus serve --open`. Shows active pin, whoami, bindings,
    /// pending approvals, doctor verdict, and recent audit.
    #[command(next_help_heading = "Local UI")]
    Dashboard {
        /// TCP port (bound to 127.0.0.1 only)
        #[arg(long, default_value_t = serve::DEFAULT_PORT)]
        port: u16,
        /// Shared secret for /api/* (env: LOCUS_DASHBOARD_TOKEN)
        #[arg(long, env = "LOCUS_DASHBOARD_TOKEN")]
        token: Option<String>,
        /// Do not open a browser (just print the URL and serve)
        #[arg(long)]
        no_open: bool,
    },

    // ───────────────────────── Maintenance ───────────────────────────
    /// Generate shell completions (bash | zsh | fish | elvish | powershell)
    #[command(next_help_heading = "Maintenance")]
    Completion {
        /// Target shell
        shell: Shell,
    },

    /// Extended help for product surfaces (dashboard, forensics, serve, goal, verify, …)
    ///
    /// Also available as `locus help topic <name>`.
    #[command(next_help_heading = "Maintenance", visible_alias = "help-topic")]
    Topic {
        /// Topic name (omit to list topics)
        name: Option<String>,
    },

    /// Print version (also available as `locus --version`)
    #[command(next_help_heading = "Maintenance")]
    Version,
}

#[derive(Subcommand, Debug)]
enum CapabilityCmd {
    /// Where control authority lives right now (env / persisted / neither) — never prints the value
    Status,
    /// Persist this shell's LOCUS_CONTROL_CAPABILITY to $LOCUS_HOME/control_capability (0600)
    Persist,
    /// Remove the persisted file (strict posture) — prints the export line so you keep a copy
    Unpersist,
}

#[derive(Subcommand, Debug)]
enum GoalCmd {
    /// Print northstar progress from GOALS.md (or embedded milestones)
    ///
    /// Walks parents of cwd for `GOALS.md`, parses `- [x]` / `- [ ]` checkboxes
    /// under milestone sections, and prints done/remaining counts. Falls back
    /// to an embedded summary when no file is found.
    Status,
}

#[derive(Subcommand, Debug)]
enum VerifyCmd {
    /// Score a free-text claim for tool grounding / confidence
    ///
    /// Heuristic stub (no ML): numbers, URLs, versions, currency ($), percentages,
    /// or absolute language (always/never) ⇒ needs_tool + low confidence.
    /// Identity language + active pin attaches whoami grounding.
    Claim {
        /// Claim text to score
        #[arg(long)]
        text: String,
    },
    /// Pack doctor + whoami + safe_next as one JSON object for hub heartbeats
    ///
    /// Machine contract: `{ kind: "session", version, whoami?, doctor, safe_next, session_ok }`.
    /// Never includes secrets. Exits nonzero when `session_ok` is false.
    /// Prefer `--json` for hub gates.
    Session,
}

#[derive(Subcommand, Debug)]
enum AgentCmd {
    /// Wire Locus into AI clients (MCP + agent guidance)
    ///
    /// Requires `--apply` or `--dry-run`. Registers locus-mcp with
    /// `LOCUS_AUTO_PIN=cwd` + `LOCUS_CLIENT=<client>` (never `LOCUS_NOTIFY=1`).
    Setup {
        /// Client: claude | cursor | codex | grok | all (generic: print-only paste entry)
        #[arg(long, default_value = "all")]
        client: String,
        /// Claude Code scope: project (.mcp.json) | user (all projects, via the
        /// claude CLI — `claude mcp add-json … --scope user`; requires `claude`
        /// on PATH)
        #[arg(long, default_value = "project", value_parser = ["project", "user"])]
        claude_scope: String,
        /// Apply changes (write MCP configs, AGENT.md, optional workspace stub)
        #[arg(long)]
        apply: bool,
        /// Show planned actions without writing
        #[arg(long)]
        dry_run: bool,
        /// Write a project `.locus.toml` stub if missing
        #[arg(long)]
        workspace: bool,
        /// Path to locus-mcp binary (default: same dir as locus, or PATH)
        #[arg(long)]
        mcp_bin: Option<String>,
    },
    /// Human-readable identity-plane readiness for AI agents
    ///
    /// Exit codes: ready=0, protected=1, unsafe=2.
    Doctor,
    /// Hub JSON readiness report (version / ready / status / pin / mcp / doctor / commands)
    ///
    /// Prefer `--json`. Never includes secret values. Exit codes: ready=0,
    /// protected=1, unsafe=2.
    Report {
        /// Emit JSON (hub contract). Also available as global `--json`.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
enum McpCmd {
    /// Mint a multi-tenant MCP grant (sealed session + bearer token)
    ///
    /// Prints `lmt_<grant_id>.<secret>` exactly once — only the token's HMAC
    /// is stored at rest. Serve with `locus-mcp --http --multi-tenant`; the
    /// client presents the token as `X-Locus-Tenant-Token` on every request.
    Mint {
        /// Binding alias to bind the grant to
        #[arg(short = 'b', long = "binding")]
        binding: String,
        /// Grant TTL (e.g. 15m, 1h). Capped by binding max_ttl.
        #[arg(long, default_value = "1h")]
        ttl: String,
        /// Free-form operator label (shown in `locus mcp list`)
        #[arg(long)]
        label: Option<String>,
        /// Allow bindings outside workspace allowlist
        #[arg(long)]
        force: bool,
        /// Emit JSON (mint always prints JSON; flag kept for consistency)
        #[arg(long)]
        json: bool,
    },
    /// List grants (operator-only; there is deliberately NO HTTP enumeration)
    List {
        /// Emit JSON
        #[arg(long)]
        json: bool,
    },
    /// Revoke grants: by id, per binding, or all
    Revoke {
        /// Grant id to revoke
        grant_id: Option<String>,
        /// Revoke every grant for this binding alias
        #[arg(short = 'b', long = "binding")]
        binding: Option<String>,
        /// Revoke every grant
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand, Debug)]
enum CiCmd {
    /// Mint a short-lived sealed CI session (JSON by default)
    ///
    /// Writes `sessions/ci-<id>.json`. Does not update active.json.
    /// Env map includes LOCUS_* + frozen scopes — never secrets unless
    /// `--resolve` **and** `LOCUS_CI_ALLOW_SECRETS=1`.
    Mint {
        /// Binding alias to mint against
        #[arg(short = 'b', long = "binding")]
        binding: String,
        /// Session TTL (e.g. 15m, 1h). Capped by binding max_ttl.
        #[arg(long, default_value = "15m")]
        ttl: String,
        /// Allow bindings outside workspace allowlist
        #[arg(long)]
        force: bool,
        /// Include resolved secrets in env map (requires LOCUS_CI_ALLOW_SECRETS=1)
        #[arg(long)]
        resolve: bool,
    },
    /// Print `export FOO=bar` lines for a freshly minted CI session
    Env {
        /// Binding alias
        #[arg(short = 'b', long = "binding")]
        binding: String,
        /// Session TTL (e.g. 15m, 1h). Capped by binding max_ttl.
        #[arg(long, default_value = "15m")]
        ttl: String,
        /// Allow bindings outside workspace allowlist
        #[arg(long)]
        force: bool,
        /// Include resolved secrets (requires LOCUS_CI_ALLOW_SECRETS=1)
        #[arg(long)]
        resolve: bool,
    },
    /// Mint a temporary CI session, run a command under it, then clean up
    Run {
        /// Binding alias
        #[arg(short = 'b', long = "binding")]
        binding: String,
        /// Session TTL (e.g. 15m, 1h). Capped by binding max_ttl.
        #[arg(long, default_value = "15m")]
        ttl: String,
        /// Allow bindings outside workspace allowlist
        #[arg(long)]
        force: bool,
        /// Do not resolve credentials; fail before effects if an upstream can resolve them
        #[arg(long)]
        no_resolve: bool,
        /// Fail if any credential_ref cannot be resolved
        #[arg(long)]
        strict_creds: bool,
        /// Command and args (after `--`)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        cmd: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
enum GraphCmd {
    /// Export bindings (+ workspace templates) to an encrypted `.locusgraph` file
    ///
    /// Passphrase: `LOCUS_GRAPH_PASSPHRASE` or interactive prompt (TTY only).
    /// Never includes secret values — CredentialRefs only.
    Export {
        /// Binding aliases to export (comma-separated). Default: all bindings.
        #[arg(long, value_delimiter = ',')]
        bindings: Option<Vec<String>>,
        /// Output path (default: `locus-graph-<timestamp>.locusgraph` in cwd)
        #[arg(long, short = 'o')]
        out: Option<PathBuf>,
    },
    /// Import an encrypted `.locusgraph` file into the local store
    Import {
        /// Path to `.locusgraph` file
        path: PathBuf,
        /// Overwrite existing bindings / workspace templates
        #[arg(long)]
        force: bool,
    },
    /// List local shareable graph surface (bindings + workspace templates)
    List,
}

#[derive(Subcommand, Debug)]
enum EngagementCmd {
    /// Create a client binding + optional workspace/README (phm: stubs only)
    Init {
        /// Binding alias (e.g. acme)
        alias: String,
        /// Tenant name (e.g. acme-corp)
        #[arg(long)]
        tenant: String,
        /// Write `.locus.toml` in cwd with allowlist + require_pin
        #[arg(long)]
        workspace: bool,
        /// Skip writing `.locus/README.md`
        #[arg(long)]
        no_readme: bool,
        /// Overwrite existing binding / workspace
        #[arg(long)]
        force: bool,
    },
    /// Close an engagement (metadata + optional audit archive; never deletes vault secrets)
    Close {
        /// Binding alias
        alias: String,
        /// Archive audit events for this binding to `$LOCUS_HOME/archives/`
        #[arg(long)]
        archive: bool,
    },
}

#[derive(Subcommand, Debug)]
enum EventsAction {
    /// Export audit events as JSON lines (fleet pulse) or OTLP logs JSON
    Export {
        /// Max events from the end of the log
        #[arg(long, default_value_t = 200)]
        last: usize,
        /// Filter by op (exact or substring)
        #[arg(long)]
        op: Option<String>,
        /// Filter by binding alias
        #[arg(long)]
        binding: Option<String>,
        /// Emit OTLP-compatible Logs JSON instead of fleet-pulse JSON lines
        #[arg(long)]
        otlp: bool,
        /// Write to file (default: stdout when sink=local)
        #[arg(long, short = 'o')]
        out: Option<PathBuf>,
        /// OTLP service.name attribute (default: locus)
        #[arg(long, default_value = "locus")]
        service_name: String,
        /// Export destination: `local` (stdout/`--out`) or `webhook` (HTTP POST)
        #[arg(long, value_enum, default_value_t = CliEventsExportSink::Local)]
        sink: CliEventsExportSink,
        /// Webhook URL when `--sink webhook` (env: `LOCUS_AUDIT_WEBHOOK_URL`)
        #[arg(long)]
        url: Option<String>,
    },
}

/// CLI mirror of [`EventsExportSink`] (clap ValueEnum).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, clap::ValueEnum)]
enum CliEventsExportSink {
    /// Stdout or `--out` file
    #[default]
    Local,
    /// POST redacted body to `--url` / `LOCUS_AUDIT_WEBHOOK_URL` (fail soft if unset)
    Webhook,
}

impl From<CliEventsExportSink> for EventsExportSink {
    fn from(s: CliEventsExportSink) -> Self {
        match s {
            CliEventsExportSink::Local => EventsExportSink::Local,
            CliEventsExportSink::Webhook => EventsExportSink::Webhook,
        }
    }
}

#[derive(Subcommand, Debug)]
enum ForensicsCmd {
    /// Export pin, bindings, audit tail, doctor, pending approvals (no secrets)
    Export {
        /// Filter audit / approvals / bindings to this alias
        #[arg(long)]
        binding: Option<String>,
        /// Max audit events to include (default: 200)
        #[arg(long, default_value_t = 200)]
        last: usize,
        /// Write pack JSON to this path (default: stdout when --json, else pack.json)
        #[arg(long, short = 'o')]
        out: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum ApproveCmd {
    /// List pending approval requests
    #[command(visible_alias = "pending")]
    List {
        /// Max rows (default: 50)
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Record a non-authoritative local advisory label
    Grant {
        /// Approval id (`appr_…`)
        id: String,
        /// Local review label (default: LOCUS_PRINCIPAL or $USER)
        #[arg(long = "as")]
        as_principal: Option<String>,
        /// Reserved external-authority TTL; local advisory records ignore it
        #[arg(long)]
        ttl: Option<String>,
        /// macOS: blocking confirmation before recording advisory evidence.
        /// Not a biometric or identity API and never establishes authority.
        /// Tests: set `LOCUS_TOUCHID_MOCK=ok` or `cancel`.
        #[arg(long)]
        touchid: bool,
    },
    /// Show status of one approval (grants, dual-control progress)
    Status {
        /// Approval id (`appr_…`)
        id: String,
    },
    /// Poll until approved, denied, or timeout (for scripts / CI)
    Wait {
        /// Approval id (`appr_…`)
        id: String,
        /// Seconds to wait (default: 120)
        #[arg(long, default_value_t = 120)]
        timeout: u64,
        /// Poll interval in milliseconds (default: 500)
        #[arg(long, default_value_t = 500)]
        interval_ms: u64,
    },
    /// Deny a pending approval
    Deny {
        /// Approval id (`appr_…`)
        id: String,
    },
}

#[derive(Subcommand, Debug)]
enum NotifyCmd {
    /// Show whether desktop banners are enabled (default: off)
    Status,
    /// Enable silent macOS banners for *new* pending approvals
    On,
    /// Disable desktop banners (default)
    Off,
}

#[derive(Subcommand, Debug)]
enum UpstreamCmd {
    /// List built-in upstream MCP recipes
    List,
    /// Suggest recipes for a provider (e.g. github, supabase, filesystem)
    Suggest {
        /// Provider id (github, supabase, demo, …)
        provider: String,
    },
}

#[derive(Subcommand, Debug)]
enum AdapterCmd {
    /// List built-in provider adapters from the registry catalog
    List,
    /// Verify adapter manifest signatures (soft by default; fail-closed with --require-signed)
    Verify {
        /// Path to a manifest.toml (default: embedded built-in catalog)
        #[arg(long)]
        path: Option<PathBuf>,
        /// Fail closed if any entry is unsigned, unknown-key, invalid, or malformed
        #[arg(long)]
        require_signed: bool,
    },
    /// Manage local adapter registry trust pins (`$LOCUS_HOME/trust/adapter-keys.toml`)
    #[command(subcommand)]
    Trust(AdapterTrustCmd),
    /// Export a canonical release manifest of the built-in adapter set
    #[command(subcommand)]
    Registry(AdapterRegistryCmd),
    /// Verify a release manifest signature (trust store) + adapter-set match (fail closed)
    VerifyManifest {
        /// Path to a locus-adapters-<tag>.json release manifest
        file: PathBuf,
        /// Permit a manifest with NO signature (drift check only).
        /// A present-but-untrusted/invalid signature still fails.
        #[arg(long)]
        allow_unsigned: bool,
    },
}

#[derive(Subcommand, Debug)]
enum AdapterRegistryCmd {
    /// Export the canonical registry manifest JSON (unsigned unless --sign)
    Export {
        /// Write to this file instead of stdout
        #[arg(long)]
        out: Option<PathBuf>,
        /// Sign with an operator ed25519 key (requires --key or LOCUS_REGISTRY_SIGNING_KEY)
        #[arg(long)]
        sign: bool,
        /// Path to the ed25519 signing key file (base64 or 64-hex of the 32-byte seed)
        #[arg(long, requires = "sign")]
        key: Option<PathBuf>,
        /// Key id recorded as `signed_by` (must match a pinned trust key id at verify time)
        #[arg(long, default_value = "root", requires = "sign")]
        key_id: String,
    },
}

#[derive(Subcommand, Debug)]
enum AdapterTrustCmd {
    /// List trusted registry keys (file store + LOCUS_ADAPTER_TRUST_KEYS overlay)
    List,
    /// Pin an ed25519 public trust key under `$LOCUS_HOME/trust/adapter-keys.toml` (mode 0600)
    Add {
        /// Key id recorded in manifest `signed_by` (e.g. `root`)
        #[arg(long)]
        id: String,
        /// Standard base64 of the 32-byte ed25519 verifying key
        #[arg(long = "ed25519-pub")]
        ed25519_pub: String,
    },
}

#[derive(Subcommand, Debug)]
// Parsed once at startup; boxing BindingAddArgs would cost clap::Args impls.
#[allow(clippy::large_enum_variant)]
enum BindingCmd {
    /// List configured bindings
    List,
    /// Show one binding
    Show { alias: String },
    /// Convert conservative legacy bare Phantom names to explicit `phm:` refs
    MigrateCredentialRefs {
        alias: String,
        /// Persist the migration; default is a dry run
        #[arg(long)]
        write: bool,
    },
    /// Create a binding (flags-first; prompts only for missing values on a TTY)
    Add(BindingAddArgs),
    /// Remove a binding file
    Rm {
        alias: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
enum ClientCmd {
    /// Walk through adding a client binding (interactive; every prompt has a flag for scripting)
    Add(BindingAddArgs),
}

/// Shared flags for `locus binding add` and `locus client add`. Every prompt
/// has a flag so the flow stays scriptable; values are only prompted for when
/// missing and stdin is a TTY.
#[derive(clap::Args, Debug, Clone, Default)]
struct BindingAddArgs {
    /// Binding alias (e.g. cash-margin)
    alias: Option<String>,
    /// Tenant label (defaults to the alias when prompted)
    #[arg(long)]
    tenant: Option<String>,
    /// Provider: supabase | github | vercel | cloudflare | aws | resend | stripe | custom
    #[arg(long)]
    provider: Option<String>,
    /// Provider account label (e.g. cmp-prod)
    #[arg(long)]
    account: Option<String>,
    /// Credential pointer — phm:NAME or env:VAR (never the raw secret)
    #[arg(long)]
    credential_ref: Option<String>,
    /// Provider scope: Supabase project ref / Vercel project id
    #[arg(long)]
    project_ref: Option<String>,
    /// Provider scope: Vercel team id
    #[arg(long)]
    team_id: Option<String>,
    /// Provider scope: AWS / Stripe / Cloudflare account id
    #[arg(long)]
    account_id: Option<String>,
    /// Provider scope: GitHub org
    #[arg(long)]
    org: Option<String>,
    /// Provider scope: comma-separated GitHub repo allowlist
    #[arg(long)]
    repos: Option<String>,
    /// Freeze the scope read-only
    #[arg(long)]
    read_only: bool,
    #[arg(long)]
    description: Option<String>,
    /// Default pin TTL written to policy.default_ttl (e.g. 2h; min 1m, max 24h)
    #[arg(long, value_name = "DUR")]
    default_ttl: Option<String>,
    /// Prompt through every value even when flags are present
    #[arg(long)]
    guided: bool,
    /// Never prompt — fail listing the missing flags instead
    #[arg(long)]
    non_interactive: bool,
    /// Validate and print the binding TOML without writing
    #[arg(long)]
    dry_run: bool,
}

fn main() {
    if let Some(result) = locus_core::run_authority_anchor_server_if_requested() {
        if let Err(error) = result {
            eprintln!("authority anchor error: {error}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(e) = run() {
        eprintln!("{} {e:#}", "error:".red().bold());
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    // Support `locus help topic <name>` without fighting clap's built-in help.
    {
        let argv: Vec<String> = env::args().collect();
        if argv.len() >= 3 && argv[1] == "help" && argv[2] == "topic" {
            let name = argv.get(3).cloned();
            return cmd_topic(name.as_deref());
        }
    }

    let cli = Cli::parse();
    match cli.command {
        Commands::Init {
            with_samples,
            no_persist_capability,
        } => cmd_init(with_samples, !no_persist_capability, cli.json),
        Commands::Quickstart {
            no_persist_capability,
        } => cmd_quickstart(!no_persist_capability, cli.json),
        Commands::Capability(sub) => cmd_capability(sub, cli.json),
        Commands::Enter {
            alias,
            force,
            client,
            ttl,
            exports,
        } => cmd_enter(alias, force, client, exports, ttl, cli.json),
        Commands::Switch {
            alias,
            force,
            client,
            ttl,
        } => cmd_switch(alias, force, client, ttl, cli.json),
        Commands::Pin {
            alias,
            force,
            client,
            ns,
            ttl,
        } => cmd_pin(alias, force, client, ns, ttl, cli.json),
        Commands::Leave { force, no_verifier } => cmd_leave(force, no_verifier, cli.json),
        Commands::Engagement(sub) => cmd_engagement(sub, cli.json),
        Commands::Graph(sub) => cmd_graph(sub, cli.json),
        Commands::Whoami => cmd_whoami(cli.json),
        Commands::Status { oneline } => cmd_status(oneline, cli.json),
        Commands::Agent(sub) => cmd_agent(sub, cli.json),
        Commands::Goal(sub) => cmd_goal(sub, cli.json),
        Commands::Verify(sub) => cmd_verify(sub, cli.json),
        Commands::Exec {
            no_resolve,
            strict_creds,
            cmd,
        } => cmd_exec(cmd, !no_resolve, strict_creds),
        Commands::Run {
            binding,
            share_pin,
            no_resolve,
            strict_creds,
            force,
            cmd,
        } => cmd_run(binding, share_pin, cmd, !no_resolve, strict_creds, force),
        Commands::Ci(sub) => cmd_ci(sub, cli.json),
        Commands::Mcp { cmd } => match cmd {
            None => cmd_mcp(),
            Some(sub) => cmd_mcp_sub(sub, cli.json),
        },
        Commands::Binding(sub) => cmd_binding(sub, cli.json),
        Commands::Client(sub) => cmd_client(sub, cli.json),
        Commands::Upstream(sub) => cmd_upstream(sub, cli.json),
        Commands::Adapter(sub) => cmd_adapter(sub, cli.json),
        Commands::Workspace {
            default,
            allow,
            require_pin,
            force,
        } => cmd_workspace(default, allow, require_pin, force),
        Commands::Doctor => cmd_doctor(cli.json),
        Commands::Watch {
            interval,
            once,
            require_ok,
        } => cmd_watch(&interval, once, require_ok, cli.json),
        Commands::Hook { shell } => cmd_hook(&shell),
        Commands::Setup {
            client,
            print,
            mcp_bin,
        } => cmd_setup(&client, print, mcp_bin),
        Commands::Approve(sub) => cmd_approve(sub, cli.json),
        Commands::Notify(sub) => cmd_notify(sub, cli.json),
        Commands::Events {
            last,
            op,
            binding,
            action,
        } => match action {
            Some(EventsAction::Export {
                last,
                op,
                binding,
                otlp,
                out,
                service_name,
                sink,
                url,
            }) => cmd_events_export(
                last,
                op,
                binding,
                otlp,
                out,
                service_name,
                sink.into(),
                url,
                cli.json,
            ),
            None => cmd_events(last, op, binding, cli.json),
        },
        Commands::Forensics(sub) => cmd_forensics(sub, cli.json),
        Commands::Serve { port, token, open } => cmd_serve(port, token, open),
        Commands::Dashboard {
            port,
            token,
            no_open,
        } => cmd_serve(port, token, !no_open),
        Commands::Completion { shell } => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "locus", &mut io::stdout());
            Ok(())
        }
        Commands::Topic { name } => cmd_topic(name.as_deref()),
        Commands::Version => {
            if cli.json {
                println!(
                    "{}",
                    serde_json::json!({ "version": VERSION, "name": "locus" })
                );
            } else {
                println!("locus {VERSION}");
            }
            Ok(())
        }
    }
}

/// Extended product-surface help (`locus topic <name>` / `locus help topic <name>`).
fn cmd_topic(name: Option<&str>) -> Result<()> {
    let topics: &[(&str, &str)] = &[
        (
            "dashboard",
            "Local identity dashboard — active pin, whoami, bindings, pending approvals,\n\
             doctor verdict, and recent audit. Loopback-only; never returns secrets.\n\n\
             Commands:\n\
               locus dashboard [--port 8750] [--token …] [--no-open]\n\
               locus serve [--port 8750] [--token …] [--open]\n\n\
             API (127.0.0.1):\n\
               GET  /api/health | /api/status | /api/whoami | /api/bindings\n\
               GET  /api/approvals | /api/doctor | /api/events\n\
               POST /api/approve/{id}/grant\n\n\
             Auth: LOCUS_DASHBOARD_TOKEN or --token (Bearer / X-Locus-Token).\n\
             UI: apps/dashboard/public/index.html (embedded in the binary).",
        ),
        (
            "forensics",
            "Shareable forensics pack — pin/session meta, binding summaries, audit tail,\n\
             doctor snapshot, pending approvals, near-miss, chain tip. No secret values.\n\n\
             Commands:\n\
               locus forensics export [--binding <alias>] [--last N] [--out pack.json]\n\
               locus forensics export --json            # stdout JSON\n\n\
             Pair with:\n\
               locus events --last N [--op …] [--binding …]\n\
               locus events export [--otlp] [--out file]  # fleet pulse / OTLP logs\n\
               locus events export --sink webhook [--url URL]  # SIEM webhook (or LOCUS_AUDIT_WEBHOOK_URL)",
        ),
        (
            "serve",
            "HTTP server for the local identity dashboard + JSON API.\n\
             Binds 127.0.0.1 only. Blocks until Ctrl-C.\n\n\
             Commands:\n\
               locus serve [--port 8750] [--token …] [--open]\n\
               locus dashboard   # serve + open browser (use --no-open to skip)\n\n\
             Health probe:\n\
               curl -s http://127.0.0.1:8750/api/health\n\n\
             See also: locus topic dashboard",
        ),
        (
            "goal",
            "Northstar goal loop — progress against GOALS.md milestones.\n\n\
             Commands:\n\
               locus goal status [--json]\n\n\
             Walks parents of cwd for GOALS.md, parses - [x] / - [ ] checkboxes under\n\
             milestone sections, prints done/remaining. Falls back to embedded milestones\n\
             when no file is found.\n\n\
             Related: GOALS.md · PLAN.md · docs/hub-integration.md",
        ),
        (
            "verify",
            "Verification plane — certain reasoning/action gates (M5 stubs).\n\n\
             Claim scoring (heuristic, no ML):\n\
               locus verify claim --text \"Deploy hits https://api.x/v2\" [--json]\n\
               → { claim, confidence, needs_tool, suggestion, signals, grounding? }\n\
               Signals: url, version, number, percentage, currency, absolute_language, identity\n\
               MCP: locus_verify_claim  { \"text\": \"…\" }\n\n\
             Session pack (hub heartbeat):\n\
               locus verify session [--json]\n\
               → { kind:\"session\", whoami?, doctor, safe_next, session_ok }\n\
               Exit 0 only when session_ok=true; JSON is still emitted on failure.\n\n\
             Continuous whoami / watch (hub stream):\n\
               locus watch [--once] [--require-ok] [--json] [--interval 5s]\n\
               Each tick: verify_session pack → NDJSON { kind:\"watch\", session_ok,\n\
               whoami?, doctor_verdict, safe_next, … }. --require-ok fails closed.\n\n\
             Identity gate checks:\n\
               locus whoami [--json]           # active pin + seal\n\
               locus doctor [--json]           # SAFE|WARN|UNSAFE (exit 0/1/2)\n\
               locus agent report --json       # hub contract (ready|protected|unsafe)\n\
               locus status --oneline          # unpinned | alias:tenant | frozen\n\n\
             Doctor may WARN (ungrounded_claims) when recent audit details look\n\
             like low-confidence factual claims (numbers/URLs/versions/currency/…).\n\n\
             Docs: docs/verification-plane.md · schema/doctor.schema.json\n\
             Isolation smoke: export LOCUS_HOME=/tmp/locus-verify && locus init --with-samples",
        ),
        (
            "agent",
            "AI-native setup + hub readiness.\n\n\
             Commands:\n\
               locus agent setup --apply|--dry-run [--client all|claude|cursor|codex|grok]\n\
               locus agent setup --apply --client claude --claude-scope user  # via claude CLI\n\
               locus agent report --json       # hub gate (exit ready=0 protected=1 unsafe=2)\n\
               locus agent doctor              # human-readable ladder\n\n\
             MCP (when pinned):\n\
               resources: locus://session · locus://doctor · locus://bindings\n\
               prompt:    locus_context\n\
               tools:     locus_whoami first; descriptions tagged [locus:<alias|unpinned>]\n\n\
             REQUIRED_SERVERS = locus + phantom (integrations/ashlr-hub/).",
        ),
        (
            "mcp",
            "locus-mcp multiplexor — tools hard-scoped to the active pin.\n\n\
             Stdio (default for Claude Code / Cursor):\n\
               locus mcp\n\
               locus-mcp\n\
               locus setup --client claude|cursor|codex|grok\n\
               locus agent setup --apply\n\n\
             HTTP (CI / remote agents, loopback by default):\n\
               LOCUS_MCP_HTTP_TOKEN=… locus-mcp --http 127.0.0.1:8742\n\
               LOCUS_MCP_HTTP=1 LOCUS_MCP_HTTP_TOKEN=… locus-mcp\n\
               GET /health · GET /mcp (capabilities) · POST /mcp (JSON-RPC)\n\n\
             Upstream recipes (per-provider MCP children):\n\
               locus upstream list\n\
               locus upstream suggest github\n\
               locus upstream suggest vercel\n\
               upstream = { recipe = \"github-mcp\", resolve_secrets = true, sandbox = true }\n\n\
             Invariants: agents cannot pin; unpinned ⇒ control tools only; no secrets in results.\n\
             Docs: docs/mcp.md · docs/workers.md",
        ),
        (
            "upstream",
            "Built-in upstream MCP recipes for binding TOML.\n\n\
             Commands:\n\
               locus upstream list [--json]\n\
               locus upstream suggest <provider> [--json]\n\n\
             In a binding:\n\
               upstream = { recipe = \"github-official\", resolve_secrets = true, sandbox = false }  # Docker: high authority\n\
               upstream = { recipe = \"supabase-mcp\", resolve_secrets = true, sandbox = true }\n\
               upstream = { recipe = \"vercel-mcp\", sandbox = false }  # OAuth bridge: explicit unsandboxed\n\
               upstream = { recipe = \"filesystem-mcp\", args = [\"-y\", \"@modelcontextprotocol/server-filesystem\", \"/tmp/demo\"] }\n\
               upstream = { command = \"npx\", args = [\"-y\", \"@pkg\"] }  # explicit still works\n\n\
             Compatible recipe sandbox defaults survive command/args overrides.\n\
             Docker/OAuth bridge recipes require explicit sandbox=false and publish risk metadata.\n\
             Recipes: github-official · github-mcp · supabase-mcp · vercel-mcp · filesystem-mcp · everything-mcp\n\
             Source: adapters/recipes.toml · Docs: docs/workers.md · examples/upstream.binding.toml",
        ),
        (
            "adapter",
            "Built-in adapter registry catalog (discovery + signature verify).\n\n\
             Commands:\n\
               locus adapter list [--json]\n\
               locus adapter verify [--path FILE] [--require-signed] [--json]\n\
               locus adapter trust list [--json]\n\
               locus adapter trust add --id root --ed25519-pub <b64> [--json]\n\
               locus adapter registry export [--out FILE] [--sign --key <keyfile>] [--key-id ID]\n\
               locus adapter verify-manifest <file> [--allow-unsigned] [--json]\n\n\
             Catalog source: adapters/manifest.toml (embedded).\n\
             Schema:         schema/adapter-manifest.schema.json\n\
             Trust store:    $LOCUS_HOME/trust/adapter-keys.toml (mode 0600)\n\n\
             Per-entry optional fields:\n\
               signature = \"ed25519:<base64>\"       # preferred\n\
               signature = \"hmac-sha256:<hex>\"      # backcompat stand-in\n\
               signed_by = \"key-id\"\n\n\
             Trust keys (merged; env wins on same id):\n\
               1) $LOCUS_HOME/trust/adapter-keys.toml\n\
               2) LOCUS_ADAPTER_TRUST_KEYS=id:ed25519:<base64-pubkey>[,id:hmac-sha256:<64-hex>]\n\n\
             Soft verify (default): unsigned OK; invalid/malformed signatures fail.\n\
             --require-signed: fail closed unless every entry has a valid trusted signature\n\
             (ed25519 or hmac-sha256 when the key id is trusted).\n\n\
             Release manifests: `registry export` emits a canonical JSON of the built-in\n\
             adapter set (id, name, version, tools, sha256 digest); `--sign` uses an operator\n\
             ed25519 key from --key <file> or LOCUS_REGISTRY_SIGNING_KEY (never printed).\n\
             `verify-manifest` is fail closed: trusted signature required (unless\n\
             --allow-unsigned) AND this binary's adapter set must match exactly.\n\
             Not a plugin loader — in-tree ProviderAdapter registration is still manual.\n\
             Docs: docs/adapter-sdk.md · docs/registry-trust.md · sibling: locus upstream (recipes)",
        ),
        (
            "http",
            "HTTP transports for Locus surfaces.\n\n\
             Dashboard API:\n\
               locus serve --port 8750\n\
               curl -s http://127.0.0.1:8750/api/health\n\n\
             MCP streamable-HTTP-lite (token required for /mcp):\n\
               LOCUS_MCP_HTTP_TOKEN=secret LOCUS_HOME=… locus-mcp --http 127.0.0.1:8742\n\
               # pin first on that LOCUS_HOME: locus pin <alias>\n\
               curl -s http://127.0.0.1:8742/health\n\
               curl -s -H \"Authorization: Bearer secret\" http://127.0.0.1:8742/mcp\n\
               curl -s -H \"Authorization: Bearer secret\" \\\n\
                 -H 'Content-Type: application/json' \\\n\
                 -H 'Accept: application/json, text/event-stream' \\\n\
                 -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{…}}' \\\n\
                 http://127.0.0.1:8742/mcp\n\n\
             Remote: bind loopback + reverse proxy (TLS); set LOCUS_MCP_HTTP_ALLOW_REMOTE=1\n\
             only when necessary. Docs: docs/mcp.md § Remote deploy.\n\n\
             Env: LOCUS_MCP_HTTP · LOCUS_MCP_HTTP_ADDR · LOCUS_MCP_HTTP_TOKEN\n\
                    LOCUS_MCP_HTTP_ALLOW_REMOTE=1 (non-loopback; default refuses)\n\
                    LOCUS_HOME (store + pin for the multiplexor process)\n\
                    LOCUS_DASHBOARD_TOKEN (optional dashboard /api gate)",
        ),
    ];

    match name.map(str::trim).filter(|s| !s.is_empty()) {
        None => {
            println!("{}", "locus topic — product surface guides".bold());
            println!();
            println!("Usage:  locus topic <name>");
            println!("        locus help topic <name>");
            println!();
            println!("{}", "Topics:".bold());
            for (n, body) in topics {
                let first = body.lines().next().unwrap_or("");
                println!("  {:12} {}", n.cyan(), first);
            }
            println!();
            println!("Also: locus help <command>  for clap flag help.");
            Ok(())
        }
        Some(n) => {
            let key = n.to_ascii_lowercase();
            if let Some((_, body)) = topics.iter().find(|(k, _)| *k == key) {
                println!("{} {}", "topic".magenta().bold(), key.cyan().bold());
                println!();
                println!("{body}");
                Ok(())
            } else {
                let names: Vec<&str> = topics.iter().map(|(k, _)| *k).collect();
                bail!("unknown topic '{n}'. Available: {}", names.join(", "));
            }
        }
    }
}

fn store() -> Result<Store> {
    Store::open_default().context("open locus store")
}

fn exact_session_selected() -> bool {
    env::var("LOCUS_SESSION_ID")
        .ok()
        .is_some_and(|id| !id.trim().is_empty())
}

/// Environment selection is a confinement signal, never an authority signal.
/// The selected session must exist and carry a valid store seal; run/CI/MCP
/// sessions remain delegated even if their descriptive client labels are edited.
fn delegated_child_session(s: &Store) -> Result<Option<Session>> {
    let selected = exact_session_selected();
    let session = match s.require_active() {
        Ok(session) => session,
        Err(_) if !selected && s.active_session()?.is_none() => return Ok(None),
        Err(error) => return Err(error).context("verify selected session authority"),
    };
    if selected || session.is_delegated() {
        Ok(Some(session))
    } else {
        Ok(None)
    }
}

fn require_local_control_boundary(operation: &str) -> Result<()> {
    store()?
        .require_local_control(operation)
        .map_err(Into::into)
}

/// Fresh-operator onboarding for `LOCUS_CONTROL_CAPABILITY` (init/quickstart only).
///
/// - env already set (valid or not): leave it alone — `control_auth` errors are
///   actionable, and silently replacing a mismatched capability is forbidden.
/// - persisted file exists: adopt it for this process (same trust boundary as
///   `eval "$(locus hook zsh)"`), reminding the operator to persist the export.
/// - nothing anywhere: mint + persist 0600 and adopt it — minting a NEW
///   capability for a store without one never weakens the gate (a live broker
///   under a different capability still refuses control, fail closed).
///
/// The bearer value is never printed; returned notes carry paths + commands only.
fn bootstrap_control_capability(s: &Store, persist: bool) -> Result<Option<String>> {
    if env::var_os(locus_core::CONTROL_CAPABILITY_ENV).is_some() {
        return Ok(None);
    }
    match bootstrap_control_capability_plan(s.home(), persist)? {
        Some((value, note)) => {
            env::set_var(locus_core::CONTROL_CAPABILITY_ENV, value);
            Ok(Some(note))
        }
        None => Ok(None),
    }
}

/// Env-free core of [`bootstrap_control_capability`]: decide the capability to
/// adopt for this process and the operator-facing note. Separated so the
/// no-persist path is testable without mutating process-global env state.
fn bootstrap_control_capability_plan(
    home: &Path,
    persist: bool,
) -> Result<Option<(String, String)>> {
    if let Some(value) = locus_core::read_persisted_control_capability(home)? {
        let mut note = format!(
            "adopted persisted control capability from {} for this run — persist for new shells: eval \"$(locus hook zsh)\"",
            locus_core::control_capability_file(home).display()
        );
        if !persist {
            note.push_str(" (already persisted; strict posture: locus capability unpersist)");
        }
        return Ok(Some((value, note)));
    }
    if persist {
        let value = locus_core::mint_persisted_control_capability(home)?;
        let note = format!(
            "minted control capability → {} (0600) — export in new shells: eval \"$(locus hook zsh)\"",
            locus_core::control_capability_file(home).display()
        );
        return Ok(Some((value, note)));
    }
    // Strict posture: env-only. The export line is the operator's only durable
    // copy — printing the bearer here is the explicit point of the flag.
    let value = locus_core::mint_ephemeral_control_capability();
    let note = format!(
        "minted control capability (env-only, NOT persisted) — add to your shell profile now or \
         this shell is its only copy: export {}=\"{}\"",
        locus_core::CONTROL_CAPABILITY_ENV,
        value
    );
    Ok(Some((value, note)))
}

/// `locus capability` — operator posture over the control capability.
///
/// `status` never prints the bearer value. `persist`/`unpersist` are the
/// explicit levers between the onboarding default (persisted 0600, ambient for
/// same-user processes) and the strict posture (shell-profile export only).
/// See SECURITY.md § Control-plane authority boundary.
fn cmd_capability(sub: CapabilityCmd, json: bool) -> Result<()> {
    let s = store()?;
    let home = s.home();
    match sub {
        CapabilityCmd::Status => {
            let status = locus_core::control_capability_status(home);
            let posture = capability_posture(&status);
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "ok": true,
                        "posture": posture,
                        "env_present": status.env_present,
                        "env_valid": status.env_valid,
                        "persisted": status.persisted,
                        "persisted_valid": status.persisted_valid,
                        "persisted_permissions_ok": status.persisted_permissions_ok,
                        "matches_persisted": status.matches_persisted,
                        "file": locus_core::control_capability_file(home).display().to_string(),
                    })
                );
            } else {
                println!(
                    "{} control capability  {}",
                    "locus".magenta().bold(),
                    posture.bold()
                );
                println!(
                    "  env        {}",
                    if status.env_valid {
                        "present (valid)".green().to_string()
                    } else if status.env_present {
                        "present but INVALID (need 64 lowercase hex)"
                            .red()
                            .to_string()
                    } else {
                        "not set".yellow().to_string()
                    }
                );
                let file = locus_core::control_capability_file(home);
                println!(
                    "  persisted  {}",
                    if status.persisted_valid {
                        format!("{} (0600)", file.display()).green().to_string()
                    } else if status.persisted {
                        format!("{} INVALID", file.display()).red().to_string()
                    } else {
                        "no".to_string()
                    }
                );
                if status.persisted && !status.persisted_permissions_ok {
                    println!(
                        "  {} file readable by group/other — fix: chmod 600 {}",
                        "!".red().bold(),
                        file.display()
                    );
                }
                if status.matches_persisted == Some(false) {
                    println!(
                        "  {} env does not match the persisted file",
                        "!".red().bold()
                    );
                }
                match posture {
                    "persisted" | "env+persisted" => println!(
                        "  {}",
                        "same-user processes can run control commands — strict posture: \
                         locus capability unpersist"
                            .dimmed()
                    ),
                    "env-only" => println!(
                        "  {}",
                        "strict posture — keep the export line in your shell profile".dimmed()
                    ),
                    _ => {}
                }
            }
            Ok(())
        }
        CapabilityCmd::Persist => {
            require_local_control_boundary("locus capability persist")?;
            let value = env::var(locus_core::CONTROL_CAPABILITY_ENV).map_err(|_| {
                anyhow::anyhow!(
                    "{} is not set in this shell — nothing to persist",
                    locus_core::CONTROL_CAPABILITY_ENV
                )
            })?;
            locus_core::persist_control_capability(home, &value)?;
            let file = locus_core::control_capability_file(home);
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "ok": true,
                        "persisted": true,
                        "file": file.display().to_string(),
                    })
                );
            } else {
                println!(
                    "{} persisted control capability → {} (0600)",
                    "ok".green().bold(),
                    file.display()
                );
                println!(
                    "   {}",
                    "new shells pick it up via: eval \"$(locus hook zsh)\"".dimmed()
                );
            }
            Ok(())
        }
        CapabilityCmd::Unpersist => {
            require_local_control_boundary("locus capability unpersist")?;
            // Read the value BEFORE removal so the operator keeps a copy —
            // once the file is gone, live shells hold the only instances.
            let value = locus_core::read_persisted_control_capability(home)
                .ok()
                .flatten();
            let removed = locus_core::unpersist_control_capability(home)?;
            let file = locus_core::control_capability_file(home);
            let export_line =
                value.map(|v| format!("export {}=\"{}\"", locus_core::CONTROL_CAPABILITY_ENV, v));
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "ok": true,
                        "removed": removed,
                        "file": file.display().to_string(),
                        "export_line": export_line,
                    })
                );
            } else if removed {
                println!("{} removed {}", "ok".green().bold(), file.display());
                if let Some(line) = export_line {
                    println!("   add to your shell profile now — this output is your only copy:");
                    println!("   {line}");
                } else {
                    println!(
                        "   {} the removed file was invalid; mint fresh: export {}=\"$(openssl rand -hex 32)\"",
                        "note:".yellow(),
                        locus_core::CONTROL_CAPABILITY_ENV
                    );
                }
            } else {
                println!(
                    "{} nothing persisted at {} — already strict",
                    "ok".green().bold(),
                    file.display()
                );
            }
            Ok(())
        }
    }
}

/// One-word posture label for `locus capability status` (never the value).
fn capability_posture(status: &locus_core::ControlCapabilityStatus) -> &'static str {
    match (status.env_valid, status.persisted) {
        (true, true) => "env+persisted",
        (true, false) => "env-only",
        (false, true) => "persisted",
        (false, false) => "absent",
    }
}

fn isolated_child_env(
    store: &Store,
    session: &Session,
    binding: &Binding,
    resolve_secrets: bool,
) -> IsolatedEnv {
    let mut isolated = build_isolated_env_opts(session, binding, resolve_secrets);
    isolated
        .vars
        .insert("LOCUS_HOME".into(), store.home().display().to_string());
    isolated
}

fn cwd() -> PathBuf {
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Levenshtein distance over chars (small alias strings only).
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Closest known alias to a typo, if plausibly close (at most 3 edits and not
/// a completely different word).
fn nearest_alias<'a>(missing: &str, aliases: &'a [String]) -> Option<&'a str> {
    let needle = missing.to_lowercase();
    aliases
        .iter()
        .map(|c| (edit_distance(&needle, &c.to_lowercase()), c))
        .filter(|(d, c)| *d <= 3 && *d < c.chars().count().max(needle.chars().count()))
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c.as_str())
}

/// Alias ergonomics for enter/pin: a bare BindingNotFound becomes a message
/// listing known aliases plus a nearest-match suggestion. Never alters the
/// fail-closed outcome — only the operator-facing text.
fn with_alias_suggestions(s: &Store, err: locus_core::LocusError) -> anyhow::Error {
    if let locus_core::LocusError::BindingNotFound(missing) = &err {
        if let Ok(list) = s.list_bindings() {
            let aliases: Vec<String> = list.into_iter().map(|b| b.alias).collect();
            if aliases.is_empty() {
                return anyhow::anyhow!(
                    "binding not found: {missing} — no bindings exist yet \
                     (run `locus init --with-samples` or `locus binding add`)"
                );
            }
            let mut msg = format!(
                "binding not found: {missing} (known aliases: {})",
                aliases.join(", ")
            );
            if let Some(best) = nearest_alias(missing, &aliases) {
                msg.push_str(&format!(" — did you mean `{best}`?"));
            }
            return anyhow::anyhow!(msg);
        }
    }
    err.into()
}

/// Local identity dashboard HTTP server (blocks until Ctrl-C).
fn cmd_serve(port: u16, token: Option<String>, open_browser: bool) -> Result<()> {
    // Fail early if home is unreadable so users get a clear error before bind.
    let _ = store()?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("tokio runtime")?;
    rt.block_on(serve::run_serve(port, token, open_browser))
}

fn cmd_init(with_samples: bool, persist_capability: bool, json: bool) -> Result<()> {
    let s = store()?;
    // Mint/adopt the operator control capability BEFORE the control boundary,
    // or a fresh operator can never get past step zero.
    let capability_note = bootstrap_control_capability(&s, persist_capability)?;
    require_local_control_boundary("locus init")?;
    let config_written = ensure_default_config(&s)?;
    if with_samples {
        write_sample_bindings(&s)?;
    }
    if json {
        println!(
            "{}",
            serde_json::json!({
                "ok": true,
                "home": s.home().display().to_string(),
                "samples": with_samples,
                "config_written": config_written,
                "control_capability": capability_note,
            })
        );
    } else {
        println!("{} locus home {}", "ok".green().bold(), s.home().display());
        println!("   seal key {}", s.seal_key_path().display());
        if let Some(note) = &capability_note {
            println!("   capability {note}");
        }
        if config_written {
            println!(
                "   config   {}  {}",
                s.config_path().display(),
                "notify.enabled=false".dimmed()
            );
        } else {
            println!("   config   {}", s.config_path().display());
        }
        if with_samples {
            println!(
                "   samples  personal, acme  {}",
                "(edit placeholders)".dimmed()
            );
        }
        println!();
        println!("{}", "next (AI-native path):".bold());
        println!(
            "  {}  {}",
            "locus agent setup --apply".cyan(),
            "# wire locus-mcp into the agent".dimmed()
        );
        println!(
            "  {}  {}",
            "locus enter personal".cyan(),
            "# or: locus pin <alias> / workspace default".dimmed()
        );
        println!(
            "  {}  {}",
            "locus doctor".cyan(),
            "# SAFE | WARN | UNSAFE before tool use".dimmed()
        );
        println!(
            "  {}  {}",
            "locus whoami".cyan(),
            "# confirm tenant before mutations".dimmed()
        );
        println!();
        println!("{}", "also useful:".dimmed());
        println!("  {}", "locus binding list".dimmed());
        println!(
            "  {}",
            "locus quickstart          # first 60s bootstrap".dimmed()
        );
        println!(
            "  {}",
            "eval \"$(locus hook zsh)\"  # prompt shows pin / frozen".dimmed()
        );
        println!(
            "  {}",
            "locus completion zsh > …  # shell completions".dimmed()
        );
    }
    Ok(())
}

/// Write `$LOCUS_HOME/config.toml` with `notify.enabled = false` if missing.
/// Returns true when a new file was created.
fn ensure_default_config(s: &Store) -> Result<bool> {
    let path = s.config_path();
    if path.exists() {
        return Ok(false);
    }
    // Explicit notify=false so first-run is quiet; comments for humans.
    let body = r#"# Locus home config — no secrets live here.
# Desktop approval banners are OFF by default (agent spam is worse than silence).
# Enable with: locus notify on   |   kill: locus notify off / LOCUS_QUIET=1

[notify]
enabled = false

# Optional shell/hook preference: cwd | none | last
# [clients]
# auto_pin = "cwd"

# Opt-in git-remote → binding auto-pin (off by default)
# [autopin]
# enabled = false
# [[autopin.remotes]]
# match = "github.com/acme-corp"
# binding = "acme"
"#;
    std::fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
    Ok(true)
}

fn write_sample_bindings(s: &Store) -> Result<()> {
    // Annotated TOML so first-run users know what to replace. Never raw secrets.
    let personal = r#"# Sample personal binding — REPLACE project_ref / team_id placeholders.
# CredentialRefs are Phantom names (phm:NAME). Never put raw tokens here.
# Store secrets with: phantom store SUPABASE_PERSONAL / GH_TOKEN_PERSONAL / …
# Then: locus enter personal && locus whoami && locus doctor

[binding]
id = "bnd_personal"
alias = "personal"
tenant = "personal"
description = "Personal projects — sample; edit scopes before real use"

[binding.policy]
default = "allow"
max_ttl = "12h"

[[binding.providers]]
provider = "supabase"
account = "personal"
credential_ref = "phm:SUPABASE_PERSONAL"
# project_ref is frozen on every tool call — set the real ref.
scope = { project_ref = "personal_ref_replace_me", read_only = false }

[[binding.providers]]
provider = "github"
account = "personal"
credential_ref = "phm:GH_TOKEN_PERSONAL"
# Empty orgs/repos = no allowlist restriction (tighten for client work).
scope = { orgs = [], repos = [] }

[[binding.providers]]
provider = "vercel"
account = "personal"
credential_ref = "phm:VERCEL_TOKEN_PERSONAL"
scope = { team_id = "team_personal_replace_me", env = ["preview", "production"] }
"#;

    let acme = r#"# Sample client binding (Acme) — REPLACE placeholders before real work.
# Client work: prefer read_only on prod Supabase; require_approval on delete/deploy.
# Switch: locus switch acme   |   leave: locus leave
# Dual-control / firm mode: docs/firm-mode.md

[binding]
id = "bnd_acme"
alias = "acme"
tenant = "acme-corp"
description = "Acme client engagement — sample"

[binding.policy]
default = "allow"
max_ttl = "8h"
# Closed external authorization is required; local approve labels are advisory only.
require_approval = ["*.delete*", "vercel.deploy.prod"]

[[binding.providers]]
provider = "supabase"
account = "acme-prod"
credential_ref = "phm:SUPABASE_ACME"
scope = { project_ref = "acme_ref_replace_me", read_only = true }

[[binding.providers]]
provider = "github"
account = "acme-corp"
credential_ref = "phm:GH_TOKEN_ACME"
# Frozen org/repo allowlist — model cannot reach outside.
scope = { orgs = ["acme-corp"], repos = ["acme-corp/*"] }

[[binding.providers]]
provider = "vercel"
account = "acme-team"
credential_ref = "phm:VERCEL_TOKEN_ACME"
scope = { team_id = "team_acme_replace_me", projects = ["acme-web"], env = ["preview"] }
"#;

    // Prefer annotated files over re-serialize (preserves comments).
    write_annotated_binding(s, "personal", personal)?;
    write_annotated_binding(s, "acme", acme)?;
    Ok(())
}

fn write_annotated_binding(s: &Store, alias: &str, toml: &str) -> Result<()> {
    // Validate before write
    let b = Binding::parse_toml(toml).with_context(|| format!("validate sample {alias}"))?;
    b.validate()
        .with_context(|| format!("validate sample {alias}"))?;
    let path = s.bindings_dir().join(format!("{alias}.toml"));
    std::fs::write(&path, toml).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// First 60 seconds: ensure home + samples, enter workspace default if unpinned, whoami + doctor.
fn cmd_quickstart(persist_capability: bool, json: bool) -> Result<()> {
    let s = store()?;
    // Mint/adopt the operator control capability BEFORE the control boundary,
    // or a fresh operator can never get past step zero.
    let capability_note = bootstrap_control_capability(&s, persist_capability)?;
    require_local_control_boundary("locus quickstart")?;
    let config_written = ensure_default_config(&s)?;

    let mut actions: Vec<String> = Vec::new();
    if let Some(note) = &capability_note {
        actions.push(note.clone());
    }
    if config_written {
        actions.push("wrote config.toml (notify.enabled=false)".into());
    }

    let had_bindings = !s.list_bindings()?.is_empty();
    if !had_bindings {
        write_sample_bindings(&s)?;
        actions.push("wrote sample bindings personal, acme".into());
    }

    let mut entered: Option<(String, String)> = None;
    let mut enter_note: Option<String> = None;
    let pinned = s.active_session()?.is_some();
    if !pinned {
        let ws = find_workspace(&cwd())?;
        if let Some((_, ref cfg)) = ws {
            if cfg.default_binding.is_some() || !cfg.allowed_bindings.is_empty() {
                match s.pin_auto(&cwd(), Some("cli".into()), false) {
                    Ok(session) => {
                        entered = Some((session.binding_alias.clone(), session.tenant.clone()));
                        actions.push(format!(
                            "entered {} ({})",
                            session.binding_alias, session.tenant
                        ));
                    }
                    Err(e) => {
                        enter_note = Some(format!("enter skipped: {e:#}"));
                    }
                }
            } else {
                enter_note = Some(
                    "workspace has no default_binding — pin with `locus enter <alias>`".into(),
                );
            }
        } else if !s.list_bindings()?.is_empty() {
            // No .locus.toml: pin personal if sample exists, else first binding.
            let alias = if s.load_binding("personal").is_ok() {
                "personal".to_string()
            } else {
                s.list_bindings()?
                    .into_iter()
                    .next()
                    .map(|b| b.alias)
                    .unwrap_or_default()
            };
            if !alias.is_empty() {
                match s.pin(&alias, &cwd(), Some("cli".into()), false) {
                    Ok(session) => {
                        entered = Some((session.binding_alias.clone(), session.tenant.clone()));
                        actions.push(format!(
                            "pinned {} ({}) — no .locus.toml; used first/sample binding",
                            session.binding_alias, session.tenant
                        ));
                    }
                    Err(e) => {
                        enter_note = Some(format!("pin skipped: {e:#}"));
                    }
                }
            }
        }
    } else {
        actions.push("already pinned".into());
    }

    // Whoami surface
    let _ = s.check_drift_and_freeze();
    let whoami = s.whoami().ok();

    // Doctor (do not hard-exit here — quickstart should finish printing).
    // Same pack as `locus doctor`, including control-capability findings.
    let report = gather_doctor_report(&s)?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "ok": true,
                "home": s.home().display().to_string(),
                "actions": actions,
                "enter_note": enter_note,
                "entered": entered.as_ref().map(|(a, t)| json!({"binding": a, "tenant": t})),
                "whoami": whoami,
                "doctor": {
                    "verdict": match report.verdict {
                        DoctorVerdict::Safe => "safe",
                        DoctorVerdict::Warn => "warn",
                        DoctorVerdict::Unsafe => "unsafe",
                    },
                    "findings": report.findings.len(),
                },
            })
        );
        return Ok(());
    }

    println!(
        "{} quickstart  home={}",
        "ok".green().bold(),
        s.home().display()
    );
    for a in &actions {
        println!("   · {a}");
    }
    if let Some(note) = &enter_note {
        println!("   · {}", note.yellow());
    }

    println!();
    if let Some(ref w) = whoami {
        println!(
            "{} {}  tenant={}  session={}",
            "whoami".magenta().bold(),
            w.binding_alias.cyan().bold(),
            w.tenant.yellow(),
            w.session_id.dimmed()
        );
        if w.frozen {
            println!(
                "   frozen={}  — re-pin: {}",
                "YES".red().bold(),
                "locus leave && locus enter <alias>".cyan()
            );
        }
    } else {
        println!(
            "{}  {}  — run {}",
            "whoami".magenta().bold(),
            "unpinned".yellow(),
            "locus enter <alias>".cyan()
        );
    }

    println!();
    let verdict = match report.verdict {
        DoctorVerdict::Safe => "SAFE".green().bold().to_string(),
        DoctorVerdict::Warn => "WARN".yellow().bold().to_string(),
        DoctorVerdict::Unsafe => "UNSAFE".red().bold().to_string(),
    };
    println!(
        "{}  {}  bindings={}  findings={}",
        "doctor".magenta().bold(),
        verdict,
        report.bindings,
        report.findings.len()
    );
    for f in report.findings.iter().take(5) {
        let mark = match f.severity {
            locus_core::IssueSeverity::Unsafe => "!".red().bold().to_string(),
            locus_core::IssueSeverity::Warn => "!".yellow().to_string(),
            locus_core::IssueSeverity::Info => "i".dimmed().to_string(),
        };
        println!("   {mark} [{}] {}", f.code, f.message);
    }
    if report.findings.len() > 5 {
        println!(
            "   {} more — run {}",
            report.findings.len() - 5,
            "locus doctor".cyan()
        );
    }

    println!();
    println!("{}", "daily path:".bold());
    println!(
        "  {}  ·  {}  ·  {}",
        "locus enter <alias>".cyan(),
        "locus whoami".cyan(),
        "locus doctor".cyan()
    );
    println!(
        "  {}  ·  {}",
        "locus agent setup --apply".dimmed(),
        "eval \"$(locus hook zsh)\"".dimmed()
    );

    // Soft exit: unsafe still signals; warn does not abort first-run UX
    match report.verdict {
        DoctorVerdict::Unsafe => std::process::exit(2),
        _ => Ok(()),
    }
}

fn cmd_enter(
    alias: Option<String>,
    force: bool,
    client: Option<String>,
    exports: bool,
    ttl: Option<String>,
    json: bool,
) -> Result<()> {
    require_local_control_boundary("locus enter")?;
    let s = store()?;
    let client = client.or_else(|| Some("cli".into()));
    let requested_ttl = ttl.as_deref().map(parse_pin_ttl).transpose()?;
    let session = match alias {
        Some(a) => s
            .pin_with_ttl(&a, &cwd(), client, force, requested_ttl)
            .map_err(|e| with_alias_suggestions(&s, e))?,
        None => s
            .pin_auto_with_ttl(&cwd(), client, force, requested_ttl)
            .map_err(|e| with_alias_suggestions(&s, e))?,
    };
    let binding = s.load_binding(&session.binding_alias)?;
    let providers_n = binding.providers.len();
    // The store caps requests at policy.max_ttl silently; surface it here.
    // 5s slack avoids false "capped" warnings from pin-time skew.
    let granted = session.expires_at - session.pinned_at;
    let ttl_capped = requested_ttl.is_some_and(|req| granted + chrono::Duration::seconds(5) < req);

    if json {
        println!(
            "{}",
            serde_json::json!({
                "entered": true,
                "binding": session.binding_alias,
                "tenant": session.tenant,
                "session_id": session.session_id,
                "expires_at": session.expires_at.to_rfc3339(),
                "expires_in_secs": (session.expires_at - chrono::Utc::now()).num_seconds().max(0),
                "ttl_capped": ttl_capped,
                "providers": providers_n,
                "prompt": format!("[locus:{}:{}]", session.binding_alias, session.tenant),
            })
        );
        return Ok(());
    }

    if exports {
        println!("export LOCUS_BINDING={}", session.binding_alias);
        println!("export LOCUS_TENANT={}", session.tenant);
        println!("export LOCUS_SESSION={}", session.session_id);
        return Ok(());
    }

    println!(
        "{} entered {} ({})",
        "ok".green().bold(),
        session.binding_alias.cyan().bold(),
        session.tenant.yellow()
    );
    println!(
        "   prompt   {}",
        format!("[locus:{}:{}]", session.binding_alias, session.tenant).cyan()
    );
    println!("   session  {}", session.session_id.dimmed());
    println!(
        "   expires  {}  {}",
        session.expires_at.to_rfc3339().dimmed(),
        format!(
            "(at {} — in {})",
            session
                .expires_at
                .with_timezone(&chrono::Local)
                .format("%H:%M"),
            human_dur(session.expires_at - chrono::Utc::now())
        )
        .yellow()
    );
    if let (true, Some(req)) = (ttl_capped, requested_ttl) {
        println!(
            "   {} requested ttl {} capped to {} by policy.max_ttl on '{}'",
            "warning:".yellow().bold(),
            human_dur(req),
            human_dur(granted),
            session.binding_alias
        );
    }
    println!("   providers {}", providers_n);
    println!();
    println!(
        "   {}  ·  {}  ·  {}",
        "locus whoami".dimmed(),
        "locus exec -- <cmd>".dimmed(),
        "locus leave".dimmed()
    );
    Ok(())
}

/// Outcome of the one-shot `locus switch` flow (leave-if-pinned + enter).
#[derive(Debug)]
struct SwitchOutcome {
    /// The session that was left, when one was active.
    left: Option<Session>,
    /// The freshly pinned session.
    session: Session,
    providers_n: usize,
    /// TTL actually granted on the new pin.
    granted: chrono::Duration,
    /// True when the `--ttl` request was silently capped by policy.max_ttl.
    ttl_capped: bool,
}

/// Core of `locus switch <alias>`: leave-if-pinned + enter, fail closed.
///
/// Pre-flights the target with the same checks `enter` runs — binding
/// existence (with alias suggestions) and the workspace allowlist — BEFORE
/// leaving, so a target `enter` would refuse never drops the current pin.
/// The pin path then re-runs every check authoritatively: the pre-flight is
/// operator UX, not the gate. Audits normally via the underlying
/// `session.leave` + `session.pin` operations.
fn switch_flow(
    s: &Store,
    alias: &str,
    cwd: &Path,
    force: bool,
    client: Option<String>,
    requested_ttl: Option<chrono::Duration>,
) -> Result<SwitchOutcome> {
    // Pre-flight: unknown target refuses (with suggestions) before leaving.
    let target = s
        .load_binding(alias)
        .map_err(|e| with_alias_suggestions(s, e))?;
    // Pre-flight: same workspace-allowlist rule the pin path enforces.
    if let Some((_, cfg)) = find_workspace(cwd)? {
        if !cfg.allows(&target.alias) && !cfg.allows(&target.id) && !force {
            return Err(with_alias_suggestions(
                s,
                LocusError::BindingNotAllowed(target.alias.clone()),
            ));
        }
    }
    // Leave-if-pinned: the normal leave path (audits session.leave, revokes
    // session authority, cleans the worker home). A wedged session fails
    // closed here exactly like `locus leave` would.
    let left = s
        .leave()
        .map_err(|e| wedged_session_recovery(anyhow::Error::new(e)))?;
    // Enter: the authoritative fail-closed path (allowlist, policy.max_ttl
    // cap, seal) — audits session.pin normally.
    let session = match s.pin_with_ttl(alias, cwd, client, force, requested_ttl) {
        Ok(session) => session,
        Err(e) => {
            let err = with_alias_suggestions(s, e);
            return Err(if left.is_some() {
                err.context(
                    "switch left the previous pin before enter failed — identity is now \
                     clear; re-pin with `locus enter <alias>`",
                )
            } else {
                err
            });
        }
    };
    // Surface a silent policy.max_ttl clamp (5s slack for pin-time skew).
    let granted = session.expires_at - session.pinned_at;
    let ttl_capped = requested_ttl.is_some_and(|req| granted + chrono::Duration::seconds(5) < req);
    Ok(SwitchOutcome {
        left,
        providers_n: target.providers.len(),
        session,
        granted,
        ttl_capped,
    })
}

/// `locus switch <alias>` — one-shot leave-if-pinned + enter + compact
/// identity block (replaces the leave → enter → whoami ritual).
fn cmd_switch(
    alias: String,
    force: bool,
    client: Option<String>,
    ttl: Option<String>,
    json: bool,
) -> Result<()> {
    require_local_control_boundary("locus switch").map_err(wedged_session_recovery)?;
    let s = store()?;
    let client = client.or_else(|| Some("cli".into()));
    let requested_ttl = ttl.as_deref().map(parse_pin_ttl).transpose()?;
    let out = switch_flow(&s, &alias, &cwd(), force, client, requested_ttl)?;
    let session = &out.session;

    if json {
        println!(
            "{}",
            json!({
                "switched": true,
                "from": out.left.as_ref().map(|l| l.binding_alias.clone()),
                "binding": session.binding_alias,
                "tenant": session.tenant,
                "session_id": session.session_id,
                "expires_at": session.expires_at.to_rfc3339(),
                "expires_in_secs": (session.expires_at - chrono::Utc::now()).num_seconds().max(0),
                "ttl_capped": out.ttl_capped,
                "providers": out.providers_n,
                "prompt": format!("[locus:{}:{}]", session.binding_alias, session.tenant),
            })
        );
        return Ok(());
    }

    match &out.left {
        Some(prev) => println!(
            "{} switched {} -> {} ({})",
            "ok".green().bold(),
            prev.binding_alias.dimmed(),
            session.binding_alias.cyan().bold(),
            session.tenant.yellow()
        ),
        None => println!(
            "{} entered {} ({}) — no previous pin",
            "ok".green().bold(),
            session.binding_alias.cyan().bold(),
            session.tenant.yellow()
        ),
    }
    // Compact identity block: everything whoami would show for a fresh pin.
    println!(
        "   prompt   {}",
        format!("[locus:{}:{}]", session.binding_alias, session.tenant).cyan()
    );
    println!("   session  {}", session.session_id.dimmed());
    println!(
        "   expires  {}  {}",
        session.expires_at.to_rfc3339().dimmed(),
        format!(
            "(at {} — in {})",
            session
                .expires_at
                .with_timezone(&chrono::Local)
                .format("%H:%M"),
            human_dur(session.expires_at - chrono::Utc::now())
        )
        .yellow()
    );
    if let (true, Some(req)) = (out.ttl_capped, requested_ttl) {
        println!(
            "   {} requested ttl {} capped to {} by policy.max_ttl on '{}'",
            "warning:".yellow().bold(),
            human_dur(req),
            human_dur(out.granted),
            session.binding_alias
        );
    }
    println!("   providers {}", out.providers_n);
    Ok(())
}

fn cmd_pin(
    alias: Option<String>,
    force: bool,
    client: Option<String>,
    ns: Option<String>,
    ttl: Option<String>,
    json: bool,
) -> Result<()> {
    require_local_control_boundary("locus pin")?;
    let s = store()?;
    let client = client.or_else(|| Some("cli".into()));
    let requested_ttl = ttl.as_deref().map(parse_pin_ttl).transpose()?;
    let session = if let Some(ns_raw) = ns {
        let mut aliases: Vec<String> = Vec::new();
        if let Some(a) = alias {
            aliases.push(a);
        }
        for part in ns_raw.split(',') {
            let p = part.trim();
            if !p.is_empty() && !aliases.iter().any(|x| x == p) {
                aliases.push(p.into());
            }
        }
        if aliases.len() < 2 {
            bail!("--ns requires at least two distinct bindings (e.g. --ns personal,acme)");
        }
        s.pin_namespaced_with_ttl(&aliases, &cwd(), client, force, requested_ttl)
            .map_err(|e| with_alias_suggestions(&s, e))?
    } else {
        match alias {
            Some(a) => s
                .pin_with_ttl(&a, &cwd(), client, force, requested_ttl)
                .map_err(|e| with_alias_suggestions(&s, e))?,
            None => s
                .pin_auto_with_ttl(&cwd(), client, force, requested_ttl)
                .map_err(|e| with_alias_suggestions(&s, e))?,
        }
    };
    // Surface a silent policy.max_ttl clamp (5s slack for pin-time skew).
    let granted = session.expires_at - session.pinned_at;
    let ttl_capped = requested_ttl.is_some_and(|req| granted + chrono::Duration::seconds(5) < req);
    // Policy surface for the pinned binding (counts only — never secrets)
    let binding = s.load_binding(&session.binding_alias)?;
    let require_approval_n = binding.policy.require_approval.len();
    let dual_control_n = binding.policy.dual_control.len();
    let rules_n = binding.policy.rules.len();
    let dual_control_all = binding.policy.dual_control_all_approvals;
    let providers_n = binding.providers.len();

    if json {
        let mut v = serde_json::to_value(&session)?;
        if let Some(obj) = v.as_object_mut() {
            obj.insert(
                "policy".into(),
                json!({
                    "rules": rules_n,
                    "require_approval": require_approval_n,
                    "dual_control": dual_control_n,
                    "dual_control_all_approvals": dual_control_all,
                    "providers": providers_n,
                }),
            );
            obj.insert(
                "expires_in_secs".into(),
                json!((session.expires_at - chrono::Utc::now())
                    .num_seconds()
                    .max(0)),
            );
            obj.insert("ttl_capped".into(), json!(ttl_capped));
        }
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        println!(
            "{} pinned {} ({})",
            "ok".green().bold(),
            session.binding_alias.cyan().bold(),
            session.tenant.dimmed()
        );
        println!("   session  {}", session.session_id);
        println!(
            "   expires  {}  {}",
            session.expires_at.to_rfc3339(),
            format!(
                "(at {} — in {})",
                session
                    .expires_at
                    .with_timezone(&chrono::Local)
                    .format("%H:%M"),
                human_dur(session.expires_at - chrono::Utc::now())
            )
            .yellow()
        );
        if let (true, Some(req)) = (ttl_capped, requested_ttl) {
            println!(
                "   {} requested ttl {} capped to {} by policy.max_ttl on '{}'",
                "warning:".yellow().bold(),
                human_dur(req),
                human_dur(granted),
                session.binding_alias
            );
        }
        println!("   worker   {}", session.worker_home);
        if session.is_namespaced() {
            println!(
                "   mode     {}  namespaces={}",
                "namespaced".yellow(),
                session.all_aliases().join(",")
            );
        }
        println!(
            "   policy   rules={}  require_approval={}  dual_control={}{}",
            rules_n,
            require_approval_n,
            dual_control_n,
            if dual_control_all {
                "  dual_control_all_approvals=true"
            } else {
                ""
            }
        );
        println!("   providers {}", providers_n);
        println!();
        println!("   {}", "locus whoami".dimmed());
        println!("   {}", "locus exec -- <cmd>".dimmed());
        if session.is_namespaced() {
            println!(
                "   {}",
                "tools: alias__tool (e.g. acme__github.scope)".dimmed()
            );
        }
    }
    Ok(())
}

/// Route wedged-session control errors to the recovery hatch.
///
/// When `leave` / `status` / `doctor` / `exec` fail closed on the ACTIVE
/// session — invalid or legacy seal, unavailable/stale authority anchor,
/// unparseable session file — the operator has no built-in way out except
/// hand-deleting `sessions/active.json`. Name the supported teardown instead.
fn wedged_session_recovery(err: anyhow::Error) -> anyhow::Error {
    use locus_core::LocusError as E;
    let wedged = matches!(
        err.downcast_ref::<E>(),
        Some(
            E::InvalidSeal
                | E::LegacySessionSeal
                | E::AuthorityAnchorUnavailable(_)
                | E::AuthorityAnchorMismatch
                | E::Json(_)
        )
    );
    if wedged {
        err.context(
            "active session is wedged — recovery: `locus leave --force` clears it \
             (control capability required), then re-pin: `locus enter <alias>`",
        )
    } else {
        err
    }
}

fn cmd_leave(force: bool, no_verifier: bool, json: bool) -> Result<()> {
    if force {
        return cmd_leave_force(no_verifier, json);
    }
    require_local_control_boundary("locus leave").map_err(wedged_session_recovery)?;
    let s = store()?;
    match s
        .leave()
        .map_err(|e| wedged_session_recovery(anyhow::Error::new(e)))?
    {
        None => {
            if json {
                println!("{}", serde_json::json!({ "left": false }));
            } else {
                println!("{} no active pin — already clear", "->".dimmed());
                println!(
                    "   re-pin: {}  or  {}",
                    "locus enter <alias>".cyan(),
                    "locus pin personal".cyan()
                );
            }
        }
        Some(session) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "left": true,
                        "binding": session.binding_alias,
                        "tenant": session.tenant,
                        "session_id": session.session_id,
                    })
                );
            } else {
                println!(
                    "{} left {} ({})",
                    "ok".green().bold(),
                    session.binding_alias.cyan().bold(),
                    session.session_id.dimmed()
                );
                println!(
                    "   identity cleared — no residual pin  {}",
                    "[locus:leave]".dimmed()
                );
                println!(
                    "   re-pin: {}  or  {}",
                    "locus enter <alias>".cyan(),
                    "locus pin personal".cyan()
                );
            }
        }
    }
    Ok(())
}

/// `locus leave --force` — tear down a wedged active session.
///
/// Deliberately skips [`require_local_control_boundary`] (which re-validates
/// the possibly-wedged session and would fail closed); the control capability
/// is still required and authenticated inside [`Store::force_leave`].
fn cmd_leave_force(no_verifier: bool, json: bool) -> Result<()> {
    let s = store()?;
    let reason = if no_verifier {
        "operator forced leave (locus leave --force --no-verifier)"
    } else {
        "operator forced leave (locus leave --force)"
    };
    let outcome = s.force_leave(reason, no_verifier)?;
    if json {
        println!(
            "{}",
            serde_json::json!({
                "left": outcome.cleared,
                "forced": true,
                "binding": outcome.binding_alias,
                "session_id": outcome.session_id,
                "diagnosis": outcome.diagnosis,
            })
        );
    } else if !outcome.cleared {
        println!("{} no active pin — already clear", "->".dimmed());
        println!(
            "   re-pin: {}  or  {}",
            "locus enter <alias>".cyan(),
            "locus pin personal".cyan()
        );
    } else {
        println!(
            "{} force-cleared active session{}{}",
            "ok".green().bold(),
            outcome
                .binding_alias
                .as_deref()
                .map(|a| format!(" for {}", a.cyan().bold()))
                .unwrap_or_default(),
            outcome
                .session_id
                .as_deref()
                .map(|id| format!(" ({})", id.dimmed()))
                .unwrap_or_default(),
        );
        if !outcome.diagnosis.is_empty() {
            println!(
                "   wedge diagnosis: {}  {}",
                outcome.diagnosis.join(", ").yellow(),
                "[locus:force_leave]".dimmed()
            );
        }
        println!(
            "   session state deleted — nothing minted; audit: {}",
            "session.force_leave".dimmed()
        );
        println!(
            "   re-pin: {}  or  {}",
            "locus enter <alias>".cyan(),
            "locus pin personal".cyan()
        );
    }
    Ok(())
}

fn cmd_graph(sub: GraphCmd, json: bool) -> Result<()> {
    let s = store()?;
    match sub {
        GraphCmd::List => {
            let entries = s.graph_list()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else if entries.is_empty() {
                println!("{} graph empty — add bindings first", "->".dimmed());
                println!("   {}", "locus binding add …".cyan());
            } else {
                println!(
                    "{} local graph surface (CredentialRefs only, no secrets)",
                    "graph".cyan().bold()
                );
                for e in &entries {
                    match e.kind.as_str() {
                        "binding" => {
                            let prov = e.providers.join(", ");
                            let sources = e
                                .credentials
                                .iter()
                                .map(|c| c.source.as_str())
                                .collect::<Vec<_>>()
                                .join(", ");
                            println!(
                                "  {} {}  tenant={}  providers=[{}]  credential_sources=[{}]",
                                "binding".green(),
                                e.name.bold(),
                                e.tenant.as_deref().unwrap_or("-"),
                                prov,
                                sources.dimmed()
                            );
                        }
                        "workspace" => {
                            let allow = e.allowed_bindings.join(", ");
                            println!(
                                "  {} {}  default={}  allow=[{}]",
                                "workspace".yellow(),
                                e.name.bold(),
                                e.default_binding.as_deref().unwrap_or("-"),
                                allow
                            );
                        }
                        other => {
                            println!("  {other} {}", e.name);
                        }
                    }
                }
                println!();
                println!(
                    "   export: {}  import: {}",
                    "locus graph export --out team.locusgraph".cyan(),
                    "locus graph import team.locusgraph".cyan()
                );
            }
            Ok(())
        }
        GraphCmd::Export { bindings, out } => {
            let passphrase = resolve_passphrase().context("graph passphrase")?;
            let out_path = out.unwrap_or_else(|| PathBuf::from(default_export_filename()));
            let aliases = bindings.as_deref();
            let result = s
                .graph_export(aliases, &out_path, passphrase.as_str())
                .context("graph export")?;
            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                println!(
                    "{} exported {} binding(s), {} workspace(s)",
                    "ok".green().bold(),
                    result.binding_aliases.len(),
                    result.workspace_names.len()
                );
                println!("   {}", result.path.cyan());
                println!(
                    "   bindings: {}",
                    result.binding_aliases.join(", ").dimmed()
                );
                if !result.workspace_names.is_empty() {
                    println!(
                        "   workspaces: {}",
                        result.workspace_names.join(", ").dimmed()
                    );
                }
                println!(
                    "   {}",
                    "secrets not included — importers must wire Phantom / env refs".dimmed()
                );
            }
            Ok(())
        }
        GraphCmd::Import { path, force } => {
            let passphrase = resolve_passphrase().context("graph passphrase")?;
            let result = s
                .graph_import(&path, passphrase.as_str(), force)
                .context("graph import")?;
            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                println!(
                    "{} imported {} binding(s), {} workspace(s)",
                    "ok".green().bold(),
                    result.bindings_imported.len(),
                    result.workspaces_imported.len()
                );
                if !result.bindings_imported.is_empty() {
                    println!(
                        "   bindings: {}",
                        result.bindings_imported.join(", ").cyan()
                    );
                }
                if !result.bindings_skipped.is_empty() {
                    println!(
                        "   skipped (exists, use --force): {}",
                        result.bindings_skipped.join(", ").yellow()
                    );
                }
                if !result.workspaces_imported.is_empty() {
                    println!(
                        "   workspaces: {}",
                        result.workspaces_imported.join(", ").cyan()
                    );
                }
                if !result.workspaces_skipped.is_empty() {
                    println!(
                        "   workspaces skipped: {}",
                        result.workspaces_skipped.join(", ").yellow()
                    );
                }
                println!(
                    "   {}",
                    "wire Phantom secrets for each credential_ref before pin".dimmed()
                );
            }
            Ok(())
        }
    }
}

fn cmd_engagement(sub: EngagementCmd, json: bool) -> Result<()> {
    let s = store()?;
    match sub {
        EngagementCmd::Init {
            alias,
            tenant,
            workspace,
            no_readme,
            force,
        } => {
            let result =
                s.engagement_init(&alias, &tenant, &cwd(), workspace, !no_readme, force)?;
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "ok": true,
                        "alias": result.alias,
                        "tenant": result.tenant,
                        "binding_path": result.binding_path.display().to_string(),
                        "workspace_path": result.workspace_path.as_ref().map(|p| p.display().to_string()),
                        "readme_path": result.readme_path.as_ref().map(|p| p.display().to_string()),
                        "credentials": result.credentials,
                    })
                );
            } else {
                println!(
                    "{} engagement {} ({})",
                    "ok".green().bold(),
                    result.alias.cyan().bold(),
                    result.tenant.yellow()
                );
                println!("   binding   {}", result.binding_path.display());
                if let Some(wp) = &result.workspace_path {
                    println!("   workspace {}", wp.display());
                }
                if let Some(rp) = &result.readme_path {
                    println!("   readme    {}", rp.display());
                }
                println!("   credentials  locators retained only in the binding file");
                println!();
                println!("next:");
                println!(
                    "  edit scopes in {}",
                    result.binding_path.display().to_string().dimmed()
                );
                println!("  {}", format!("locus enter {}", result.alias).cyan());
                println!("  {}", "locus whoami".dimmed());
            }
            Ok(())
        }
        EngagementCmd::Close { alias, archive } => {
            let result = s.engagement_close(&alias, archive)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                println!(
                    "{} closed {} ({})",
                    "ok".green().bold(),
                    result.alias.cyan().bold(),
                    result.tenant.yellow()
                );
                println!("   closed_at {}", result.closed_at.dimmed());
                if result.left_session {
                    println!("   pin       left active session for this binding");
                }
                if let Some(ap) = &result.archive_path {
                    println!("   archive   {ap}");
                }
                println!();
                println!(
                    "{}",
                    "checklist (manual — Locus does not delete vault secrets):".bold()
                );
                for (i, item) in result.checklist.iter().enumerate() {
                    println!("   {}. {item}", i + 1);
                }
            }
            Ok(())
        }
    }
}

fn cmd_whoami(json: bool) -> Result<()> {
    let s = store()?;
    // Detect drift before reporting
    let _ = s.check_drift_and_freeze();
    let w = s.whoami()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&w)?);
        return Ok(());
    }
    println!(
        "{} {}  tenant={}  session={}",
        "locus".magenta().bold(),
        w.binding_alias.cyan().bold(),
        w.tenant.yellow(),
        w.session_id.dimmed()
    );
    if let Some(p) = &w.principal {
        println!("  principal {}", p);
    }
    println!(
        "  expires   {}  {}",
        w.expires_at,
        format!(
            "(in {})",
            human_dur(chrono::Duration::seconds(w.expires_in_secs))
        )
        .dimmed()
    );
    if w.expires_in_secs > 0 && w.expires_in_secs < 300 {
        println!(
            "            {}",
            format!(
                "expiring soon — re-pin: locus enter {} --ttl 2h",
                w.binding_alias
            )
            .yellow()
        );
    }
    println!(
        "  seal      {}",
        if w.seal_ok { "ok".green() } else { "BAD".red() }
    );
    if w.frozen {
        println!(
            "  frozen    {}  reason={}",
            "YES".red().bold(),
            w.frozen_reason.as_deref().unwrap_or("re-pin")
        );
        println!(
            "            {}",
            "session_frozen: re-pin — run `locus leave` then `locus pin <alias>`".yellow()
        );
    }
    if w.mode == "namespaced" || !w.namespaces.is_empty() {
        let mut all = vec![w.binding_alias.clone()];
        all.extend(w.namespaces.iter().cloned());
        println!(
            "  mode      {}  namespaces={}",
            "namespaced".yellow(),
            all.join(",")
        );
    }
    println!("  providers");
    for p in &w.providers {
        let mut bits = vec![format!("account={}", p.account)];
        if let Some(r) = &p.project_ref {
            bits.push(format!("project_ref={r}"));
        }
        if let Some(t) = &p.team_id {
            bits.push(format!("team_id={t}"));
        }
        if let Some(a) = &p.account_id {
            bits.push(format!("account_id={a}"));
        }
        if let Some(true) = p.read_only {
            bits.push("read_only".into());
        }
        if !p.orgs.is_empty() {
            bits.push(format!("orgs={}", p.orgs.join(",")));
        }
        println!(
            "    {}  {}  {}",
            p.provider.cyan(),
            bits.join("  ").dimmed(),
            format!("credential={}", p.credential.source).dimmed()
        );
    }
    Ok(())
}

fn cmd_status(oneline: bool, json: bool) -> Result<()> {
    let s = store()?;
    let _ = s.check_drift_and_freeze();
    let require_pin = find_workspace(&cwd())?
        .map(|(_, cfg)| cfg.require_pin)
        .unwrap_or(false);
    match s
        .active_session()
        .map_err(|e| wedged_session_recovery(anyhow::Error::new(e)))?
    {
        None => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "pinned": false,
                        "require_pin": require_pin,
                    })
                );
            } else if oneline {
                // Hook-friendly tokens:
                //   unpinned | require_pin | frozen | invalid | alias:tenant
                if require_pin {
                    println!("require_pin");
                } else {
                    println!("unpinned");
                }
            } else {
                if require_pin {
                    println!(
                        "{} unpinned — workspace require_pin: run `locus enter <alias>`",
                        "!".red().bold()
                    );
                } else {
                    println!(
                        "{} unpinned — run `locus enter <alias>`",
                        "!".yellow().bold()
                    );
                }
            }
        }
        Some(session) => {
            let key = s.seal_key()?;
            let seal_ok = session.verify_seal(&key).is_ok();
            let frozen = session.frozen;
            let ok = seal_ok && !frozen && !session.is_expired();
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "pinned": true,
                        "binding": session.binding_alias,
                        "tenant": session.tenant,
                        "session_id": session.session_id,
                        "seal_ok": seal_ok,
                        "frozen": frozen,
                        "frozen_reason": session.frozen_reason,
                        "expired": session.is_expired(),
                        "require_pin": require_pin,
                        "mode": if session.is_namespaced() { "namespaced" } else { "exclusive" },
                        "namespaces": session.all_aliases(),
                    })
                );
            } else if oneline {
                if frozen {
                    println!("frozen");
                } else if !ok {
                    println!("invalid");
                } else {
                    println!("{}:{}", session.binding_alias, session.tenant);
                }
            } else {
                let mark = if frozen {
                    "FROZEN".red()
                } else if ok {
                    "ok".green()
                } else {
                    "INVALID".red()
                };
                println!(
                    "{} {} ({})  {}",
                    mark,
                    session.binding_alias.cyan().bold(),
                    session.tenant,
                    session.session_id.dimmed()
                );
                if frozen {
                    println!(
                        "  session_frozen: re-pin ({})",
                        session.frozen_reason.as_deref().unwrap_or("binding_drift")
                    );
                }
            }
            if !ok {
                std::process::exit(2);
            }
        }
    }
    Ok(())
}

fn cmd_exec(cmd: Vec<String>, resolve_secrets: bool, strict_creds: bool) -> Result<()> {
    if cmd.is_empty() {
        bail!("usage: locus exec -- <command> [args...]");
    }
    // Allow `locus exec -- foo` style; clap already collected trailing
    let mut args = cmd;
    if args.first().map(|s| s.as_str()) == Some("--") {
        args.remove(0);
    }
    if args.is_empty() {
        bail!("usage: locus exec -- <command> [args...]");
    }

    let s = store()?;
    if let Some(session) = delegated_child_session(&s)? {
        if resolve_secrets {
            bail!(
                "locus exec cannot resolve credentials inside a delegated session; use --no-resolve"
            );
        }
        let binding = s.load_binding(&session.binding_alias)?;
        return run_exact_session_child(
            &s,
            &session,
            &binding,
            &args,
            ChildLaunchSurface::Exec,
            "session.exec.delegated",
        );
    }
    let session = s
        .require_active()
        .map_err(|e| wedged_session_recovery(anyhow::Error::new(e)))
        .context("need active pin for exec")?;
    let binding = s.load_binding(&session.binding_alias)?;
    preflight_child_launch(&binding, resolve_secrets, ChildLaunchSurface::Exec)?;

    // Drift check before privileged exec
    let drift = s
        .check_drift_and_freeze()
        .map_err(|e| wedged_session_recovery(anyhow::Error::new(e)))?;
    if drift.frozen {
        bail!(
            "session_frozen: re-pin — binding drifted under active pin ({})",
            drift.issues.join(", ")
        );
    }
    let iso = isolated_child_env(&s, &session, &binding, resolve_secrets);
    if strict_creds && !iso.secrets_failed.is_empty() {
        bail!(
            "credential resolve failed: {}",
            format_credential_issues(&iso.secrets_failed)
        );
    }
    let executor_capability = s.grant_executor_capability(&session)?;

    let program = &args[0];
    let rest = &args[1..];

    let mut child = Command::new(program);
    child
        .args(rest)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .env_clear();
    for (k, v) in &iso.vars {
        child.env(k, v);
    }
    child.env(locus_core::EXECUTOR_CAPABILITY_ENV, executor_capability);

    eprintln!(
        "{} exec as {} ({}) — scrubbed {} ambient · resolved {} secret(s)",
        "->".dimmed(),
        session.binding_alias.cyan(),
        session.tenant.dimmed(),
        iso.scrubbed.len(),
        iso.secrets_resolved
    );
    if !iso.secrets_failed.is_empty() {
        eprintln!(
            "{} unresolved credentials: {}",
            "warn".yellow(),
            format_credential_issues(&iso.secrets_failed)
        );
    }

    let status = child.status().with_context(|| format!("spawn {program}"))?;
    s.audit(
        "session.exec",
        &session.binding_alias,
        Some(serde_json::json!({
            "cmd": args,
            "exit": status.code(),
            "secrets_resolved": iso.secrets_resolved,
            // names only — never values
            "secrets_failed": iso.secrets_failed,
        })),
    )?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

fn cmd_ci(sub: CiCmd, json: bool) -> Result<()> {
    match sub {
        CiCmd::Mint {
            binding,
            ttl,
            force,
            resolve,
        } => cmd_ci_mint(binding, ttl, force, resolve, json),
        CiCmd::Env {
            binding,
            ttl,
            force,
            resolve,
        } => cmd_ci_env(binding, ttl, force, resolve),
        CiCmd::Run {
            binding,
            ttl,
            force,
            no_resolve,
            strict_creds,
            cmd,
        } => cmd_ci_run(binding, ttl, force, !no_resolve, strict_creds, cmd),
    }
}

/// Mint a CI session and print identity JSON (primary contract for pipelines).
fn cmd_ci_mint(
    binding_alias: String,
    ttl: String,
    force: bool,
    resolve: bool,
    _json: bool,
) -> Result<()> {
    require_local_control_boundary("locus ci mint")?;
    let s = store()?;
    let ttl_dur = parse_ttl(&ttl).context("parse --ttl")?;
    let (session, path) = s
        .create_ci_session(&binding_alias, &cwd(), force, Some(ttl_dur))
        .with_context(|| format!("mint CI session for `{binding_alias}`"))?;
    let binding = s.load_binding(&session.binding_alias)?;

    if resolve && !ci_secrets_allowed() {
        eprintln!(
            "{} --resolve ignored: set LOCUS_CI_ALLOW_SECRETS=1 to include secrets",
            "warn".yellow()
        );
    }
    let mut env_map = build_ci_env_map(&session, &binding, resolve);
    env_map.insert(
        locus_core::EXECUTOR_CAPABILITY_ENV.into(),
        s.grant_executor_capability(&session)?,
    );
    let secrets_in_output = resolve && ci_secrets_allowed();

    // Always emit JSON for mint (machine-first); `--json` is accepted for consistency.
    println!(
        "{}",
        json!({
            "session_id": session.session_id,
            "binding": session.binding_alias,
            "binding_id": session.binding_id,
            "tenant": session.tenant,
            "expires_at": session.expires_at.to_rfc3339(),
            "seal": session.seal,
            "path": path.display().to_string(),
            "worker_home": session.worker_home,
            "secrets_resolved": secrets_in_output,
            "env": env_map,
        })
    );
    Ok(())
}

/// Mint a CI session and print shell `export` lines (eval-friendly).
fn cmd_ci_env(binding_alias: String, ttl: String, force: bool, resolve: bool) -> Result<()> {
    require_local_control_boundary("locus ci env")?;
    let s = store()?;
    let ttl_dur = parse_ttl(&ttl).context("parse --ttl")?;
    let (session, _path) = s
        .create_ci_session(&binding_alias, &cwd(), force, Some(ttl_dur))
        .with_context(|| format!("mint CI session for `{binding_alias}`"))?;
    let binding = s.load_binding(&session.binding_alias)?;

    if resolve && !ci_secrets_allowed() {
        eprintln!(
            "{} --resolve ignored: set LOCUS_CI_ALLOW_SECRETS=1 to include secrets",
            "warn".yellow()
        );
    }
    let mut env_map = build_ci_env_map(&session, &binding, resolve);
    env_map.insert(
        locus_core::EXECUTOR_CAPABILITY_ENV.into(),
        s.grant_executor_capability(&session)?,
    );
    for (k, v) in &env_map {
        // Shell-safe single-quoted export; escape embedded single quotes.
        let escaped = v.replace('\'', "'\\''");
        println!("export {k}='{escaped}'");
    }
    Ok(())
}

/// Mint temp CI session + exec command with isolated env, then leave session file.
fn cmd_ci_run(
    binding_alias: String,
    ttl: String,
    force: bool,
    resolve_secrets: bool,
    strict_creds: bool,
    cmd: Vec<String>,
) -> Result<()> {
    if cmd.is_empty() {
        bail!("usage: locus ci run -b <alias> -- <command> [args...]");
    }
    let mut args = cmd;
    if args.first().map(|s| s.as_str()) == Some("--") {
        args.remove(0);
    }
    if args.is_empty() {
        bail!("usage: locus ci run -b <alias> -- <command> [args...]");
    }

    let s = store()?;
    if let Some(session) = delegated_child_session(&s)? {
        if binding_alias != session.binding_alias && binding_alias != session.binding_id {
            bail!(
                "locus ci run cannot select binding `{binding_alias}` from exact session bound to `{}`",
                session.binding_alias
            );
        }
        if force {
            bail!("locus ci run --force is unavailable inside a delegated session");
        }
        if resolve_secrets {
            bail!(
                "locus ci run cannot resolve credentials inside a delegated session; use --no-resolve"
            );
        }
        let binding = s.load_binding(&session.binding_alias)?;
        return run_exact_session_child(
            &s,
            &session,
            &binding,
            &args,
            ChildLaunchSurface::CiRun,
            "ci.run.delegated",
        );
    }
    let binding = s.load_binding(&binding_alias)?;
    preflight_child_launch(&binding, resolve_secrets, ChildLaunchSurface::CiRun)?;

    let ttl_dur = parse_ttl(&ttl).context("parse --ttl")?;
    // Snapshot parent pin (active.json / LOCUS_SESSION_ID) before mint.
    let parent_before = s.active_session()?;

    let (session, ci_path) = s
        .create_ci_session(&binding_alias, &cwd(), force, Some(ttl_dur))
        .with_context(|| format!("mint CI session for `{binding_alias}`"))?;

    // Parent pin must remain intact (create_ci_session never writes active.json
    // and does not set LOCUS_SESSION_ID in this process).
    let parent_after = s.active_session()?;
    match (&parent_before, &parent_after) {
        (None, None) => {}
        (Some(a), Some(b)) if a.session_id == b.session_id => {}
        _ => {
            let _ = s.cleanup_ci_session(&ci_path, &session);
            bail!("internal: ci session mutated global pin unexpectedly");
        }
    }

    // Child gets full isolated env (may resolve secrets for the command to work).
    let iso = isolated_child_env(&s, &session, &binding, resolve_secrets);
    if strict_creds && !iso.secrets_failed.is_empty() {
        let _ = s.cleanup_ci_session(&ci_path, &session);
        bail!(
            "credential resolve failed: {}",
            format_credential_issues(&iso.secrets_failed)
        );
    }
    let executor_capability = s.grant_executor_capability(&session)?;

    let program = &args[0];
    let rest = &args[1..];

    let mut child = Command::new(program);
    child
        .args(rest)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .env_clear();
    for (k, v) in &iso.vars {
        child.env(k, v);
    }
    child.env(locus_core::EXECUTOR_CAPABILITY_ENV, executor_capability);

    eprintln!(
        "{} ci run as {} ({}) — ttl_until={} · scrubbed {} · resolved {} secret(s)",
        "->".dimmed(),
        session.binding_alias.cyan(),
        session.tenant.dimmed(),
        session.expires_at.to_rfc3339().dimmed(),
        iso.scrubbed.len(),
        iso.secrets_resolved
    );
    if !iso.secrets_failed.is_empty() {
        eprintln!(
            "{} unresolved credentials: {}",
            "warn".yellow(),
            format_credential_issues(&iso.secrets_failed)
        );
    }

    let status = child.status().with_context(|| format!("spawn {program}"))?;
    s.audit(
        "ci.run",
        &session.binding_alias,
        Some(serde_json::json!({
            "cmd": args,
            "exit": status.code(),
            "session_id": session.session_id,
            "secrets_resolved": iso.secrets_resolved,
            "secrets_failed": iso.secrets_failed,
        })),
    )?;

    let _ = s.cleanup_ci_session(&ci_path, &session);

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

/// One-shot temporary pin for a child command. Does not overwrite active.json
/// unless `--share-pin` is set.
fn cmd_run(
    binding_alias: String,
    share_pin: bool,
    cmd: Vec<String>,
    resolve_secrets: bool,
    strict_creds: bool,
    force: bool,
) -> Result<()> {
    if cmd.is_empty() {
        bail!("usage: locus run -b <alias> -- <command> [args...]");
    }
    let mut args = cmd;
    if args.first().map(|s| s.as_str()) == Some("--") {
        args.remove(0);
    }
    if args.is_empty() {
        bail!("usage: locus run -b <alias> -- <command> [args...]");
    }

    let s = store()?;
    if let Some(session) = delegated_child_session(&s)? {
        if binding_alias != session.binding_alias && binding_alias != session.binding_id {
            bail!(
                "locus run cannot select binding `{binding_alias}` from exact session bound to `{}`",
                session.binding_alias
            );
        }
        if force {
            bail!("locus run --force is unavailable inside a delegated session");
        }
        if share_pin {
            bail!("locus run --share-pin is unavailable inside a delegated session");
        }
        if resolve_secrets {
            bail!(
                "locus run cannot resolve credentials inside a delegated session; use --no-resolve"
            );
        }
        let binding = s.load_binding(&session.binding_alias)?;
        return run_exact_session_child(
            &s,
            &session,
            &binding,
            &args,
            ChildLaunchSurface::Run,
            "session.run.delegated",
        );
    }
    let binding = s.load_binding(&binding_alias)?;
    preflight_child_launch(&binding, resolve_secrets, ChildLaunchSurface::Run)?;
    // Capture parent pin (if any) so we can prove it is unchanged after run
    // when share_pin is false.
    let parent_before = s.active_session()?;

    let suffix = format!("{}", std::process::id());
    let (session, run_path) = s
        .create_run_session(
            &binding_alias,
            &cwd(),
            Some("run".into()),
            force,
            share_pin,
            &suffix,
        )
        .with_context(|| format!("create run session for `{binding_alias}`"))?;

    if !share_pin {
        // Parent pin must remain intact
        let parent_after = s.active_session()?;
        match (&parent_before, &parent_after) {
            (None, None) => {}
            (Some(a), Some(b)) if a.session_id == b.session_id => {}
            _ if parent_before.is_none() && parent_after.is_none() => {}
            _ => {
                // Should not happen — fail closed and cleanup
                let _ = s.cleanup_run_session(&run_path, &session);
                bail!("internal: run session mutated global pin unexpectedly");
            }
        }
    }

    let iso = isolated_child_env(&s, &session, &binding, resolve_secrets);
    if strict_creds && !iso.secrets_failed.is_empty() {
        let _ = s.cleanup_run_session(&run_path, &session);
        bail!(
            "credential resolve failed: {}",
            format_credential_issues(&iso.secrets_failed)
        );
    }
    let executor_capability = s.grant_executor_capability(&session)?;

    // Ensure composite workers when upstream is present (best-effort for CLI run).
    // Child process gets LOCUS_* env; upstream MCP is primarily for locus-mcp.
    {
        use locus_core::CompositeWorkerManager;
        if binding.providers.iter().any(|p| p.has_upstream()) {
            let mut mgr = CompositeWorkerManager::new();
            if let Err(e) = mgr.ensure_binding(&session, &binding) {
                eprintln!(
                    "{} worker ensure (upstream) soft-failed: {e}",
                    "warn".yellow()
                );
            }
            // Drop tears down children after ensure probe; child cmd uses env only.
            drop(mgr);
        }
    }

    let program = &args[0];
    let rest = &args[1..];

    let mut child = Command::new(program);
    child
        .args(rest)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .env_clear();
    for (k, v) in &iso.vars {
        child.env(k, v);
    }
    child.env(locus_core::EXECUTOR_CAPABILITY_ENV, executor_capability);

    eprintln!(
        "{} run as {} ({}) — temporary={} · scrubbed {} · resolved {} secret(s)",
        "->".dimmed(),
        session.binding_alias.cyan(),
        session.tenant.dimmed(),
        if share_pin {
            "false(--share-pin)"
        } else {
            "true"
        },
        iso.scrubbed.len(),
        iso.secrets_resolved
    );
    if !iso.secrets_failed.is_empty() {
        eprintln!(
            "{} unresolved credentials: {}",
            "warn".yellow(),
            format_credential_issues(&iso.secrets_failed)
        );
    }

    let status = child.status().with_context(|| format!("spawn {program}"))?;
    s.audit(
        "session.run",
        &session.binding_alias,
        Some(serde_json::json!({
            "cmd": args,
            "exit": status.code(),
            "share_pin": share_pin,
            "session_id": session.session_id,
            "secrets_resolved": iso.secrets_resolved,
            "secrets_failed": iso.secrets_failed,
        })),
    )?;

    // Cleanup temporary run session (and worker home) unless share_pin (active owns it).
    if !share_pin {
        let _ = s.cleanup_run_session(&run_path, &session);
    }

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

/// Execute a credential-free child without minting or switching sessions.
/// This is the only raw-child path available to an agent-selected session.
fn run_exact_session_child(
    s: &Store,
    session: &Session,
    binding: &Binding,
    args: &[String],
    surface: ChildLaunchSurface,
    audit_op: &str,
) -> Result<()> {
    preflight_child_launch(binding, false, surface)?;

    let drift = s.check_drift_and_freeze()?;
    if !drift.ok {
        bail!(
            "delegated session is not healthy: {}",
            drift.issues.join(", ")
        );
    }

    let iso = isolated_child_env(s, session, binding, false);
    if iso.secrets_resolved != 0 {
        bail!("internal: delegated no-resolve child resolved credentials");
    }
    let executor_capability = env::var(locus_core::EXECUTOR_CAPABILITY_ENV)
        .context("delegated child requires a supervised executor capability")?;

    let program = &args[0];
    let mut child = Command::new(program);
    child
        .args(&args[1..])
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .env_clear();
    for (k, v) in &iso.vars {
        child.env(k, v);
    }
    child.env(locus_core::EXECUTOR_CAPABILITY_ENV, executor_capability);

    let status = child.status().with_context(|| format!("spawn {program}"))?;
    s.audit(
        audit_op,
        &session.binding_alias,
        Some(json!({
            "cmd": args,
            "exit": status.code(),
            "session_id": session.session_id,
            "secrets_resolved": 0,
            "exact_session": true,
        })),
    )?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChildLaunchSurface {
    Exec,
    Run,
    CiRun,
}

impl ChildLaunchSurface {
    const fn command_name(self) -> &'static str {
        match self {
            Self::Exec => "locus exec",
            Self::Run => "locus run",
            Self::CiRun => "locus ci run",
        }
    }
}

/// Central fail-closed guard for every user-command child launch.
///
/// Recipe defaults are expanded before classification so `--no-resolve`
/// cannot be bypassed by omitting `resolve_secrets` from a recipe declaration.
/// Callers must invoke this before environment construction, worker startup,
/// session creation/mutation, or requested child startup.
fn preflight_child_launch(
    binding: &Binding,
    resolve_secrets: bool,
    surface: ChildLaunchSurface,
) -> Result<()> {
    if resolve_secrets {
        return Ok(());
    }
    let resolving_upstreams = credential_resolving_upstreams(binding).with_context(|| {
        format!(
            "inspect upstreams for {} --no-resolve",
            surface.command_name()
        )
    })?;
    if resolving_upstreams.is_empty() {
        return Ok(());
    }
    bail!(
        "--no-resolve refused {} for binding `{}`: credential-resolving upstream(s) declared for {}; no child or upstream worker was started and no session or credential effect occurred",
        surface.command_name(),
        binding.alias,
        resolving_upstreams.join(", ")
    )
}

/// Providers whose upstream worker would independently resolve credentials.
fn credential_resolving_upstreams(binding: &Binding) -> Result<Vec<String>> {
    let mut providers = Vec::new();
    for provider in &binding.providers {
        let Some(upstream) = provider.upstream.as_ref().filter(|u| u.is_declared()) else {
            continue;
        };
        if upstream.expand()?.resolve_secrets {
            providers.push(provider.provider.clone());
        }
    }
    Ok(providers)
}

fn cmd_mcp() -> Result<()> {
    // Re-exec locus-mcp if on PATH; otherwise tell user to run the binary.
    let status = Command::new("locus-mcp")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();
    match status {
        Ok(s) => {
            if !s.success() {
                std::process::exit(s.code().unwrap_or(1));
            }
            Ok(())
        }
        Err(_) => {
            // Fallback: try same directory as current exe
            if let Ok(exe) = env::current_exe() {
                if let Some(dir) = exe.parent() {
                    let candidate = dir.join("locus-mcp");
                    if candidate.exists() {
                        let s = Command::new(candidate)
                            .stdin(Stdio::inherit())
                            .stdout(Stdio::inherit())
                            .stderr(Stdio::inherit())
                            .status()?;
                        if !s.success() {
                            std::process::exit(s.code().unwrap_or(1));
                        }
                        return Ok(());
                    }
                }
            }
            bail!(
                "locus-mcp not found on PATH — install with: cargo install --path crates/locus-mcp"
            );
        }
    }
}

fn cmd_mcp_sub(sub: McpCmd, json_flag: bool) -> Result<()> {
    match sub {
        McpCmd::Mint {
            binding,
            ttl,
            label,
            force,
            json,
        } => cmd_mcp_mint(binding, ttl, label, force, json || json_flag),
        McpCmd::List { json } => cmd_mcp_list(json || json_flag),
        McpCmd::Revoke {
            grant_id,
            binding,
            all,
        } => cmd_mcp_revoke(grant_id, binding, all),
    }
}

/// Mint a multi-tenant MCP grant. The bearer token is printed exactly once
/// (JSON, machine-first — mirrors `locus ci mint`); at rest only its HMAC.
fn cmd_mcp_mint(
    binding_alias: String,
    ttl: String,
    label: Option<String>,
    force: bool,
    _json: bool,
) -> Result<()> {
    require_local_control_boundary("locus mcp mint")?;
    let s = store()?;
    let ttl_dur = parse_ttl(&ttl).context("parse --ttl")?;
    let (session, grant, token) = s
        .create_mcp_grant(&binding_alias, &cwd(), Some(ttl_dur), label, force)
        .with_context(|| format!("mint MCP grant for `{binding_alias}`"))?;
    // Always JSON (machine-first). Token appears here exactly once.
    println!(
        "{}",
        json!({
            "grant_id": grant.grant_id,
            "token": token,
            "session_id": session.session_id,
            "binding": session.binding_alias,
            "tenant": session.tenant,
            "expires_at": grant.expires_at.to_rfc3339(),
            "label": grant.label,
            "serve": "locus-mcp --http --multi-tenant  (client header: X-Locus-Tenant-Token)",
        })
    );
    eprintln!(
        "{} token shown once — only its HMAC is stored under mcp-grants/",
        "note".yellow()
    );
    Ok(())
}

/// Count live (non-expired) tenant HTTP sessions per grant from the
/// multi-tenant session dir — the same partition the server writes
/// (`locus_core::http_sessions::http_session_dir_mt()`, which honors
/// `LOCUS_MCP_SESSION_DIR` with its `-mt` suffix).
fn mt_live_sessions_by_grant(
    dir: Option<&std::path::Path>,
) -> std::collections::HashMap<String, usize> {
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let Some(dir) = dir else {
        return counts;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return counts;
    };
    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    for ent in entries.flatten() {
        let path = ent.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        // Expired records are dead sessions awaiting sweep — never "live".
        let Some(last_seen) = v.get("last_seen_unix").and_then(|x| x.as_u64()) else {
            continue;
        };
        if locus_core::http_sessions::http_session_record_expired(
            last_seen,
            now_unix,
            locus_core::http_sessions::DEFAULT_HTTP_SESSION_TTL,
        ) {
            continue;
        }
        if let Some(gid) = v
            .get("tenant")
            .and_then(|t| t.get("grant_id"))
            .and_then(|g| g.as_str())
        {
            *counts.entry(gid.to_string()).or_insert(0) += 1;
        }
    }
    counts
}

/// Operator-only grant roster (grant_id / alias / tenant / expiry / sessions).
/// Values-free: never tokens or credentials.
fn cmd_mcp_list(json: bool) -> Result<()> {
    require_local_control_boundary("locus mcp list")?;
    let s = store()?;
    let grants = s.list_mcp_grants()?;
    let live =
        mt_live_sessions_by_grant(locus_core::http_sessions::http_session_dir_mt().as_deref());
    if json {
        let rows: Vec<serde_json::Value> = grants
            .iter()
            .map(|g| {
                json!({
                    "grant_id": g.grant_id,
                    "binding": g.binding_alias,
                    "tenant": g.tenant,
                    "label": g.label,
                    "expires_at": g.expires_at.to_rfc3339(),
                    "expired": g.is_expired(),
                    "revoked": g.revoked,
                    "session_id": g.session_id,
                    "live_http_sessions": live.get(&g.grant_id).copied().unwrap_or(0),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if grants.is_empty() {
        println!(
            "{} no MCP grants (mint: `locus mcp mint --binding <alias>`)",
            "->".dimmed()
        );
        return Ok(());
    }
    println!("{} multi-tenant MCP grants:\n", "locus mcp".cyan().bold());
    for g in &grants {
        let state = if g.revoked {
            "revoked".red().to_string()
        } else if g.is_expired() {
            "expired".yellow().to_string()
        } else {
            "active".green().to_string()
        };
        println!(
            "  {}  {}  ·  {}  ·  expires {}  ·  {} live session(s){}",
            g.grant_id.green().bold(),
            g.binding_alias,
            state,
            g.expires_at.to_rfc3339().dimmed(),
            live.get(&g.grant_id).copied().unwrap_or(0),
            g.label
                .as_deref()
                .map(|l| format!("  ·  {}", l.dimmed()))
                .unwrap_or_default(),
        );
    }
    Ok(())
}

/// Revoke grants and sweep their tenant HTTP session records.
fn cmd_mcp_revoke(grant_id: Option<String>, binding: Option<String>, all: bool) -> Result<()> {
    require_local_control_boundary("locus mcp revoke")?;
    let s = store()?;
    let targets: Vec<String> = if all {
        s.list_mcp_grants()?
            .iter()
            .map(|g| g.grant_id.clone())
            .collect()
    } else if let Some(alias) = binding {
        s.list_mcp_grants()?
            .iter()
            .filter(|g| g.binding_alias == alias)
            .map(|g| g.grant_id.clone())
            .collect()
    } else if let Some(id) = grant_id {
        vec![id]
    } else {
        bail!("specify a <grant_id>, --binding <alias>, or --all");
    };
    if targets.is_empty() {
        println!("{} no matching grants", "->".dimmed());
        return Ok(());
    }
    for id in &targets {
        match s.revoke_mcp_grant(id)? {
            Some(g) => {
                // Best-effort sweep of this grant's tenant session records
                // (same MT partition the server writes; expired records of
                // the grant are removed too).
                if let Some(entries) = locus_core::http_sessions::http_session_dir_mt()
                    .and_then(|dir| std::fs::read_dir(dir).ok())
                {
                    for ent in entries.flatten() {
                        let path = ent.path();
                        if path.extension().and_then(|e| e.to_str()) != Some("json") {
                            continue;
                        }
                        let Ok(raw) = std::fs::read_to_string(&path) else {
                            continue;
                        };
                        let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
                            continue;
                        };
                        if v.get("tenant")
                            .and_then(|t| t.get("grant_id"))
                            .and_then(|gid| gid.as_str())
                            == Some(id.as_str())
                        {
                            let _ = std::fs::remove_file(&path);
                        }
                    }
                }
                println!(
                    "{} revoked grant {} (binding `{}`)",
                    "ok".green(),
                    g.grant_id,
                    g.binding_alias
                );
            }
            None => println!(
                "{} unknown grant `{id}` (already revoked?)",
                "warn".yellow()
            ),
        }
    }
    Ok(())
}

#[cfg(test)]
mod mt_session_reconcile_tests {
    use super::mt_live_sessions_by_grant;

    fn write_record(dir: &std::path::Path, name: &str, last_seen_unix: u64, grant_id: &str) {
        let rec = serde_json::json!({
            "v": 1,
            "id": "a".repeat(32),
            "created_at_unix": last_seen_unix,
            "last_seen_unix": last_seen_unix,
            "tenant": {
                "grant_id": grant_id,
                "session_id": "sess",
                "binding_alias": "acme",
                "tenant": "acme-corp",
            },
        });
        std::fs::write(dir.join(name), rec.to_string()).unwrap();
    }

    #[test]
    fn live_counts_exclude_expired_records_and_missing_dir_is_zero() {
        let dir = tempfile::tempdir().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let ttl = locus_core::http_sessions::DEFAULT_HTTP_SESSION_TTL.as_secs();
        write_record(dir.path(), "live-1.json", now, "g1");
        write_record(dir.path(), "stale.json", now - ttl - 1, "g1");
        write_record(dir.path(), "live-2.json", now, "g2");
        // Non-record noise is ignored.
        std::fs::write(dir.path().join("notes.txt"), "x").unwrap();

        let counts = mt_live_sessions_by_grant(Some(dir.path()));
        assert_eq!(counts.get("g1"), Some(&1), "expired record must not count");
        assert_eq!(counts.get("g2"), Some(&1));

        assert!(mt_live_sessions_by_grant(None).is_empty());
    }
}

fn cmd_adapter(sub: AdapterCmd, json: bool) -> Result<()> {
    match sub {
        AdapterCmd::List => {
            let providers = list_adapters().context("load built-in adapter registry")?;
            if json {
                println!("{}", serde_json::to_string_pretty(&providers)?);
                return Ok(());
            }
            if providers.is_empty() {
                println!("{} no built-in adapters", "->".dimmed());
                return Ok(());
            }
            println!(
                "{} built-in adapter registry (catalog only — not a plugin loader):\n",
                "locus adapter".cyan().bold()
            );
            for p in &providers {
                let name = if p.name.is_empty() {
                    p.id.clone()
                } else {
                    p.name.clone()
                };
                let syn = if p.synthetic { "synthetic" } else { "upstream" };
                println!(
                    "  {}  {}  ·  {}  ·  {}",
                    p.id.green().bold(),
                    name.dimmed(),
                    p.status.yellow(),
                    syn.dimmed()
                );
                if !p.tools.is_empty() {
                    println!("      tools: {}", p.tools.join(", ").cyan());
                }
                if !p.frozen_selectors.is_empty() {
                    println!("      freeze: {}", p.frozen_selectors.join(", ").yellow());
                }
                if !p.capabilities.is_empty() {
                    println!("      caps: {}", p.capabilities.join(", ").dimmed());
                }
                let sig = match (p.signature.as_deref(), p.signed_by.as_deref()) {
                    (Some(_), Some(by)) => format!("signed ({by})"),
                    (Some(_), None) => "signed (no key id)".into(),
                    _ => "unsigned".into(),
                };
                println!("      registry: {}", sig.dimmed());
                if !p.description.trim().is_empty() {
                    let first = p
                        .description
                        .lines()
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or("")
                        .trim();
                    if !first.is_empty() {
                        println!("      {}", first.dimmed());
                    }
                }
                println!();
            }
            println!(
                "{}",
                "Verify:  locus adapter verify [--require-signed] [--json]".dimmed()
            );
            println!(
                "{}",
                "Trust:   locus adapter trust list | trust add --id <id> --ed25519-pub <b64>"
                    .dimmed()
            );
            println!(
                "{}",
                "Schema:  schema/adapter-manifest.schema.json · docs/adapter-sdk.md".dimmed()
            );
            Ok(())
        }
        AdapterCmd::Verify {
            path,
            require_signed,
        } => {
            let (source_label, manifest) = if let Some(p) = path {
                let body = std::fs::read_to_string(&p)
                    .with_context(|| format!("read adapter manifest {}", p.display()))?;
                (
                    p.display().to_string(),
                    parse_manifest(&body).context("parse adapter manifest")?,
                )
            } else {
                (
                    "builtin:adapters/manifest.toml".into(),
                    builtin_manifest().context("load built-in adapter manifest")?,
                )
            };
            // Fresh merge of file + env each invocation (not process OnceLock).
            let store = Store::open_default().context("open LOCUS_HOME")?;
            let keys = load_merged_trust_keys(store.home());
            let report = verify_manifest_with_keys(&manifest, require_signed, &keys);
            if json {
                let mut v = serde_json::to_value(&report)?;
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("source".into(), json!(source_label));
                    obj.insert(
                        "trust_keys".into(),
                        json!(keys
                            .iter()
                            .map(|k| json!({
                                "id": k.id,
                                "scheme": k.scheme(),
                            }))
                            .collect::<Vec<_>>()),
                    );
                    obj.insert(
                        "trust_file".into(),
                        json!(adapter_trust_keys_path(store.home()).display().to_string()),
                    );
                }
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else {
                let verdict = if report.ok {
                    "ok".green().bold()
                } else {
                    "FAIL".red().bold()
                };
                println!(
                    "{} adapter registry verify  {}",
                    "locus adapter".cyan().bold(),
                    verdict
                );
                println!("  source          {}", source_label.dimmed());
                println!(
                    "  trust_keys      {}  ·  {}",
                    keys.len(),
                    adapter_trust_keys_path(store.home())
                        .display()
                        .to_string()
                        .dimmed()
                );
                println!(
                    "  providers       {}  ·  trusted {}  ·  unsigned {}  ·  failed {}",
                    report.provider_count, report.trusted, report.unsigned, report.failed
                );
                println!(
                    "  require_signed  {}",
                    if require_signed {
                        "true (fail closed)".yellow().to_string()
                    } else {
                        "false".dimmed().to_string()
                    }
                );
                println!();
                for e in &report.entries {
                    let st = e.status.as_str();
                    let colored = match st {
                        "valid" => st.green().to_string(),
                        "unsigned" => st.dimmed().to_string(),
                        "unknown_key" => st.yellow().to_string(),
                        _ => st.red().to_string(),
                    };
                    let by = e
                        .signed_by
                        .as_deref()
                        .map(|s| format!("  key={s}"))
                        .unwrap_or_default();
                    println!("  {:<16} {}{}", e.id.green(), colored, by.dimmed());
                    if let Some(d) = &e.detail {
                        if e.status.as_str() != "unsigned" {
                            println!("      {}", d.dimmed());
                        }
                    }
                }
                if !report.errors.is_empty() {
                    println!();
                    for err in &report.errors {
                        println!("  {} {}", "×".red(), err);
                    }
                }
                if !report.ok && require_signed {
                    println!();
                    println!(
                        "{}",
                        "hint: pin a registry root with `locus adapter trust add --id root --ed25519-pub <b64>`,"
                            .dimmed()
                    );
                    println!(
                        "{}",
                        "      or set LOCUS_ADAPTER_TRUST_KEYS; built-in catalog ships unsigned in v0"
                            .dimmed()
                    );
                }
            }
            if !report.ok {
                bail!("adapter registry verify failed");
            }
            Ok(())
        }
        AdapterCmd::Trust(trust_sub) => cmd_adapter_trust(trust_sub, json),
        AdapterCmd::Registry(registry_sub) => cmd_adapter_registry(registry_sub, json),
        AdapterCmd::VerifyManifest {
            file,
            allow_unsigned,
        } => cmd_adapter_verify_manifest(&file, allow_unsigned, json),
    }
}

fn cmd_adapter_registry(sub: AdapterRegistryCmd, json: bool) -> Result<()> {
    match sub {
        AdapterRegistryCmd::Export {
            out,
            sign,
            key,
            key_id,
        } => {
            let mut manifest =
                build_release_manifest().context("build adapter release manifest")?;

            if sign {
                // Key material comes from a file (--key) or the env var — it is
                // parsed in memory and NEVER printed, logged, or generated here.
                let raw = if let Some(key_path) = &key {
                    std::fs::read_to_string(key_path)
                        .with_context(|| format!("read signing key file {}", key_path.display()))?
                } else {
                    match env::var(LOCUS_REGISTRY_SIGNING_KEY_ENV) {
                        Ok(v) if !v.trim().is_empty() => v,
                        _ => bail!(
                            "--sign requires a signing key: pass --key <file> or set \
                             {LOCUS_REGISTRY_SIGNING_KEY_ENV} (refusing to export without one)"
                        ),
                    }
                };
                let signing_key =
                    parse_ed25519_signing_key(&raw).context("parse ed25519 signing key")?;
                sign_release_manifest(&mut manifest, &key_id, &signing_key);
            }

            let body = release_manifest_json(&manifest).context("serialize release manifest")?;
            let signed_label = if sign { "signed" } else { "unsigned" };

            if let Some(out_path) = out {
                std::fs::write(&out_path, &body)
                    .with_context(|| format!("write manifest {}", out_path.display()))?;
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&json!({
                            "ok": true,
                            "path": out_path.display().to_string(),
                            "signed": sign,
                            "signed_by": manifest.signed_by,
                            "locus_version": manifest.locus_version,
                            "adapter_count": manifest.adapters.len(),
                        }))?
                    );
                    return Ok(());
                }
                println!(
                    "{} exported {} registry manifest ({} adapters, locus {})",
                    "✓".green().bold(),
                    signed_label,
                    manifest.adapters.len(),
                    manifest.locus_version
                );
                println!("  path  {}", out_path.display().to_string().dimmed());
                if let Some(by) = &manifest.signed_by {
                    println!("  signed_by  {}", by.green());
                }
                println!(
                    "{}",
                    "Verify:  locus adapter verify-manifest <file> · Docs: docs/registry-trust.md"
                        .dimmed()
                );
            } else {
                // Manifest JSON goes to stdout as-is (pipe-friendly).
                print!("{body}");
            }
            Ok(())
        }
    }
}

fn cmd_adapter_verify_manifest(file: &Path, allow_unsigned: bool, json: bool) -> Result<()> {
    let body = std::fs::read_to_string(file)
        .with_context(|| format!("read release manifest {}", file.display()))?;
    let manifest = parse_release_manifest(&body).context("parse release manifest")?;

    // Fresh merge of file + env trust keys each invocation.
    let store = Store::open_default().context("open LOCUS_HOME")?;
    let keys = load_merged_trust_keys(store.home());
    let report = verify_release_manifest_with_keys(&manifest, &keys, allow_unsigned)
        .context("verify release manifest")?;

    if json {
        let mut v = serde_json::to_value(&report)?;
        if let Some(obj) = v.as_object_mut() {
            obj.insert("source".into(), json!(file.display().to_string()));
            obj.insert(
                "trust_file".into(),
                json!(adapter_trust_keys_path(store.home()).display().to_string()),
            );
        }
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        let verdict = if report.ok {
            "ok".green().bold()
        } else {
            "FAIL".red().bold()
        };
        println!(
            "{} release manifest verify  {}",
            "locus adapter".cyan().bold(),
            verdict
        );
        println!("  source     {}", file.display().to_string().dimmed());
        let sig = report.signature.as_str();
        let sig_colored = match report.signature {
            EntryVerifyStatus::Valid => sig.green().to_string(),
            EntryVerifyStatus::Unsigned => sig.yellow().to_string(),
            _ => sig.red().to_string(),
        };
        let by = report
            .signed_by
            .as_deref()
            .map(|s| format!("  key={s}"))
            .unwrap_or_default();
        println!("  signature  {}{}", sig_colored, by.dimmed());
        if let Some(d) = &report.signature_detail {
            println!("      {}", d.dimmed());
        }
        println!(
            "  versions   manifest {}  ·  binary {}",
            report.manifest_version, report.binary_version
        );
        println!("  adapters   {}", report.adapter_count);
        if allow_unsigned {
            println!(
                "  {}",
                "--allow-unsigned: drift check only (bad signatures still fail)".yellow()
            );
        }
        if report.drift.is_empty() {
            println!("  drift      {}", "none — adapter set matches".green());
        } else {
            println!(
                "  drift      {}",
                format!("{} finding(s)", report.drift.len()).red()
            );
            for d in &report.drift {
                println!("      {} {}", "×".red(), d);
            }
        }
        if !report.ok && report.signature == EntryVerifyStatus::Unsigned && !allow_unsigned {
            println!();
            println!(
                "{}",
                "hint: release assets ship unsigned; ask your operator for the signed manifest,"
                    .dimmed()
            );
            println!(
                "{}",
                "      pin their key (locus adapter trust add), or pass --allow-unsigned for a drift-only check"
                    .dimmed()
            );
        }
    }
    if !report.ok {
        bail!("release manifest verify failed");
    }
    Ok(())
}

fn cmd_adapter_trust(sub: AdapterTrustCmd, json: bool) -> Result<()> {
    let store = Store::open_default().context("open LOCUS_HOME")?;
    let home = store.home();
    let path = adapter_trust_keys_path(home);
    match sub {
        AdapterTrustCmd::List => {
            let listings = list_trust_keys_with_origin(home).context("list adapter trust keys")?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "trust_file": path.display().to_string(),
                        "keys": listings,
                    }))?
                );
                return Ok(());
            }
            println!(
                "{} adapter registry trust store\n",
                "locus adapter trust".cyan().bold()
            );
            println!("  file  {}", path.display().to_string().dimmed());
            if listings.is_empty() {
                println!();
                println!("{} no trust keys pinned", "->".dimmed());
                println!(
                    "{}",
                    "Add:  locus adapter trust add --id root --ed25519-pub <base64-pubkey>"
                        .dimmed()
                );
                println!(
                    "{}",
                    "Env:  LOCUS_ADAPTER_TRUST_KEYS=id:ed25519:<b64>[,id:hmac-sha256:<64-hex>]"
                        .dimmed()
                );
                return Ok(());
            }
            println!();
            for k in &listings {
                let origin = match k.origin {
                    TrustKeyOrigin::File => "file",
                    TrustKeyOrigin::Env => "env",
                };
                println!(
                    "  {}  {}  ·  {}  ·  {}",
                    k.id.green().bold(),
                    k.scheme.yellow(),
                    origin.dimmed(),
                    k.material.dimmed()
                );
            }
            println!();
            println!(
                "{}",
                "Verify uses merged keys (file + LOCUS_ADAPTER_TRUST_KEYS; env wins on same id)."
                    .dimmed()
            );
            Ok(())
        }
        AdapterTrustCmd::Add { id, ed25519_pub } => {
            let result = add_ed25519_trust_key(home, &id, &ed25519_pub)
                .with_context(|| format!("add trust key `{id}`"))?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "ok": true,
                        "id": result.key.id,
                        "scheme": result.key.scheme(),
                        "replaced": result.replaced,
                        "path": result.path.display().to_string(),
                    }))?
                );
                return Ok(());
            }
            let action = if result.replaced { "updated" } else { "added" };
            println!(
                "{} trust key {} `{}` ({})",
                "✓".green().bold(),
                action,
                result.key.id.green().bold(),
                result.key.scheme().yellow()
            );
            println!("  path  {}", result.path.display().to_string().dimmed());
            println!(
                "{}",
                "List:  locus adapter trust list · Verify: locus adapter verify --require-signed"
                    .dimmed()
            );
            Ok(())
        }
    }
}

fn cmd_upstream(sub: UpstreamCmd, json: bool) -> Result<()> {
    match sub {
        UpstreamCmd::List => {
            let recipes = all_recipes().context("load built-in upstream recipes")?;
            if json {
                println!("{}", serde_json::to_string_pretty(&recipes)?);
                return Ok(());
            }
            if recipes.is_empty() {
                println!("{} no built-in recipes", "->".dimmed());
                return Ok(());
            }
            println!(
                "{} built-in upstream recipes (use in binding TOML):\n",
                "locus upstream".cyan().bold()
            );
            for r in &recipes {
                let providers = if r.providers.is_empty() {
                    "—".into()
                } else {
                    r.providers.join(", ")
                };
                println!(
                    "  {}  {}",
                    r.id.green().bold(),
                    if r.title.is_empty() {
                        "".into()
                    } else {
                        r.title.clone()
                    }
                    .dimmed()
                );
                println!(
                    "      providers: {}  ·  {} {}",
                    providers.yellow(),
                    r.command.cyan(),
                    r.args.join(" ").dimmed()
                );
                println!("      {}", recipe_toml_snippet(r).dimmed());
                println!(
                    "      readiness: {}  ·  sandbox: {}",
                    r.readiness.as_str().yellow(),
                    r.sandbox_compatibility.as_str().yellow()
                );
                if !r.risks.is_empty() {
                    println!("      risks: {}", r.risks.join(", ").yellow());
                }
                if !r.notes.trim().is_empty() {
                    let first = r.notes.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                    if !first.is_empty() {
                        println!("      note: {}", first.trim().dimmed());
                    }
                }
                println!();
            }
            println!(
                "{}",
                "Suggest for a provider:  locus upstream suggest github".dimmed()
            );
            Ok(())
        }
        UpstreamCmd::Suggest { provider } => {
            let recipes = suggest_for_provider(&provider)
                .with_context(|| format!("suggest recipes for `{provider}`"))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&recipes)?);
                return Ok(());
            }
            if recipes.is_empty() {
                println!(
                    "{} no recipes tagged for provider `{}`\n  try `locus upstream list`",
                    "->".dimmed(),
                    provider.yellow()
                );
                return Ok(());
            }
            println!(
                "{} recipes for provider {}:\n",
                "suggest".cyan().bold(),
                provider.yellow().bold()
            );
            for r in &recipes {
                println!("  {}  {}", r.id.green().bold(), r.title.dimmed());
                println!("      {} {}", r.command.cyan(), r.args.join(" ").dimmed());
                println!("      copy: {}", recipe_toml_snippet(r));
                println!(
                    "      readiness: {}  ·  sandbox: {}",
                    r.readiness.as_str().yellow(),
                    r.sandbox_compatibility.as_str().yellow()
                );
                if !r.risks.is_empty() {
                    println!("      risks: {}", r.risks.join(", ").yellow());
                }
                if !r.env_hints.is_empty() {
                    println!("      env hints: {}", r.env_hints.join(", ").dimmed());
                }
                if !r.notes.trim().is_empty() {
                    for line in r.notes.lines().take(6) {
                        let t = line.trim();
                        if !t.is_empty() {
                            println!("      {}", t.dimmed());
                        }
                    }
                }
                println!();
            }
            Ok(())
        }
    }
}

fn cmd_binding(sub: BindingCmd, json: bool) -> Result<()> {
    let s = store()?;
    match sub {
        BindingCmd::List => {
            let list = s.list_bindings()?;
            // Active-pin marker (display only; a read failure just drops it).
            let active = s.active_session().ok().flatten();
            let pinned_aliases: Vec<String> = active
                .as_ref()
                .map(|sess| sess.all_aliases())
                .unwrap_or_default();
            // Remaining-TTL suffix; skipped when expired/unreadable.
            let pinned_left: Option<String> = active.as_ref().and_then(|sess| {
                let rem = sess.expires_at - chrono::Utc::now();
                (rem > chrono::Duration::zero()).then(|| human_dur(rem))
            });
            if json {
                let mut rows: Vec<serde_json::Value> = Vec::with_capacity(list.len());
                for b in &list {
                    let mut v = serde_json::to_value(b)?;
                    if let Some(obj) = v.as_object_mut() {
                        obj.insert(
                            "pinned".into(),
                            json!(pinned_aliases.iter().any(|a| a == &b.alias)),
                        );
                    }
                    rows.push(v);
                }
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else if list.is_empty() {
                println!(
                    "{} no bindings — try `locus init --with-samples`",
                    "->".dimmed()
                );
            } else {
                for b in list {
                    let marker = if pinned_aliases.iter().any(|a| a == &b.alias) {
                        match &pinned_left {
                            Some(left) => {
                                format!("  {}", format!("* pinned ({left} left)").green().bold())
                            }
                            None => format!("  {}", "* pinned".green().bold()),
                        }
                    } else {
                        String::new()
                    };
                    println!(
                        "  {}  {}  [{}]{marker}",
                        b.alias.cyan().bold(),
                        b.tenant.yellow(),
                        b.providers.join(", ").dimmed()
                    );
                    if let Some(d) = b.description {
                        println!("      {}", d.dimmed());
                    }
                }
            }
        }
        BindingCmd::Show { alias } => {
            let mut b = s.load_binding(&alias)?;
            for provider in &mut b.providers {
                let source = locus_core::credential_metadata(&provider.credential_ref).source;
                provider.credential_ref = format!("<redacted:{source}>");
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&b)?);
            } else {
                println!("{}", b.to_toml()?);
            }
        }
        BindingCmd::MigrateCredentialRefs { alias, write } => {
            let result = s.migrate_legacy_credential_refs(&alias, write)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else if result.written {
                println!(
                    "{} migrated {} credential reference(s) in {}",
                    "ok".green().bold(),
                    result.migrated,
                    result.alias
                );
                if result.audit_pending || result.recovery_pending {
                    println!(
                        "{} migration committed; run the same command again to reconcile durable audit state",
                        "warning".yellow().bold()
                    );
                }
            } else {
                println!(
                    "{} dry run: {} credential reference(s) can be migrated in {}; pass --write to persist",
                    "ok".green().bold(),
                    result.migrated,
                    result.alias
                );
            }
        }
        BindingCmd::Add(args) => cmd_binding_add(args, false, json)?,
        BindingCmd::Rm { alias, yes } => {
            if !yes {
                bail!("refusing to remove without --yes");
            }
            s.remove_binding(&alias)?;
            if json {
                println!("{}", serde_json::json!({ "removed": alias }));
            } else {
                println!("{} removed {alias}", "ok".green().bold());
            }
        }
    }
    Ok(())
}

fn cmd_client(sub: ClientCmd, json: bool) -> Result<()> {
    match sub {
        ClientCmd::Add(args) => cmd_binding_add(args, true, json),
    }
}

/// Resolved answers for a binding-add flow (flags + prompts). Everything
/// downstream of the prompt loop is pure and unit-testable.
#[derive(Debug, Clone, Default)]
struct AddAnswers {
    alias: String,
    tenant: String,
    provider: String,
    account: String,
    credential_ref: String,
    project_ref: Option<String>,
    team_id: Option<String>,
    account_id: Option<String>,
    org: Option<String>,
    repos: Vec<String>,
    read_only: bool,
    description: Option<String>,
    default_ttl: Option<String>,
}

fn missing_add_flags(args: &BindingAddArgs) -> Vec<&'static str> {
    let mut m = Vec::new();
    if args.alias.is_none() {
        m.push("<alias>");
    }
    if args.tenant.is_none() {
        m.push("--tenant");
    }
    if args.provider.is_none() {
        m.push("--provider");
    }
    if args.account.is_none() {
        m.push("--account");
    }
    if args.credential_ref.is_none() {
        m.push("--credential-ref");
    }
    m
}

fn split_repos(raw: Option<&str>) -> Vec<String> {
    raw.map(|r| {
        r.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect()
    })
    .unwrap_or_default()
}

/// A credential_ref is a pointer, never the secret. Rejects raw values with
/// the core error and adds a did-you-mean for conservative bare Phantom names.
fn validate_credential_ref_input(raw: &str) -> Result<()> {
    if let Err(e) = CredentialRef::validate(raw) {
        if let Some(suggest) = migrate_legacy_phantom_ref(raw) {
            bail!("{e} — did you mean '{suggest}'?");
        }
        bail!("{e}");
    }
    Ok(())
}

/// Resolve answers from flags alone (non-interactive / piped stdin / --json).
/// Fails closed listing every absent required flag.
fn resolve_add_answers(args: &BindingAddArgs) -> Result<AddAnswers> {
    let missing = missing_add_flags(args);
    if !missing.is_empty() {
        bail!("missing {} (non-interactive)", missing.join(" "));
    }
    let alias = args.alias.clone().expect("presence checked");
    validate_name_component("alias", &alias)?;
    let credential_ref = args.credential_ref.clone().expect("presence checked");
    validate_credential_ref_input(&credential_ref)?;
    if let Some(ref v) = args.default_ttl {
        parse_pin_ttl(v).map_err(|e| anyhow!("invalid --default-ttl: {e}"))?;
    }
    Ok(AddAnswers {
        alias,
        tenant: args.tenant.clone().expect("presence checked"),
        provider: args.provider.clone().expect("presence checked"),
        account: args.account.clone().expect("presence checked"),
        credential_ref,
        project_ref: args.project_ref.clone(),
        team_id: args.team_id.clone(),
        account_id: args.account_id.clone(),
        org: args.org.clone(),
        repos: split_repos(args.repos.as_deref()),
        read_only: args.read_only,
        description: args.description.clone(),
        default_ttl: args.default_ttl.clone(),
    })
}

/// Guided resolution: reuse every flag that was passed, prompt for the rest.
/// Only reachable when stdin is a TTY.
fn resolve_add_answers_interactive(s: &Store, args: &BindingAddArgs) -> Result<AddAnswers> {
    let known: Vec<String> = s
        .list_bindings()
        .map(|l| l.into_iter().map(|b| b.alias).collect())
        .unwrap_or_default();

    let alias = match &args.alias {
        Some(a) => a.clone(),
        None => {
            let v = prompt_value("alias", "<alias>", None, &|v| {
                if let Err(e) = validate_name_component("alias", v) {
                    return Err(format!("{e}"));
                }
                if v.starts_with("locus") {
                    return Err(format!(
                        "alias '{v}' is reserved: aliases starting with 'locus' collide with \
                         the control-tool namespace — choose a different alias"
                    ));
                }
                if known.iter().any(|k| k == v) {
                    return Err(format!(
                        "alias '{v}' already exists — edit it (locus binding show {v}) or pick another"
                    ));
                }
                Ok(())
            })?;
            if let Some(best) = nearest_alias(&v, &known) {
                // Soft nudge only — never a hard block (short aliases collide easily).
                if !prompt_confirm(
                    &format!("similar alias '{best}' already exists — continue with '{v}'?"),
                    false,
                )? {
                    bail!("aborted — nothing written");
                }
            }
            v
        }
    };

    let tenant = match &args.tenant {
        Some(t) => t.clone(),
        None => prompt_value("tenant", "--tenant", Some(&alias), &|_| Ok(()))?,
    };

    let provider = match &args.provider {
        Some(p) => p.clone(),
        None => {
            println!(
                "  providers with built-in adapters: {}",
                known_providers().join(", ").cyan()
            );
            let v = prompt_value("provider", "--provider", None, &|_| Ok(()))?;
            if !known_providers().contains(&v.as_str()) {
                println!(
                    "{} no built-in adapter for '{}' — tools require an upstream MCP recipe (locus upstream list)",
                    "warning:".yellow().bold(),
                    v
                );
            }
            v
        }
    };

    let account = match &args.account {
        Some(a) => a.clone(),
        None => prompt_value("account", "--account", None, &|_| Ok(()))?,
    };

    // Provider scope fields (CLI-side convenience table; scope freeze at the
    // gate stays the enforcement).
    let mut project_ref = args.project_ref.clone();
    let mut team_id = args.team_id.clone();
    let mut account_id = args.account_id.clone();
    let mut org = args.org.clone();
    let mut repos = split_repos(args.repos.as_deref());
    for &(field, required) in scope_prompts(&provider) {
        if field == "repos" {
            if repos.is_empty() {
                let v = prompt_optional("repos (comma-separated allowlist, empty = all in org)")?;
                repos = split_repos(v.as_deref());
            }
            continue;
        }
        let slot: &mut Option<String> = match field {
            "project_ref" => &mut project_ref,
            "team_id" => &mut team_id,
            "account_id" => &mut account_id,
            "org" => &mut org,
            _ => continue,
        };
        if slot.is_none() {
            let flag = format!("--{}", field.replace('_', "-"));
            *slot = if required {
                Some(prompt_value(field, &flag, None, &|_| Ok(()))?)
            } else {
                prompt_optional(&format!("{field} (optional)"))?
            };
        }
    }
    let read_only = args.read_only || prompt_confirm("freeze scope read-only?", false)?;

    let credential_ref = match &args.credential_ref {
        Some(c) => {
            validate_credential_ref_input(c)?;
            c.clone()
        }
        None => {
            println!("  credential_ref is a pointer, never the secret itself:");
            println!(
                "    {}  Phantom vault (recommended — phantom add NAME, https://phm.dev)",
                "phm:NAME".cyan()
            );
            println!(
                "    {}   read from the environment at exec time",
                "env:VAR".cyan()
            );
            prompt_value("credential_ref", "--credential-ref", None, &|v| {
                match CredentialRef::validate(v) {
                    Ok(_) => Ok(()),
                    Err(e) => match migrate_legacy_phantom_ref(v) {
                        Some(suggest) => Err(format!("{e} — did you mean '{suggest}'?")),
                        None => Err(format!("{e}")),
                    },
                }
            })?
        }
    };

    let default_ttl = match &args.default_ttl {
        Some(v) => {
            parse_pin_ttl(v).map_err(|e| anyhow!("invalid --default-ttl: {e}"))?;
            Some(v.clone())
        }
        None => loop {
            match prompt_optional("default pin ttl (e.g. 2h; empty = policy max_ttl)")? {
                None => break None,
                Some(t) => match parse_pin_ttl(&t) {
                    Ok(_) => break Some(t),
                    Err(e) => eprintln!("  {e}"),
                },
            }
        },
    };

    Ok(AddAnswers {
        alias,
        tenant,
        provider,
        account,
        credential_ref,
        project_ref,
        team_id,
        account_id,
        org,
        repos,
        read_only,
        description: args.description.clone(),
        default_ttl,
    })
}

/// Pure mapping from resolved answers to a Binding (unit-testable).
fn binding_from_answers(a: &AddAnswers) -> Binding {
    let mut scope = Scope {
        project_ref: a.project_ref.clone(),
        team_id: a.team_id.clone(),
        account_id: a.account_id.clone(),
        read_only: if a.read_only { Some(true) } else { None },
        ..Scope::default()
    };
    if let Some(o) = &a.org {
        scope.orgs = vec![o.clone()];
    }
    scope.repos = a.repos.clone();
    Binding::from_body(BindingBody {
        id: format!("bnd_{}", a.alias),
        alias: a.alias.clone(),
        tenant: a.tenant.clone(),
        principal: None,
        description: a.description.clone(),
        policy: Policy {
            default_ttl: a.default_ttl.clone(),
            ..Policy::default()
        },
        providers: vec![ProviderBinding {
            provider: a.provider.clone(),
            account: a.account.clone(),
            credential_ref: a.credential_ref.clone(),
            scope,
            upstream: None,
        }],
    })
}

/// Per-provider guided scope prompts: (field, required).
fn scope_prompts(provider: &str) -> &'static [(&'static str, bool)] {
    match provider.to_ascii_lowercase().as_str() {
        "supabase" => &[("project_ref", true)],
        "vercel" => &[("team_id", true), ("project_ref", false)],
        "github" => &[("org", true), ("repos", false)],
        "aws" | "stripe" | "cloudflare" => &[("account_id", true)],
        _ => &[],
    }
}

/// Shared handler for `locus binding add` (guided_default=false) and
/// `locus client add` (guided_default=true). The write path is exactly
/// `Store::save_binding` — validation, bindings lock, reserved-alias check,
/// audit `binding.save`.
fn cmd_binding_add(args: BindingAddArgs, guided_default: bool, json: bool) -> Result<()> {
    use std::io::IsTerminal;
    let s = store()?;
    let guided = guided_default || args.guided;
    let can_prompt = !args.non_interactive && !json && io::stdin().is_terminal();
    let answers = if can_prompt && (guided || !missing_add_flags(&args).is_empty()) {
        resolve_add_answers_interactive(&s, &args)?
    } else {
        resolve_add_answers(&args)?
    };
    let b = binding_from_answers(&answers);
    let toml = b.to_toml()?;

    if args.dry_run {
        if json {
            println!(
                "{}",
                serde_json::json!({ "ok": true, "dry_run": true, "alias": answers.alias, "toml": toml })
            );
        } else {
            println!("{toml}");
            println!("{} dry run — nothing written", "ok".green().bold());
        }
        return Ok(());
    }

    if can_prompt && guided {
        println!();
        println!("{toml}");
        if !prompt_confirm(&format!("write binding '{}'?", answers.alias), true)? {
            bail!("aborted — nothing written");
        }
    }

    let path = s.save_binding(&b)?;
    if json {
        println!(
            "{}",
            serde_json::json!({ "ok": true, "path": path.display().to_string() })
        );
        return Ok(());
    }
    println!("{} wrote {}", "ok".green().bold(), path.display());
    println!();
    println!("   next steps:");
    println!(
        "   {}",
        format!("locus enter {} --ttl 2h", answers.alias).cyan()
    );
    println!("   {}", "locus agent setup --apply".dimmed());
    println!(
        "   {}  {}",
        "locus doctor".dimmed(),
        "(unresolved phm: refs are flagged)".dimmed()
    );
    if let Some(name) = answers.credential_ref.strip_prefix("phm:") {
        println!(
            "   {}  {}",
            format!("phantom add {name}").dimmed(),
            "(store the secret in Phantom — https://phm.dev)".dimmed()
        );
    }
    Ok(())
}

/// Prompt for a required value on a TTY; fail closed when stdin is not a TTY.
fn prompt_value(
    label: &str,
    flag_hint: &str,
    default: Option<&str>,
    validate: &dyn Fn(&str) -> std::result::Result<(), String>,
) -> Result<String> {
    use std::io::{IsTerminal, Write};
    if !io::stdin().is_terminal() {
        bail!("missing {label} — pass {flag_hint} (stdin is not a TTY)");
    }
    loop {
        match default {
            Some(d) => print!("  {label} [{d}]: "),
            None => print!("  {label}: "),
        }
        io::stdout().flush()?;
        let mut line = String::new();
        if io::stdin().read_line(&mut line)? == 0 {
            bail!("missing {label} — input closed (pass {flag_hint})");
        }
        let trimmed = line.trim();
        let v = if trimmed.is_empty() {
            match default {
                Some(d) => d,
                None => {
                    eprintln!("  {label} is required");
                    continue;
                }
            }
        } else {
            trimmed
        };
        match validate(v) {
            Ok(()) => return Ok(v.to_string()),
            Err(msg) => eprintln!("  {msg}"),
        }
    }
}

/// Optional prompt: empty input → None. Non-TTY → None (flags rule).
fn prompt_optional(label: &str) -> Result<Option<String>> {
    use std::io::{IsTerminal, Write};
    if !io::stdin().is_terminal() {
        return Ok(None);
    }
    print!("  {label}: ");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let t = line.trim();
    Ok(if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    })
}

/// Y/n confirm. Non-TTY returns the default (never blocks scripts).
fn prompt_confirm(question: &str, default_yes: bool) -> Result<bool> {
    use std::io::{IsTerminal, Write};
    if !io::stdin().is_terminal() {
        return Ok(default_yes);
    }
    let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
    print!("  {question} {hint} ");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let t = line.trim().to_ascii_lowercase();
    Ok(match t.as_str() {
        "" => default_yes,
        "y" | "yes" => true,
        _ => false,
    })
}

/// Bounds-checked `--ttl` parse. `locus_core::parse_ttl` is lenient (accepts
/// "", zero, negatives) — this wrapper is the fail-closed gate for operator
/// input: min 1m, max 24h.
fn parse_pin_ttl(raw: &str) -> Result<chrono::Duration> {
    let trimmed = raw.trim();
    let invalid = || anyhow!("invalid --ttl '{raw}': use 30m / 2h / 1d (min 1m, max 24h)");
    if trimmed.is_empty() {
        return Err(invalid());
    }
    let d = parse_ttl(trimmed).map_err(|_| invalid())?;
    if d < chrono::Duration::minutes(1) {
        bail!("--ttl {raw} is too short: minimum is 1m");
    }
    if d > chrono::Duration::hours(24) {
        bail!(
            "--ttl {raw} is too long: maximum is 24h — for standing access set policy.max_ttl \
             in the binding TOML instead"
        );
    }
    Ok(d)
}

/// "2h", "1h30m", "45m", "90s" — humanized duration for operator output.
fn human_dur(d: chrono::Duration) -> String {
    let secs = d.num_seconds().max(0);
    if secs < 120 {
        return format!("{secs}s");
    }
    let mins = secs / 60;
    let (h, m) = (mins / 60, mins % 60);
    if h > 0 && m > 0 {
        format!("{h}h{m}m")
    } else if h > 0 {
        format!("{h}h")
    } else {
        format!("{m}m")
    }
}

fn cmd_workspace(
    default: String,
    allow: Option<String>,
    require_pin: bool,
    force: bool,
) -> Result<()> {
    require_local_control_boundary("locus workspace")?;
    let path = cwd().join(".locus.toml");
    if path.exists() && !force {
        bail!(
            "{} already exists (pass --force to overwrite)",
            path.display()
        );
    }
    let mut allowed = vec![default.clone()];
    if let Some(a) = allow {
        for part in a.split(',') {
            let p = part.trim();
            if !p.is_empty() && !allowed.iter().any(|x| x == p) {
                allowed.push(p.into());
            }
        }
    }
    let cfg = WorkspaceConfig {
        version: 1,
        default_binding: Some(default),
        allowed_bindings: allowed,
        require_pin,
    };
    std::fs::write(&path, cfg.to_toml()?)?;
    println!("{} wrote {}", "ok".green().bold(), path.display());
    Ok(())
}

fn cmd_doctor(json: bool) -> Result<()> {
    let s = store()?;

    // Continuous whoami: freeze session if binding material drifted under pin.
    // Best-effort — a wedged session must not abort doctor itself;
    // build_doctor_report re-runs this and classifies the wedge as a
    // stale_session finding with the `locus leave --force` recovery.
    let _ = s.check_drift_and_freeze();

    let report = gather_doctor_report(&s)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_doctor_human(&report);
    }

    match report.verdict {
        DoctorVerdict::Safe => Ok(()),
        DoctorVerdict::Warn => std::process::exit(1),
        DoctorVerdict::Unsafe => std::process::exit(2),
    }
}

/// AI-native agent plane: setup / doctor / report.
fn cmd_agent(sub: AgentCmd, json: bool) -> Result<()> {
    match sub {
        AgentCmd::Setup {
            client,
            claude_scope,
            apply,
            dry_run,
            workspace,
            mcp_bin,
        } => cmd_agent_setup(
            &client,
            &claude_scope,
            apply,
            dry_run,
            workspace,
            mcp_bin,
            json,
        ),
        AgentCmd::Doctor => cmd_agent_doctor(json),
        AgentCmd::Report { json: report_json } => cmd_agent_report(json || report_json),
    }
}

/// Northstar goal progress from `GOALS.md` (or embedded fallback).
fn cmd_goal(sub: GoalCmd, json: bool) -> Result<()> {
    match sub {
        GoalCmd::Status => cmd_goal_status(json),
    }
}

fn cmd_verify(sub: VerifyCmd, json: bool) -> Result<()> {
    match sub {
        VerifyCmd::Claim { text } => cmd_verify_claim(&text, json),
        VerifyCmd::Session => cmd_verify_session(json),
    }
}

fn cmd_verify_claim(text: &str, json: bool) -> Result<()> {
    let s = store()?;
    let _ = s.check_drift_and_freeze();
    let who = s.whoami().ok();
    let result = verify_claim(text, who.as_ref());
    let binding = who
        .as_ref()
        .map(|w| w.binding_alias.as_str())
        .unwrap_or("-");
    let _ = s.audit(
        "verify.claim",
        binding,
        Some(json!({
            "confidence": result.confidence.as_str(),
            "needs_tool": result.needs_tool,
            "signals": result.signals,
            "claim_len": result.claim.len(),
            "claim_preview": result.claim.chars().take(120).collect::<String>(),
        })),
    );

    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }

    // Structured human view — same fields as the JSON contract.
    println!(
        "{} verify claim  confidence={}  needs_tool={}",
        "locus".magenta().bold(),
        match result.confidence.as_str() {
            "high" => result.confidence.as_str().green().to_string(),
            "medium" => result.confidence.as_str().cyan().to_string(),
            "low" => result.confidence.as_str().yellow().to_string(),
            _ => result.confidence.as_str().dimmed().to_string(),
        },
        if result.needs_tool {
            "true".yellow().to_string()
        } else {
            "false".dimmed().to_string()
        }
    );
    println!("  claim       {}", result.claim);
    if !result.signals.is_empty() {
        println!("  signals     {}", result.signals.join(", ").dimmed());
    }
    if let Some(ref g) = result.grounding {
        println!(
            "  grounding   {}  pin={}  tenant={}  seal={}{}",
            g.kind.cyan(),
            g.binding_alias.cyan().bold(),
            g.tenant.yellow(),
            if g.seal_ok {
                "ok".green().to_string()
            } else {
                "BAD".red().to_string()
            },
            if g.frozen {
                format!("  {}", "FROZEN".red().bold())
            } else {
                String::new()
            }
        );
    }
    println!("  suggestion  {}", result.suggestion);
    // Always emit machine JSON line for pipelines that ignore human formatting.
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}

fn cmd_verify_session(json: bool) -> Result<()> {
    let s = store()?;
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let external = gather_doctor_external(&s, cwd.clone())?;
    let mut pack = verify_session(&s, &cwd, external)?;
    // Same finding pack as `locus doctor` / `locus watch`: hub consumers treat
    // `verify session --json` and `watch --once --json` as interchangeable
    // heartbeat probes (integrations/ashlr-hub/locus.ts), so both must agree
    // on session_ok for identical operator-shell state.
    let cap_status = locus_core::control_capability_status(s.home());
    attach_control_capability_findings(s.home(), &cap_status, &mut pack);
    let binding = pack
        .whoami
        .as_ref()
        .map(|w| w.binding_alias.as_str())
        .unwrap_or("-");
    let _ = s.audit(
        "verify.session",
        binding,
        Some(json!({
            "session_ok": pack.session_ok,
            "safe_next": pack.safe_next.action,
            "doctor_verdict": pack.doctor.verdict,
            "doctor_ok": pack.doctor.ok,
            "has_whoami": pack.whoami.is_some(),
        })),
    );

    if json {
        println!("{}", serde_json::to_string_pretty(&pack)?);
        return verify_session_exit(pack.session_ok);
    }

    println!(
        "{} verify session  session_ok={}  safe_next={}  doctor={}",
        "locus".magenta().bold(),
        if pack.session_ok {
            "true".green().to_string()
        } else {
            "false".yellow().to_string()
        },
        pack.safe_next.action.cyan(),
        format!("{:?}", pack.doctor.verdict).dimmed()
    );
    if let Some(ref w) = pack.whoami {
        println!(
            "  whoami      pin={}  tenant={}  seal={}",
            w.binding_alias.cyan().bold(),
            w.tenant.yellow(),
            if w.seal_ok {
                "ok".green().to_string()
            } else {
                "BAD".red().to_string()
            }
        );
    } else {
        println!("  whoami      {}", "(unpinned)".dimmed());
    }
    println!(
        "  doctor      verdict={:?}  ok={}  findings={}",
        pack.doctor.verdict,
        pack.doctor.ok,
        pack.doctor.findings.len()
    );
    println!(
        "  safe_next   {} — {}",
        pack.safe_next.action, pack.safe_next.message
    );
    if let Some(ref cmd) = pack.safe_next.command {
        println!("  command     {}", cmd.cyan());
    }
    // Machine JSON line for pipelines.
    println!("{}", serde_json::to_string(&pack)?);
    verify_session_exit(pack.session_ok)
}

fn verify_session_exit(session_ok: bool) -> Result<()> {
    if session_ok {
        Ok(())
    } else {
        bail!("session verification is not ready (session_ok=false)")
    }
}

fn cmd_goal_status(json: bool) -> Result<()> {
    let found = find_goals_md();
    let (source, path, milestones) = match &found {
        Some(p) => {
            let body =
                std::fs::read_to_string(p).with_context(|| format!("read {}", p.display()))?;
            (
                "goals_md",
                Some(p.display().to_string()),
                parse_goals_milestones(&body),
            )
        }
        None => ("embedded", None, embedded_goal_milestones()),
    };

    let total_done: usize = milestones.iter().map(|m| m.done).sum();
    let total_items: usize = milestones.iter().map(|m| m.total).sum();
    let total_remaining = total_items.saturating_sub(total_done);

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "version": VERSION,
                "vision": "wrong-account impossible · AI-native · hub-native",
                "source": source,
                "path": path,
                "metrics": {
                    "wrong_account_incidents": "0 / dogfood quarter",
                    "time_to_safe_context": "<15s",
                    "agent_report_ready": "status=ready before mutate",
                },
                "milestones": milestones.iter().map(|m| json!({
                    "id": m.id,
                    "title": m.title,
                    "state": m.state,
                    "done": m.done,
                    "total": m.total,
                    "remaining": m.total.saturating_sub(m.done),
                })).collect::<Vec<_>>(),
                "totals": {
                    "done": total_done,
                    "total": total_items,
                    "remaining": total_remaining,
                },
            }))?
        );
        return Ok(());
    }

    println!(
        "{}  {}",
        "locus goal".bold(),
        "wrong-account impossible · AI-native · hub-native".dimmed()
    );
    match &path {
        Some(p) => println!("  source   GOALS.md  {}", p.dimmed()),
        None => println!(
            "  source   embedded milestones  {}",
            "(no GOALS.md in cwd/parents)".dimmed()
        ),
    }
    println!();
    println!(
        "  {}  0 wrong-account  ·  <15s enter  ·  agent report ready",
        "metrics".bold()
    );
    println!();

    for m in &milestones {
        let mark = match m.state.as_str() {
            "done" => "✓".green().bold(),
            "in_progress" | "mostly_done" => "…".yellow().bold(),
            _ => "·".dimmed(),
        };
        let counts = if m.total > 0 {
            format!("{}/{}", m.done, m.total)
        } else {
            "—".into()
        };
        println!(
            "  {} {}  {:<14}  {}  {}",
            mark,
            m.id.bold(),
            m.state,
            counts.dimmed(),
            m.title
        );
    }

    println!();
    println!(
        "  progress  {} done · {} remaining · {} total",
        total_done.to_string().green(),
        total_remaining.to_string().yellow(),
        total_items
    );
    println!();
    println!(
        "  {}  edit checkboxes in GOALS.md  ·  hub: locus agent report --json",
        "next".dimmed()
    );
    Ok(())
}

#[derive(Clone, Debug)]
struct GoalMilestone {
    id: String,
    title: String,
    state: String,
    done: usize,
    total: usize,
}

/// Walk cwd → parents for `GOALS.md` (cap 12 levels).
fn find_goals_md() -> Option<PathBuf> {
    let mut dir = cwd();
    for _ in 0..12 {
        let candidate = dir.join("GOALS.md");
        if candidate.is_file() {
            return Some(candidate);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

/// Parse `### M1 — Title · **state**` sections and `- [x]` / `- [ ]` items.
fn parse_goals_milestones(body: &str) -> Vec<GoalMilestone> {
    let mut out: Vec<GoalMilestone> = Vec::new();
    let mut current: Option<GoalMilestone> = None;

    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("### ") {
            if let Some(m) = current.take() {
                out.push(m);
            }
            // e.g. "M1 — Identity plane (core) · **done** (v0.1.0)"
            let id = rest
                .split_whitespace()
                .next()
                .unwrap_or("M?")
                .trim_end_matches(['.', ':'])
                .to_string();
            let title = rest.split('·').next().unwrap_or(rest).trim().to_string();
            let lower = rest.to_ascii_lowercase();
            let state = if lower.contains("**done**") && !lower.contains("mostly") {
                "done".into()
            } else if lower.contains("mostly done") || lower.contains("mostly_done") {
                "mostly_done".into()
            } else if lower.contains("in progress") || lower.contains("in_progress") {
                "in_progress".into()
            } else if lower.contains("future") {
                "future".into()
            } else {
                "unknown".into()
            };
            current = Some(GoalMilestone {
                id,
                title,
                state,
                done: 0,
                total: 0,
            });
            continue;
        }

        if let Some(m) = current.as_mut() {
            let lower = trimmed.to_ascii_lowercase();
            // Markdown task list: - [x] / - [ ] / * [X]
            if lower.starts_with("- [x]")
                || lower.starts_with("* [x]")
                || lower.starts_with("- [X]")
                || lower.starts_with("* [X]")
            {
                m.done += 1;
                m.total += 1;
            } else if lower.starts_with("- [ ]") || lower.starts_with("* [ ]") {
                m.total += 1;
            }
        }
    }
    if let Some(m) = current.take() {
        out.push(m);
    }

    if out.is_empty() {
        return embedded_goal_milestones();
    }
    out
}

fn embedded_goal_milestones() -> Vec<GoalMilestone> {
    vec![
        GoalMilestone {
            id: "M1".into(),
            title: "M1 — Identity plane (core)".into(),
            state: "done".into(),
            done: 9,
            total: 9,
        },
        GoalMilestone {
            id: "M2".into(),
            title: "M2 — Firm UX".into(),
            state: "done".into(),
            done: 7,
            total: 7,
        },
        GoalMilestone {
            id: "M3".into(),
            title: "M3 — AI surface".into(),
            state: "mostly_done".into(),
            done: 7,
            total: 9,
        },
        GoalMilestone {
            id: "M4".into(),
            title: "M4 — Hub composition".into(),
            state: "in_progress".into(),
            done: 5,
            total: 9,
        },
        GoalMilestone {
            id: "M5".into(),
            title: "M5 — Verification plane".into(),
            state: "partial".into(),
            done: 6,
            total: 10,
        },
    ]
}

fn gather_doctor_report(s: &Store) -> Result<locus_core::DoctorReport> {
    let mut report = build_doctor_report(s, gather_doctor_external(s, cwd())?)?;
    // Operator-shell control capability readiness. CLI-only by design:
    // locus-mcp runs executor-restricted and legitimately lacks the control
    // capability, so these findings never attach to MCP doctor surfaces.
    let status = locus_core::control_capability_status(s.home());
    for f in locus_core::control_capability_findings(&status, s.home()) {
        report.push_finding(f.severity, &f.code, f.message);
    }
    Ok(report)
}

/// Attach operator-shell control-capability findings to a `verify_session`
/// pack and re-derive `session_ok` from the escalated doctor verdict.
///
/// Keeps `locus watch` and `locus verify session` on the same finding pack
/// as `locus doctor` ([`gather_doctor_report`]), so the two hub heartbeat
/// probes never disagree on `session_ok` for identical operator-shell state.
/// CLI-only by design: locus-mcp runs
/// executor-restricted and legitimately lacks the control capability, so
/// these findings never attach to MCP verify_session surfaces. Env-free core
/// (status passed in) so tests are deterministic under ambient
/// `LOCUS_CONTROL_CAPABILITY`.
fn attach_control_capability_findings(
    home: &Path,
    status: &locus_core::ControlCapabilityStatus,
    pack: &mut locus_core::SessionVerificationPack,
) {
    for f in locus_core::control_capability_findings(status, home) {
        pack.doctor.push_finding(f.severity, &f.code, f.message);
    }
    pack.session_ok = pack.doctor.ok && pack.safe_next.ready;
}

fn gather_doctor_external(s: &Store, cwd: PathBuf) -> Result<DoctorExternal> {
    // phantom --version is process-cached (locus_core::phantom_on_path).
    gather_doctor_external_with_phantom_status(s, cwd, phantom_on_path())
}

fn gather_doctor_external_with_phantom_status(
    s: &Store,
    cwd: PathBuf,
    phantom: bool,
) -> Result<DoctorExternal> {
    let unresolved_phm = collect_unresolved_phm_refs(s, phantom)?;
    Ok(DoctorExternal {
        phantom_on_path: phantom,
        unresolved_phm,
        cwd: Some(cwd),
    })
}

fn build_hub_agent_report(s: &Store) -> Result<locus_core::AgentReport> {
    let doctor = gather_doctor_report(s)?;
    let user_home = dirs::home_dir();
    let mut opts = probe_agent_options(&cwd(), user_home.as_deref());
    opts.home_ready = s.seal_key_path().exists() && s.home().exists();
    Ok(agent_report_from_doctor(doctor, opts))
}

fn print_agent_report_human(report: &locus_core::AgentReport) {
    let status_s = match report.status {
        AgentStatus::Ready => "ready".green().bold().to_string(),
        AgentStatus::Protected => "protected".yellow().bold().to_string(),
        AgentStatus::Unsafe => "unsafe".red().bold().to_string(),
    };
    println!(
        "{} agent readiness  {}  v{}",
        "locus".magenta().bold(),
        status_s,
        report.version
    );
    println!(
        "  ready     {}",
        if report.ready {
            "true".green().to_string()
        } else {
            "false".yellow().to_string()
        }
    );
    let d = &report.doctor;
    let dv = match d.verdict {
        DoctorVerdict::Safe => "SAFE".green().bold().to_string(),
        DoctorVerdict::Warn => "WARN".yellow().bold().to_string(),
        DoctorVerdict::Unsafe => "UNSAFE".red().bold().to_string(),
    };
    println!(
        "  doctor    {}  seal={}  bindings={}  pending={}",
        dv,
        if d.seal_ok {
            "ok".green().to_string()
        } else {
            "FAIL".red().to_string()
        },
        d.bindings,
        d.pending_approvals
    );
    if let Some(ref pin) = report.pin {
        println!(
            "  pin       {}  tenant={}  seal={}{}",
            pin.alias.cyan().bold(),
            pin.tenant.yellow(),
            if pin.seal_ok {
                "ok".green().to_string()
            } else {
                "FAIL".red().to_string()
            },
            if pin.expired {
                format!("  {}", "EXPIRED".red().bold())
            } else {
                String::new()
            }
        );
    } else {
        println!("  pin       {}", "none".dimmed());
    }
    let yn = |v: bool| {
        if v {
            "yes".green().to_string()
        } else {
            "no".dimmed().to_string()
        }
    };
    println!(
        "  mcp       claude={}  cursor={}  codex={}  grok={}",
        yn(report.mcp_registered.claude),
        yn(report.mcp_registered.cursor),
        yn(report.mcp_registered.codex),
        yn(report.mcp_registered.grok)
    );
    if report.findings.is_empty() {
        println!(
            "  {}",
            "all clear — identity plane ready for agent work"
                .green()
                .bold()
        );
    } else {
        println!("  findings");
        for f in &report.findings {
            println!("    · {f}");
        }
    }
    if !report.next_steps.is_empty() {
        println!("  next");
        for s in &report.next_steps {
            println!("    {} {}", "→".dimmed(), s.cyan());
        }
    }
    println!(
        "  commands  {}  ·  {}",
        report.commands.enter.dimmed(),
        report.commands.whoami.dimmed()
    );
    println!(
        "  {}",
        "machine contract: locus agent report --json".dimmed()
    );
}

fn cmd_agent_doctor(json: bool) -> Result<()> {
    let s = store()?;
    let _ = s.check_drift_and_freeze()?;
    let report = build_hub_agent_report(&s)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_agent_report_human(&report);
    }
    std::process::exit(report.exit_code);
}

fn cmd_agent_report(json: bool) -> Result<()> {
    let s = store()?;
    let _ = s.check_drift_and_freeze()?;
    let report = build_hub_agent_report(&s)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_agent_report_human(&report);
    }
    std::process::exit(report.exit_code);
}

fn cmd_agent_setup(
    client: &str,
    claude_scope: &str,
    apply: bool,
    dry_run: bool,
    write_workspace: bool,
    mcp_bin: Option<String>,
    json: bool,
) -> Result<()> {
    if !apply && !dry_run {
        bail!("pass --apply or --dry-run (refusing to mutate without intent)");
    }
    if apply && dry_run {
        bail!("pass only one of --apply or --dry-run");
    }
    if apply {
        require_local_control_boundary("locus agent setup --apply")?;
    }
    let claude_user_scope = match claude_scope {
        "project" => false,
        "user" => true,
        other => bail!("unknown --claude-scope '{other}' (use project|user)"),
    };
    let clients: Vec<&str> = match client {
        // `all` covers clients with a known on-disk config path — including
        // Grok Build (`~/.grok/config.toml`, Codex-style TOML). `generic`
        // stays print-only and must be asked for explicitly.
        "all" => vec!["claude", "cursor", "codex", "grok"],
        "claude" | "cursor" | "codex" | "grok" | "generic" => vec![client],
        other => bail!("unknown client '{other}' (use claude|cursor|codex|grok|generic|all)"),
    };
    // User-scope Claude registration goes through the claude CLI — Locus
    // never hand-edits `~/.claude.json` (mixed runtime state owned by Claude
    // Code, no stability guarantee). Resolve the CLI up front; refuse to
    // apply without it.
    let claude_cli: Option<PathBuf> = if claude_user_scope && clients.contains(&"claude") {
        let found = find_on_path("claude");
        if apply && found.is_none() {
            bail!(
                "--claude-scope user requires the `claude` CLI on PATH \
                 (user-scope servers live in ~/.claude.json, which Locus never \
                 edits directly).\n  \
                 Install Claude Code, or register manually:\n    \
                 claude mcp add-json locus '<server-json>' --scope user\n  \
                 Or use --claude-scope project (writes project .mcp.json)."
            );
        }
        found
    } else {
        None
    };

    // Ensure ~/.locus (or LOCUS_HOME) exists — init layout + seal key if needed.
    let s = store()?;
    if let Some(b) = mcp_bin.as_deref() {
        validate_explicit_mcp_bin(b)?;
    }
    let (bin, bin_fallback) = resolve_mcp_bin_with_fallback(mcp_bin);
    if bin_fallback {
        eprintln!(
            "{} mcp bin resolved to bare 'locus-mcp' (no sibling next to the locus binary) — \
             GUI clients launched with a minimal PATH may fail to start it; \
             pass --mcp-bin /path/to/locus-mcp to pin an absolute path",
            "warn:".yellow().bold()
        );
    }
    let mut actions: Vec<String> = Vec::new();
    let project = cwd();
    actions.push(format!("ensure locus home → {}", s.home().display()));

    // Grok Build / generic stdio clients: canonical entry emitted for paste
    // (JSON + TOML shapes) — never a guessed config-path write.
    let mut paste_entry: Option<(serde_json::Value, String)> = None;

    for c in &clients {
        let env_map = mcp_agent_env(c);
        // Invariant: agent setup never enables desktop notify spam.
        debug_assert!(
            !env_map.contains_key("LOCUS_NOTIFY"),
            "LOCUS_NOTIFY must not be set by agent setup"
        );
        // `type: "stdio"` is documented as required by Cursor and accepted by
        // Claude Code / mcpServers-style clients — harmless where optional.
        let server_entry = json!({
            "type": "stdio",
            "command": &bin,
            "args": [],
            "env": env_map,
        });
        match *c {
            "claude" if claude_user_scope => {
                actions.push(format!(
                    "mcp claude (user scope) → claude mcp add-json locus … --scope user \
                     (CLI-managed ~/.claude.json; claude CLI: {})",
                    claude_cli
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "NOT FOUND on PATH".into())
                ));
                if apply {
                    let cli = claude_cli
                        .as_ref()
                        .expect("claude CLI resolved before apply");
                    claude_user_scope_register(cli, &server_entry)?;
                }
            }
            "claude" => {
                let path = project.join(".mcp.json");
                actions.push(format!(
                    "mcp claude → {} (LOCUS_AUTO_PIN=cwd, LOCUS_CLIENT=claude)",
                    path.display()
                ));
                if apply {
                    merge_mcp_json(&path, "locus", &server_entry)?;
                }
            }
            "cursor" => {
                let path = project.join(".cursor").join("mcp.json");
                actions.push(format!(
                    "mcp cursor → {} (LOCUS_AUTO_PIN=cwd, LOCUS_CLIENT=cursor)",
                    path.display()
                ));
                if apply {
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    merge_mcp_json(&path, "locus", &server_entry)?;
                    if let Some(home) = dirs::home_dir() {
                        let global = home.join(".cursor").join("mcp.json");
                        if global.exists() {
                            merge_mcp_json(&global, "locus", &server_entry)?;
                            actions.push(format!("mcp cursor global → {}", global.display()));
                        }
                    }
                }
            }
            "codex" => {
                let home = dirs::home_dir().context("home dir for codex config")?;
                let path = home.join(".codex").join("config.toml");
                actions.push(format!(
                    "mcp codex → {} (LOCUS_AUTO_PIN=cwd, LOCUS_CLIENT=codex)",
                    path.display()
                ));
                if apply {
                    merge_codex_mcp(&path, &bin, c)?;
                }
            }
            "grok" => {
                // Grok Build's documented config: ~/.grok/config.toml with
                // Codex-style [mcp_servers.<name>] tables — same fail-closed
                // toml_edit merge as codex (parse error ⇒ abort untouched).
                let home = dirs::home_dir().context("home dir for grok config")?;
                let path = home.join(".grok").join("config.toml");
                actions.push(format!(
                    "mcp grok → {} (LOCUS_AUTO_PIN=cwd, LOCUS_CLIENT=grok)",
                    path.display()
                ));
                if apply {
                    merge_codex_mcp(&path, &bin, c)?;
                }
            }
            "generic" => {
                actions.push(
                    "mcp generic → print-only (no known on-disk config path — paste the \
                     emitted server entry into the client's MCP settings; register the \
                     probe with LOCUS_GROK_MCP_CONFIG=<path-to-its-config>)"
                        .to_string(),
                );
                paste_entry = Some((
                    json!({ "mcpServers": { "locus": server_entry.clone() } }),
                    stdio_server_entry_toml(&bin, c),
                ));
            }
            _ => {}
        }
    }

    let agent_path = agent_md_path(&project);
    actions.push(format!("agent guidance → {}", agent_path.display()));
    if apply {
        if let Some(parent) = agent_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&agent_path, agent_md_content())?;
    }

    if write_workspace {
        let ws = project.join(".locus.toml");
        if ws.exists() {
            actions.push(format!(
                "workspace stub skipped (exists) → {}",
                ws.display()
            ));
        } else {
            actions.push(format!("workspace stub → {}", ws.display()));
            if apply {
                std::fs::write(&ws, workspace_stub_toml())?;
            }
        }
    }

    // Post-write verification: re-read what we just wrote via the same probe
    // doctor uses. A merge no-op or unwritable config must not report ok.
    let mut verified: Option<McpRegistered> = None;
    let mut verify_failures: Vec<String> = Vec::new();
    if apply {
        let user_home = dirs::home_dir();
        let probe = probe_mcp_registered(&project, user_home.as_deref());
        // User-scope claude is CLI-managed (~/.claude.json) — the file probe
        // cannot see it, so it is verified via the claude CLI below instead.
        let probe_clients: Vec<&str> = clients
            .iter()
            .copied()
            .filter(|c| !(claude_user_scope && *c == "claude"))
            .collect();
        for c in mcp_verify_failures(&probe_clients, &probe) {
            let path = match c {
                "claude" => project.join(".mcp.json").display().to_string(),
                "cursor" => project
                    .join(".cursor")
                    .join("mcp.json")
                    .display()
                    .to_string(),
                "codex" => user_home
                    .as_ref()
                    .map(|h| h.join(".codex").join("config.toml").display().to_string())
                    .unwrap_or_else(|| "~/.codex/config.toml".into()),
                "grok" => user_home
                    .as_ref()
                    .map(|h| h.join(".grok").join("config.toml").display().to_string())
                    .unwrap_or_else(|| "~/.grok/config.toml".into()),
                _ => continue,
            };
            verify_failures.push(format!("{c}: {path}"));
        }
        if claude_user_scope && clients.contains(&"claude") {
            let cli = claude_cli.as_ref().expect("claude CLI resolved for apply");
            if !claude_user_scope_verify(cli).unwrap_or(false) {
                verify_failures
                    .push("claude: user scope (`claude mcp get locus` did not confirm)".into());
            }
        }
        verified = Some(probe);
    }

    if json {
        println!(
            "{}",
            json!({
                "ok": verify_failures.is_empty(),
                "apply": apply,
                "dry_run": dry_run,
                "clients": clients,
                "home": s.home().display().to_string(),
                "mcp_bin": bin,
                "mcp_bin_fallback": bin_fallback,
                "actions": actions,
                "verified": verified,
                "verify_failures": verify_failures,
                "paste_server_entry": paste_entry.as_ref().map(|(j, _)| j.clone()),
                "paste_server_toml": paste_entry.as_ref().map(|(_, t)| t.clone()),
                "env": {
                    "LOCUS_AUTO_PIN": "cwd",
                    "LOCUS_CLIENT": "<client>",
                    "LOCUS_NOTIFY": null,
                },
            })
        );
        if !verify_failures.is_empty() {
            std::process::exit(1);
        }
    } else if !verify_failures.is_empty() {
        eprintln!(
            "{} agent setup wrote config, but post-write verification failed:",
            "error:".red().bold()
        );
        for f in &verify_failures {
            eprintln!("   · locus entry not found after write — {f}");
        }
        eprintln!("   inspect the file(s) above, then re-run `locus agent setup --apply`");
        std::process::exit(1);
    } else {
        let mode = if dry_run { "dry-run" } else { "applied" };
        println!("{} agent setup ({mode})", "ok".green().bold());
        for a in &actions {
            println!("   · {a}");
        }
        if let Some((entry_json, entry_toml)) = &paste_entry {
            println!();
            println!(
                "{}",
                "Paste ONE of the following into the client's MCP settings:".bold()
            );
            println!("# JSON shape (mcpServers-style clients):");
            println!("{}", serde_json::to_string_pretty(entry_json)?);
            println!();
            println!("# TOML shape (Codex-style clients):");
            println!("{entry_toml}");
            println!("# Then restart the client. Optional: export LOCUS_GROK_MCP_CONFIG=<path>");
            println!("# so `locus agent doctor` can verify the registration (JSON or TOML).");
        }
        if dry_run {
            println!("   {}", "re-run with --apply to write".dimmed());
        } else {
            println!();
            println!("{}", "Next steps".bold());
            println!(
                "  1. {}  (or set default_binding in .locus.toml)",
                "locus enter / locus pin <alias>".cyan()
            );
            println!("  2. {}", "locus whoami".cyan());
            println!("  3. {}", "locus agent doctor".cyan());
            println!("  4. Restart your AI client so MCP picks up locus-mcp");
            println!();
            println!(
                "  {} MCP env: LOCUS_AUTO_PIN=cwd + LOCUS_CLIENT — never LOCUS_NOTIFY by default",
                "note:".dimmed()
            );
            println!(
                "  {} MCP auto-pin is advisory only — the server never pins itself; a human runs `locus enter` (kill switch: LOCUS_MCP_AUTO_PIN=0, see .locus/AGENT.md)",
                "note:".dimmed()
            );
        }
    }
    Ok(())
}

/// Merge/write `[mcp_servers.locus]` into Codex config.toml with agent env.
///
/// Format-preserving upsert via `toml_edit`: an existing locus entry is healed
/// in place (stale `command`, missing env) instead of being skipped, every
/// other table/server is preserved verbatim, and a duplicate `locus` key is
/// structurally impossible. Fail closed: an unparseable file is left
/// unchanged and the merge errors with a remediation hint.
fn merge_codex_mcp(path: &std::path::Path, bin: &str, client: &str) -> Result<()> {
    use toml_edit::{value, DocumentMut, Item, Table};

    let mut doc: DocumentMut = if path.exists() {
        let raw = std::fs::read_to_string(path)?;
        raw.parse().map_err(|e| {
            anyhow::anyhow!(
                "refusing to modify {}: not valid TOML ({e}).\n  \
                 Fix the file or move it aside, then re-run.\n  \
                 Locus never overwrites a config it cannot parse — other MCP servers \
                 registered there would be lost.",
                path.display()
            )
        })?
    } else {
        DocumentMut::new()
    };

    let servers = doc
        .entry("mcp_servers")
        .or_insert(Item::Table(Table::new()));
    let servers = servers.as_table_like_mut().ok_or_else(|| {
        anyhow::anyhow!(
            "refusing to modify {}: `mcp_servers` is not a table.\n  \
             Fix the file or move it aside, then re-run.",
            path.display()
        )
    })?;
    let locus = servers.entry("locus").or_insert(Item::Table(Table::new()));
    // Normalize an inline-table entry (`locus = { command = "…" }`) to a
    // standard table so the nested `env` table can be attached.
    if let Some(inline) = locus.as_inline_table() {
        *locus = Item::Table(inline.clone().into_table());
    }
    let locus = locus.as_table_like_mut().ok_or_else(|| {
        anyhow::anyhow!(
            "refusing to modify {}: `mcp_servers.locus` is not a table.\n  \
             Fix the file or move it aside, then re-run.",
            path.display()
        )
    })?;
    locus.insert("command", value(bin));
    let env = locus.entry("env").or_insert(Item::Table(Table::new()));
    let env = env.as_table_like_mut().ok_or_else(|| {
        anyhow::anyhow!(
            "refusing to modify {}: `mcp_servers.locus.env` is not a table.\n  \
             Fix the file or move it aside, then re-run.",
            path.display()
        )
    })?;
    env.insert("LOCUS_AUTO_PIN", value("cwd"));
    env.insert("LOCUS_CLIENT", value(client));

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, doc.to_string())?;
    Ok(())
}

/// Register the locus server at Claude Code **user scope** by shelling out to
/// the claude CLI (`claude mcp add-json locus '<json>' --scope user`).
///
/// `~/.claude.json` is a mixed-state file owned by Claude Code (per-project
/// state, approval lists, runtime state) with no documented stability
/// guarantee, and a running session may write it concurrently — Locus never
/// hand-edits it.
///
/// Add-first: the add is attempted **without** removing anything, so a
/// failing add (not signed in, CLI broken, …) leaves a previous working
/// registration untouched. Only when the CLI refuses with "already exists"
/// (it never upserts) is the stale entry removed and the add retried — and
/// if that retry then fails, the error reports honestly that the previous
/// registration was removed and `locus` is currently unregistered at user
/// scope (the CLI gives us no restorable snapshot of the old entry).
fn claude_user_scope_register(
    claude_bin: &std::path::Path,
    server_entry: &serde_json::Value,
) -> Result<()> {
    let payload = serde_json::to_string(server_entry)?;
    let run_add = || -> Result<std::process::Output> {
        Command::new(claude_bin)
            .args(["mcp", "add-json", "locus", &payload, "--scope", "user"])
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("failed to run {} mcp add-json", claude_bin.display()))
    };
    let cli_error_text = |out: &std::process::Output| -> String {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if stderr.is_empty() {
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        } else {
            stderr
        }
    };

    let first = run_add()?;
    if first.status.success() {
        return Ok(());
    }
    let first_error = cli_error_text(&first);
    if !first_error.to_ascii_lowercase().contains("already exists") {
        // Nothing was removed — the previous registration (if any) is intact.
        bail!(
            "`claude mcp add-json locus … --scope user` failed ({}):\n{}\n  \
             Nothing was changed (any existing registration is intact). Fix \
             the claude CLI error above and re-run, or use \
             --claude-scope project.",
            first.status,
            first_error
        );
    }

    // Stale entry: the CLI refuses to overwrite an existing name, so heal by
    // removing it and re-adding. Only now is the previous registration
    // touched. Remove failure falls through to the re-add, which reports the
    // real error.
    let _ = Command::new(claude_bin)
        .args(["mcp", "remove", "locus", "--scope", "user"])
        .stdin(Stdio::null())
        .output();

    let second = run_add()?;
    if !second.status.success() {
        bail!(
            "`claude mcp add-json locus … --scope user` failed ({}) after the \
             existing `locus` entry was removed to replace it:\n{}\n  \
             The previous user-scope registration was removed and could not \
             be restored — `locus` is currently NOT registered at user scope. \
             Fix the claude CLI error above and re-run, register manually \
             (`claude mcp add-json locus '<server-json>' --scope user`), or \
             use --claude-scope project.",
            second.status,
            cli_error_text(&second)
        );
    }
    Ok(())
}

/// Post-write verification for user-scope Claude registration: the config
/// lives in CLI-managed `~/.claude.json`, so ask the CLI (`claude mcp get
/// locus`) instead of probing files. Exit 0 ⇒ registered.
fn claude_user_scope_verify(claude_bin: &std::path::Path) -> Result<bool> {
    let out = Command::new(claude_bin)
        .args(["mcp", "get", "locus"])
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("failed to run {} mcp get locus", claude_bin.display()))?;
    Ok(out.status.success())
}

/// Compact NDJSON tick for hub continuous whoami / `locus watch`.
///
/// Derived from [`verify_session`]; never includes secrets — aliases and
/// verdicts only. Distinct `kind` from the full session pack (`"session"`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
struct WatchHeartbeat {
    /// Stable stream tag (`watch`).
    kind: String,
    /// True when doctor ok and safe_next.ready (same as session pack).
    session_ok: bool,
    /// Active binding alias when whoami is available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    whoami: Option<String>,
    /// Doctor verdict: SAFE | WARN | UNSAFE.
    doctor_verdict: String,
    /// Machine-readable safe_next action (ready | enter | re_pin | …).
    safe_next: String,
    /// Whether a pin is currently present (whoami or doctor pin).
    pinned: bool,
    /// Runtime frozen (binding drift under live session).
    frozen: bool,
}

impl WatchHeartbeat {
    fn from_pack(pack: &locus_core::SessionVerificationPack) -> Self {
        let whoami_alias = pack.whoami.as_ref().map(|w| w.binding_alias.clone());
        let pinned =
            whoami_alias.is_some() || pack.doctor.pin.is_some() || pack.doctor.runtime.pinned;
        Self {
            kind: "watch".into(),
            session_ok: pack.session_ok,
            whoami: whoami_alias,
            doctor_verdict: pack.doctor.verdict.as_str().to_string(),
            safe_next: pack.safe_next.action.clone(),
            pinned,
            frozen: pack.doctor.runtime.frozen,
        }
    }
}

/// Whether a watch tick should end the process with a non-zero exit.
///
/// - `require_ok`: fail closed on any `session_ok=false` (hub / CI).
/// - otherwise (typical `--once`): fail only when a pin was expected/present.
fn watch_should_fail(session_ok: bool, pin_expected: bool, require_ok: bool) -> bool {
    if session_ok {
        return false;
    }
    require_ok || pin_expected
}

/// Poll `verify_session` until interrupted (or once with `--once`).
///
/// Each tick freezes on binding drift (via doctor/verify_session) and emits a
/// hub-suitable heartbeat. With `--json`, one NDJSON object per line.
fn cmd_watch(interval: &str, once: bool, require_ok: bool, json: bool) -> Result<()> {
    let s = store()?;
    let sleep_dur = parse_watch_interval(interval)?;
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    loop {
        let external = gather_doctor_external(&s, cwd.clone())?;
        let mut pack = verify_session(&s, &cwd, external)?;
        // Same finding pack as `locus doctor`: operator-shell control
        // capability readiness must reach hub heartbeat consumers too.
        let cap_status = locus_core::control_capability_status(s.home());
        attach_control_capability_findings(s.home(), &cap_status, &mut pack);
        let hb = WatchHeartbeat::from_pack(&pack);
        let pin_expected = hb.pinned || hb.frozen;

        let binding = hb.whoami.as_deref().unwrap_or("-");
        let _ = s.audit(
            "watch.tick",
            binding,
            Some(json!({
                "session_ok": hb.session_ok,
                "safe_next": hb.safe_next,
                "doctor_verdict": hb.doctor_verdict,
                "pinned": hb.pinned,
                "frozen": hb.frozen,
                "once": once,
                "require_ok": require_ok,
            })),
        );

        if json {
            // NDJSON: one compact object per tick for hub stream consumers.
            println!("{}", serde_json::to_string(&hb)?);
        } else {
            let ok_s = if hb.session_ok {
                "ok".green().bold().to_string()
            } else {
                "not_ok".yellow().bold().to_string()
            };
            let alias = hb.whoami.as_deref().unwrap_or("unpinned");
            let frozen_s = if hb.frozen {
                format!("  {}", "FROZEN".red().bold())
            } else {
                String::new()
            };
            println!(
                "{} watch  {}  {}  session_ok={}  doctor={}  safe_next={}{}",
                "locus".magenta().bold(),
                ok_s,
                alias.cyan(),
                hb.session_ok,
                hb.doctor_verdict.dimmed(),
                hb.safe_next.cyan(),
                frozen_s,
            );
            if !hb.session_ok {
                println!(
                    "  next  {} — {}",
                    pack.safe_next.action, pack.safe_next.message
                );
                if let Some(ref cmd) = pack.safe_next.command {
                    println!("  run   {}", cmd.cyan());
                }
            }
        }

        // Exit after one tick (`--once`) or fail-closed on first bad tick (`--require-ok`).
        if once || (require_ok && !hb.session_ok) {
            if watch_should_fail(hb.session_ok, pin_expected, require_ok) {
                bail!(
                    "watch session_ok=false (doctor={} safe_next={} pinned={})",
                    hb.doctor_verdict,
                    hb.safe_next,
                    hb.pinned
                );
            }
            return Ok(());
        }
        std::thread::sleep(sleep_dur);
    }
}

fn parse_watch_interval(s: &str) -> Result<std::time::Duration> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(std::time::Duration::from_secs(5));
    }
    // Bare seconds (including multi-digit) before unit split so "10" → 10s not 1s.
    if s.chars().all(|c| c.is_ascii_digit()) {
        let n: u64 = s.parse().context("invalid watch interval")?;
        return Ok(std::time::Duration::from_secs(n));
    }
    let (num, unit) = s.split_at(s.len().saturating_sub(1));
    let n: u64 = num.parse().context("invalid watch interval")?;
    match unit {
        "s" | "S" => Ok(std::time::Duration::from_secs(n)),
        "m" | "M" => Ok(std::time::Duration::from_secs(n.saturating_mul(60))),
        "h" | "H" => Ok(std::time::Duration::from_secs(n.saturating_mul(3600))),
        _ => bail!("invalid watch interval '{s}' (use e.g. 5s, 30s, 1m)"),
    }
}

fn print_doctor_human(report: &locus_core::DoctorReport) {
    let verdict_colored = match report.verdict {
        DoctorVerdict::Safe => "SAFE".green().bold().to_string(),
        DoctorVerdict::Warn => "WARN".yellow().bold().to_string(),
        DoctorVerdict::Unsafe => "UNSAFE".red().bold().to_string(),
    };

    println!("{} locus {}", "doctor".magenta().bold(), report.version);
    println!("  verdict   {verdict_colored}");
    println!("  home      {}", report.home);
    println!("  bindings  {}", report.bindings);
    println!(
        "  seal      {}",
        if report.seal_ok {
            "ok".green().to_string()
        } else {
            "FAIL".red().to_string()
        }
    );

    // Active pin
    if let Some(ref pin) = report.pin {
        println!(
            "  pin       {}  tenant={}  expires={}",
            pin.alias.cyan().bold(),
            pin.tenant.yellow(),
            pin.expires_at.dimmed()
        );
        println!(
            "  pin seal  {}",
            if pin.seal_ok {
                "ok".green().to_string()
            } else {
                "FAIL".red().to_string()
            }
        );
        if pin.expired {
            println!("  pin age   {}", "EXPIRED".red().bold());
        }
    } else {
        println!("  pin       {}", "none".dimmed());
    }

    // Runtime drift
    let rt = &report.runtime;
    if rt.pinned {
        let drift_s = if rt.ok {
            "ok".green().to_string()
        } else {
            format!("issues={}", rt.issues.join(","))
                .yellow()
                .to_string()
        };
        println!(
            "  runtime   {}  seal={} id_match={} tenant_match={} expired={}",
            drift_s, rt.seal_ok, rt.binding_id_match, rt.tenant_match, rt.expired
        );
    } else {
        println!("  runtime   {}", "not_pinned".dimmed());
    }

    // Approvals + dual-control
    let appr = &report.approvals;
    let appr_status = if !appr.exists {
        "missing".red().to_string()
    } else if !appr.writable {
        "not writable".red().to_string()
    } else if appr.corrupt > 0 {
        format!("{} corrupt", appr.corrupt).yellow().to_string()
    } else {
        "ok".green().to_string()
    };
    println!(
        "  approvals {}  pending={} dual_control_waiting={} approved={} untrusted={} expired_authenticated={} denied={}",
        appr_status,
        report.pending_approvals,
        report.dual_control_waiting,
        appr.approved_valid,
        appr.untrusted_approved,
        appr.expired_grants,
        appr.denied
    );

    // Phantom
    println!(
        "  phantom   {}",
        if report.phantom_on_path {
            "on PATH".green().to_string()
        } else {
            "missing".yellow().to_string()
        }
    );
    if !report.unresolved_phm.is_empty() {
        println!(
            "  credentials  {} unavailable: {}",
            report.unresolved_phm.len(),
            format_credential_issues(&report.unresolved_phm).dimmed()
        );
    } else if report.phantom_on_path {
        println!("  credentials  {}", "ok".green());
    }

    // Autopin / config.toml
    let ap = &report.autopin;
    let ap_s = if !ap.present {
        "no config.toml".dimmed().to_string()
    } else if !ap.ok {
        "invalid".yellow().to_string()
    } else if ap.remote_autopin_enabled {
        format!(
            "remote=on rules={} auto_pin={}",
            ap.remote_rules,
            ap.auto_pin.as_deref().unwrap_or("-")
        )
        .green()
        .to_string()
    } else {
        format!(
            "remote=off auto_pin={}",
            ap.auto_pin.as_deref().unwrap_or("-")
        )
        .dimmed()
        .to_string()
    };
    println!("  autopin   {ap_s}");
    if let Some(ref note) = ap.note {
        println!("            {}", note.dimmed());
    }

    // Workspace
    let ws = &report.workspace;
    if ws.found {
        println!(
            "  workspace {}  default={} require_pin={} allow=[{}]",
            "found".green(),
            ws.default_binding.as_deref().unwrap_or("-"),
            ws.require_pin,
            ws.allowed_bindings.join(", ")
        );
        if let Some(ref p) = ws.path {
            println!("            {}", p.dimmed());
        }
    } else {
        println!("  workspace {}", "none".dimmed());
    }

    // Recent audit + near-miss (24h)
    let au = &report.audit;
    println!(
        "  audit     total={}  recent_scope_freeze={}  recent_deny={}",
        au.total, au.scope_freeze, au.deny
    );
    println!(
        "  near_miss last_24h={}  (scope_freeze={} require_approval={})",
        report.near_miss_count, report.near_miss.scope_freeze, report.near_miss.require_approval
    );
    for ev in au.last.iter().take(5) {
        println!(
            "            {}  {}  {}",
            ev.ts.dimmed(),
            ev.op.cyan(),
            ev.binding.yellow()
        );
    }

    // Findings + verdict
    if report.findings.is_empty() {
        println!(
            "  {}",
            "all clear — SAFE to act under current pin".green().bold()
        );
    } else {
        for f in &report.findings {
            let mark = match f.severity {
                locus_core::IssueSeverity::Unsafe => "!".red().bold().to_string(),
                locus_core::IssueSeverity::Warn => "!".yellow().to_string(),
                locus_core::IssueSeverity::Info => "i".dimmed().to_string(),
            };
            println!("  {mark} [{}] {}", f.code, f.message);
        }
    }
}

fn format_credential_issues(issues: &[CredentialResolutionIssue]) -> String {
    issues
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Check Phantom locators internally and return provider/source metadata only.
///
/// Delegates to `locus_core` so the timeout-hardened, TTL-cached
/// `phantom list` path is shared with the MCP doctor/heartbeat surfaces.
fn collect_unresolved_phm_refs(
    s: &Store,
    phantom_on_path: bool,
) -> Result<Vec<CredentialResolutionIssue>> {
    Ok(locus_core::collect_unresolved_phm_refs(s, phantom_on_path)?)
}

fn cmd_hook(shell: &str) -> Result<()> {
    match shell {
        "zsh" | "bash" => {
            // status --oneline tokens: unpinned | require_pin | frozen | invalid | alias:tenant
            // zsh colors: red=unpinned/require_pin warn, yellow=frozen, cyan=healthy pin
            println!(
                r#"# Locus prompt + optional auto-enter — eval "$(locus hook zsh)"
# Prompt:
#   [locus:enter]           unpinned
#   [locus:enter!]          unpinned in require_pin workspace (warn)
#   [locus:FROZEN]          pin frozen after binding drift — re-pin
#   [locus:alias:tenant]    healthy pin
# LOCUS_AUTO_ENTER=1 → on directory change, try `locus enter` (workspace default / autopin).
# Never forces allowlist; never overrides with secrets.
# Control capability: export the persisted operator capability (0600 file
# minted by `locus quickstart`) when this shell does not already carry one.
if [[ -z "${{LOCUS_CONTROL_CAPABILITY:-}}" ]]; then
  _locus_cap="${{LOCUS_HOME:-$HOME/.locus}}/control_capability"
  if [[ -r "$_locus_cap" ]]; then
    export LOCUS_CONTROL_CAPABILITY="$(cat "$_locus_cap")"
  fi
  unset _locus_cap
fi
_locus_prompt() {{
  local s
  s="$(locus status --oneline 2>/dev/null)" || s="unpinned"
  if [[ "$s" == "require_pin" ]]; then
    echo "%F{{red}}%B[locus:enter!]%b%f"
  elif [[ "$s" == "unpinned" ]]; then
    echo "%F{{red}}[locus:enter]%f"
  elif [[ "$s" == "frozen" ]]; then
    echo "%F{{yellow}}%B[locus:FROZEN]%b%f"
  elif [[ "$s" == "invalid" ]]; then
    echo "%F{{red}}[locus:invalid]%f"
  else
    echo "%F{{cyan}}[locus:$s]%f"
  fi
}}
_locus_auto_enter() {{
  [[ "${{LOCUS_AUTO_ENTER:-0}}" == "1" ]] || return 0
  locus enter >/dev/null 2>&1 || true
}}
if [[ -n "${{ZSH_VERSION:-}}" ]]; then
  autoload -Uz add-zsh-hook 2>/dev/null
  add-zsh-hook chpwd _locus_auto_enter 2>/dev/null || true
elif [[ -n "${{BASH_VERSION:-}}" ]]; then
  # bash: run on PROMPT_COMMAND (best-effort). Colors via ANSI for bash PS1 use.
  if [[ -z "${{_LOCUS_PROMPT_CMD:-}}" ]]; then
    _LOCUS_PROMPT_CMD=1
    PROMPT_COMMAND="_locus_auto_enter${{PROMPT_COMMAND:+;$PROMPT_COMMAND}}"
  fi
fi
# Optional: add to PROMPT via: PROMPT='$(_locus_prompt) '"$PROMPT
"#
            );
        }
        "fish" => {
            println!(
                r#"# Locus prompt helper for fish
# [locus:enter] | [locus:enter!] (require_pin) | [locus:FROZEN] | [locus:alias:tenant]
# LOCUS_AUTO_ENTER=1 → try enter when changing directories
# Control capability: export the persisted operator capability if unset.
set -l _locus_cap (test -n "$LOCUS_HOME"; and echo "$LOCUS_HOME"; or echo "$HOME/.locus")/control_capability
if test -z "$LOCUS_CONTROL_CAPABILITY"; and test -r "$_locus_cap"
  set -gx LOCUS_CONTROL_CAPABILITY (cat "$_locus_cap")
end
function locus_prompt
  set -l s (locus status --oneline 2>/dev/null; or echo unpinned)
  if test "$s" = "require_pin"
    set_color --bold red
    echo -n "[locus:enter!]"
    set_color normal
  else if test "$s" = "unpinned"
    set_color red
    echo -n "[locus:enter]"
    set_color normal
  else if test "$s" = "frozen"
    set_color --bold yellow
    echo -n "[locus:FROZEN]"
    set_color normal
  else if test "$s" = "invalid"
    set_color red
    echo -n "[locus:invalid]"
    set_color normal
  else
    set_color cyan
    echo -n "[locus:$s]"
    set_color normal
  end
  echo
end
function _locus_auto_enter --on-variable PWD
  if test "$LOCUS_AUTO_ENTER" = "1"
    locus enter >/dev/null 2>&1; or true
  end
end
"#
            );
        }
        other => bail!("unsupported shell: {other} (use zsh|bash|fish)"),
    }
    Ok(())
}

fn resolve_mcp_bin(mcp_bin: Option<String>) -> String {
    resolve_mcp_bin_with_fallback(mcp_bin).0
}

/// Escape a string for interpolation into a TOML basic (double-quoted)
/// string: backslashes, quotes, and control characters — a binary path with
/// a quote or backslash must never produce invalid (or value-mangling) TOML.
fn toml_basic_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{0008}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{000C}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 || c == '\u{007F}' => {
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// Canonical `[mcp_servers.locus]` TOML shape for paste into any stdio MCP
/// client (Grok Build, Codex-style configs). Env comes from `mcp_agent_env`
/// (LOCUS_AUTO_PIN=cwd + LOCUS_CLIENT — never LOCUS_NOTIFY). All interpolated
/// values are TOML basic-string escaped.
fn stdio_server_entry_toml(bin: &str, client: &str) -> String {
    let mut out = String::new();
    out.push_str("[mcp_servers.locus]\n");
    out.push_str(&format!("command = \"{}\"\n", toml_basic_escape(bin)));
    out.push_str("args = []\n\n[mcp_servers.locus.env]\n");
    for (k, v) in mcp_agent_env(client) {
        if let Some(value) = v.as_str() {
            out.push_str(&format!("{k} = \"{}\"\n", toml_basic_escape(value)));
        }
    }
    out
}

/// Resolve the locus-mcp launch command. The bool is true when resolution
/// fell back to the bare name `locus-mcp` (no explicit `--mcp-bin`, no
/// sibling binary next to the CLI) — GUI clients with a minimal PATH may
/// fail to launch that.
fn resolve_mcp_bin_with_fallback(mcp_bin: Option<String>) -> (String, bool) {
    if let Some(p) = mcp_bin {
        return (p, false);
    }
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("locus-mcp");
            if candidate.exists() {
                return (candidate.display().to_string(), false);
            }
        }
    }
    ("locus-mcp".into(), true)
}

/// Fail closed on an explicit `--mcp-bin` that cannot launch: the path must
/// exist, or a bare command name must be findable on PATH.
fn validate_explicit_mcp_bin(bin: &str) -> Result<()> {
    let p = std::path::Path::new(bin);
    if p.exists() {
        return Ok(());
    }
    let is_bare = !bin.contains(std::path::MAIN_SEPARATOR) && !bin.contains('/');
    if is_bare && find_on_path(bin).is_some() {
        return Ok(());
    }
    bail!(
        "--mcp-bin {bin} does not exist{} — pass the path to a built locus-mcp \
         (e.g. ./target/release/locus-mcp) or omit --mcp-bin to auto-resolve",
        if is_bare {
            " and was not found on PATH"
        } else {
            ""
        }
    );
}

/// Locate a bare command name on PATH (first hit wins).
fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|c| c.is_file())
}

/// Requested clients whose registration the post-write probe could not
/// confirm — the regression net for merge no-ops and unwritable configs.
fn mcp_verify_failures<'a>(clients: &[&'a str], probe: &McpRegistered) -> Vec<&'a str> {
    clients
        .iter()
        .copied()
        .filter(|c| match *c {
            "claude" => !probe.claude,
            "cursor" => !probe.cursor,
            "codex" => !probe.codex,
            "grok" => !probe.grok,
            _ => false,
        })
        .collect()
}

fn cmd_setup(client: &str, print_only: bool, mcp_bin: Option<String>) -> Result<()> {
    if !print_only && matches!(client, "claude" | "cursor" | "codex" | "grok") {
        require_local_control_boundary("locus setup")?;
    }
    let bin = resolve_mcp_bin(mcp_bin);
    // Same env path as `locus agent setup` — LOCUS_AUTO_PIN=cwd + LOCUS_CLIENT,
    // never LOCUS_NOTIFY. Keeps first-run `locus setup` from silently diverging
    // from the agent-setup playbooks (auto-pin missing = ambient identity).
    // `type: "stdio"` is documented as required by Cursor; harmless elsewhere.
    let server_entry = serde_json::json!({
        "type": "stdio",
        "command": bin,
        "args": [],
        "env": mcp_agent_env(client),
    });

    match client {
        "claude" => {
            // Project-local .mcp.json or user can merge into Claude settings
            let entry = serde_json::json!({
                "mcpServers": {
                    "locus": server_entry
                }
            });
            if print_only {
                println!("{}", serde_json::to_string_pretty(&entry)?);
                return Ok(());
            }
            let path = cwd().join(".mcp.json");
            merge_mcp_json(&path, "locus", &server_entry)?;
            println!("{} wrote/merged {}", "ok".green().bold(), path.display());
            println!(
                "   also pin before agent work: {}",
                "locus pin <alias>".cyan()
            );
            println!("   verify: locus whoami && claude");
        }
        "cursor" => {
            let entry = serde_json::json!({
                "mcpServers": {
                    "locus": server_entry
                }
            });
            if print_only {
                println!("{}", serde_json::to_string_pretty(&entry)?);
                return Ok(());
            }
            // project-level
            let project = cwd().join(".cursor").join("mcp.json");
            if let Some(parent) = project.parent() {
                std::fs::create_dir_all(parent)?;
            }
            merge_mcp_json(&project, "locus", &server_entry)?;
            println!("{} wrote/merged {}", "ok".green().bold(), project.display());
            // global
            if let Some(home) = dirs::home_dir() {
                let global = home.join(".cursor").join("mcp.json");
                if global.exists() {
                    merge_mcp_json(&global, "locus", &server_entry)?;
                    println!("{} also merged {}", "ok".green().bold(), global.display());
                }
            }
        }
        "grok" => {
            // Grok Build: documented config at ~/.grok/config.toml with
            // Codex-style [mcp_servers.<name>] tables — same fail-closed
            // toml_edit merge as codex.
            if print_only {
                println!("{}", stdio_server_entry_toml(&bin, client));
                return Ok(());
            }
            let home = dirs::home_dir().context("home dir for grok config")?;
            let path = home.join(".grok").join("config.toml");
            merge_codex_mcp(&path, &bin, "grok")?;
            println!("{} wrote/merged {}", "ok".green().bold(), path.display());
            println!("   verify: locus agent doctor   # mcp_registered.grok");
        }
        "generic" => {
            // Any generic stdio MCP client: no known on-disk config path, so
            // never write — emit both shapes for paste (`--print` implied).
            println!("# Locus MCP server entry — paste into the client's MCP settings.");
            println!("# Generic client config path unknown; nothing was written.");
            println!("# JSON shape (mcpServers-style clients):");
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "mcpServers": { "locus": server_entry }
                }))?
            );
            println!();
            println!("# TOML shape (Codex-style clients):");
            println!("{}", stdio_server_entry_toml(&bin, client));
            println!("# Optional: export LOCUS_GROK_MCP_CONFIG=<path-to-its-config> so");
            println!("# `locus agent doctor` / `locus agent report` can verify registration");
            println!("# (JSON mcpServers shape or TOML [mcp_servers] shape).");
            return Ok(());
        }
        "codex" => {
            if print_only {
                println!("[mcp_servers.locus]");
                println!("command = \"{bin}\"");
                println!();
                println!("[mcp_servers.locus.env]");
                println!("LOCUS_AUTO_PIN = \"cwd\"");
                println!("LOCUS_CLIENT = \"codex\"");
                return Ok(());
            }
            let home = dirs::home_dir().context("home dir for codex config")?;
            let path = home.join(".codex").join("config.toml");
            merge_codex_mcp(&path, &bin, "codex")?;
            println!("{} wrote/merged {}", "ok".green().bold(), path.display());
        }
        other => bail!("unknown client: {other} (claude|cursor|codex|grok|generic)"),
    }

    println!();
    println!("Phantom pairing:");
    println!("  credential_ref = \"phm:MY_SECRET\"  # locus exec resolves via phantom reveal");
    println!("  Or env:VAR for CI. Never put raw secrets in binding files.");
    Ok(())
}

/// Thin audit reader: last N events from `$LOCUS_HOME/audit/events.jsonl`.
/// Never resolves credentials; ops/details only.
fn cmd_events(last: usize, op: Option<String>, binding: Option<String>, json: bool) -> Result<()> {
    let s = store()?;
    let all = s.read_audit_events()?;
    let events = filter_audit_events(&all, last, op.as_deref(), binding.as_deref());

    if json {
        println!("{}", serde_json::to_string_pretty(&events)?);
        return Ok(());
    }

    if events.is_empty() {
        let mut filt = String::new();
        if let Some(ref o) = op {
            filt.push_str(&format!(" op={o}"));
        }
        if let Some(ref b) = binding {
            filt.push_str(&format!(" binding={b}"));
        }
        println!(
            "{} no audit events{}",
            "->".dimmed(),
            if filt.is_empty() {
                String::new()
            } else {
                format!(" matching{filt}")
            }
        );
        println!("   {}", s.audit_path().display().to_string().dimmed());
        return Ok(());
    }

    println!(
        "{} {} event(s){}{}  {}",
        "events".magenta().bold(),
        events.len(),
        op.as_ref().map(|o| format!("  op={o}")).unwrap_or_default(),
        binding
            .as_ref()
            .map(|b| format!("  binding={b}"))
            .unwrap_or_default(),
        s.audit_path().display().to_string().dimmed()
    );
    for e in &events {
        let detail = e
            .detail
            .as_ref()
            .map(|d| {
                // Compact single-line detail for terminals
                serde_json::to_string(d).unwrap_or_else(|_| "{}".into())
            })
            .unwrap_or_default();
        println!(
            "  {}  {}  binding={}{}",
            e.ts.dimmed(),
            e.op.cyan(),
            e.binding.yellow(),
            if detail.is_empty() {
                String::new()
            } else {
                format!("  {detail}")
            }
        );
    }
    Ok(())
}

/// Export audit events as fleet-pulse JSON lines or OTLP logs JSON.
///
/// `--sink webhook` POSTs the same redacted body (no secrets). Unset URL fails
/// soft (skip, exit 0). Secret-looking bodies fail closed (error, no POST).
#[allow(clippy::too_many_arguments)]
fn cmd_events_export(
    last: usize,
    op: Option<String>,
    binding: Option<String>,
    otlp: bool,
    out: Option<PathBuf>,
    service_name: String,
    sink: EventsExportSink,
    url: Option<String>,
    _json: bool,
) -> Result<()> {
    let s = store()?;
    let all = s.read_audit_events()?;
    let format = if otlp {
        EventsExportFormat::Otlp
    } else {
        EventsExportFormat::JsonLines
    };
    let body = export_events(
        &all,
        &EventsExportOptions {
            last: Some(last),
            op,
            binding,
            format,
            service_name: Some(service_name),
        },
    )?;

    match sink {
        EventsExportSink::Webhook => {
            let resolved = resolve_audit_webhook_url(url.as_deref());
            match resolved {
                None => {
                    eprintln!(
                        "{} webhook sink skipped — set {} or --url (fail soft)",
                        "events export".magenta().bold(),
                        AUDIT_WEBHOOK_URL_ENV
                    );
                }
                Some(webhook_url) => {
                    let ct = export_content_type(format);
                    let result = post_audit_webhook(&webhook_url, &body, ct)?;
                    eprintln!(
                        "{} webhook POST {} → HTTP {} ({} bytes)",
                        "events export".magenta().bold(),
                        result.host.cyan(),
                        result.status,
                        result.bytes
                    );
                }
            }
            // Optional local copy alongside webhook.
            if let Some(path) = out {
                write_events_export_file(&path, &body)?;
            }
        }
        EventsExportSink::Local => {
            if let Some(path) = out {
                write_events_export_file(&path, &body)?;
            } else {
                print!("{body}");
            }
        }
    }
    Ok(())
}

fn write_events_export_file(path: &std::path::Path, body: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, body)?;
    eprintln!(
        "{} wrote {} ({} bytes)",
        "events export".magenta().bold(),
        path.display(),
        body.len()
    );
    Ok(())
}

fn cmd_forensics(sub: ForensicsCmd, json: bool) -> Result<()> {
    match sub {
        ForensicsCmd::Export { binding, last, out } => {
            let s = store()?;
            let phantom = phantom_on_path();
            let unresolved = collect_unresolved_phm_refs(&s, phantom).unwrap_or_default();
            let pack = export_forensics_pack(
                &s,
                ForensicsExportOptions {
                    binding: binding.clone(),
                    audit_last: Some(last),
                    doctor_external: Some(DoctorExternal {
                        phantom_on_path: phantom,
                        unresolved_phm: unresolved,
                        cwd: std::env::current_dir().ok(),
                    }),
                },
            )?;

            let pretty = serde_json::to_string_pretty(&pack)?;
            let target = out.or_else(|| {
                if json {
                    None
                } else {
                    Some(PathBuf::from("pack.json"))
                }
            });

            if let Some(path) = target {
                if let Some(parent) = path.parent() {
                    if !parent.as_os_str().is_empty() {
                        std::fs::create_dir_all(parent)?;
                    }
                }
                std::fs::write(&path, &pretty)?;
                if json {
                    // Still emit path metadata as JSON for machine consumers.
                    println!(
                        "{}",
                        serde_json::json!({
                            "ok": true,
                            "path": path.display().to_string(),
                            "pack_version": pack.pack_version,
                            "near_miss_count": pack.near_miss.count,
                            "audit_event_count": pack.audit_event_count,
                            "verdict": pack.doctor.verdict,
                        })
                    );
                } else {
                    println!("{} wrote {}", "forensics".magenta().bold(), path.display());
                    println!(
                        "  bindings={}  audit_events={}  pending_approvals={}  near_miss_24h={}  doctor={}",
                        pack.bindings.len(),
                        pack.audit_event_count,
                        pack.pending_approvals.len(),
                        pack.near_miss.count,
                        pack.doctor.verdict.as_str()
                    );
                    if let Some(ref tip) = pack.chain_tip.last_event_digest {
                        println!(
                            "  chain_tip events={} digest={}…",
                            pack.chain_tip.event_count,
                            tip.chars().take(12).collect::<String>()
                        );
                    }
                }
            } else {
                // --json with no --out: full pack on stdout
                println!("{pretty}");
            }
            Ok(())
        }
    }
}

fn cmd_notify(sub: NotifyCmd, json: bool) -> Result<()> {
    use locus_core::{load_config, notifications_enabled};

    let s = store()?;
    let home = s.home();
    match sub {
        NotifyCmd::Status => {
            let cfg = load_config(home);
            let effective = notifications_enabled();
            if json {
                println!(
                    "{}",
                    json!({
                        "config_enabled": cfg.notify.enabled,
                        "effective": effective,
                        "default": "off",
                        "env_LOCUS_NOTIFY": std::env::var("LOCUS_NOTIFY").ok(),
                        "hint": "Banners are opt-in. Enable with `locus notify on` or LOCUS_NOTIFY=1.",
                    })
                );
            } else {
                println!(
                    "{} desktop notifications: {} (config={})",
                    if effective {
                        "on".green().bold()
                    } else {
                        "off".yellow().bold()
                    },
                    if effective { "enabled" } else { "disabled" },
                    if cfg.notify.enabled { "true" } else { "false" }
                );
                if !effective {
                    println!(
                        "   {}",
                        "default is off so agents don't spam Notification Center".dimmed()
                    );
                    println!(
                        "   enable: {}  or  {}",
                        "locus notify on".cyan(),
                        "LOCUS_NOTIFY=1".cyan()
                    );
                }
            }
        }
        NotifyCmd::On => {
            s.require_local_control("locus notify on")?;
            let mut cfg = load_config(home);
            cfg.notify.enabled = true;
            let path = s.save_config(&cfg)?;
            if json {
                println!(
                    "{}",
                    json!({ "ok": true, "enabled": true, "path": path.display().to_string() })
                );
            } else {
                println!(
                    "{} notifications enabled (silent banners, rate-limited)",
                    "ok".green().bold()
                );
                println!("   wrote {}", path.display());
            }
        }
        NotifyCmd::Off => {
            s.require_local_control("locus notify off")?;
            let mut cfg = load_config(home);
            cfg.notify.enabled = false;
            let path = s.save_config(&cfg)?;
            if json {
                println!(
                    "{}",
                    json!({ "ok": true, "enabled": false, "path": path.display().to_string() })
                );
            } else {
                println!("{} notifications disabled", "ok".green().bold());
                println!("   wrote {}", path.display());
                if std::env::var("LOCUS_NOTIFY").is_ok() {
                    println!(
                        "   {}",
                        "note: unset LOCUS_NOTIFY in your shell if it is set".yellow()
                    );
                }
            }
        }
    }
    Ok(())
}

fn cmd_approve(sub: ApproveCmd, json: bool) -> Result<()> {
    let s = store()?;
    match sub {
        ApproveCmd::List { limit } => {
            let mut pending = s.pending_approvals()?;
            if pending.len() > limit {
                pending.truncate(limit);
            }

            if json {
                println!("{}", serde_json::to_string_pretty(&pending)?);
                return Ok(());
            }

            if pending.is_empty() {
                println!(
                    "{} no pending approvals in {}",
                    "->".dimmed(),
                    s.approvals_dir().display()
                );
                println!(
                    "   {}",
                    "Blocked tool calls write appr_*.json when agents hit policy.require_approval / rules."
                        .dimmed()
                );
                println!(
                    "   {}",
                    "Advisory: locus approve grant <id> --as <label> [--touchid]   Deny: locus approve deny <id>"
                        .dimmed()
                );
                return Ok(());
            }

            println!(
                "{} {} pending approval(s)",
                "approve".magenta().bold(),
                pending.len()
            );
            for rec in &pending {
                let dual = s.tool_requires_dual_control(&rec.binding, &rec.tool);
                let required = locus_core::required_grant_count(dual);
                let grants_n = locus_core::format_grants_progress(rec.grants.len(), required);
                let progress = if dual {
                    format!("grants {grants_n} (dual_control)")
                } else {
                    format!("grants {grants_n}")
                };
                println!(
                    "  {}  {}  binding={}  tool={}",
                    rec.id.cyan().bold(),
                    rec.created_at.to_rfc3339().dimmed(),
                    rec.binding.yellow(),
                    rec.tool.yellow()
                );
                println!(
                    "      {}  requester={}  session={}",
                    progress,
                    if rec.requester.is_empty() {
                        "-"
                    } else {
                        rec.requester.as_str()
                    },
                    rec.session_id.dimmed()
                );
            }
            println!();
            println!(
                "{}",
                "Advisory: locus approve grant <id> --as <label> [--touchid]   status: locus approve status <id>   wait: locus approve wait <id>"
                    .dimmed()
            );
            Ok(())
        }
        ApproveCmd::Grant {
            id,
            as_principal,
            ttl,
            touchid,
        } => {
            let principal = as_principal
                .or_else(|| env::var("LOCUS_PRINCIPAL").ok().filter(|s| !s.is_empty()))
                .or_else(|| env::var("USER").ok().filter(|s| !s.is_empty()))
                .unwrap_or_else(|| "unknown".into());
            let ttl_dur = match ttl {
                Some(ref t) => Some(parse_ttl(t)?),
                None => None,
            };
            if touchid {
                // Confirm *before* mutating the approval record (fail closed).
                let preview = s.load_approval(&id)?;
                confirm_grant_touchid(&principal, &id, &preview.tool, &preview.binding)?;
            }
            let rec = s.grant_approval(&id, ttl_dur, &principal)?;
            let dual = s.tool_requires_dual_control(&rec.binding, &rec.tool);
            let required = locus_core::required_grant_count(dual);
            if json {
                let mut v = serde_json::to_value(&rec)?;
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("dual_control".into(), json!(dual));
                    obj.insert("required_grants".into(), json!(required));
                    obj.insert("approval_authority".into(), json!("local_advisory"));
                    obj.insert("authoritative_grants".into(), json!(0));
                    obj.insert("required_authoritative_grants".into(), json!(required));
                    obj.insert("authoritative_path_enabled".into(), json!(false));
                    obj.insert("grants_progress".into(), json!(format!("0/{required}")));
                    obj.insert("advisory_assertions".into(), json!(rec.grants.len()));
                    obj.insert(
                        "authority_blocker".into(),
                        json!(locus_core::EXTERNAL_APPROVAL_AUTHORITY_BLOCKER),
                    );
                }
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else {
                print_grant_summary(&rec, &principal, dual, required, ttl.as_deref());
            }
            Ok(())
        }
        ApproveCmd::Status { id } => {
            let rec = s.load_approval(&id)?;
            let dual = s.tool_requires_dual_control(&rec.binding, &rec.tool);
            let required = if dual { 2 } else { 1 };
            if json {
                let mut v = serde_json::to_value(&rec)?;
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("dual_control".into(), json!(dual));
                    obj.insert("required_grants".into(), json!(required));
                    obj.insert("approval_authority".into(), json!("local_advisory"));
                    obj.insert("authoritative_grants".into(), json!(0));
                    obj.insert("required_authoritative_grants".into(), json!(required));
                    obj.insert("authoritative_path_enabled".into(), json!(false));
                    obj.insert("grants_progress".into(), json!(format!("0/{required}")));
                    obj.insert("advisory_assertions".into(), json!(rec.grants.len()));
                    obj.insert(
                        "authority_blocker".into(),
                        json!(locus_core::EXTERNAL_APPROVAL_AUTHORITY_BLOCKER),
                    );
                }
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else {
                println!(
                    "{} {}  status={}",
                    "approve".magenta().bold(),
                    rec.id.cyan(),
                    rec.status.as_str()
                );
                println!("   tool      {}", rec.tool);
                println!("   binding   {}", rec.binding);
                println!(
                    "   requester {}",
                    if rec.requester.is_empty() {
                        "-"
                    } else {
                        &rec.requester
                    }
                );
                println!(
                    "   dual      {}  authoritative 0/{}  advisory {}",
                    if dual { "yes" } else { "no" },
                    required,
                    rec.grants.len()
                );
                if rec.grants.is_empty() {
                    println!("   grants    (none)");
                } else {
                    for g in &rec.grants {
                        println!(
                            "   grant     {} @ {}",
                            g.principal.yellow(),
                            g.granted_at.to_rfc3339().dimmed()
                        );
                    }
                }
                if let Some(exp) = rec.expires_at {
                    println!("   expires   {}", exp.to_rfc3339());
                }
                println!("   digest    {}", rec.args_digest.dimmed());
                println!("   session   {}", rec.session_id.dimmed());
            }
            Ok(())
        }
        ApproveCmd::Wait {
            id,
            timeout,
            interval_ms,
        } => {
            use locus_core::ApprovalStatus;
            use std::thread;
            use std::time::{Duration as StdDuration, Instant};

            let deadline = Instant::now() + StdDuration::from_secs(timeout);
            let interval = StdDuration::from_millis(interval_ms.max(50));
            loop {
                let rec = s.load_approval(&id)?;
                let dual = s.tool_requires_dual_control(&rec.binding, &rec.tool);
                let required = if dual { 2 } else { 1 };
                match rec.status {
                    ApprovalStatus::Approved => {
                        if !rec.is_valid_grant() {
                            bail!(
                                "approval {} is not authoritative: {}",
                                rec.id,
                                locus_core::EXTERNAL_APPROVAL_AUTHORITY_BLOCKER
                            );
                        }
                        if json {
                            let mut v = serde_json::to_value(&rec)?;
                            if let Some(obj) = v.as_object_mut() {
                                obj.insert("wait_result".into(), json!("approved"));
                                obj.insert("dual_control".into(), json!(dual));
                                obj.insert("required_grants".into(), json!(required));
                            }
                            println!("{}", serde_json::to_string_pretty(&v)?);
                        } else {
                            println!(
                                "{} {} approved  tool={}  binding={}  grants {}/{}",
                                "ok".green().bold(),
                                rec.id.cyan(),
                                rec.tool,
                                rec.binding,
                                rec.grants.len(),
                                required
                            );
                            if let Some(exp) = rec.expires_at {
                                println!("   expires  {}", exp.to_rfc3339());
                            }
                        }
                        return Ok(());
                    }
                    ApprovalStatus::Denied => {
                        if json {
                            let mut v = serde_json::to_value(&rec)?;
                            if let Some(obj) = v.as_object_mut() {
                                obj.insert("wait_result".into(), json!("denied"));
                            }
                            println!("{}", serde_json::to_string_pretty(&v)?);
                        }
                        bail!(
                            "approval {} denied (tool={} binding={})",
                            rec.id,
                            rec.tool,
                            rec.binding
                        );
                    }
                    ApprovalStatus::Pending => {
                        if Instant::now() >= deadline {
                            if json {
                                let mut v = serde_json::to_value(&rec)?;
                                if let Some(obj) = v.as_object_mut() {
                                    obj.insert("wait_result".into(), json!("timeout"));
                                    obj.insert("timeout_secs".into(), json!(timeout));
                                    obj.insert(
                                        "grants_progress".into(),
                                        json!(format!("{}/{}", rec.grants.len(), required)),
                                    );
                                }
                                println!("{}", serde_json::to_string_pretty(&v)?);
                            }
                            bail!(
                                "timeout after {}s waiting for approval {} (status=pending, grants {}/{})",
                                timeout,
                                rec.id,
                                rec.grants.len(),
                                required
                            );
                        }
                        thread::sleep(interval);
                    }
                }
            }
        }
        ApproveCmd::Deny { id } => {
            let rec = s.deny_approval(&id)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&rec)?);
            } else {
                println!(
                    "{} denied {}  tool={}  binding={}",
                    "ok".green().bold(),
                    rec.id.cyan(),
                    rec.tool,
                    rec.binding
                );
            }
            Ok(())
        }
    }
}

/// Human-readable summary after `locus approve grant`.
fn print_grant_summary(
    rec: &locus_core::ApprovalRecord,
    principal: &str,
    dual: bool,
    required: usize,
    _ttl_flag: Option<&str>,
) {
    let principals: Vec<String> = rec.grants.iter().map(|g| g.principal.clone()).collect();
    let progress = locus_core::format_dual_control_progress(
        rec.grants.len(),
        required,
        &principals,
        dual,
        false,
    );
    println!(
        "{} recorded local advisory {} as {}",
        "ok".yellow().bold(),
        rec.id.cyan(),
        principal.yellow()
    );
    println!("   tool          {}", rec.tool.yellow());
    println!("   binding       {}", rec.binding.yellow());
    println!("   advisory      {}", progress);
    println!("   authoritative 0/{required}");
    println!("   status        pending");
    println!(
        "   blocker       {}",
        locus_core::EXTERNAL_APPROVAL_AUTHORITY_BLOCKER
    );
    println!("   local labels and Touch ID confirmation never authorize provider execution");
}

/// Local UI confirmation before recording an advisory assertion.
///
/// Fail closed: cancel / non-confirm / missing osascript aborts the grant.
/// Test hooks: `LOCUS_TOUCHID_MOCK=ok` | `cancel` (any OS).
fn confirm_grant_touchid(principal: &str, id: &str, tool: &str, binding: &str) -> Result<()> {
    if let Ok(v) = env::var("LOCUS_TOUCHID_MOCK") {
        let v = v.trim().to_ascii_lowercase();
        match v.as_str() {
            "ok" | "1" | "true" | "yes" | "confirm" => return Ok(()),
            "cancel" | "deny" | "0" | "false" | "no" | "abort" => {
                bail!(
                    "Touch ID / confirm cancelled — grant of {id} as {principal} aborted (fail closed)"
                );
            }
            other => bail!("invalid LOCUS_TOUCHID_MOCK={other:?} (use ok or cancel)"),
        }
    }

    #[cfg(target_os = "macos")]
    {
        use std::path::Path;
        let osascript = Path::new("/usr/bin/osascript");
        if !osascript.is_file() {
            bail!(
                "--touchid requires /usr/bin/osascript (not found) — grant aborted (fail closed)"
            );
        }
        // Blocking dialog — not real biometrics; user must click Confirm.
        let prompt = format!(
            "Record local advisory as {principal}?\n\nApproval: {id}\nTool: {tool}\nBinding: {binding}\n\nThis does not authorize provider execution."
        );
        let script = format!(
            r#"display dialog "{}" with title "Locus approve grant" buttons {{"Cancel", "Confirm"}} default button "Confirm" cancel button "Cancel" with icon caution"#,
            escape_applescript_dialog(&prompt)
        );
        let status = Command::new(osascript)
            .arg("-e")
            .arg(&script)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .with_context(|| "failed to run osascript for --touchid confirm")?;
        if !status.success() {
            bail!(
                "Touch ID / confirm cancelled — grant of {id} as {principal} aborted (fail closed)"
            );
        }
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (principal, id, tool, binding);
        bail!(
            "--touchid is only supported on macOS (or set LOCUS_TOUCHID_MOCK=ok|cancel for tests)"
        );
    }
}

/// Escape for AppleScript double-quoted string literals.
#[cfg(target_os = "macos")]
fn escape_applescript_dialog(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .collect()
}

/// Merge a single server entry into an mcpServers JSON file.
///
/// Fail closed: a malformed file (bad JSON, non-object root, non-object
/// `mcpServers`) is left byte-for-byte unchanged and the merge errors with a
/// remediation hint — replacing it would silently destroy every other
/// registered MCP server.
fn merge_mcp_json(path: &std::path::Path, name: &str, server: &serde_json::Value) -> Result<()> {
    let mut root: serde_json::Value = if path.exists() {
        let raw = std::fs::read_to_string(path)?;
        serde_json::from_str(&raw).map_err(|e| {
            anyhow::anyhow!(
                "refusing to modify {}: not valid JSON ({e}).\n  \
                 Fix the file (check it with `jq . {}`) or move it aside, then re-run.\n  \
                 Locus never overwrites a config it cannot parse — other MCP servers \
                 registered there would be lost.",
                path.display(),
                path.display()
            )
        })?
    } else {
        json!({ "mcpServers": {} })
    };
    if !root.is_object() {
        bail!(
            "refusing to modify {}: JSON root is not an object.\n  \
             Expected {{ \"mcpServers\": {{ ... }} }}. Fix the file or move it aside, then re-run.",
            path.display()
        );
    }
    let servers = root
        .as_object_mut()
        .unwrap()
        .entry("mcpServers")
        .or_insert_with(|| json!({}));
    if !servers.is_object() {
        bail!(
            "refusing to modify {}: \"mcpServers\" is not an object.\n  \
             Fix the file or move it aside, then re-run.",
            path.display()
        );
    }
    servers
        .as_object_mut()
        .unwrap()
        .insert(name.to_string(), server.clone());
    std::fs::write(path, serde_json::to_string_pretty(&root)?)?;
    Ok(())
}

#[cfg(test)]
mod setup_merge_tests {
    use super::{
        claude_user_scope_register, claude_user_scope_verify, mcp_verify_failures, merge_codex_mcp,
        merge_mcp_json, resolve_mcp_bin_with_fallback, validate_explicit_mcp_bin,
    };
    use locus_core::McpRegistered;
    use serde_json::json;

    fn server_entry() -> serde_json::Value {
        json!({"command": "/bin/locus-mcp", "args": [], "env": {"LOCUS_AUTO_PIN": "cwd"}})
    }

    // ── merge_mcp_json: fail closed on malformed input ──────────────────

    #[test]
    fn mcp_json_malformed_file_left_unchanged_and_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        let malformed = "{ this is not json";
        std::fs::write(&path, malformed).unwrap();
        let r = merge_mcp_json(&path, "locus", &server_entry());
        assert!(r.is_err(), "malformed JSON must fail closed");
        let msg = r.unwrap_err().to_string();
        assert!(msg.contains("refusing to modify"), "remediation msg: {msg}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            malformed,
            "file must be byte-for-byte unchanged"
        );
    }

    #[test]
    fn mcp_json_non_object_root_left_unchanged_and_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        std::fs::write(&path, "[1, 2, 3]").unwrap();
        let r = merge_mcp_json(&path, "locus", &server_entry());
        assert!(r.is_err(), "non-object root must fail closed");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[1, 2, 3]");
    }

    #[test]
    fn mcp_json_non_object_mcp_servers_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        std::fs::write(&path, r#"{"mcpServers": "oops"}"#).unwrap();
        assert!(merge_mcp_json(&path, "locus", &server_entry()).is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            r#"{"mcpServers": "oops"}"#
        );
    }

    #[test]
    fn mcp_json_preserves_other_servers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        std::fs::write(
            &path,
            r#"{"mcpServers": {"phantom": {"command": "phm-mcp"}}, "otherKey": true}"#,
        )
        .unwrap();
        merge_mcp_json(&path, "locus", &server_entry()).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["mcpServers"]["phantom"]["command"], "phm-mcp");
        assert_eq!(v["mcpServers"]["locus"]["command"], "/bin/locus-mcp");
        assert_eq!(v["otherKey"], true);
    }

    #[test]
    fn mcp_json_creates_fresh_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        merge_mcp_json(&path, "locus", &server_entry()).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["mcpServers"]["locus"]["env"]["LOCUS_AUTO_PIN"], "cwd");
    }

    // ── merge_codex_mcp: format-preserving upsert ───────────────────────

    #[test]
    fn codex_malformed_file_left_unchanged_and_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let malformed = "[mcp_servers.locus\ncommand=";
        std::fs::write(&path, malformed).unwrap();
        let r = merge_codex_mcp(&path, "/bin/locus-mcp", "codex");
        assert!(r.is_err(), "malformed TOML must fail closed");
        assert!(r.unwrap_err().to_string().contains("refusing to modify"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), malformed);
    }

    #[test]
    fn codex_stale_entry_updated_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        // Pre-env-era entry with a stale binary path and no env table.
        std::fs::write(
            &path,
            "[mcp_servers.locus]\ncommand = \"/old/gone/locus-mcp\"\n",
        )
        .unwrap();
        merge_codex_mcp(&path, "/new/locus-mcp", "codex").unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        let v: toml::Value = raw.parse().unwrap();
        let locus = &v["mcp_servers"]["locus"];
        assert_eq!(locus["command"].as_str(), Some("/new/locus-mcp"));
        assert_eq!(locus["env"]["LOCUS_AUTO_PIN"].as_str(), Some("cwd"));
        assert_eq!(locus["env"]["LOCUS_CLIENT"].as_str(), Some("codex"));
        assert!(!raw.contains("/old/gone/"), "stale command must be healed");
    }

    #[test]
    fn codex_other_servers_and_comments_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "# my codex config\nmodel = \"o4\"\n\n[mcp_servers.phantom]\ncommand = \"phm-mcp\"\n",
        )
        .unwrap();
        merge_codex_mcp(&path, "/bin/locus-mcp", "codex").unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("# my codex config"), "comments preserved");
        let v: toml::Value = raw.parse().unwrap();
        assert_eq!(v["model"].as_str(), Some("o4"));
        assert_eq!(
            v["mcp_servers"]["phantom"]["command"].as_str(),
            Some("phm-mcp")
        );
        assert_eq!(
            v["mcp_servers"]["locus"]["command"].as_str(),
            Some("/bin/locus-mcp")
        );
    }

    #[test]
    fn codex_repeat_merge_no_duplicate_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        merge_codex_mcp(&path, "/bin/locus-mcp", "codex").unwrap();
        merge_codex_mcp(&path, "/bin/locus-mcp-v2", "codex").unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        // Must stay valid TOML (duplicate table headers would fail the parse).
        let v: toml::Value = raw.parse().unwrap();
        assert_eq!(
            v["mcp_servers"]["locus"]["command"].as_str(),
            Some("/bin/locus-mcp-v2")
        );
        assert_eq!(raw.matches("[mcp_servers.locus]").count(), 1);
    }

    #[test]
    fn codex_inline_table_entry_upserted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[mcp_servers]\nlocus = { command = \"/old/locus-mcp\" }\n",
        )
        .unwrap();
        merge_codex_mcp(&path, "/new/locus-mcp", "codex").unwrap();
        let v: toml::Value = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(
            v["mcp_servers"]["locus"]["command"].as_str(),
            Some("/new/locus-mcp")
        );
        assert_eq!(
            v["mcp_servers"]["locus"]["env"]["LOCUS_CLIENT"].as_str(),
            Some("codex")
        );
    }

    #[test]
    fn codex_creates_parent_dir_and_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".codex").join("config.toml");
        merge_codex_mcp(&path, "/bin/locus-mcp", "codex").unwrap();
        let v: toml::Value = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(
            v["mcp_servers"]["locus"]["env"]["LOCUS_AUTO_PIN"].as_str(),
            Some("cwd")
        );
    }

    // ── agent setup post-write verification helpers ─────────────────────

    #[test]
    fn verify_failures_name_unregistered_clients_only() {
        let probe = McpRegistered {
            claude: true,
            ..Default::default()
        };
        assert_eq!(
            mcp_verify_failures(&["claude", "cursor", "codex"], &probe),
            vec!["cursor", "codex"]
        );
        assert!(mcp_verify_failures(&["claude"], &probe).is_empty());
        // Grok Build now has a real write path (~/.grok/config.toml) and is
        // verified like the other on-disk clients.
        assert_eq!(
            mcp_verify_failures(&["claude", "grok"], &probe),
            vec!["grok"]
        );
        let probe_grok = McpRegistered {
            grok: true,
            ..probe.clone()
        };
        assert!(mcp_verify_failures(&["grok"], &probe_grok).is_empty());
    }

    // ── claude user scope: CLI-managed registration (mock PATH shim) ────

    /// Executable `claude` stand-in recording its argv; unix-only (shebang).
    #[cfg(unix)]
    fn write_claude_shim(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("claude");
        std::fs::write(&p, body).unwrap();
        let mut perms = std::fs::metadata(&p).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&p, perms).unwrap();
        p
    }

    #[cfg(unix)]
    #[test]
    fn claude_user_scope_register_adds_first_without_touching_existing_entry() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let shim = write_claude_shim(
            dir.path(),
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 0\n",
                log.display()
            ),
        );
        let entry = json!({
            "type": "stdio",
            "command": "/bin/locus-mcp",
            "args": [],
            "env": {"LOCUS_AUTO_PIN": "cwd", "LOCUS_CLIENT": "claude"},
        });
        claude_user_scope_register(&shim, &entry).unwrap();

        let calls = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = calls.lines().collect();
        // Add-first: a clean add never removes anything.
        assert_eq!(lines.len(), 1, "no removal on a clean add: {calls}");
        let add = lines[0];
        let payload = add
            .strip_prefix("mcp add-json locus ")
            .and_then(|r| r.strip_suffix(" --scope user"))
            .unwrap_or_else(|| panic!("unexpected add-json argv: {add}"));
        // Payload is the exact server entry — stdio type, agent env, never
        // LOCUS_NOTIFY.
        let v: serde_json::Value = serde_json::from_str(payload).unwrap();
        assert_eq!(v, entry);
        assert!(v["env"].get("LOCUS_NOTIFY").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn claude_user_scope_register_heals_stale_entry_with_remove_then_readd() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let marker = dir.path().join("removed.marker");
        // "already exists" until the entry is removed; then adds succeed.
        let shim = write_claude_shim(
            dir.path(),
            &format!(
                "#!/bin/sh\n\
                 printf '%s\\n' \"$*\" >> '{log}'\n\
                 if [ \"$2\" = \"remove\" ]; then touch '{marker}'; exit 0; fi\n\
                 if [ \"$2\" = \"add-json\" ] && [ ! -f '{marker}' ]; then\n\
                   echo 'MCP server locus already exists in user config' >&2; exit 1\n\
                 fi\n\
                 exit 0\n",
                log = log.display(),
                marker = marker.display()
            ),
        );
        let entry = json!({"type": "stdio", "command": "/bin/locus-mcp"});
        claude_user_scope_register(&shim, &entry).unwrap();

        let calls = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = calls.lines().collect();
        assert_eq!(lines.len(), 3, "{calls}");
        assert!(lines[0].starts_with("mcp add-json locus "), "{calls}");
        assert_eq!(lines[1], "mcp remove locus --scope user", "{calls}");
        assert!(lines[2].starts_with("mcp add-json locus "), "{calls}");
    }

    #[cfg(unix)]
    #[test]
    fn claude_user_scope_register_fails_closed_with_cli_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let shim = write_claude_shim(
            dir.path(),
            &format!(
                "#!/bin/sh\n\
                 printf '%s\\n' \"$*\" >> '{}'\n\
                 if [ \"$2\" = \"add-json\" ]; then echo 'boom: not signed in' >&2; exit 3; fi\n\
                 exit 0\n",
                log.display()
            ),
        );
        let entry = json!({"type": "stdio", "command": "/bin/locus-mcp"});
        let err = claude_user_scope_register(&shim, &entry)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("claude mcp add-json"),
            "names the command: {err}"
        );
        assert!(
            err.contains("boom: not signed in"),
            "surfaces stderr: {err}"
        );
        // Honest: nothing was removed, and the error says so.
        assert!(err.contains("Nothing was changed"), "{err}");
        let calls = std::fs::read_to_string(&log).unwrap();
        assert!(
            !calls.contains("mcp remove"),
            "a failing add must never delete the previous registration: {calls}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn claude_user_scope_register_reports_honestly_when_readd_fails_after_removal() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("removed.marker");
        // First add: "already exists". Remove succeeds. Re-add: hard failure.
        let shim = write_claude_shim(
            dir.path(),
            &format!(
                "#!/bin/sh\n\
                 if [ \"$2\" = \"remove\" ]; then touch '{marker}'; exit 0; fi\n\
                 if [ \"$2\" = \"add-json\" ] && [ ! -f '{marker}' ]; then\n\
                   echo 'MCP server locus already exists in user config' >&2; exit 1\n\
                 fi\n\
                 if [ \"$2\" = \"add-json\" ]; then echo 'boom: broke mid-heal' >&2; exit 3; fi\n\
                 exit 0\n",
                marker = marker.display()
            ),
        );
        let entry = json!({"type": "stdio", "command": "/bin/locus-mcp"});
        let err = claude_user_scope_register(&shim, &entry)
            .unwrap_err()
            .to_string();
        assert!(err.contains("boom: broke mid-heal"), "{err}");
        // Honest about the resulting state: previous entry removed, not restored.
        assert!(err.contains("was removed"), "{err}");
        assert!(err.contains("NOT registered"), "{err}");
        assert!(
            !err.contains("Nothing was changed"),
            "must not claim nothing changed after the removal: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn claude_user_scope_verify_reflects_cli_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let ok = write_claude_shim(dir.path(), "#!/bin/sh\nexit 0\n");
        assert!(claude_user_scope_verify(&ok).unwrap());
        let dir2 = tempfile::tempdir().unwrap();
        let missing = write_claude_shim(dir2.path(), "#!/bin/sh\nexit 1\n");
        assert!(!claude_user_scope_verify(&missing).unwrap());
    }

    #[test]
    fn explicit_mcp_bin_must_exist() {
        let r = validate_explicit_mcp_bin("/definitely/not/a/real/locus-mcp");
        assert!(r.is_err(), "nonexistent explicit --mcp-bin must error");
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("locus-mcp");
        std::fs::write(&f, "").unwrap();
        assert!(validate_explicit_mcp_bin(&f.display().to_string()).is_ok());
    }

    #[test]
    fn resolve_mcp_bin_reports_explicit_as_non_fallback() {
        let (bin, fallback) = resolve_mcp_bin_with_fallback(Some("/x/locus-mcp".into()));
        assert_eq!(bin, "/x/locus-mcp");
        assert!(!fallback);
    }
}

#[cfg(test)]
mod capability_posture_tests {
    use super::{bootstrap_control_capability_plan, capability_posture};
    use locus_core::{Binding, BindingBody, Policy, ProviderBinding, Scope, Store};

    fn sample_binding(alias: &str) -> Binding {
        Binding::from_body(BindingBody {
            id: format!("bnd_{alias}"),
            alias: alias.into(),
            tenant: "t".into(),
            principal: None,
            description: None,
            policy: Policy::default(),
            providers: vec![ProviderBinding {
                provider: "github".into(),
                account: "a".into(),
                credential_ref: "phm:X".into(),
                scope: Scope::default(),
                upstream: None,
            }],
        })
    }

    #[test]
    fn no_persist_flag_mints_env_only_and_pin_still_works() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();

        let (value, note) = bootstrap_control_capability_plan(s.home(), false)
            .unwrap()
            .expect("fresh store must mint");
        // Never written to disk; the export line is the operator's copy.
        assert!(!locus_core::control_capability_file(s.home()).exists());
        assert_eq!(value.len(), 64);
        assert!(note.contains("NOT persisted"), "{note}");
        assert!(
            note.contains(&format!("export LOCUS_CONTROL_CAPABILITY=\"{value}\"")),
            "{note}"
        );

        // Control-plane operations keep working under the strict posture.
        s.save_binding(&sample_binding("personal")).unwrap();
        s.pin("personal", dir.path(), None, false).unwrap();
        assert!(s.active_session().unwrap().is_some());
        assert!(!locus_core::control_capability_file(s.home()).exists());
    }

    #[test]
    fn default_plan_persists_and_never_prints_the_value() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();

        let (value, note) = bootstrap_control_capability_plan(s.home(), true)
            .unwrap()
            .expect("fresh store must mint");
        assert!(locus_core::control_capability_file(s.home()).exists());
        assert!(
            !note.contains(&value),
            "persisted note must not carry the bearer: {note}"
        );
        assert!(note.contains("(0600)"), "{note}");

        // Second run adopts the persisted value (both postures).
        for persist in [true, false] {
            let (adopted, note) = bootstrap_control_capability_plan(s.home(), persist)
                .unwrap()
                .expect("persisted value adopted");
            assert_eq!(adopted, value);
            assert!(note.contains("adopted persisted"), "{note}");
            if !persist {
                assert!(note.contains("locus capability unpersist"), "{note}");
            }
        }
    }

    #[test]
    fn persist_unpersist_round_trip_via_core() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let value = locus_core::mint_ephemeral_control_capability();

        locus_core::persist_control_capability(home, &value).unwrap();
        assert_eq!(
            locus_core::read_persisted_control_capability(home).unwrap(),
            Some(value.clone())
        );
        assert!(locus_core::unpersist_control_capability(home).unwrap());
        assert_eq!(
            locus_core::read_persisted_control_capability(home).unwrap(),
            None
        );
        assert!(!locus_core::unpersist_control_capability(home).unwrap());
    }

    #[test]
    fn status_posture_labels() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();

        // Neither env nor file (status reads real process env for the control
        // var, which test runs leave unset).
        let status = locus_core::control_capability_status(home);
        let label = capability_posture(&status);
        assert!(label == "absent" || label == "env+persisted" || label == "env-only");
        if !status.env_present {
            assert_eq!(label, "absent");

            // Persisted only.
            let value = locus_core::mint_ephemeral_control_capability();
            locus_core::persist_control_capability(home, &value).unwrap();
            let status = locus_core::control_capability_status(home);
            assert_eq!(capability_posture(&status), "persisted");
            assert!(status.persisted_valid && status.persisted_permissions_ok);
        }
    }
}

#[cfg(test)]
mod switch_flow_tests {
    use super::switch_flow;
    use locus_core::{Binding, BindingBody, Policy, ProviderBinding, Scope, Store};
    use std::fs;
    use tempfile::tempdir;

    fn binding(alias: &str, tenant: &str) -> Binding {
        Binding::from_body(BindingBody {
            id: format!("bnd_{alias}"),
            alias: alias.into(),
            tenant: tenant.into(),
            principal: None,
            description: None,
            policy: Policy::default(),
            providers: vec![ProviderBinding {
                provider: "github".into(),
                account: "a".into(),
                credential_ref: "phm:X".into(),
                scope: Scope::default(),
                upstream: None,
            }],
        })
    }

    fn store_with(dir: &std::path::Path, aliases: &[(&str, &str)]) -> Store {
        let s = Store::open(dir).unwrap();
        for (a, t) in aliases {
            s.save_binding(&binding(a, t)).unwrap();
        }
        s
    }

    #[test]
    fn switch_leaves_current_pin_and_enters_target() {
        let dir = tempdir().unwrap();
        let s = store_with(dir.path(), &[("personal", "me"), ("acme", "acme-corp")]);
        s.pin("personal", dir.path(), None, false).unwrap();

        let out = switch_flow(&s, "acme", dir.path(), false, Some("cli".into()), None).unwrap();
        assert_eq!(
            out.left.as_ref().map(|l| l.binding_alias.as_str()),
            Some("personal")
        );
        assert_eq!(out.session.binding_alias, "acme");
        assert_eq!(out.session.tenant, "acme-corp");
        assert_eq!(out.providers_n, 1);
        assert_eq!(
            s.active_session().unwrap().unwrap().binding_alias,
            "acme",
            "active pin must be the switch target"
        );

        // Audits normally via the underlying ops: session.leave for the old
        // pin, then session.pin for the new one.
        let audit = fs::read_to_string(s.audit_path()).unwrap();
        let leave_at = audit.find("session.leave").expect("leave audited");
        let last_pin_at = audit.rfind("session.pin").expect("pin audited");
        assert!(
            leave_at < last_pin_at,
            "old pin's leave must be audited before the new pin"
        );
    }

    #[test]
    fn switch_when_unpinned_just_enters() {
        let dir = tempdir().unwrap();
        let s = store_with(dir.path(), &[("acme", "acme-corp")]);

        let out = switch_flow(&s, "acme", dir.path(), false, None, None).unwrap();
        assert!(out.left.is_none());
        assert_eq!(out.session.binding_alias, "acme");
        assert_eq!(s.active_session().unwrap().unwrap().binding_alias, "acme");
    }

    /// A target `enter` would refuse (unknown alias) must refuse BEFORE the
    /// current pin is dropped — same error surface as enter, incl. the
    /// did-you-mean suggestion.
    #[test]
    fn switch_unknown_target_keeps_current_pin() {
        let dir = tempdir().unwrap();
        let s = store_with(dir.path(), &[("personal", "me"), ("acme", "acme-corp")]);
        s.pin("personal", dir.path(), None, false).unwrap();

        let err = switch_flow(&s, "acmee", dir.path(), false, None, None).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("acmee"), "{msg}");
        assert!(msg.contains("did you mean `acme`?"), "{msg}");
        assert_eq!(
            s.active_session().unwrap().unwrap().binding_alias,
            "personal",
            "a refused switch must not drop the current pin"
        );
    }

    /// Workspace-allowlist refusal is the same fail-closed rule enter/pin
    /// enforce — and it must also fire before the current pin is dropped.
    /// `--force` overrides it exactly like enter.
    #[test]
    fn switch_disallowed_by_workspace_keeps_pin_and_force_overrides() {
        let dir = tempdir().unwrap();
        let s = store_with(dir.path(), &[("personal", "me"), ("acme", "acme-corp")]);
        let project = dir.path().join("proj");
        fs::create_dir_all(&project).unwrap();
        fs::write(
            project.join(".locus.toml"),
            "version = 1\nallowed_bindings = [\"personal\"]\n",
        )
        .unwrap();
        s.pin("personal", &project, None, false).unwrap();

        let err = switch_flow(&s, "acme", &project, false, None, None).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("not allowed in this workspace"), "{msg}");
        assert_eq!(
            s.active_session().unwrap().unwrap().binding_alias,
            "personal"
        );

        // --force takes the same escape hatch enter has.
        let out = switch_flow(&s, "acme", &project, true, None, None).unwrap();
        assert_eq!(out.session.binding_alias, "acme");
    }

    #[test]
    fn switch_cli_parses_alias_ttl_and_force() {
        use clap::Parser;
        let cli =
            super::Cli::try_parse_from(["locus", "switch", "acme", "--ttl", "45m", "--force"])
                .unwrap();
        match cli.command {
            super::Commands::Switch {
                alias, force, ttl, ..
            } => {
                assert_eq!(alias, "acme");
                assert!(force);
                assert_eq!(ttl.as_deref(), Some("45m"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn switch_honors_ttl_request() {
        let dir = tempdir().unwrap();
        let s = store_with(dir.path(), &[("personal", "me"), ("acme", "acme-corp")]);
        s.pin("personal", dir.path(), None, false).unwrap();

        let req = chrono::Duration::minutes(5);
        let out = switch_flow(&s, "acme", dir.path(), false, None, Some(req)).unwrap();
        let granted = out.session.expires_at - out.session.pinned_at;
        assert!(
            granted >= chrono::Duration::minutes(4) && granted <= chrono::Duration::minutes(6),
            "granted ttl should honor the request (got {granted})"
        );
        assert!(!out.ttl_capped);
    }
}

#[cfg(test)]
mod alias_ux_tests {
    use super::{edit_distance, nearest_alias, stdio_server_entry_toml, with_alias_suggestions};
    use locus_core::{Binding, BindingBody, LocusError, Policy, ProviderBinding, Scope, Store};

    fn binding(alias: &str) -> Binding {
        Binding::from_body(BindingBody {
            id: format!("bnd_{alias}"),
            alias: alias.into(),
            tenant: "t".into(),
            principal: None,
            description: None,
            policy: Policy::default(),
            providers: vec![ProviderBinding {
                provider: "github".into(),
                account: "a".into(),
                credential_ref: "phm:X".into(),
                scope: Scope::default(),
                upstream: None,
            }],
        })
    }

    #[test]
    fn edit_distance_and_nearest_alias() {
        assert_eq!(edit_distance("personal", "personal"), 0);
        assert_eq!(edit_distance("personol", "personal"), 1);
        assert_eq!(edit_distance("", "abc"), 3);

        let aliases = vec![
            "personal".to_string(),
            "ashlrai".to_string(),
            "cash-margin-partners".to_string(),
        ];
        assert_eq!(nearest_alias("personol", &aliases), Some("personal"));
        assert_eq!(nearest_alias("ashlari", &aliases), Some("ashlrai"));
        // Case-insensitive.
        assert_eq!(nearest_alias("Personal", &aliases), Some("personal"));
        // A completely different word gets no suggestion.
        assert_eq!(nearest_alias("zzzzzzzzzz", &aliases), None);
    }

    #[test]
    fn binding_not_found_lists_aliases_and_suggests() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        s.save_binding(&binding("personal")).unwrap();
        s.save_binding(&binding("ashlrai")).unwrap();

        let err = with_alias_suggestions(&s, LocusError::BindingNotFound("personol".into()));
        let msg = format!("{err:#}");
        assert!(msg.contains("binding not found: personol"), "{msg}");
        assert!(msg.contains("personal") && msg.contains("ashlrai"), "{msg}");
        assert!(msg.contains("did you mean `personal`?"), "{msg}");

        // Other error kinds pass through untouched.
        let err = with_alias_suggestions(&s, LocusError::NotPinned);
        assert!(format!("{err:#}").contains("no active pin"));
    }

    #[test]
    fn binding_not_found_empty_store_points_at_init() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        let err = with_alias_suggestions(&s, LocusError::BindingNotFound("acme".into()));
        let msg = format!("{err:#}");
        assert!(msg.contains("no bindings exist yet"), "{msg}");
        assert!(msg.contains("locus init --with-samples"), "{msg}");
    }

    #[test]
    fn stdio_toml_entry_carries_agent_env() {
        let toml = stdio_server_entry_toml("/usr/local/bin/locus-mcp", "grok");
        assert!(toml.contains("[mcp_servers.locus]"));
        assert!(toml.contains("command = \"/usr/local/bin/locus-mcp\""));
        assert!(toml.contains("[mcp_servers.locus.env]"));
        assert!(toml.contains("LOCUS_AUTO_PIN = \"cwd\""));
        assert!(toml.contains("LOCUS_CLIENT = \"grok\""));
        assert!(!toml.contains("LOCUS_NOTIFY"));
    }

    /// Regression: a binary path containing quotes/backslashes must produce
    /// valid TOML that round-trips the exact path (basic-string escaping).
    #[test]
    fn stdio_toml_entry_escapes_hostile_paths() {
        for bin in [
            "/we\"ird/pa\\th/locus-mcp",
            "C:\\Program Files\\locus\\locus-mcp.exe",
            "/tab\there/locus-mcp",
        ] {
            let toml = stdio_server_entry_toml(bin, "grok");
            let doc: toml_edit::DocumentMut = toml
                .parse()
                .unwrap_or_else(|e| panic!("emitted TOML must parse ({bin}): {e}\n{toml}"));
            let command = doc["mcp_servers"]["locus"]["command"]
                .as_str()
                .expect("command is a string");
            assert_eq!(command, bin, "escaped value must round-trip:\n{toml}");
        }
    }
}

#[cfg(test)]
mod ttl_and_client_add_tests {
    use super::{
        binding_from_answers, human_dur, missing_add_flags, parse_pin_ttl, resolve_add_answers,
        scope_prompts, split_repos, AddAnswers, BindingAddArgs,
    };
    use locus_core::Store;

    fn full_args() -> BindingAddArgs {
        BindingAddArgs {
            alias: Some("cmp".into()),
            tenant: Some("cash-margin-partners".into()),
            provider: Some("supabase".into()),
            account: Some("cmp-prod".into()),
            credential_ref: Some("phm:CMP_SUPABASE".into()),
            project_ref: Some("abcdefghij".into()),
            ..BindingAddArgs::default()
        }
    }

    #[test]
    fn parse_pin_ttl_bounds() {
        assert_eq!(parse_pin_ttl("90m").unwrap(), chrono::Duration::minutes(90));
        assert_eq!(parse_pin_ttl("2h").unwrap(), chrono::Duration::hours(2));
        assert_eq!(parse_pin_ttl("1d").unwrap(), chrono::Duration::hours(24));
        assert_eq!(parse_pin_ttl("1m").unwrap(), chrono::Duration::minutes(1));

        for (bad, frag) in [
            ("0m", "too short"),
            ("-5m", "too short"),
            ("30s", "too short"),
            ("25h", "too long"),
            ("abc", "invalid --ttl"),
            ("", "invalid --ttl"),
        ] {
            let err = parse_pin_ttl(bad).unwrap_err().to_string();
            assert!(err.contains(frag), "{bad}: {err}");
        }
    }

    #[test]
    fn human_dur_table() {
        use chrono::Duration as D;
        assert_eq!(human_dur(D::seconds(90)), "90s");
        assert_eq!(human_dur(D::minutes(45)), "45m");
        assert_eq!(human_dur(D::minutes(90)), "1h30m");
        assert_eq!(human_dur(D::hours(2)), "2h");
        assert_eq!(human_dur(D::seconds(-5)), "0s");
    }

    #[test]
    fn missing_flags_enumerated() {
        let args = BindingAddArgs::default();
        let err = resolve_add_answers(&args).unwrap_err().to_string();
        assert!(err.contains("<alias>"), "{err}");
        for f in ["--tenant", "--provider", "--account", "--credential-ref"] {
            assert!(err.contains(f), "{err}");
        }
        assert!(err.contains("non-interactive"), "{err}");
        assert!(missing_add_flags(&full_args()).is_empty());
    }

    #[test]
    fn raw_secrets_never_accepted_as_credential_ref() {
        for raw in ["sk-live-abc123", "some raw secret", "test:x"] {
            let mut args = full_args();
            args.credential_ref = Some(raw.into());
            let err = resolve_add_answers(&args).unwrap_err().to_string();
            assert!(err.contains("invalid credential_ref"), "{raw}: {err}");
        }
        // Conservative bare Phantom name → did-you-mean, still rejected.
        let mut args = full_args();
        args.credential_ref = Some("MYSECRET".into());
        let err = resolve_add_answers(&args).unwrap_err().to_string();
        assert!(err.contains("did you mean 'phm:MYSECRET'"), "{err}");
        // Pointers accepted.
        for good in ["phm:CMP_SUPABASE", "env:CMP_TOKEN"] {
            let mut args = full_args();
            args.credential_ref = Some(good.into());
            assert!(resolve_add_answers(&args).is_ok(), "{good}");
        }
    }

    #[test]
    fn binding_from_answers_maps_provider_scopes() {
        let base = AddAnswers {
            alias: "cmp".into(),
            tenant: "cmp".into(),
            account: "acct".into(),
            credential_ref: "phm:X".into(),
            ..AddAnswers::default()
        };

        let b = binding_from_answers(&AddAnswers {
            provider: "supabase".into(),
            project_ref: Some("ref123".into()),
            read_only: true,
            ..base.clone()
        });
        let p = &b.providers[0];
        assert_eq!(p.scope.project_ref.as_deref(), Some("ref123"));
        assert_eq!(p.scope.read_only, Some(true));

        let b = binding_from_answers(&AddAnswers {
            provider: "github".into(),
            org: Some("cmp-org".into()),
            repos: split_repos(Some("a, b,,c")),
            ..base.clone()
        });
        let p = &b.providers[0];
        assert_eq!(p.scope.orgs, vec!["cmp-org"]);
        assert_eq!(p.scope.repos, vec!["a", "b", "c"]);
        assert_eq!(p.scope.read_only, None, "read_only only when set");

        for prov in ["aws", "stripe", "cloudflare"] {
            let b = binding_from_answers(&AddAnswers {
                provider: prov.into(),
                account_id: Some("acct-1".into()),
                ..base.clone()
            });
            assert_eq!(b.providers[0].scope.account_id.as_deref(), Some("acct-1"));
        }

        let b = binding_from_answers(&AddAnswers {
            provider: "vercel".into(),
            team_id: Some("team_1".into()),
            ..base.clone()
        });
        assert_eq!(b.providers[0].scope.team_id.as_deref(), Some("team_1"));

        let b = binding_from_answers(&AddAnswers {
            provider: "resend".into(),
            default_ttl: Some("2h".into()),
            ..base
        });
        assert_eq!(b.policy.default_ttl.as_deref(), Some("2h"));
        assert_eq!(b.id, "bnd_cmp");
    }

    #[test]
    fn scope_prompt_table_covers_known_providers() {
        assert_eq!(scope_prompts("supabase"), &[("project_ref", true)]);
        assert_eq!(scope_prompts("github"), &[("org", true), ("repos", false)]);
        assert_eq!(
            scope_prompts("vercel"),
            &[("team_id", true), ("project_ref", false)]
        );
        assert_eq!(scope_prompts("aws"), &[("account_id", true)]);
        assert!(scope_prompts("resend").is_empty());
        assert!(scope_prompts("custom-x").is_empty());
    }

    #[test]
    fn non_interactive_add_writes_binding_toml() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        let mut args = full_args();
        args.default_ttl = Some("2h".into());
        let answers = resolve_add_answers(&args).unwrap();
        let b = binding_from_answers(&answers);
        let path = s.save_binding(&b).unwrap();
        let toml = std::fs::read_to_string(&path).unwrap();
        assert!(toml.contains("alias = \"cmp\""), "{toml}");
        assert!(toml.contains("tenant = \"cash-margin-partners\""), "{toml}");
        assert!(
            toml.contains("credential_ref = \"phm:CMP_SUPABASE\""),
            "{toml}"
        );
        assert!(toml.contains("default_ttl = \"2h\""), "{toml}");
        assert!(toml.contains("project_ref = \"abcdefghij\""), "{toml}");

        // Reserved ^locus alias fails closed at save (store-side check).
        let mut answers2 = answers.clone();
        answers2.alias = "locus-cmp".into();
        let err = s
            .save_binding(&binding_from_answers(&answers2))
            .unwrap_err()
            .to_string();
        assert!(err.contains("reserved"), "{err}");

        // Bad default_ttl fails closed at resolve, before any write.
        let mut args3 = full_args();
        args3.default_ttl = Some("nonsense".into());
        assert!(resolve_add_answers(&args3).is_err());
    }

    #[test]
    fn cli_surface_parses_old_and_new_invocations() {
        use clap::Parser;
        // Old flags-first binding add form (required→optional is accept-superset).
        let cli = super::Cli::try_parse_from([
            "locus",
            "binding",
            "add",
            "cmp2",
            "--tenant",
            "t",
            "--provider",
            "github",
            "--account",
            "a",
            "--credential-ref",
            "phm:X",
            "--org",
            "o",
            "--read-only",
        ])
        .unwrap();
        match cli.command {
            super::Commands::Binding(super::BindingCmd::Add(args)) => {
                assert_eq!(args.alias.as_deref(), Some("cmp2"));
                assert_eq!(args.tenant.as_deref(), Some("t"));
                assert!(args.read_only);
                assert!(!args.guided && !args.non_interactive && !args.dry_run);
            }
            other => panic!("unexpected: {other:?}"),
        }
        // New guided front door.
        let cli = super::Cli::try_parse_from(["locus", "client", "add", "cmp3"]).unwrap();
        match cli.command {
            super::Commands::Client(super::ClientCmd::Add(args)) => {
                assert_eq!(args.alias.as_deref(), Some("cmp3"));
            }
            other => panic!("unexpected: {other:?}"),
        }
        // enter/pin --ttl.
        let cli = super::Cli::try_parse_from(["locus", "enter", "cmp", "--ttl", "2h"]).unwrap();
        match cli.command {
            super::Commands::Enter { ttl, .. } => assert_eq!(ttl.as_deref(), Some("2h")),
            other => panic!("unexpected: {other:?}"),
        }
        let cli = super::Cli::try_parse_from(["locus", "pin", "cmp", "--ttl", "45m"]).unwrap();
        match cli.command {
            super::Commands::Pin { ttl, .. } => assert_eq!(ttl.as_deref(), Some("45m")),
            other => panic!("unexpected: {other:?}"),
        }
        // agent setup --claude-scope: defaults to project; user accepted;
        // anything else rejected at parse time.
        let cli = super::Cli::try_parse_from(["locus", "agent", "setup", "--dry-run"]).unwrap();
        match cli.command {
            super::Commands::Agent(super::AgentCmd::Setup { claude_scope, .. }) => {
                assert_eq!(claude_scope, "project")
            }
            other => panic!("unexpected: {other:?}"),
        }
        let cli = super::Cli::try_parse_from([
            "locus",
            "agent",
            "setup",
            "--dry-run",
            "--client",
            "claude",
            "--claude-scope",
            "user",
        ])
        .unwrap();
        match cli.command {
            super::Commands::Agent(super::AgentCmd::Setup { claude_scope, .. }) => {
                assert_eq!(claude_scope, "user")
            }
            other => panic!("unexpected: {other:?}"),
        }
        assert!(super::Cli::try_parse_from([
            "locus",
            "agent",
            "setup",
            "--dry-run",
            "--claude-scope",
            "global",
        ])
        .is_err());
    }
}

#[cfg(test)]
mod touchid_tests {
    use super::{
        confirm_grant_touchid, credential_resolving_upstreams, preflight_child_launch,
        ChildLaunchSurface,
    };
    use locus_core::Binding;
    use std::sync::Mutex;

    /// Serialize env-var mutations across tests in this binary.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn touchid_mock_ok_allows_grant_confirm() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("LOCUS_TOUCHID_MOCK", "ok");
        let r = confirm_grant_touchid("alice", "appr_aabbccddeeff001122334455", "t", "b");
        std::env::remove_var("LOCUS_TOUCHID_MOCK");
        assert!(r.is_ok(), "mock ok should allow: {r:?}");
    }

    #[test]
    fn touchid_mock_cancel_fails_closed() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("LOCUS_TOUCHID_MOCK", "cancel");
        let r = confirm_grant_touchid("alice", "appr_aabbccddeeff001122334455", "t", "b");
        std::env::remove_var("LOCUS_TOUCHID_MOCK");
        assert!(r.is_err(), "mock cancel must fail closed");
        let msg = r.unwrap_err().to_string();
        assert!(
            msg.contains("cancelled") || msg.contains("fail closed"),
            "unexpected err: {msg}"
        );
        assert!(msg.contains("aborted"));
    }

    #[test]
    fn touchid_mock_deny_fails_closed() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("LOCUS_TOUCHID_MOCK", "deny");
        let r = confirm_grant_touchid("bob", "appr_aabbccddeeff001122334455", "t", "b");
        std::env::remove_var("LOCUS_TOUCHID_MOCK");
        assert!(r.is_err());
    }

    #[test]
    fn no_resolve_guard_covers_every_child_surface_and_expanded_worker_kind() {
        let explicit = Binding::parse_toml(
            r#"
id = "bnd_explicit"
alias = "explicit"
tenant = "tenant"

[[providers]]
provider = "github"
account = "acme"
credential_ref = "env:GH_TOKEN"
upstream = { command = "provider-worker", resolve_secrets = true }
"#,
        )
        .unwrap();
        assert_eq!(
            credential_resolving_upstreams(&explicit).unwrap(),
            vec!["github"]
        );

        let recipe = Binding::parse_toml(
            r#"
id = "bnd_recipe"
alias = "recipe"
tenant = "tenant"

[[providers]]
provider = "github"
account = "acme"
credential_ref = "env:GH_TOKEN"
upstream = { recipe = "github-official", sandbox = false }
"#,
        )
        .unwrap();
        assert_eq!(
            credential_resolving_upstreams(&recipe).unwrap(),
            vec!["github"]
        );

        let credential_free = Binding::parse_toml(
            r#"
id = "bnd_free"
alias = "free"
tenant = "tenant"

[[providers]]
provider = "filesystem"
account = "local"
credential_ref = "env:UNUSED"
upstream = { command = "credential-free-worker", resolve_secrets = false }
"#,
        )
        .unwrap();
        assert!(credential_resolving_upstreams(&credential_free)
            .unwrap()
            .is_empty());

        for surface in [
            ChildLaunchSurface::Exec,
            ChildLaunchSurface::Run,
            ChildLaunchSurface::CiRun,
        ] {
            for binding in [&explicit, &recipe] {
                let error = preflight_child_launch(binding, false, surface).unwrap_err();
                let message = error.to_string();
                assert!(message.contains(surface.command_name()), "{message}");
                assert!(message.contains("no session or credential effect occurred"));
            }
            preflight_child_launch(&credential_free, false, surface).unwrap();
            preflight_child_launch(&explicit, true, surface).unwrap();
        }
    }
}

#[cfg(test)]
mod verify_session_tests {
    use super::{gather_doctor_external_with_phantom_status, verify_session_exit};
    use locus_core::{verify_session, Binding, Store};
    use tempfile::tempdir;

    #[test]
    fn verify_session_uses_unresolved_credential_evidence() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path().join("locus-home")).unwrap();
        let binding = Binding::parse_toml(
            r#"
[binding]
id = "bnd_verify"
alias = "verify"
tenant = "tenant"

[[binding.providers]]
provider = "github"
account = "tenant"
credential_ref = "phm:VERIFY_SESSION_MISSING"
"#,
        )
        .unwrap();
        store.save_binding(&binding).unwrap();
        store
            .pin("verify", dir.path(), Some("test".into()), false)
            .unwrap();

        let external =
            gather_doctor_external_with_phantom_status(&store, dir.path().to_path_buf(), false)
                .unwrap();
        assert_eq!(external.unresolved_phm.len(), 1);
        let pack = verify_session(&store, dir.path(), external).unwrap();
        assert!(!pack.session_ok);
        assert!(pack
            .doctor
            .findings
            .iter()
            .any(|finding| finding.code == "unresolved_phm"));
    }

    #[test]
    fn verify_session_exit_follows_session_ok() {
        assert!(verify_session_exit(true).is_ok());
        assert!(verify_session_exit(false).is_err());
    }
}

#[cfg(test)]
mod watch_heartbeat_tests {
    use super::{
        attach_control_capability_findings, gather_doctor_external_with_phantom_status,
        parse_watch_interval, watch_should_fail, WatchHeartbeat,
    };
    use locus_core::{verify_session, Binding, Store};
    use std::time::Duration;
    use tempfile::tempdir;

    #[test]
    fn parse_watch_interval_units() {
        assert_eq!(parse_watch_interval("5s").unwrap(), Duration::from_secs(5));
        assert_eq!(
            parse_watch_interval("30s").unwrap(),
            Duration::from_secs(30)
        );
        assert_eq!(parse_watch_interval("1m").unwrap(), Duration::from_secs(60));
        assert_eq!(
            parse_watch_interval("2h").unwrap(),
            Duration::from_secs(7200)
        );
        assert_eq!(parse_watch_interval("10").unwrap(), Duration::from_secs(10));
        assert_eq!(parse_watch_interval("").unwrap(), Duration::from_secs(5));
        assert!(parse_watch_interval("nope").is_err());
        assert!(parse_watch_interval("5x").is_err());
    }

    #[test]
    fn watch_should_fail_policy() {
        // Healthy tick never fails.
        assert!(!watch_should_fail(true, false, false));
        assert!(!watch_should_fail(true, true, true));

        // Unpinned / not ok without require_ok: soft (exit 0 on --once).
        assert!(!watch_should_fail(false, false, false));

        // Pin present and not ok: fail on --once.
        assert!(watch_should_fail(false, true, false));

        // --require-ok fail-closed regardless of pin.
        assert!(watch_should_fail(false, false, true));
        assert!(watch_should_fail(false, true, true));
    }

    #[test]
    fn watch_heartbeat_from_unpinned_pack() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path().join("locus-home")).unwrap();
        store
            .save_binding(
                &Binding::parse_toml(
                    r#"
[binding]
id = "bnd_watch"
alias = "watchme"
tenant = "tenant"

[[binding.providers]]
provider = "github"
account = "tenant"
credential_ref = "env:GH_TOKEN"
"#,
                )
                .unwrap(),
            )
            .unwrap();

        let external =
            gather_doctor_external_with_phantom_status(&store, dir.path().to_path_buf(), true)
                .unwrap();
        let pack = verify_session(&store, dir.path(), external).unwrap();
        let hb = WatchHeartbeat::from_pack(&pack);

        assert_eq!(hb.kind, "watch");
        assert!(!hb.session_ok, "unpinned must not be session_ok");
        assert!(hb.whoami.is_none());
        assert!(!hb.pinned);
        assert!(!hb.frozen);
        assert!(!hb.doctor_verdict.is_empty());
        assert_eq!(hb.safe_next, "enter");

        // NDJSON shape: required hub fields present, no secret-looking values.
        let line = serde_json::to_string(&hb).unwrap();
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["kind"], "watch");
        assert_eq!(v["session_ok"], false);
        assert!(v.get("doctor_verdict").is_some());
        assert_eq!(v["safe_next"], "enter");
        assert!(v.get("whoami").is_none() || v["whoami"].is_null());
        let blob = line.to_lowercase();
        for bad in ["sk-", "ghp_", "gho_", "github_pat_", "secret_value"] {
            assert!(
                !blob.contains(bad),
                "heartbeat must not leak secrets: {bad}"
            );
        }

        // Soft --once without pin does not fail; require_ok does.
        assert!(!watch_should_fail(
            hb.session_ok,
            hb.pinned || hb.frozen,
            false
        ));
        assert!(watch_should_fail(
            hb.session_ok,
            hb.pinned || hb.frozen,
            true
        ));
    }

    #[test]
    fn watch_pack_attaches_control_capability_findings() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path().join("locus-home")).unwrap();
        let external =
            gather_doctor_external_with_phantom_status(&store, dir.path().to_path_buf(), true)
                .unwrap();
        let mut pack = verify_session(&store, dir.path(), external).unwrap();

        // Deterministic status (no ambient env reads): capability entirely
        // missing from the operator shell.
        let status = locus_core::ControlCapabilityStatus {
            env_present: false,
            env_valid: false,
            persisted: false,
            persisted_valid: false,
            persisted_permissions_ok: true,
            matches_persisted: None,
            test_fallback: false,
        };
        attach_control_capability_findings(store.home(), &status, &mut pack);

        // Same finding the doctor pack carries — watch must not hide it.
        assert!(
            pack.doctor
                .findings
                .iter()
                .any(|f| f.code == "control_capability_missing"),
            "missing capability must surface in the watch pack"
        );
        // Warn escalates the verdict, and session_ok is re-derived from it.
        assert!(!pack.doctor.ok);
        assert!(!pack.session_ok);
        let hb = WatchHeartbeat::from_pack(&pack);
        assert!(!hb.session_ok);

        // Satisfied capability attaches nothing and keeps the pack unchanged.
        let external =
            gather_doctor_external_with_phantom_status(&store, dir.path().to_path_buf(), true)
                .unwrap();
        let mut ok_pack = verify_session(&store, dir.path(), external).unwrap();
        let before = ok_pack.doctor.findings.len();
        let satisfied = locus_core::ControlCapabilityStatus {
            env_present: true,
            env_valid: true,
            persisted: false,
            persisted_valid: false,
            persisted_permissions_ok: true,
            matches_persisted: None,
            test_fallback: false,
        };
        let session_ok_before = ok_pack.session_ok;
        attach_control_capability_findings(store.home(), &satisfied, &mut ok_pack);
        assert_eq!(ok_pack.doctor.findings.len(), before);
        assert_eq!(ok_pack.session_ok, session_ok_before);
    }

    #[test]
    fn watch_heartbeat_from_pinned_pack() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path().join("locus-home")).unwrap();
        store
            .save_binding(
                &Binding::parse_toml(
                    r#"
[binding]
id = "bnd_watch_pin"
alias = "watchpin"
tenant = "tenant-a"

[[binding.providers]]
provider = "github"
account = "tenant-a"
credential_ref = "env:GH_TOKEN"
"#,
                )
                .unwrap(),
            )
            .unwrap();
        store
            .pin("watchpin", dir.path(), Some("test".into()), false)
            .unwrap();

        let external =
            gather_doctor_external_with_phantom_status(&store, dir.path().to_path_buf(), true)
                .unwrap();
        let pack = verify_session(&store, dir.path(), external).unwrap();
        let hb = WatchHeartbeat::from_pack(&pack);

        assert_eq!(hb.kind, "watch");
        assert_eq!(hb.whoami.as_deref(), Some("watchpin"));
        assert!(hb.pinned);
        // session_ok depends on doctor (phantom/mcp may WARN); pin must be reported either way.
        let line = serde_json::to_string(&hb).unwrap();
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["kind"], "watch");
        assert_eq!(v["whoami"], "watchpin");
        assert!(v["session_ok"].is_boolean());
        assert!(v["doctor_verdict"].is_string());
        assert!(v["safe_next"].is_string());

        if !hb.session_ok {
            // Pinned but not ok → --once fails without needing --require-ok.
            assert!(watch_should_fail(
                hb.session_ok,
                hb.pinned || hb.frozen,
                false
            ));
        } else {
            assert!(!watch_should_fail(
                hb.session_ok,
                hb.pinned || hb.frozen,
                false
            ));
        }
    }
}

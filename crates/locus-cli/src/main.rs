//! Locus CLI — identity plane for coding agents.
//!
//! ```text
//! locus pin acme
//! locus whoami
//! locus exec -- gh pr list
//! locus leave
//! ```

mod serve;

use anyhow::{bail, Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};
use colored::Colorize;
use locus_core::{
    agent_md_content, agent_md_path, agent_report_from_doctor, all_recipes, build_ci_env_map,
    build_doctor_report, build_isolated_env_opts, ci_secrets_allowed, default_export_filename,
    export_events, export_forensics_pack, filter_audit_events, find_workspace, mcp_agent_env,
    parse_ttl, phantom_on_path, probe_agent_options, recipe_toml_snippet, resolve_passphrase,
    suggest_for_provider, verify_claim, workspace_stub_toml, AgentStatus, Binding, BindingBody,
    DoctorExternal, DoctorVerdict, EventsExportFormat, EventsExportOptions, ForensicsExportOptions,
    Policy, ProviderBinding, Scope, Store, WorkspaceConfig, VERSION,
};
use serde_json::json;
use std::env;
use std::io;
use std::path::PathBuf;
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
  Setup         init · quickstart · setup · agent · doctor · watch · workspace · hook · mcp · engagement · graph · goal · verify · upstream\n  \
  Daily use     enter · pin · leave · whoami · status · exec · run · binding\n  \
  CI            ci mint · ci env · ci run\n  \
  Approvals     approve · notify\n  \
  Audit         events · forensics\n  \
  Local UI      serve · dashboard\n  \
  Maintenance   completion · topic · version\n\n\
Topic help:  locus topic <name>  or  locus help topic <name>\n  \
  Topics: dashboard · forensics · serve · goal · verify · agent · mcp · http · upstream"
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
    },

    /// First 60 seconds: init samples if needed, enter workspace default, whoami + doctor
    #[command(next_help_heading = "Setup")]
    Quickstart,

    /// Register locus-mcp with an AI client config
    #[command(next_help_heading = "Setup")]
    Setup {
        /// Client: claude | cursor | codex
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

    /// Continuously check active pin for binding drift (freezes on change)
    #[command(next_help_heading = "Setup")]
    Watch {
        /// Poll interval (e.g. 5s, 30s, 1m). Default: 5s
        #[arg(long, default_value = "5s")]
        interval: String,
        /// Exit after one check
        #[arg(long)]
        once: bool,
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

    /// Run the locus-mcp stdio server (same as the locus-mcp binary)
    #[command(next_help_heading = "Setup")]
    Mcp,

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
        /// Print `export LOCUS_*=…` lines for eval
        #[arg(long)]
        exports: bool,
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
    },

    /// Leave the active pin (clear identity) and suggest re-enter
    #[command(next_help_heading = "Daily use")]
    Leave,

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

    /// Verification plane — score claims before acting (M5 heuristic stubs)
    ///
    /// `locus verify claim --text "…"` returns
    /// `{ claim, confidence, needs_tool, suggestion, signals, grounding? }`.
    /// No ML — pure heuristics for hub/agent extension. See docs/verification-plane.md.
    #[command(next_help_heading = "Setup", subcommand)]
    Verify(VerifyCmd),

    /// Run a command with only the pinned binding's identity surface
    #[command(next_help_heading = "Daily use")]
    Exec {
        /// Do not resolve phm:/env: credential refs into secret env vars
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
        /// Do not resolve phm:/env: credential refs into secret env vars
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

    /// Built-in upstream MCP recipes (command/args for common servers)
    ///
    /// Bindings may set `upstream = { recipe = "github-mcp" }` instead of
    /// hand-writing command/args. See also `docs/workers.md`.
    #[command(next_help_heading = "Setup", subcommand)]
    Upstream(UpstreamCmd),

    /// CI / ephemeral pin minting (short-lived sealed sessions)
    ///
    /// Mints sealed sessions under `sessions/ci-*.json` without touching
    /// `active.json`. Children should set `LOCUS_SESSION_ID` (exported by
    /// `ci mint` / `ci env` / `ci run`) so `require_active` and locus-mcp
    /// resolve the ephemeral pin.
    #[command(next_help_heading = "CI", subcommand)]
    Ci(CiCmd),

    // ────────────────────────── Approvals ────────────────────────────
    /// Manage require_approval / dual-control grants for blocked tool calls
    #[command(next_help_heading = "Approvals", subcommand)]
    Approve(ApproveCmd),

    /// Desktop approval banners (OFF by default — opt in explicitly)
    #[command(next_help_heading = "Approvals", subcommand)]
    Notify(NotifyCmd),

    // ──────────────────────────── Audit ──────────────────────────────
    /// Read recent local audit events (`$LOCUS_HOME/audit/events.jsonl`)
    ///
    /// Subcommand: `locus events export [--otlp] [--out file]` for fleet pulse /
    /// OTLP-compatible log export. See docs/observability.md.
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
    /// Heuristic stub (no ML): numbers, URLs, or versions ⇒ needs_tool + low
    /// confidence. Identity language + active pin attaches whoami grounding.
    Claim {
        /// Claim text to score
        #[arg(long)]
        text: String,
    },
}

#[derive(Subcommand, Debug)]
enum AgentCmd {
    /// Wire Locus into AI clients (MCP + agent guidance)
    ///
    /// Requires `--apply` or `--dry-run`. Registers locus-mcp with
    /// `LOCUS_AUTO_PIN=cwd` + `LOCUS_CLIENT=<client>` (never `LOCUS_NOTIFY=1`).
    Setup {
        /// Client: claude | cursor | codex | all
        #[arg(long, default_value = "all")]
        client: String,
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
        /// Do not resolve phm:/env: credential refs into secret env vars
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
        /// Write to file (default: stdout)
        #[arg(long, short = 'o')]
        out: Option<PathBuf>,
        /// OTLP service.name attribute (default: locus)
        #[arg(long, default_value = "locus")]
        service_name: String,
    },
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
    List {
        /// Max rows (default: 50)
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Grant a pending approval as a principal (default TTL 15m)
    Grant {
        /// Approval id (`appr_…`)
        id: String,
        /// Principal granting (default: LOCUS_PRINCIPAL or $USER)
        #[arg(long = "as")]
        as_principal: Option<String>,
        /// Grant lifetime once fully approved (e.g. 15m, 1h). Default: 15m
        #[arg(long)]
        ttl: Option<String>,
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
enum BindingCmd {
    /// List configured bindings
    List,
    /// Show one binding
    Show { alias: String },
    /// Create a binding (minimal interactive flags)
    Add {
        alias: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        provider: String,
        #[arg(long)]
        account: String,
        #[arg(long)]
        credential_ref: String,
        #[arg(long)]
        project_ref: Option<String>,
        #[arg(long)]
        team_id: Option<String>,
        #[arg(long)]
        org: Option<String>,
        #[arg(long)]
        read_only: bool,
        #[arg(long)]
        description: Option<String>,
    },
    /// Remove a binding file
    Rm {
        alias: String,
        #[arg(long)]
        yes: bool,
    },
}

fn main() {
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
        Commands::Init { with_samples } => cmd_init(with_samples, cli.json),
        Commands::Quickstart => cmd_quickstart(cli.json),
        Commands::Enter {
            alias,
            force,
            client,
            exports,
        } => cmd_enter(alias, force, client, exports, cli.json),
        Commands::Pin {
            alias,
            force,
            client,
            ns,
        } => cmd_pin(alias, force, client, ns, cli.json),
        Commands::Leave => cmd_leave(cli.json),
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
        Commands::Mcp => cmd_mcp(),
        Commands::Binding(sub) => cmd_binding(sub, cli.json),
        Commands::Upstream(sub) => cmd_upstream(sub, cli.json),
        Commands::Workspace {
            default,
            allow,
            require_pin,
            force,
        } => cmd_workspace(default, allow, require_pin, force),
        Commands::Doctor => cmd_doctor(cli.json),
        Commands::Watch { interval, once } => cmd_watch(&interval, once, cli.json),
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
            }) => cmd_events_export(last, op, binding, otlp, out, service_name, cli.json),
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
               locus events export [--otlp] [--out file]  # fleet pulse / OTLP logs",
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
               MCP: locus_verify_claim  { \"text\": \"…\" }\n\n\
             Identity gate checks:\n\
               locus whoami [--json]           # active pin + seal\n\
               locus doctor [--json]           # SAFE|WARN|UNSAFE (exit 0/1/2)\n\
               locus agent report --json       # hub contract (ready|protected|unsafe)\n\
               locus status --oneline          # unpinned | alias:tenant | frozen\n\n\
             Doctor may WARN (ungrounded_claims) when recent audit details look\n\
             like low-confidence factual claims (numbers/URLs/versions).\n\n\
             Docs: docs/verification-plane.md · schema/doctor.schema.json\n\
             Isolation smoke: export LOCUS_HOME=/tmp/locus-verify && locus init --with-samples",
        ),
        (
            "agent",
            "AI-native setup + hub readiness.\n\n\
             Commands:\n\
               locus agent setup --apply|--dry-run [--client all|claude|cursor|codex]\n\
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
               locus setup --client claude|cursor|codex\n\
               locus agent setup --apply\n\n\
             HTTP (CI / remote agents, loopback by default):\n\
               LOCUS_MCP_HTTP_TOKEN=… locus-mcp --http 127.0.0.1:8742\n\
               LOCUS_MCP_HTTP=1 LOCUS_MCP_HTTP_TOKEN=… locus-mcp\n\
               POST /mcp  (JSON-RPC) · GET /health\n\n\
             Upstream recipes (per-provider MCP children):\n\
               locus upstream list\n\
               locus upstream suggest github\n\
               upstream = { recipe = \"github-mcp\", resolve_secrets = true }\n\n\
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
               upstream = { recipe = \"github-mcp\", resolve_secrets = true }\n\
               upstream = { recipe = \"filesystem-mcp\", args = [\"-y\", \"@modelcontextprotocol/server-filesystem\", \"/tmp/demo\"] }\n\
               upstream = { command = \"npx\", args = [\"-y\", \"@pkg\"] }  # explicit still works\n\n\
             Recipes: github-mcp · github-official · supabase-mcp · filesystem-mcp · everything-mcp\n\
             Source: adapters/recipes.toml · Docs: docs/workers.md · examples/upstream.binding.toml",
        ),
        (
            "http",
            "HTTP transports for Locus surfaces.\n\n\
             Dashboard API:\n\
               locus serve --port 8750\n\
               curl -s http://127.0.0.1:8750/api/health\n\n\
             MCP over HTTP (JSON-RPC POST /mcp):\n\
               LOCUS_MCP_HTTP_TOKEN=secret locus-mcp --http 127.0.0.1:8742\n\
               curl -s -H \"Authorization: Bearer secret\" \\\n\
                 -H 'Content-Type: application/json' \\\n\
                 -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{…}}' \\\n\
                 http://127.0.0.1:8742/mcp\n\n\
             Env: LOCUS_MCP_HTTP · LOCUS_MCP_HTTP_ADDR · LOCUS_MCP_HTTP_TOKEN\n\
                    LOCUS_MCP_HTTP_ALLOW_REMOTE=1 (non-loopback; default refuses)\n\
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

fn cwd() -> PathBuf {
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
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

fn cmd_init(with_samples: bool, json: bool) -> Result<()> {
    let s = store()?;
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
            })
        );
    } else {
        println!("{} locus home {}", "ok".green().bold(), s.home().display());
        println!("   seal key {}", s.seal_key_path().display());
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
            "locus setup --client claude".cyan(),
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
# Switch: locus enter acme   |   leave: locus leave
# Dual-control / firm mode: docs/firm-mode.md

[binding]
id = "bnd_acme"
alias = "acme"
tenant = "acme-corp"
description = "Acme client engagement — sample"

[binding.policy]
default = "allow"
max_ttl = "8h"
# Human must grant before these globs run (locus approve grant …).
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
fn cmd_quickstart(json: bool) -> Result<()> {
    let s = store()?;
    let config_written = ensure_default_config(&s)?;

    let mut actions: Vec<String> = Vec::new();
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
        let ws = find_workspace(&cwd());
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

    // Doctor (do not hard-exit here — quickstart should finish printing)
    let phantom = phantom_on_path();
    let unresolved_phm = collect_unresolved_phm_refs(&s, phantom).unwrap_or_default();
    let report = build_doctor_report(
        &s,
        DoctorExternal {
            phantom_on_path: phantom,
            unresolved_phm,
            cwd: Some(cwd()),
        },
    )?;

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
        "locus setup --client claude".dimmed(),
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
    json: bool,
) -> Result<()> {
    let s = store()?;
    let client = client.or_else(|| Some("cli".into()));
    let session = match alias {
        Some(a) => s.pin(&a, &cwd(), client, force)?,
        None => s.pin_auto(&cwd(), client, force)?,
    };
    let binding = s.load_binding(&session.binding_alias)?;
    let providers_n = binding.providers.len();

    if json {
        println!(
            "{}",
            serde_json::json!({
                "entered": true,
                "binding": session.binding_alias,
                "tenant": session.tenant,
                "session_id": session.session_id,
                "expires_at": session.expires_at.to_rfc3339(),
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
    println!("   expires  {}", session.expires_at.to_rfc3339().dimmed());
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

fn cmd_pin(
    alias: Option<String>,
    force: bool,
    client: Option<String>,
    ns: Option<String>,
    json: bool,
) -> Result<()> {
    let s = store()?;
    let client = client.or_else(|| Some("cli".into()));
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
        s.pin_namespaced(&aliases, &cwd(), client, force)?
    } else {
        match alias {
            Some(a) => s.pin(&a, &cwd(), client, force)?,
            None => s.pin_auto(&cwd(), client, force)?,
        }
    };
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
        println!("   expires  {}", session.expires_at.to_rfc3339());
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

fn cmd_leave(json: bool) -> Result<()> {
    let s = store()?;
    match s.leave()? {
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
                            let refs = e.credential_refs.join(", ");
                            println!(
                                "  {} {}  tenant={}  providers=[{}]  refs=[{}]",
                                "binding".green(),
                                e.name.bold(),
                                e.tenant.as_deref().unwrap_or("-"),
                                prov,
                                refs.dimmed()
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
                        "credential_refs": result.credential_refs,
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
                println!("   phm refs  (create in Phantom — never commit values)");
                for r in &result.credential_refs {
                    println!("      {}", r.dimmed());
                }
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
    println!("  expires   {}", w.expires_at);
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
            p.credential_ref.dimmed()
        );
    }
    Ok(())
}

fn cmd_status(oneline: bool, json: bool) -> Result<()> {
    let s = store()?;
    let _ = s.check_drift_and_freeze();
    let require_pin = find_workspace(&cwd())
        .map(|(_, cfg)| cfg.require_pin)
        .unwrap_or(false);
    match s.active_session()? {
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
    // Drift check before privileged exec
    let drift = s.check_drift_and_freeze()?;
    if drift.frozen {
        bail!(
            "session_frozen: re-pin — binding drifted under active pin ({})",
            drift.issues.join(", ")
        );
    }
    let session = s.require_active().context("need active pin for exec")?;
    let binding = s.load_binding(&session.binding_alias)?;
    if strict_creds {
        std::env::set_var("LOCUS_SOFT_CREDS", "0");
    } else {
        std::env::set_var("LOCUS_SOFT_CREDS", "1");
    }
    let iso = build_isolated_env_opts(&session, &binding, resolve_secrets);
    if strict_creds && !iso.secrets_failed.is_empty() {
        bail!(
            "credential resolve failed: {}",
            iso.secrets_failed.join("; ")
        );
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
            "{} unresolved credential_refs: {}",
            "warn".yellow(),
            iso.secrets_failed.join(", ")
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
    let env_map = build_ci_env_map(&session, &binding, resolve);
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
    let env_map = build_ci_env_map(&session, &binding, resolve);
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

    let binding = s.load_binding(&session.binding_alias)?;
    if strict_creds {
        std::env::set_var("LOCUS_SOFT_CREDS", "0");
    } else {
        std::env::set_var("LOCUS_SOFT_CREDS", "1");
    }
    // Child gets full isolated env (may resolve secrets for the command to work).
    let iso = build_isolated_env_opts(&session, &binding, resolve_secrets);
    if strict_creds && !iso.secrets_failed.is_empty() {
        let _ = s.cleanup_ci_session(&ci_path, &session);
        bail!(
            "credential resolve failed: {}",
            iso.secrets_failed.join("; ")
        );
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
            "{} unresolved credential_refs: {}",
            "warn".yellow(),
            iso.secrets_failed.join(", ")
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

    let binding = s.load_binding(&session.binding_alias)?;
    if strict_creds {
        std::env::set_var("LOCUS_SOFT_CREDS", "0");
    } else {
        std::env::set_var("LOCUS_SOFT_CREDS", "1");
    }
    let iso = build_isolated_env_opts(&session, &binding, resolve_secrets);
    if strict_creds && !iso.secrets_failed.is_empty() {
        let _ = s.cleanup_run_session(&run_path, &session);
        bail!(
            "credential resolve failed: {}",
            iso.secrets_failed.join("; ")
        );
    }

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
            "{} unresolved credential_refs: {}",
            "warn".yellow(),
            iso.secrets_failed.join(", ")
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
            if json {
                println!("{}", serde_json::to_string_pretty(&list)?);
            } else if list.is_empty() {
                println!(
                    "{} no bindings — try `locus init --with-samples`",
                    "->".dimmed()
                );
            } else {
                for b in list {
                    println!(
                        "  {}  {}  [{}]",
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
            let b = s.load_binding(&alias)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&b)?);
            } else {
                println!("{}", b.to_toml()?);
            }
        }
        BindingCmd::Add {
            alias,
            tenant,
            provider,
            account,
            credential_ref,
            project_ref,
            team_id,
            org,
            read_only,
            description,
        } => {
            let mut scope = Scope {
                project_ref,
                team_id,
                read_only: if read_only { Some(true) } else { None },
                ..Scope::default()
            };
            if let Some(o) = org {
                scope.orgs = vec![o];
            }
            let b = Binding::from_body(BindingBody {
                id: format!("bnd_{alias}"),
                alias: alias.clone(),
                tenant,
                principal: None,
                description,
                policy: Policy::default(),
                providers: vec![ProviderBinding {
                    provider,
                    account,
                    credential_ref,
                    scope,
                    upstream: None,
                }],
            });
            let path = s.save_binding(&b)?;
            if json {
                println!(
                    "{}",
                    serde_json::json!({ "ok": true, "path": path.display().to_string() })
                );
            } else {
                println!("{} wrote {}", "ok".green().bold(), path.display());
            }
        }
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

fn cmd_workspace(
    default: String,
    allow: Option<String>,
    require_pin: bool,
    force: bool,
) -> Result<()> {
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

    // Continuous whoami: freeze session if binding material drifted under pin
    let _ = s.check_drift_and_freeze()?;

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
            apply,
            dry_run,
            workspace,
            mcp_bin,
        } => cmd_agent_setup(&client, apply, dry_run, workspace, mcp_bin, json),
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
            done: 5,
            total: 10,
        },
    ]
}

fn gather_doctor_report(s: &Store) -> Result<locus_core::DoctorReport> {
    // phantom --version is process-cached (locus_core::phantom_on_path).
    let phantom = phantom_on_path();
    let unresolved_phm = collect_unresolved_phm_refs(s, phantom)?;
    build_doctor_report(
        s,
        DoctorExternal {
            phantom_on_path: phantom,
            unresolved_phm,
            cwd: Some(cwd()),
        },
    )
    .map_err(Into::into)
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
        "  mcp       claude={}  cursor={}  codex={}",
        yn(report.mcp_registered.claude),
        yn(report.mcp_registered.cursor),
        yn(report.mcp_registered.codex)
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
    let clients: Vec<&str> = match client {
        "all" => vec!["claude", "cursor", "codex"],
        "claude" | "cursor" | "codex" => vec![client],
        other => bail!("unknown client '{other}' (use claude|cursor|codex|all)"),
    };

    // Ensure ~/.locus (or LOCUS_HOME) exists — init layout + seal key if needed.
    let s = store()?;
    let bin = resolve_mcp_bin(mcp_bin);
    let mut actions: Vec<String> = Vec::new();
    let project = cwd();
    actions.push(format!("ensure locus home → {}", s.home().display()));

    for c in &clients {
        let env_map = mcp_agent_env(c);
        // Invariant: agent setup never enables desktop notify spam.
        debug_assert!(
            !env_map.contains_key("LOCUS_NOTIFY"),
            "LOCUS_NOTIFY must not be set by agent setup"
        );
        let server_entry = json!({
            "command": &bin,
            "args": [],
            "env": env_map,
        });
        match *c {
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

    if json {
        println!(
            "{}",
            json!({
                "ok": true,
                "apply": apply,
                "dry_run": dry_run,
                "clients": clients,
                "home": s.home().display().to_string(),
                "mcp_bin": bin,
                "actions": actions,
                "env": {
                    "LOCUS_AUTO_PIN": "cwd",
                    "LOCUS_CLIENT": "<client>",
                    "LOCUS_NOTIFY": null,
                },
            })
        );
    } else {
        let mode = if dry_run { "dry-run" } else { "applied" };
        println!("{} agent setup ({mode})", "ok".green().bold());
        for a in &actions {
            println!("   · {a}");
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
                "  {} auto-pin kill switch: LOCUS_MCP_AUTO_PIN=0 (see .locus/AGENT.md)",
                "note:".dimmed()
            );
        }
    }
    Ok(())
}

/// Merge/write `[mcp_servers.locus]` into Codex config.toml with agent env.
fn merge_codex_mcp(path: &std::path::Path, bin: &str, client: &str) -> Result<()> {
    let section = format!(
        r#"
[mcp_servers.locus]
command = "{bin}"

[mcp_servers.locus.env]
LOCUS_AUTO_PIN = "cwd"
LOCUS_CLIENT = "{client}"
"#
    );
    if path.exists() {
        let raw = std::fs::read_to_string(path)?;
        if raw.contains("[mcp_servers.locus]") {
            return Ok(());
        }
        let mut out = raw;
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&section);
        std::fs::write(path, out)?;
    } else {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, section.trim_start())?;
    }
    Ok(())
}

/// Poll `check_drift_and_freeze` until interrupted (or once with `--once`).
fn cmd_watch(interval: &str, once: bool, json: bool) -> Result<()> {
    let s = store()?;
    let sleep_dur = parse_watch_interval(interval)?;

    loop {
        let drift = s.check_drift_and_freeze()?;
        if json {
            println!("{}", serde_json::to_string(&drift)?);
        } else if !drift.pinned {
            println!(
                "{} watch  {}",
                "locus".magenta().bold(),
                "not_pinned".dimmed()
            );
        } else if drift.frozen {
            println!(
                "{} watch  {}  {}  issues={}",
                "locus".magenta().bold(),
                "FROZEN".red().bold(),
                drift.binding_alias.as_deref().unwrap_or("?"),
                drift.issues.join(",")
            );
        } else if drift.ok {
            println!(
                "{} watch  {}  {}  ok",
                "locus".magenta().bold(),
                "ok".green().bold(),
                drift.binding_alias.as_deref().unwrap_or("?")
            );
        } else {
            println!(
                "{} watch  {}  {}  issues={}",
                "locus".magenta().bold(),
                "drift".yellow().bold(),
                drift.binding_alias.as_deref().unwrap_or("?"),
                drift.issues.join(",")
            );
        }
        if once {
            if drift.frozen || (!drift.ok && drift.pinned) {
                std::process::exit(1);
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
    let (num, unit) = s.split_at(s.len().saturating_sub(1));
    let n: u64 = num
        .parse()
        .or_else(|_| s.parse())
        .context("invalid watch interval")?;
    match unit {
        "s" | "S" => Ok(std::time::Duration::from_secs(n)),
        "m" | "M" => Ok(std::time::Duration::from_secs(n.saturating_mul(60))),
        "h" | "H" => Ok(std::time::Duration::from_secs(n.saturating_mul(3600))),
        _ if s.chars().all(|c| c.is_ascii_digit()) => Ok(std::time::Duration::from_secs(n)),
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
        "  approvals {}  pending={} dual_control_waiting={} approved={} expired={} denied={}",
        appr_status,
        report.pending_approvals,
        report.dual_control_waiting,
        appr.approved_valid,
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
            "  phm refs  {} unresolved: {}",
            report.unresolved_phm.len(),
            report.unresolved_phm.join(", ").dimmed()
        );
    } else if report.phantom_on_path {
        println!("  phm refs  {}", "ok".green());
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
            };
            println!("  {mark} [{}] {}", f.code, f.message);
        }
    }
}

/// Collect `phm:NAME` credential refs from all bindings that are not present
/// in `phantom list` output. Returns secret **names only** (never values).
fn collect_unresolved_phm_refs(s: &Store, phantom_on_path: bool) -> Result<Vec<String>> {
    use locus_core::CredentialRef;

    let summaries = s.list_bindings()?;
    let mut needed: Vec<String> = Vec::new();
    for sum in summaries {
        let b = match s.load_binding(&sum.alias) {
            Ok(b) => b,
            Err(_) => continue,
        };
        for p in &b.providers {
            if let CredentialRef::Phantom { name } = CredentialRef::parse(&p.credential_ref) {
                if !needed.iter().any(|n| n == &name) {
                    needed.push(name);
                }
            }
        }
    }
    if needed.is_empty() {
        return Ok(Vec::new());
    }
    if !phantom_on_path {
        // Cannot verify — report all as unresolved so doctor surfaces the gap.
        return Ok(needed);
    }

    let known = phantom_list_names()?;
    let mut unresolved: Vec<String> = needed
        .into_iter()
        .filter(|n| !known.iter().any(|k| k == n))
        .collect();
    unresolved.sort();
    Ok(unresolved)
}

/// Parse secret names from `phantom list` (best-effort; stdout shape may vary).
fn phantom_list_names() -> Result<Vec<String>> {
    let output = Command::new("phantom")
        .arg("list")
        .output()
        .context("run phantom list")?;
    if !output.status.success() {
        // Treat as empty known set — doctor will flag all phm refs.
        return Ok(Vec::new());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut names = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Common formats: bare NAME, "NAME ...", "  NAME", JSON-ish "name": "NAME"
        let token = line
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_matches(|c: char| c == '"' || c == '\'' || c == ',' || c == ':');
        if token.is_empty() || token.contains('=') {
            continue;
        }
        // Skip table headers / chrome
        let lower = token.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "name" | "secret" | "secrets" | "key" | "---" | "total"
        ) {
            continue;
        }
        if !names.iter().any(|n| n == token) {
            names.push(token.to_string());
        }
    }
    Ok(names)
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
if [[ -n "$ZSH_VERSION" ]]; then
  autoload -Uz add-zsh-hook 2>/dev/null
  add-zsh-hook chpwd _locus_auto_enter 2>/dev/null || true
elif [[ -n "$BASH_VERSION" ]]; then
  # bash: run on PROMPT_COMMAND (best-effort). Colors via ANSI for bash PS1 use.
  if [[ -z "$_LOCUS_PROMPT_CMD" ]]; then
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
    if let Some(p) = mcp_bin {
        return p;
    }
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("locus-mcp");
            if candidate.exists() {
                return candidate.display().to_string();
            }
        }
    }
    "locus-mcp".into()
}

fn cmd_setup(client: &str, print_only: bool, mcp_bin: Option<String>) -> Result<()> {
    let bin = resolve_mcp_bin(mcp_bin);
    let server_entry = serde_json::json!({
        "command": bin,
        "args": [],
        "env": {}
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
        "codex" => {
            println!("{}", "Codex setup".magenta().bold());
            println!("Add to ~/.codex/config.toml:");
            println!();
            println!("[mcp_servers.locus]");
            println!("command = \"{bin}\"");
            if print_only {
                return Ok(());
            }
            println!();
            println!(
                "{} printed instructions (codex uses TOML — merge manually)",
                "->".dimmed()
            );
        }
        other => bail!("unknown client: {other} (claude|cursor|codex)"),
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
fn cmd_events_export(
    last: usize,
    op: Option<String>,
    binding: Option<String>,
    otlp: bool,
    out: Option<PathBuf>,
    service_name: String,
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

    if let Some(path) = out {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        std::fs::write(&path, &body)?;
        eprintln!(
            "{} wrote {} ({} bytes)",
            "events export".magenta().bold(),
            path.display(),
            body.len()
        );
    } else {
        print!("{body}");
    }
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
    use locus_core::{load_config, notifications_enabled, save_config};

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
            let mut cfg = load_config(home);
            cfg.notify.enabled = true;
            let path = save_config(home, &cfg)?;
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
            let mut cfg = load_config(home);
            cfg.notify.enabled = false;
            let path = save_config(home, &cfg)?;
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
                    "Grant: locus approve grant <id> --as <principal>   Deny: locus approve deny <id>"
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
                let required = if dual { 2 } else { 1 };
                let grants_n = format!("grants {}/{}", rec.grants.len(), required);
                let progress = if dual {
                    format!("{grants_n} (dual_control)")
                } else {
                    grants_n
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
                "Grant: locus approve grant <id> --as <principal>   status: locus approve status <id>   wait: locus approve wait <id>"
                    .dimmed()
            );
            Ok(())
        }
        ApproveCmd::Grant {
            id,
            as_principal,
            ttl,
        } => {
            let principal = as_principal
                .or_else(|| env::var("LOCUS_PRINCIPAL").ok().filter(|s| !s.is_empty()))
                .or_else(|| env::var("USER").ok().filter(|s| !s.is_empty()))
                .unwrap_or_else(|| "unknown".into());
            let ttl_dur = match ttl {
                Some(ref t) => Some(parse_ttl(t)?),
                None => None,
            };
            let rec = s.grant_approval(&id, ttl_dur, &principal)?;
            let dual = s.tool_requires_dual_control(&rec.binding, &rec.tool);
            let required = if dual { 2 } else { 1 };
            if json {
                let mut v = serde_json::to_value(&rec)?;
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("dual_control".into(), json!(dual));
                    obj.insert("required_grants".into(), json!(required));
                    obj.insert(
                        "grants_progress".into(),
                        json!(format!("{}/{}", rec.grants.len(), required)),
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
                    obj.insert(
                        "grants_progress".into(),
                        json!(format!("{}/{}", rec.grants.len(), required)),
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
                    "   dual      {}  grants {}/{}",
                    if dual { "yes" } else { "no" },
                    rec.grants.len(),
                    required
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
    ttl_flag: Option<&str>,
) {
    let principals: String = rec
        .grants
        .iter()
        .map(|g| g.principal.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let grants_progress = format!("{}/{}", rec.grants.len(), required);

    if rec.status == locus_core::ApprovalStatus::Approved {
        println!(
            "{} granted {} as {}",
            "ok".green().bold(),
            rec.id.cyan(),
            principal.yellow()
        );
        println!("   tool      {}", rec.tool.yellow());
        println!("   binding   {}", rec.binding.yellow());
        println!(
            "   grants    {}  ({})",
            grants_progress.bold(),
            if principals.is_empty() {
                "-"
            } else {
                &principals
            }
        );
        if dual {
            println!("   dual      yes (fully approved)");
        }
        if let Some(exp) = rec.expires_at {
            let ttl_note = ttl_flag.unwrap_or("15m");
            println!("   ttl       {}  expires {}", ttl_note, exp.to_rfc3339());
        }
        println!(
            "   {}",
            "Re-call the tool with the same args (or confirm=true + approval_id).".dimmed()
        );
    } else {
        println!(
            "{} partial grant {} as {}",
            "ok".yellow().bold(),
            rec.id.cyan(),
            principal.yellow()
        );
        println!("   tool      {}", rec.tool.yellow());
        println!("   binding   {}", rec.binding.yellow());
        println!(
            "   grants    {}  ({}){}",
            grants_progress.bold(),
            if principals.is_empty() {
                "-"
            } else {
                &principals
            },
            if dual { "  dual_control" } else { "" }
        );
        let remaining = required.saturating_sub(rec.grants.len());
        println!(
            "   {}",
            format!(
                "Need {remaining} more distinct principal(s) — run `locus approve grant {} --as <other>`",
                rec.id
            )
            .dimmed()
        );
        println!(
            "   {}",
            format!("Or wait: locus approve wait {} --timeout 120", rec.id).dimmed()
        );
    }
}

/// Merge a single server entry into an mcpServers JSON file.
fn merge_mcp_json(path: &std::path::Path, name: &str, server: &serde_json::Value) -> Result<()> {
    let mut root: serde_json::Value = if path.exists() {
        let raw = std::fs::read_to_string(path)?;
        serde_json::from_str(&raw).unwrap_or_else(|_| json!({ "mcpServers": {} }))
    } else {
        json!({ "mcpServers": {} })
    };
    if !root.is_object() {
        root = json!({ "mcpServers": {} });
    }
    let servers = root
        .as_object_mut()
        .unwrap()
        .entry("mcpServers")
        .or_insert_with(|| json!({}));
    if !servers.is_object() {
        *servers = json!({});
    }
    servers
        .as_object_mut()
        .unwrap()
        .insert(name.to_string(), server.clone());
    std::fs::write(path, serde_json::to_string_pretty(&root)?)?;
    Ok(())
}

//! Locus CLI — identity plane for coding agents.
//!
//! ```text
//! locus pin acme
//! locus whoami
//! locus exec -- gh pr list
//! locus leave
//! ```

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use colored::Colorize;
use locus_core::{
    build_doctor_report, build_isolated_env_opts, filter_audit_events, parse_ttl, Binding,
    BindingBody, DoctorExternal, DoctorVerdict, Policy, ProviderBinding, Scope, Store,
    WorkspaceConfig, VERSION,
};
use serde_json::json;
use std::env;
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
  Setup         init · setup · doctor · watch · workspace · hook · mcp · engagement\n  \
  Daily use     enter · pin · leave · whoami · status · exec · run · binding\n  \
  Approvals     approve · notify\n  \
  Audit         events\n  \
  Maintenance   version"
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

    // ────────────────────────── Approvals ────────────────────────────
    /// Manage require_approval / dual-control grants for blocked tool calls
    #[command(next_help_heading = "Approvals", subcommand)]
    Approve(ApproveCmd),

    /// Desktop approval banners (OFF by default — opt in explicitly)
    #[command(next_help_heading = "Approvals", subcommand)]
    Notify(NotifyCmd),

    // ──────────────────────────── Audit ──────────────────────────────
    /// Read recent local audit events (`$LOCUS_HOME/audit/events.jsonl`)
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
    },

    // ───────────────────────── Maintenance ───────────────────────────
    /// Print version (also available as `locus --version`)
    #[command(next_help_heading = "Maintenance")]
    Version,
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
    let cli = Cli::parse();
    match cli.command {
        Commands::Init { with_samples } => cmd_init(with_samples, cli.json),
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
        Commands::Whoami => cmd_whoami(cli.json),
        Commands::Status { oneline } => cmd_status(oneline, cli.json),
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
        Commands::Mcp => cmd_mcp(),
        Commands::Binding(sub) => cmd_binding(sub, cli.json),
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
        Commands::Events { last, op, binding } => cmd_events(last, op, binding, cli.json),
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

fn store() -> Result<Store> {
    Store::open_default().context("open locus store")
}

fn cwd() -> PathBuf {
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn cmd_init(with_samples: bool, json: bool) -> Result<()> {
    let s = store()?;
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
            })
        );
    } else {
        println!("{} locus home {}", "ok".green().bold(), s.home().display());
        println!("   seal key {}", s.seal_key_path().display());
        if with_samples {
            println!("   samples  personal, acme");
        }
        println!();
        println!("next:");
        println!("  locus binding list");
        println!("  locus pin personal");
        println!("  locus whoami");
    }
    Ok(())
}

fn write_sample_bindings(s: &Store) -> Result<()> {
    let personal = Binding::from_body(BindingBody {
        id: "bnd_personal".into(),
        alias: "personal".into(),
        tenant: "personal".into(),
        principal: None,
        description: Some("Personal projects".into()),
        policy: Policy::default(),
        providers: vec![
            ProviderBinding {
                provider: "supabase".into(),
                account: "personal".into(),
                credential_ref: "phm:SUPABASE_PERSONAL".into(),
                scope: Scope {
                    project_ref: Some("personal_ref_replace_me".into()),
                    read_only: Some(false),
                    ..Scope::default()
                },
                upstream: None,
            },
            ProviderBinding {
                provider: "github".into(),
                account: "personal".into(),
                credential_ref: "phm:GH_TOKEN_PERSONAL".into(),
                scope: Scope {
                    orgs: vec![],
                    ..Scope::default()
                },
                upstream: None,
            },
            ProviderBinding {
                provider: "vercel".into(),
                account: "personal".into(),
                credential_ref: "phm:VERCEL_TOKEN_PERSONAL".into(),
                scope: Scope {
                    team_id: Some("team_personal_replace_me".into()),
                    ..Scope::default()
                },
                upstream: None,
            },
        ],
    });
    let acme = Binding::from_body(BindingBody {
        id: "bnd_acme".into(),
        alias: "acme".into(),
        tenant: "acme-corp".into(),
        principal: None,
        description: Some("Acme client engagement".into()),
        policy: Policy {
            require_approval: vec!["*.delete*".into(), "vercel.deploy.prod".into()],
            max_ttl: Some("8h".into()),
            ..Policy::default()
        },
        providers: vec![
            ProviderBinding {
                provider: "supabase".into(),
                account: "acme-prod".into(),
                credential_ref: "phm:SUPABASE_ACME".into(),
                scope: Scope {
                    project_ref: Some("acme_ref_replace_me".into()),
                    read_only: Some(true),
                    ..Scope::default()
                },
                upstream: None,
            },
            ProviderBinding {
                provider: "github".into(),
                account: "acme-corp".into(),
                credential_ref: "phm:GH_TOKEN_ACME".into(),
                scope: Scope {
                    orgs: vec!["acme-corp".into()],
                    repos: vec!["acme-corp/*".into()],
                    ..Scope::default()
                },
                upstream: None,
            },
            ProviderBinding {
                provider: "vercel".into(),
                account: "acme-team".into(),
                credential_ref: "phm:VERCEL_TOKEN_ACME".into(),
                scope: Scope {
                    team_id: Some("team_acme_replace_me".into()),
                    projects: vec!["acme-web".into()],
                    env: vec!["preview".into()],
                    ..Scope::default()
                },
                upstream: None,
            },
        ],
    });
    s.save_binding(&personal)?;
    s.save_binding(&acme)?;
    Ok(())
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
    match s.active_session()? {
        None => {
            if json {
                println!("{}", serde_json::json!({ "pinned": false }));
            } else if oneline {
                println!("unpinned");
            } else {
                println!("{} unpinned — run `locus pin <alias>`", "!".yellow().bold());
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

    // Phantom on PATH (external)
    let phantom_on_path = Command::new("phantom")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|st| st.success())
        .unwrap_or(false);
    let unresolved_phm = collect_unresolved_phm_refs(&s, phantom_on_path)?;

    let report = build_doctor_report(
        &s,
        DoctorExternal {
            phantom_on_path,
            unresolved_phm,
            cwd: Some(cwd()),
        },
    )?;

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

    // Recent audit
    let au = &report.audit;
    println!(
        "  audit     total={}  recent_scope_freeze={}  recent_deny={}",
        au.total, au.scope_freeze, au.deny
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
            println!(
                r#"# Locus prompt + optional auto-enter — eval "$(locus hook zsh)"
# Prompt shows [locus:enter] when unpinned, [locus:alias:tenant] when pinned.
# LOCUS_AUTO_ENTER=1 → on directory change, try `locus enter` (workspace default / autopin).
# Never forces allowlist; never overrides with secrets.
_locus_prompt() {{
  local s
  s="$(locus status --oneline 2>/dev/null)" || s="unpinned"
  if [[ "$s" == "unpinned" ]]; then
    echo "%F{{red}}[locus:enter]%f"
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
  # bash: run on PROMPT_COMMAND (best-effort)
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
                r#"# Locus prompt helper for fish — [locus:enter] | [locus:alias:tenant]
# LOCUS_AUTO_ENTER=1 → try enter when changing directories
function locus_prompt
  set -l s (locus status --oneline 2>/dev/null; or echo unpinned)
  if test "$s" = "unpinned"
    echo "[locus:enter]"
  else if test "$s" = "invalid"
    echo "[locus:invalid]"
  else
    echo "[locus:$s]"
  end
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

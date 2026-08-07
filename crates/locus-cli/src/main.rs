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
    build_isolated_env_opts, Binding, BindingBody, Policy, ProviderBinding, Scope, Store,
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
Every CLI exec and (soon) MCP tool call is hard-scoped to that pin.\n\
Sibling to Phantom Secrets: Phantom protects secrets in context;\n\
Locus protects which identity acts."
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
    /// Initialize ~/.locus (or LOCUS_HOME)
    Init {
        /// Also write a sample personal + acme binding pair
        #[arg(long)]
        with_samples: bool,
    },

    /// Pin the current session to a binding
    Pin {
        /// Binding alias or id (default: .locus.toml default_binding)
        alias: Option<String>,
        /// Allow bindings outside workspace allowlist
        #[arg(long)]
        force: bool,
        /// Client label recorded on the session (claude, cursor, cli)
        #[arg(long)]
        client: Option<String>,
    },

    /// Clear the active pin and tear down worker dirs
    Leave,

    /// Show who you are acting as (active pin)
    Whoami,

    /// Short status line for prompts / CI
    Status {
        /// One-line machine form: `unpinned` or `alias:tenant`
        #[arg(long)]
        oneline: bool,
    },

    /// Run a command with only the pinned binding's identity surface
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

    /// Run the locus-mcp stdio server (same as the locus-mcp binary)
    Mcp,

    /// Manage bindings
    #[command(subcommand)]
    Binding(BindingCmd),

    /// Write a .locus.toml in the current directory
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

    /// Health check for the local control plane
    Doctor,

    /// Shell hook snippet (prints eval-able code)
    Hook {
        /// Shell: zsh | bash | fish
        shell: String,
    },

    /// Register locus-mcp with an AI client config
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

    /// List pending require_approval tool calls from the audit log (stub)
    ///
    /// Phase 1: read-only listing. Granting approval UX lands later — for now
    /// re-call the tool with `confirm=true` after human review, or adjust
    /// binding.policy.require_approval.
    Approve {
        /// Limit number of events shown (default: 50)
        #[arg(long, default_value_t = 50)]
        limit: usize,
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
    let cli = Cli::parse();
    match cli.command {
        Commands::Init { with_samples } => cmd_init(with_samples, cli.json),
        Commands::Pin {
            alias,
            force,
            client,
        } => cmd_pin(alias, force, client, cli.json),
        Commands::Leave => cmd_leave(cli.json),
        Commands::Whoami => cmd_whoami(cli.json),
        Commands::Status { oneline } => cmd_status(oneline, cli.json),
        Commands::Exec {
            no_resolve,
            strict_creds,
            cmd,
        } => cmd_exec(cmd, !no_resolve, strict_creds),
        Commands::Mcp => cmd_mcp(),
        Commands::Binding(sub) => cmd_binding(sub, cli.json),
        Commands::Workspace {
            default,
            allow,
            require_pin,
            force,
        } => cmd_workspace(default, allow, require_pin, force),
        Commands::Doctor => cmd_doctor(cli.json),
        Commands::Hook { shell } => cmd_hook(&shell),
        Commands::Setup {
            client,
            print,
            mcp_bin,
        } => cmd_setup(&client, print, mcp_bin),
        Commands::Approve { limit } => cmd_approve(limit, cli.json),
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
            },
            ProviderBinding {
                provider: "github".into(),
                account: "personal".into(),
                credential_ref: "phm:GH_TOKEN_PERSONAL".into(),
                scope: Scope {
                    orgs: vec![],
                    ..Scope::default()
                },
            },
            ProviderBinding {
                provider: "vercel".into(),
                account: "personal".into(),
                credential_ref: "phm:VERCEL_TOKEN_PERSONAL".into(),
                scope: Scope {
                    team_id: Some("team_personal_replace_me".into()),
                    ..Scope::default()
                },
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
            },
        ],
    });
    s.save_binding(&personal)?;
    s.save_binding(&acme)?;
    Ok(())
}

fn cmd_pin(alias: Option<String>, force: bool, client: Option<String>, json: bool) -> Result<()> {
    let s = store()?;
    let client = client.or_else(|| Some("cli".into()));
    let session = match alias {
        Some(a) => s.pin(&a, &cwd(), client, force)?,
        None => s.pin_auto(&cwd(), client, force)?,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&session)?);
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
        println!();
        println!("   {}", "locus whoami".dimmed());
        println!("   {}", "locus exec -- <cmd>".dimmed());
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
                println!("{} no active pin", "->".dimmed());
            }
        }
        Some(session) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "left": true,
                        "binding": session.binding_alias,
                        "session_id": session.session_id,
                    })
                );
            } else {
                println!(
                    "{} left {} ({})",
                    "ok".green().bold(),
                    session.binding_alias,
                    session.session_id
                );
            }
        }
    }
    Ok(())
}

fn cmd_whoami(json: bool) -> Result<()> {
    let s = store()?;
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
            let ok = session.verify(&key).is_ok();
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "pinned": true,
                        "binding": session.binding_alias,
                        "tenant": session.tenant,
                        "session_id": session.session_id,
                        "seal_ok": ok,
                        "expired": session.is_expired(),
                    })
                );
            } else if oneline {
                if !ok {
                    println!("invalid");
                } else {
                    println!("{}:{}", session.binding_alias, session.tenant);
                }
            } else {
                let mark = if ok { "ok".green() } else { "INVALID".red() };
                println!(
                    "{} {} ({})  {}",
                    mark,
                    session.binding_alias.cyan().bold(),
                    session.tenant,
                    session.session_id.dimmed()
                );
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
    let bindings = s.list_bindings()?;
    let active = s.active_session()?;
    let seal_key_ok = s.seal_key().is_ok();
    let drift = s.verify_runtime()?;
    let mut issues: Vec<String> = Vec::new();

    // Bindings count
    if bindings.is_empty() {
        issues.push("no bindings configured (locus init --with-samples)".into());
    }

    // Seal key file present
    if !seal_key_ok {
        issues.push("seal key missing or unreadable".into());
    }

    // Active pin seal (when pinned)
    let mut pin_seal_ok: Option<bool> = None;
    if let Some(ref sess) = active {
        match sess.verify(&s.seal_key()?) {
            Ok(()) => pin_seal_ok = Some(true),
            Err(e) => {
                pin_seal_ok = Some(false);
                issues.push(format!("active pin seal invalid: {e}"));
            }
        }
        if !drift.ok {
            for tag in &drift.issues {
                if tag != "not_pinned" {
                    issues.push(format!("runtime drift: {tag}"));
                }
            }
        }
    }

    // Phantom on PATH
    let phantom_on_path = Command::new("phantom")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|st| st.success())
        .unwrap_or(false);
    if !phantom_on_path {
        issues
            .push("phantom not on PATH (install Phantom Secrets for phm: credential refs)".into());
    }

    // Unresolved phm: refs — names only via `phantom list` when available
    let unresolved_phm = collect_unresolved_phm_refs(&s, phantom_on_path)?;
    if !unresolved_phm.is_empty() {
        issues.push(format!(
            "unresolved phm refs (names only): {}",
            unresolved_phm.join(", ")
        ));
    }

    let report = serde_json::json!({
        "version": VERSION,
        "home": s.home().display().to_string(),
        "bindings": bindings.len(),
        "pinned": active.as_ref().map(|a| a.binding_alias.clone()),
        "seal_ok": seal_key_ok,
        "pin_seal_ok": pin_seal_ok,
        "phantom_on_path": phantom_on_path,
        "unresolved_phm": unresolved_phm,
        "runtime": drift,
        "issues": issues,
        "ok": issues.is_empty(),
    });

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("{} locus {}", "doctor".magenta().bold(), VERSION);
        println!("  home      {}", s.home().display());
        println!("  bindings  {}", bindings.len());
        println!(
            "  pin       {}",
            active
                .as_ref()
                .map(|a| a.binding_alias.as_str())
                .unwrap_or("none")
        );
        println!(
            "  seal      {}",
            if seal_key_ok {
                "ok".green().to_string()
            } else {
                "FAIL".red().to_string()
            }
        );
        if let Some(ok) = pin_seal_ok {
            println!(
                "  pin seal  {}",
                if ok {
                    "ok".green().to_string()
                } else {
                    "FAIL".red().to_string()
                }
            );
        }
        println!(
            "  phantom   {}",
            if phantom_on_path {
                "on PATH".green().to_string()
            } else {
                "missing".yellow().to_string()
            }
        );
        if !unresolved_phm.is_empty() {
            println!(
                "  phm refs  {} unresolved: {}",
                unresolved_phm.len(),
                unresolved_phm.join(", ").dimmed()
            );
        } else if phantom_on_path {
            println!("  phm refs  {}", "ok".green());
        }
        if issues.is_empty() {
            println!("  {}", "all clear".green().bold());
        } else {
            for i in &issues {
                println!("  {} {i}", "!".yellow());
            }
        }
    }
    if !issues.is_empty() {
        std::process::exit(1);
    }
    Ok(())
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
                r#"# Locus prompt helper — eval "$(locus hook zsh)"
_locus_prompt() {{
  local s
  s="$(locus status --oneline 2>/dev/null)" || s="unpinned"
  if [[ "$s" == "unpinned" || "$s" == "invalid" ]]; then
    echo "%F{{red}}[locus:$s]%f"
  else
    echo "%F{{cyan}}[locus:$s]%f"
  fi
}}
# Optional: add to PROMPT via: PROMPT='$(_locus_prompt) '"$PROMPT
"#
            );
        }
        "fish" => {
            println!(
                r#"# Locus prompt helper for fish
function locus_prompt
  set -l s (locus status --oneline 2>/dev/null; or echo unpinned)
  echo "[locus:$s]"
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

/// List pending `mcp.require_approval` audit events (phase-1 stub — no grant).
fn cmd_approve(limit: usize, json: bool) -> Result<()> {
    let s = store()?;
    let mut pending = s.pending_approvals()?;
    // newest last in file — reverse for most-recent-first display
    pending.reverse();
    if pending.len() > limit {
        pending.truncate(limit);
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&pending)?);
        return Ok(());
    }

    if pending.is_empty() {
        println!(
            "{} no pending require_approval events in {}",
            "->".dimmed(),
            s.audit_path().display()
        );
        println!(
            "   {}",
            "Blocked tool calls are recorded when agents hit policy.require_approval.".dimmed()
        );
        println!(
            "   {}",
            "After review: re-call with confirm=true, or edit binding.policy.require_approval."
                .dimmed()
        );
        return Ok(());
    }

    println!(
        "{} {} pending approval event(s)  {}",
        "approve".magenta().bold(),
        pending.len(),
        "(stub — list only)".dimmed()
    );
    for (i, ev) in pending.iter().enumerate() {
        let tool = ev
            .detail
            .as_ref()
            .and_then(|d| d.get("tool"))
            .and_then(|t| t.as_str())
            .unwrap_or("?");
        let detail = ev
            .detail
            .as_ref()
            .and_then(|d| d.get("detail"))
            .and_then(|t| t.as_str())
            .unwrap_or("");
        println!(
            "  {}. {}  binding={}  tool={}",
            i + 1,
            ev.ts.dimmed(),
            ev.binding.cyan(),
            tool.yellow()
        );
        if !detail.is_empty() {
            println!("      {}", detail.dimmed());
        }
    }
    println!();
    println!(
        "{}",
        "Phase 1: granting is manual — re-call tool with confirm=true after human review.".dimmed()
    );
    Ok(())
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

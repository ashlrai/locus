//! Operator launch for the swarm broker. All protocol and execution gates live
//! in locus-mcp; the CLI never mints grants or resolves credentials here.

use anyhow::{bail, Context, Result};
use locus_core::{CredentialRef, Store, SwarmGraph, CONTROL_CAPABILITY_ENV};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, Stdio};

const SERVER_TOKEN_ENV: &str = "LOCUS_MCP_HTTP_TOKEN";

pub fn run(port: u16, graph_path: &Path, mcp_bin: Option<String>) -> Result<()> {
    if port == 0 {
        bail!("port must be non-zero");
    }
    let graph = SwarmGraph::load(graph_path)?;
    let graph_path = graph_path
        .canonicalize()
        .context("swarm_manifest_unreadable")?;
    let store = super::store()?;
    store.require_local_control("serve swarm")?;
    let credential_names = credential_names(&store, &graph)?;
    let parent: BTreeMap<_, _> = std::env::vars_os().collect();
    let child_env = broker_environment(&parent, store.home(), &credential_names)?;

    let explicit = mcp_bin.is_some();
    let (bin, bare) = super::resolve_mcp_bin_with_fallback(mcp_bin);
    if explicit {
        super::validate_explicit_mcp_bin(&bin)?;
    } else if bare && super::find_on_path(&bin).is_none() {
        bail!("locus-mcp not found; install the matching locus-mcp binary");
    }
    let mut child = Command::new(&bin);
    child
        .arg("--http")
        .arg(format!("127.0.0.1:{port}"))
        .arg("--multi-tenant")
        .arg("--swarm")
        .arg(graph_path)
        .env_clear()
        .envs(child_env)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    store.audit(
        "swarm.serve",
        "-",
        Some(json!({ "phase": "start", "agents": graph.agent_count(), "port": port })),
    )?;
    eprintln!("locus serve: swarm MCP broker on http://127.0.0.1:{port}/mcp");
    let spawned = super::run_child(&mut child, &bin);
    let (exit, _) = super::child_audit_fields(&spawned);
    store.audit(
        "swarm.serve",
        "-",
        Some(json!({ "phase": "exit", "exit": exit, "spawn_failed": spawned.is_err() })),
    )?;
    let status = spawned?;
    if !status.success() {
        std::process::exit(super::child_exit_code(&status));
    }
    Ok(())
}

/// Only live graph grants may contribute credential locator names to the
/// server environment. Values stay opaque until the MCP execution gate resolves
/// the selected binding; unrelated provider identities are never inherited.
fn credential_names(store: &Store, graph: &SwarmGraph) -> Result<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    for agent in graph.agents() {
        let grant = store
            .load_mcp_grant(&agent.grant_id)?
            .filter(|grant| !grant.revoked && !grant.is_expired())
            .context("swarm_grant_unavailable")?;
        let session = store
            .require_session_by_id(&grant.session_id)
            .context("swarm_grant_session_unavailable")?;
        if session.binding_alias != grant.binding_alias || session.tenant != grant.tenant {
            bail!("swarm_grant_session_mismatch");
        }
        let resolved = store
            .load_session_by_id_resolved(&grant.session_id)?
            .context("swarm_grant_session_unavailable")?;
        if !store.check_drift_and_freeze_for(&resolved)?.ok {
            bail!("swarm_grant_session_unhealthy");
        }
        for alias in session.all_aliases() {
            let binding = store.load_binding(&alias)?;
            for provider in &binding.providers {
                if let CredentialRef::Env { var } =
                    CredentialRef::validate(&provider.credential_ref)?
                {
                    if reserved_credential_name(&var) {
                        bail!("swarm_credential_env_reserved");
                    }
                    names.insert(var);
                }
            }
        }
    }
    Ok(names)
}

fn runtime_name(name: &str) -> bool {
    matches!(
        name,
        "PATH"
            | "LANG"
            | "TERM"
            | "COLORTERM"
            | "NO_COLOR"
            | "TMPDIR"
            | "TMP"
            | "TEMP"
            | "SystemRoot"
            | "WINDIR"
            | "PATHEXT"
            | "ComSpec"
            | "HOME"
            | "USERPROFILE"
    ) || name.starts_with("LC_")
}

fn reserved_credential_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    // Stock bindings use these credential-only namespaces. They are never
    // transport/session knobs; every other LOCUS_* name remains reserved.
    let locus_credential_source = ["LOCUS_GH_TOKEN_", "LOCUS_SUPABASE_", "LOCUS_VERCEL_TOKEN_"]
        .iter()
        .any(|prefix| {
            upper
                .strip_prefix(*prefix)
                .is_some_and(|suffix| !suffix.is_empty())
        })
        || matches!(upper.as_str(), "LOCUS_PROVIDER_TOKEN" | "LOCUS_DEMO_UNUSED");
    (upper.starts_with("LOCUS_") && !locus_credential_source)
        || upper.starts_with("LD_")
        || upper.starts_with("DYLD_")
        || runtime_name(name)
        || runtime_name(&upper)
        || matches!(
            upper.as_str(),
            "COMSPEC"
                | "SYSTEMROOT"
                | "HTTP_PROXY"
                | "HTTPS_PROXY"
                | "ALL_PROXY"
                | "NO_PROXY"
                | "GH_CONFIG_DIR"
                | "AWS_CONFIG_FILE"
                | "AWS_SHARED_CREDENTIALS_FILE"
                | "OPENSSL_CONF"
                | "SSL_CERT_FILE"
                | "SSL_CERT_DIR"
        )
}

fn broker_environment(
    parent: &BTreeMap<OsString, OsString>,
    home: &Path,
    credential_names: &BTreeSet<String>,
) -> Result<BTreeMap<OsString, OsString>> {
    let mut vars: BTreeMap<_, _> = parent
        .iter()
        .filter(|(key, _)| key.to_str().is_some_and(runtime_name))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    for name in credential_names {
        if reserved_credential_name(name) {
            bail!("swarm_credential_env_reserved");
        }
        if let Some(value) = parent.get(std::ffi::OsStr::new(name)) {
            vars.insert(name.into(), value.clone());
        }
    }
    // These are transport/operator capabilities, never provider tool arguments.
    // Explicitly copying them avoids inherited transport mode or session knobs.
    for name in [CONTROL_CAPABILITY_ENV, SERVER_TOKEN_ENV] {
        let value = parent
            .get(std::ffi::OsStr::new(name))
            .filter(|value| !value.is_empty())
            .with_context(|| format!("{name} is required for the swarm broker"))?;
        vars.insert(name.into(), value.clone());
    }
    vars.insert("LOCUS_HOME".into(), home.as_os_str().to_owned());
    Ok(vars)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn swarm_cli_parses_and_preserves_dashboard_default() {
        let cli = crate::Cli::try_parse_from([
            "locus",
            "serve",
            "--swarm",
            "graph.json",
            "--port",
            "9000",
            "--mcp-bin",
            "./locus-mcp",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            crate::Commands::Serve {
                swarm: Some(_),
                port: 9000,
                ..
            }
        ));
        let cli = crate::Cli::try_parse_from(["locus", "serve"]).unwrap();
        assert!(matches!(
            cli.command,
            crate::Commands::Serve { swarm: None, .. }
        ));
    }

    #[test]
    fn swarm_cli_refuses_dashboard_options_and_unscoped_binary() {
        for args in [
            vec!["locus", "serve", "--swarm", "graph.json", "--open"],
            vec![
                "locus",
                "serve",
                "--swarm",
                "graph.json",
                "--token",
                "fixture",
            ],
            vec!["locus", "serve", "--mcp-bin", "./locus-mcp"],
        ] {
            assert!(crate::Cli::try_parse_from(args).is_err());
        }
    }

    fn parent_env() -> BTreeMap<OsString, OsString> {
        [
            ("PATH", "/usr/bin"),
            ("HOME", "/home/operator"),
            (CONTROL_CAPABILITY_ENV, "fixture-control"),
            (SERVER_TOKEN_ENV, "fixture-http"),
            ("CLIENT_A_KEY", "fixture-a"),
            ("CLIENT_B_KEY", "fixture-b"),
            ("GH_TOKEN", "fixture-ambient"),
            ("AWS_PROFILE", "ambient"),
            ("LOCUS_MCP_HTTP_ALLOW_REMOTE", "1"),
            ("LOCUS_SESSION_ID", "ambient-session"),
            ("LD_PRELOAD", "ambient-loader"),
            ("HTTPS_PROXY", "ambient-proxy"),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect()
    }

    #[test]
    fn broker_environment_keeps_only_runtime_transport_and_scoped_locators() {
        let vars = broker_environment(
            &parent_env(),
            Path::new("/private/locus"),
            &BTreeSet::from(["CLIENT_A_KEY".into()]),
        )
        .unwrap();
        assert_eq!(
            vars.get(std::ffi::OsStr::new("CLIENT_A_KEY")).unwrap(),
            "fixture-a"
        );
        assert_eq!(
            vars.get(std::ffi::OsStr::new("LOCUS_HOME")).unwrap(),
            "/private/locus"
        );
        for forbidden in [
            "CLIENT_B_KEY",
            "GH_TOKEN",
            "AWS_PROFILE",
            "LOCUS_SESSION_ID",
            "LOCUS_MCP_HTTP_ALLOW_REMOTE",
            "LD_PRELOAD",
            "HTTPS_PROXY",
        ] {
            assert!(!vars.contains_key(std::ffi::OsStr::new(forbidden)));
        }
    }

    #[test]
    fn broker_environment_refuses_reserved_locators_and_missing_capabilities() {
        for name in [
            "LOCUS_MCP_MULTI_TENANT",
            "LOCUS_CONTROL_CAPABILITY",
            "LOCUS_EXECUTOR_CAPABILITY",
            "LOCUS_MCP_HTTP_TOKEN",
            "LOCUS_HOME",
            "LOCUS_SESSION_ID",
            "HOME",
            "PATH",
            "LD_PRELOAD",
            "DYLD_INSERT_LIBRARIES",
            "DYLD_FRAMEWORK_PATH",
            "LD_AUDIT",
            "path",
            "https_proxy",
            "GH_CONFIG_DIR",
            "AWS_CONFIG_FILE",
            "SystemRoot",
            "windir",
            "PathExt",
            "comspec",
            "SSL_CERT_FILE",
        ] {
            assert!(broker_environment(
                &parent_env(),
                Path::new("/private/locus"),
                &BTreeSet::from([name.into()])
            )
            .is_err());
        }
        for name in [CONTROL_CAPABILITY_ENV, SERVER_TOKEN_ENV] {
            let mut parent = parent_env();
            parent.remove(std::ffi::OsStr::new(name));
            assert!(
                broker_environment(&parent, Path::new("/private/locus"), &BTreeSet::new()).is_err()
            );
        }
    }

    #[test]
    fn broker_environment_supports_stock_locus_credential_namespaces() {
        let names = BTreeSet::from([
            "LOCUS_GH_TOKEN_PERSONAL".into(),
            "LOCUS_SUPABASE_PERSONAL".into(),
            "LOCUS_VERCEL_TOKEN_ACME".into(),
            "LOCUS_PROVIDER_TOKEN".into(),
            "LOCUS_DEMO_UNUSED".into(),
        ]);
        let mut parent = parent_env();
        for name in &names {
            parent.insert(OsString::from(name), "synthetic-scoped-credential".into());
        }
        let vars = broker_environment(&parent, Path::new("/private/locus"), &names).unwrap();
        for name in names {
            assert_eq!(
                vars.get(std::ffi::OsStr::new(&name)).unwrap(),
                "synthetic-scoped-credential"
            );
        }
        assert!(reserved_credential_name("LOCUS_GH_TOKEN_"));
        assert!(reserved_credential_name("LOCUS_SUPABASE_"));
        assert!(reserved_credential_name("LOCUS_VERCEL_TOKEN_"));
    }

    #[test]
    fn unknown_graph_grant_fails_preflight_without_pin_fallback() {
        let home = tempfile::tempdir().unwrap();
        let store = Store::open(home.path()).unwrap();
        let graph = SwarmGraph::from_slice(
            br#"{"version":1,"nodes":[{"id":"worker","grant_id":"0123456789abcdef","tools":[]}]}"#,
        )
        .unwrap();
        assert_eq!(
            credential_names(&store, &graph).unwrap_err().to_string(),
            "swarm_grant_unavailable"
        );
    }
}

//! Per-command `--dry-run` plans (`locus.plan.v1`, see `dry_run`).
//!
//! Each builder runs the read-only half of its command — the same
//! resolution, allowlist and refusal checks — and describes the writes the
//! real run would make. None opens a store that does not exist yet, starts
//! the authority broker, mints anything, or prints a secret.

use super::*;

/// The control-capability step of init/quickstart, planned without minting.
/// Mirrors [`bootstrap_control_capability`]; never reads or prints a value.
pub(super) fn plan_capability_bootstrap(plan: &mut dry_run::Plan, home: &Path, persist: bool) {
    if env::var_os(locus_core::CONTROL_CAPABILITY_ENV).is_some() {
        return;
    }
    let file = locus_core::control_capability_file(home);
    if file.exists() {
        plan.note(format!(
            "adopts the persisted control capability from {}",
            file.display()
        ));
    } else if persist {
        plan.change(
            "mint",
            file.display().to_string(),
            Some("control capability, 0600".into()),
        );
    } else {
        plan.change(
            "mint",
            locus_core::CONTROL_CAPABILITY_ENV,
            Some("env-only control capability; the real run prints its export line".into()),
        );
    }
}

/// `--dry-run` for `locus init`: never opens (and so never creates) the store.
pub(super) fn plan_init(
    command: &str,
    home: &Path,
    with_samples: bool,
    persist: bool,
) -> dry_run::Plan {
    let mut plan = dry_run::Plan::new(command);
    if !home.join("daemon.key").exists() {
        plan.change(
            "create",
            home.display().to_string(),
            Some("locus home + seal key (daemon.key)".into()),
        );
    }
    plan_capability_bootstrap(&mut plan, home, persist);
    let config = home.join("config.toml");
    if !config.exists() {
        plan.change(
            "create",
            config.display().to_string(),
            Some("notify.enabled=false".into()),
        );
    }
    if with_samples {
        for alias in ["personal", "acme"] {
            plan.write_file(
                &home.join("bindings").join(format!("{alias}.toml")),
                Some(format!("sample binding {alias}")),
            );
        }
    }
    plan.needs_control()
        .with("home", home.display().to_string())
        .with("samples", with_samples);
    plan
}

/// `--dry-run` for `locus quickstart`: init (samples only when there are no
/// bindings), then the pin quickstart would make, resolved read-only.
pub(super) fn plan_quickstart(persist_capability: bool, json: bool) -> Result<()> {
    let home = locus_core::locus_home()?;
    let initialized = home.join("daemon.key").exists();
    let s = if initialized {
        Some(dry_run::existing_store()?)
    } else {
        None
    };
    let had_bindings = match &s {
        Some(s) => !s.list_bindings()?.is_empty(),
        None => false,
    };
    let mut plan = plan_init("locus quickstart", &home, !had_bindings, persist_capability);
    let here = cwd();
    let pinned = s
        .as_ref()
        .map(|s| s.active_session().ok().flatten().is_some())
        .unwrap_or(false);
    if pinned {
        plan.note("already pinned — quickstart leaves the pin as is");
    } else {
        // Same choice as cmd_quickstart: workspace default, else `personal`,
        // else the first binding (samples count once written).
        let target = match find_workspace(&here)? {
            Some((_, cfg)) if cfg.default_binding.is_some() || !cfg.allowed_bindings.is_empty() => {
                match &s {
                    Some(s) => s.resolve_auto_pin(&here).ok().map(|t| t.alias),
                    None => cfg.default_binding.clone(),
                }
            }
            Some(_) => None,
            None => match &s {
                Some(s) if had_bindings => {
                    if s.load_binding("personal").is_ok() {
                        Some("personal".to_string())
                    } else {
                        s.list_bindings()?.into_iter().next().map(|b| b.alias)
                    }
                }
                _ => Some("personal".to_string()),
            },
        };
        match target {
            Some(alias) => {
                plan.change("pin", alias.clone(), None);
                plan.change(
                    "create",
                    home.join("sessions")
                        .join("active.json")
                        .display()
                        .to_string(),
                    Some("sealed session".into()),
                );
                plan.with("pin", alias);
            }
            None => {
                plan.note("no pin target — pin with `locus enter <alias>` afterwards");
            }
        }
    }
    plan.note("then prints whoami + doctor (read-only)");
    plan.emit(json)
}

/// `--dry-run` for `enter` / `pin` / `switch`: resolve the target through
/// [`Store::resolve_pin`] (the real pin's binding, allowlist, namespace and
/// TTL logic) and describe the session swap. Mints nothing.
pub(super) fn plan_pin(
    command: &str,
    alias: Option<&str>,
    namespaces: Option<Vec<String>>,
    force: bool,
    requested_ttl: Option<chrono::Duration>,
    json: bool,
) -> Result<()> {
    let s = dry_run::existing_store()?;
    let cwd = cwd();
    let (alias, force, source) = match alias {
        Some(a) => (a.to_string(), force, "explicit".to_string()),
        None => {
            let target = s
                .resolve_auto_pin(&cwd)
                .map_err(|e| with_auto_alias_suggestions(&s, &cwd, e))?;
            // Autopin never uses --force (same as Store::pin_auto_with_ttl).
            let force = force && !matches!(target.source, locus_core::PinSource::Autopin { .. });
            (target.alias, force, target.reason)
        }
    };
    let r = s
        .resolve_pin(&alias, &cwd, force, namespaces, requested_ttl)
        .map_err(|e| with_alias_suggestions(&s, e))?;
    // Pin replaces active.json, never a LOCUS_SESSION_ID session.
    // A malformed or unreadable active.json would make the real pin fail
    // before writing; surface the same recovery guidance as `leave`.
    let current = s
        .active_pin_session()
        .map_err(|e| wedged_session_recovery(anyhow::Error::new(e)))?;
    let ttl_capped = requested_ttl.is_some_and(|req| r.ttl < req);

    let mut plan = dry_run::Plan::new(format!("{command} {}", r.binding.alias));
    if let Some(cur) = &current {
        plan.change(
            "unpin",
            cur.binding_alias.clone(),
            Some(format!("session {} ends", cur.session_id)),
        );
    }
    let mut detail = format!(
        "tenant={} ttl={} ({}) providers={}",
        r.binding.tenant,
        human_dur(r.ttl),
        r.ttl_source,
        r.binding.providers.len()
    );
    if !r.namespaces.is_empty() {
        detail.push_str(&format!(" namespaces={}", r.namespaces.join(",")));
    }
    plan.change("pin", r.binding.alias.clone(), Some(detail));
    plan.write_file(&s.active_session_path(), Some("sealed session".into()));
    if ttl_capped {
        plan.note(format!(
            "requested ttl capped to {} by policy.max_ttl",
            human_dur(r.ttl)
        ));
    }
    plan.needs_control()
        .with("binding", &r.binding.alias)
        .with("tenant", &r.binding.tenant)
        .with("source", source)
        .with("namespaces", &r.namespaces)
        .with("providers", r.binding.providers.len())
        .with("ttl_secs", r.ttl.num_seconds())
        .with("ttl_source", r.ttl_source)
        .with("ttl_capped", ttl_capped)
        .with(
            "replaces",
            current.as_ref().map(|c| c.binding_alias.clone()),
        )
        .with(
            "workspace",
            r.workspace.as_ref().map(|(p, _)| p.display().to_string()),
        );
    plan.emit(json)
}

/// `--dry-run` for `leave` (and `leave --force`): what would be cleared.
pub(super) fn plan_leave(force: bool, json: bool) -> Result<()> {
    let s = dry_run::existing_store()?;
    let path = s.active_session_path();
    let mut plan = dry_run::Plan::new(if force {
        "locus leave --force"
    } else {
        "locus leave"
    });
    if path.exists() {
        // A wedged session may not parse or verify; --force still clears it.
        match s.active_pin_session() {
            Ok(Some(cur)) => {
                plan.change(
                    "unpin",
                    cur.binding_alias.clone(),
                    Some(format!("session {} ends", cur.session_id)),
                );
                plan.with("binding", cur.binding_alias.clone());
            }
            Ok(None) => {}
            Err(e) if force => {
                plan.note(format!("active session is wedged ({e}); --force clears it"));
            }
            Err(e) => return Err(wedged_session_recovery(anyhow::Error::new(e))),
        }
        plan.delete_file(&path, Some("active session".into()));
    } else {
        plan.note("no active pin — already clear");
    }
    plan.needs_control().emit(json)
}

/// `--dry-run` for `locus renew`.
pub(super) fn plan_renew(json: bool) -> Result<()> {
    let s = dry_run::existing_store()?;
    let path = s.active_session_path();
    let mut plan = dry_run::Plan::new("locus renew");
    // Mirror the real command's local refusals (no broker, no capability).
    if std::env::var("LOCUS_SESSION_ID").is_ok_and(|v| !v.trim().is_empty()) {
        bail!("locus renew only renews the active.json pin; unset LOCUS_SESSION_ID");
    }
    match s.active_pin_session() {
        Ok(Some(cur)) => {
            if cur.is_expired() {
                bail!(
                    "pin expired at {}; renew refused, human re-pin required (`locus enter {}`)",
                    cur.expires_at.to_rfc3339(),
                    cur.binding_alias
                );
            }
            if cur.frozen {
                bail!("pin is frozen; renew refused, human re-pin required");
            }
            if cur.authority != locus_core::SessionAuthority::LocalControl {
                bail!("renew requires an authenticated local-control session");
            }
            let binding = s.load_binding(&cur.binding_alias)?;
            locus_core::renew_identity_check(&s, &cur, &binding)?;
            let (new_expiry, _, capped_by) = locus_core::renew_bounds(&cur, &binding)?;
            plan.with("expires_at", new_expiry.to_rfc3339());
            plan.with("capped_by", capped_by.to_string());
            plan.change(
                "renew",
                cur.binding_alias.clone(),
                Some(format!(
                    "session {} expiry extended within policy.max_ttl and policy.max_session_lifetime",
                    cur.session_id
                )),
            );
            plan.with("binding", cur.binding_alias.clone());
            plan.write_file(&path, Some("active session (new expiry)".into()));
        }
        Ok(None) => {
            plan.note("no active pin to renew");
        }
        Err(e) => return Err(wedged_session_recovery(anyhow::Error::new(e))),
    }
    plan.needs_control().emit(json)
}

/// `--dry-run` for `graph export` / `graph import`.
pub(super) fn plan_graph(sub: GraphCmd, json: bool) -> Result<()> {
    let s = dry_run::existing_store()?;
    match sub {
        GraphCmd::List => unreachable!("graph list is read-only"),
        GraphCmd::Export { bindings, out } => {
            let out_path = out.unwrap_or_else(|| PathBuf::from(default_export_filename()));
            let (selected, workspaces) = s.graph_export_selection(bindings.as_deref())?;
            let aliases: Vec<_> = selected.iter().map(|b| b.alias.clone()).collect();
            let names: Vec<_> = workspaces.iter().map(|w| w.name.clone()).collect();
            let mut plan = dry_run::Plan::new("locus graph export");
            plan.write_file(
                &out_path,
                Some(format!(
                    "encrypted graph: {} binding(s), {} workspace(s); no secrets",
                    aliases.len(),
                    names.len()
                )),
            )
            .needs_control()
            .with("path", out_path.display().to_string())
            .with("bindings", aliases)
            .with("workspaces", names);
            if env::var_os(locus_core::GRAPH_PASSPHRASE_ENV).is_none() {
                plan.note(format!(
                    "the real run needs {} (or an interactive prompt)",
                    locus_core::GRAPH_PASSPHRASE_ENV
                ));
            }
            plan.emit(json)
        }
        GraphCmd::Import { path, force } => {
            // Decrypts (read-only) so a wrong passphrase fails like the real run.
            let passphrase = graph_passphrase(json)?;
            let preview = s
                .graph_import_preview(&path, passphrase.as_str(), force)
                .context("graph import")?;
            let mut plan = dry_run::Plan::new(format!("locus graph import {}", path.display()));
            for alias in &preview.bindings_imported {
                plan.write_file(
                    &s.bindings_dir().join(format!("{alias}.toml")),
                    Some(format!("binding {alias}")),
                );
            }
            for name in &preview.workspaces_imported {
                plan.write_file(
                    &s.home().join("workspaces").join(format!("{name}.toml")),
                    Some(format!("workspace template {name}")),
                );
            }
            if !preview.bindings_skipped.is_empty() || !preview.workspaces_skipped.is_empty() {
                plan.note("existing entries are skipped (use --force to overwrite)");
            }
            plan.needs_control()
                .with("bindings_imported", &preview.bindings_imported)
                .with("bindings_skipped", &preview.bindings_skipped)
                .with("workspaces_imported", &preview.workspaces_imported)
                .with("workspaces_skipped", &preview.workspaces_skipped);
            plan.emit(json)
        }
    }
}

/// `--dry-run` for `engagement init` / `engagement close`.
pub(super) fn plan_engagement(sub: EngagementCmd, json: bool) -> Result<()> {
    let s = dry_run::existing_store()?;
    match sub {
        EngagementCmd::Init {
            alias,
            tenant,
            workspace,
            no_readme,
            force,
        } => {
            let p =
                s.engagement_init_preflight(&alias, &tenant, &cwd(), workspace, !no_readme, force)?;
            let mut plan = dry_run::Plan::new(format!("locus engagement init {alias}"));
            plan.write_file(
                &p.binding_path,
                Some(format!("binding template, tenant={tenant}")),
            )
            .write_file(&p.meta_path, Some("engagement meta".into()));
            if let Some(w) = &p.workspace_path {
                plan.write_file(w, Some(format!("default_binding={alias}, require_pin")));
            }
            if let Some(r) = &p.readme_path {
                plan.write_file(r, Some("engagement README".into()));
            }
            plan.needs_control()
                .with("alias", &alias)
                .with("tenant", &tenant)
                .emit(json)
        }
        EngagementCmd::Close { alias, archive } => {
            validate_name_component("alias", &alias)?;
            let binding = s.load_binding(&alias).ok();
            let meta_path = s.home().join("engagements").join(format!("{alias}.json"));
            if binding.is_none() && !meta_path.exists() {
                return Err(LocusError::BindingNotFound(alias).into());
            }
            let mut plan = dry_run::Plan::new(format!("locus engagement close {alias}"));
            if let Some(cur) = s.active_session().ok().flatten() {
                if cur.binding_alias == alias {
                    plan.change(
                        "unpin",
                        alias.clone(),
                        Some(format!("session {} ends", cur.session_id)),
                    );
                    plan.delete_file(&s.active_session_path(), Some("active session".into()));
                }
            }
            if archive {
                let date = chrono::Utc::now().format("%Y%m%d");
                plan.write_file(
                    &s.home()
                        .join("archives")
                        .join(format!("{alias}-{date}.jsonl")),
                    Some("audit events for this binding".into()),
                );
            }
            plan.write_file(&meta_path, Some("mark closed_at".into()))
                .note("the binding file and vault secrets are kept")
                .needs_control()
                .with("alias", &alias)
                .emit(json)
        }
    }
}

/// `--dry-run` for `ci mint` / `ci env`: the binding, allowlist and TTL cap
/// through [`Store::resolve_pin`]; no session, no executor capability.
pub(super) fn plan_ci_mint(
    command: &str,
    binding: &str,
    ttl: &str,
    force: bool,
    resolve: bool,
    json: bool,
) -> Result<()> {
    let s = dry_run::existing_store()?;
    let ttl_dur = parse_mint_ttl(ttl)?;
    let r = s
        .resolve_pin(binding, &cwd(), force, None, Some(ttl_dur))
        .map_err(|e| with_alias_suggestions(&s, e))?;
    let mut plan = dry_run::Plan::new(format!("{command} -b {}", r.binding.alias));
    plan.change(
        "mint",
        s.home()
            .join("sessions")
            .join("ci-<id>.json")
            .display()
            .to_string(),
        Some(format!(
            "sealed CI session, tenant={} ttl={}; active pin unchanged",
            r.binding.tenant,
            human_dur(r.ttl)
        )),
    )
    .change(
        "grant",
        locus_core::EXECUTOR_CAPABILITY_ENV,
        Some("executor capability for the CI session".into()),
    );
    if resolve {
        plan.note(if ci_secrets_allowed() {
            "--resolve: the real run includes resolved secrets in its output"
        } else {
            "--resolve ignored without LOCUS_CI_ALLOW_SECRETS=1"
        });
    }
    plan.needs_control()
        .with("binding", &r.binding.alias)
        .with("tenant", &r.binding.tenant)
        .with("ttl_secs", r.ttl.num_seconds())
        .emit(json)
}

/// `--dry-run` for `mcp mint`: binding, allowlist and TTL through
/// [`Store::resolve_pin`]. No grant, no token.
pub(super) fn plan_mcp_mint(
    binding: &str,
    ttl: &str,
    label: Option<String>,
    force: bool,
    json: bool,
) -> Result<()> {
    let s = dry_run::existing_store()?;
    let ttl_dur = parse_mint_ttl(ttl)?;
    let r = s
        .resolve_pin(binding, &cwd(), force, None, Some(ttl_dur))
        .map_err(|e| with_alias_suggestions(&s, e))?;
    let mut plan = dry_run::Plan::new(format!("locus mcp mint -b {}", r.binding.alias));
    plan.change(
        "mint",
        format!("mcp grant for {}", r.binding.alias),
        Some(format!(
            "tenant={} ttl={}; the real run prints its bearer token once",
            r.binding.tenant,
            human_dur(r.ttl)
        )),
    )
    .needs_control()
    .with("binding", &r.binding.alias)
    .with("tenant", &r.binding.tenant)
    .with("ttl_secs", r.ttl.num_seconds())
    .with("label", label);
    plan.emit(json)
}

/// `--dry-run` for `locus onboard`: detection (read-only, printed as notes)
/// plus the bindings the saved reviewed plan would write. Never prompts.
pub(super) fn plan_onboard(detect_only: bool, reset: bool, json: bool) -> Result<()> {
    let s = dry_run::existing_store()?;
    let plan_path = locus_core::OnboardPlan::plan_path(s.home());
    let user_home = env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| s.home().to_path_buf());
    let candidates = locus_core::detect_ambient_candidates(&user_home, &cwd());
    let mut plan = dry_run::Plan::new("locus onboard");
    plan.with(
        "candidates",
        candidates
            .iter()
            .map(|c| json!({"provider": c.provider, "source": c.source, "account_hint": c.account_hint}))
            .collect::<Vec<_>>(),
    );
    if detect_only {
        plan.note("--detect-only writes nothing");
        return plan.emit(json);
    }
    let saved = if reset {
        if plan_path.exists() {
            plan.delete_file(&plan_path, Some("saved refs-only onboard plan".into()));
        }
        locus_core::OnboardPlan::default()
    } else {
        locus_core::OnboardPlan::load(s.home())?
    };
    if saved.bindings.is_empty() {
        plan.note(
            "no reviewed bindings in the saved plan: the real run prompts on a TTY (or refuses)",
        );
    }
    for draft in &saved.bindings {
        draft.validate()?;
        validate_name_component("alias", &draft.alias)?;
        let binding = draft.clone().into_binding()?;
        let path = s.bindings_dir().join(format!("{}.toml", binding.alias));
        if !path.exists() {
            plan.change(
                "create",
                path.display().to_string(),
                Some(format!("binding {}", binding.alias)),
            );
        } else if s.load_binding(&binding.alias)? != binding {
            bail!("existing binding differs from plan; refusing overwrite");
        }
    }
    if !saved.bindings.is_empty() {
        plan.write_file(&plan_path, Some("mark onboarding steps done".into()));
    }
    plan.needs_control().emit(json)
}

/// `--dry-run` for the mutating adapter subcommands. Install still fetches
/// and verifies the signed manifest (read-only network), so a plan fails on
/// the same signature or trust errors as the real install.
pub(super) fn plan_adapter(sub: AdapterCmd, json: bool) -> Result<()> {
    let s = dry_run::existing_store()?;
    let home = s.home();
    let adapters_dir = home.join(locus_core::marketplace::ADAPTERS_DIR);
    match sub {
        AdapterCmd::Install { id, yes } => {
            let hit = community_search_in(home, Some(&id))?
                .into_iter()
                .find(|hit| hit.id == id)
                .ok_or_else(|| anyhow!("community adapter not found in configured indexes"))?;
            let keys = load_merged_trust_keys(home);
            if keys.is_empty() {
                bail!("install requires explicit trusted publisher trust keys (locus adapter trust add)");
            }
            let manifest = locus_core::marketplace::fetch_manifest(&hit)?;
            locus_core::marketplace::verify_community_manifest_with_keys(&manifest, &keys)?;
            let mut plan = dry_run::Plan::new(format!("locus adapter install {id}"));
            plan.write_file(
                &adapters_dir.join(format!("{id}.json")),
                Some(format!(
                    "community adapter {} {} (executes publisher-supplied code)",
                    manifest.entry.id, manifest.version
                )),
            )
            .needs_control()
            .with("id", &manifest.entry.id)
            .with("version", &manifest.version)
            .with("tools", &manifest.entry.tools);
            if !yes {
                plan.note("the real run needs --yes after reviewing the executable envelope");
            }
            plan.emit(json)
        }
        AdapterCmd::Uninstall { id, yes } => {
            let installed = locus_core::marketplace::installed_manifests(home)
                .into_iter()
                .any(|(_, m)| m.entry.id == id);
            let mut plan = dry_run::Plan::new(format!("locus adapter uninstall {id}"));
            if installed {
                plan.delete_file(
                    &adapters_dir.join(format!("{id}.json")),
                    Some(format!("community adapter {id}")),
                );
            } else {
                plan.note(format!("{id} is not installed — nothing to remove"));
            }
            if !yes {
                plan.note("the real run asks for confirmation (or pass --yes)");
            }
            plan.needs_control().with("id", &id).emit(json)
        }
        AdapterCmd::Trust(AdapterTrustCmd::Add { id, ed25519_pub }) => {
            let id = id.trim().to_string();
            if id.is_empty() || id.contains([':', ',', ';']) {
                bail!("trust key id must be non-empty and must not contain ':', ',', or ';'");
            }
            locus_core::RegistryTrustKey::ed25519_public(&id, ed25519_pub.trim())
                .ed25519_verifying_key()
                .with_context(|| format!("add trust key `{id}`"))?;
            let replaced = locus_core::list_trust_keys_file(home)?
                .iter()
                .any(|k| k.id.eq_ignore_ascii_case(&id));
            let path = adapter_trust_keys_path(home);
            let mut plan = dry_run::Plan::new(format!("locus adapter trust add --id {id}"));
            plan.change(
                if path.exists() { "update" } else { "create" },
                path.display().to_string(),
                Some(format!(
                    "{} ed25519 trust key `{id}`",
                    if replaced { "replace" } else { "add" }
                )),
            )
            .with("id", &id)
            .with("replaced", replaced);
            plan.emit(json)
        }
        AdapterCmd::Registry(AdapterRegistryCmd::Index(sub)) => {
            let sources = locus_core::marketplace::load_index_sources(home)?;
            let file = home.join(locus_core::marketplace::INDEXES_FILE);
            let mut plan = dry_run::Plan::new("locus adapter registry index");
            match sub {
                AdapterIndexCmd::Add { url, name } => {
                    // A plan makes no network call: the real run fetches the
                    // index (and takes its name from it when --name is absent).
                    locus_core::marketplace::check_marketplace_url(&url)?;
                    if let Some(name) = name.as_deref() {
                        validate_name_component("index name", name)?;
                    }
                    if sources
                        .iter()
                        .any(|src| src.url == url || name.as_deref() == Some(src.name.as_str()))
                    {
                        bail!("index name or URL is already registered");
                    }
                    let detail = match name.as_deref() {
                        Some(n) => format!("add index source {n}"),
                        None => "add index source (name from the fetched index)".to_string(),
                    };
                    plan.write_file(&file, Some(detail))
                        .with("name", name)
                        .with("url", redact_url(&url))
                        .note("--dry-run does not fetch the index; the real run fetches and verifies it");
                }
                AdapterIndexCmd::Remove { name } => {
                    if sources.iter().any(|src| src.name == name) {
                        plan.write_file(&file, Some(format!("remove index source {name}")));
                    } else {
                        plan.note(format!("no index source named {name} — nothing to remove"));
                    }
                    plan.with("name", name);
                }
                AdapterIndexCmd::List => unreachable!("index list is read-only"),
            }
            plan.needs_control().emit(json)
        }
        AdapterCmd::Registry(AdapterRegistryCmd::Export { out, sign, key, .. }) => {
            let manifest = build_release_manifest().context("build adapter release manifest")?;
            let mut plan = dry_run::Plan::new("locus adapter registry export");
            let detail = format!(
                "{} release manifest, {} adapters",
                if sign { "signed" } else { "unsigned" },
                manifest.adapters.len()
            );
            match &out {
                Some(path) => plan.write_file(path, Some(detail)),
                None => plan.change("print", "stdout", Some(detail)),
            };
            if sign
                && key.is_none()
                && env::var(LOCUS_REGISTRY_SIGNING_KEY_ENV).map_or(true, |v| v.trim().is_empty())
            {
                bail!(
                    "--sign requires a signing key: pass --key <file> or set \
                     {LOCUS_REGISTRY_SIGNING_KEY_ENV} (refusing to export without one)"
                );
            }
            plan.with("adapter_count", manifest.adapters.len())
                .with("signed", sign);
            plan.emit(json)
        }
        _ => unreachable!("read-only adapter subcommands do not plan"),
    }
}

/// `doctor --fix` fixes as a `locus.plan.v1` plan; `report` (the
/// `locus.doctor.fix.v1` body) rides along as `fix_report`.
pub(super) fn doctor_fix_plan(
    plan: &locus_core::doctor_fix::FixPlan,
    report: &serde_json::Value,
) -> dry_run::Plan {
    use locus_core::doctor_fix::PlannedFix;
    let mut out = dry_run::Plan::new("locus doctor --fix --apply");
    for fix in &plan.fixes {
        let target = match fix {
            PlannedFix::StaleAllowlistEntry { file, .. }
            | PlannedFix::StaleDefaultBinding { file, .. } => file.display().to_string(),
            PlannedFix::LegacyPhantomRef { alias, .. } => format!("binding {alias}"),
        };
        out.change("update", target, Some(fix.describe()));
    }
    if !plan.report_only.is_empty() {
        out.note(format!(
            "{} finding(s) need a manual look and are never changed automatically",
            plan.report_only.len()
        ));
    }
    if !report.is_null() {
        out.with("fix_report", report);
    }
    out.note("each changed file is backed up first");
    out
}

/// `binding add --dry-run` plan: refs-only TOML, never a secret value.
pub(super) fn binding_add_plan(
    path: &Path,
    alias: &str,
    toml: &str,
    exists: bool,
) -> dry_run::Plan {
    let mut plan = dry_run::Plan::new(format!("locus binding add {alias}"));
    plan.change(
        if exists { "overwrite" } else { "create" },
        path.display().to_string(),
        Some(format!("binding {alias}")),
    )
    .needs_control()
    .with("alias", alias)
    .with("toml", toml);
    plan
}

/// Human output keeps the TOML preview above the plan.
pub(super) fn emit_binding_add_plan(plan: &dry_run::Plan, json: bool, toml: &str) -> Result<()> {
    if json {
        return plan.emit(true);
    }
    println!("{toml}");
    plan.emit(false)?;
    // Last line kept from before `locus.plan.v1`: scripts match on it.
    println!("{} dry run — nothing written", "ok".green().bold());
    Ok(())
}

/// A URL safe to print in a plan: scheme, host, port and path only.
/// Userinfo (`user:token@`), the query and the fragment can carry
/// credentials, so they are dropped and marked.
pub(super) fn redact_url(raw: &str) -> String {
    let mut rest = raw;
    let mut marked = false;
    if let Some(i) = rest.find('#') {
        rest = &rest[..i];
        marked = true;
    }
    if let Some(i) = rest.find('?') {
        rest = &rest[..i];
        marked = true;
    }
    let (scheme, after) = match rest.find("://") {
        Some(i) => (&rest[..i + 3], &rest[i + 3..]),
        None => ("", rest),
    };
    let auth_end = after.find('/').unwrap_or(after.len());
    let (authority, path) = after.split_at(auth_end);
    let host = match authority.rfind('@') {
        Some(i) => {
            marked = true;
            &authority[i + 1..]
        }
        None => authority,
    };
    let mut out = format!("{scheme}{host}{path}");
    if marked {
        out.push_str(" (redacted)");
    }
    out
}

#[cfg(test)]
mod redact_tests {
    use super::redact_url;

    #[test]
    fn redact_url_strips_userinfo_query_and_fragment() {
        assert_eq!(
            redact_url("https://user:s3cret@idx.example.com:8443/a/index.json?token=t0k#frag"),
            "https://idx.example.com:8443/a/index.json (redacted)"
        );
        assert_eq!(
            redact_url("https://tok@idx.example.com/index.json"),
            "https://idx.example.com/index.json (redacted)"
        );
        assert_eq!(
            redact_url("https://idx.example.com/index.json"),
            "https://idx.example.com/index.json"
        );
        let r = redact_url("https://a:b@h/p?q=s3cret#s3cret");
        assert!(!r.contains("s3cret") && !r.contains("a:b"), "{r}");
    }
}

//! Read-only launch-gate snapshot, never a grant or an upstream auth probe.
use locus_core::{locus_home, recovery, Store};
use serde::Serialize;

#[derive(Serialize)]
struct Preflight {
    schema: &'static str,
    exec_ready: bool,
    reason: &'static str,
    human_command: Option<String>,
}

pub(crate) fn run() -> anyhow::Result<()> {
    let mut report = Preflight {
        schema: "locus.fleet_preflight.v1",
        exec_ready: false,
        reason: "uninitialized",
        human_command: Some("locus init".into()),
    };
    if let Ok(store) = locus_home().and_then(Store::open_existing) {
        observe(&store, &mut report);
    }
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}

fn observe(store: &Store, report: &mut Preflight) {
    report.reason = "session_invalid";
    report.human_command = Some(recovery::stale_pin_recovery(None));
    let resolved = match store.resolve_active_session() {
        Ok(Some(resolved)) => resolved,
        Ok(None) if !super::exact_session_selected() => {
            report.reason = "unpinned";
            report.human_command = Some(format!(
                "{}; locus enter",
                recovery::user_hook_eval_command()
            ));
            return;
        }
        _ => return,
    };
    let alias = Some(resolved.session.binding_alias.as_str());
    report.human_command = Some(recovery::stale_pin_recovery(alias));
    if let Err(error) = store.require_active_resolved() {
        report.reason = if matches!(error, locus_core::LocusError::SessionExpired(_)) {
            "expired"
        } else {
            "session_invalid"
        };
        report.human_command = Some(
            if matches!(
                error,
                locus_core::LocusError::ExecutorAuthorityUnavailable(_)
            ) {
                recovery::user_hook_eval_command()
            } else {
                recovery::recovery_for_error(&error, alias)
            },
        );
        return;
    }
    // Delegated exec has additional grants/budgets; never claim those are ready.
    if resolved.session.is_delegated() {
        report.reason = "operator_pin_required";
        return;
    }
    if !store.verify_runtime_for(&resolved).is_ok_and(|d| d.ok) {
        report.reason = "drift";
        return;
    }
    let Ok(binding) = store.load_binding(&resolved.session.binding_alias) else {
        return;
    };
    report.human_command = Some("locus doctor".into());
    if super::preflight_child_launch(&binding, true, super::ChildLaunchSurface::Exec).is_err() {
        report.reason = "credential_unsupported";
        return;
    }
    if !binding.providers.iter().any(|p| {
        locus_core::adapters::adapter_for(&p.provider)
            .is_some_and(|adapter| adapter.cli_name() == Some("gh"))
    }) {
        report.reason = "github_missing";
        return;
    }
    if !binding
        .providers
        .iter()
        .all(|p| locus_core::credential::credential_metadata(&p.credential_ref).present)
    {
        report.reason = "credential_unavailable";
        return;
    }
    if !std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| {
            let Ok(meta) = dir.join("gh").metadata() else {
                return false;
            };
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                meta.is_file() && meta.permissions().mode() & 0o111 != 0
            }
            #[cfg(not(unix))]
            {
                meta.is_file()
            }
        })
    }) {
        report.reason = "gh_missing";
        report.human_command = Some("locus doctor".into());
        return;
    }
    report.exec_ready = true;
    report.reason = "ready";
    report.human_command = None;
}

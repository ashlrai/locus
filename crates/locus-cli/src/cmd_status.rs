//! `locus status`: `locus.status.v1` JSON and the existing human output.

use super::{
    cwd, exit, json_out, pin_time_left, status_pin_check, store, wedged_session_recovery,
    PIN_EXPIRY_WARN_SECS,
};
use anyhow::Result;
use colored::Colorize;
use locus_core::find_workspace;
use serde_json::json;

pub(super) fn run(oneline: bool, json: bool) -> Result<()> {
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
                json_out::print_line(
                    json_out::STATUS,
                    &json!({
                        "pinned": false,
                        "require_pin": require_pin,
                    }),
                )?;
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
            let check = status_pin_check(&s, &session, seal_ok);
            let ok = seal_ok && check.usable && !frozen && !session.is_expired();
            if json {
                json_out::print_line(
                    json_out::STATUS,
                    &json!({
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
                        "authority_verified": check.verified,
                        "expires_at": session.expires_at.to_rfc3339(),
                        "expires_in_secs": (session.expires_at - chrono::Utc::now()).num_seconds().max(0),
                        "ok": ok,
                        "error": check.error,
                        "recovery": check.recovery,
                    }),
                )?;
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
                } else if ok && !check.verified {
                    "UNVERIFIED".yellow()
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
                if !frozen {
                    let left = (session.expires_at - chrono::Utc::now()).num_seconds();
                    let line = format!("  expires  {}", pin_time_left(left));
                    if left < PIN_EXPIRY_WARN_SECS {
                        println!(
                            "{}  {}",
                            line.yellow(),
                            format!("(renew: locus enter {})", session.binding_alias).dimmed()
                        );
                    } else {
                        println!("{}", line.dimmed());
                    }
                }
                if frozen {
                    println!(
                        "  session_frozen: re-pin ({})",
                        session.frozen_reason.as_deref().unwrap_or("binding_drift")
                    );
                }
                if let Some(error) = &check.error {
                    let label = if ok { "note " } else { "error" };
                    println!("  {label}    {error}");
                }
                if let Some(recovery) = &check.recovery {
                    println!("  recover  {}", recovery.cyan());
                }
            }
            if !ok {
                exit::ExitCode::Unsafe.exit();
            }
        }
    }
    Ok(())
}

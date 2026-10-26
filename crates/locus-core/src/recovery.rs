//! Operator recovery recipes for a pin that can no longer be verified.
//!
//! A pin's authority broker is supervised by the process that pinned, so a
//! new shell (or a crashed one) leaves a sealed session behind that no
//! command can validate. Every surface that detects this (`enter`, `whoami`,
//! `status`, `agent report`, `doctor`) names a human-only recovery, picked
//! from what [`crate::Store::require_local_control_to_replace_active`]
//! actually accepts:
//!
//! - Dead broker (authority anchor unavailable or mismatched, seal intact):
//!   `locus enter` replaces the dead pin directly, so the hint is the hook
//!   plus `locus enter`, with `locus leave --force` only as a fallback:
//!
//!   ```text
//!   eval "$(locus hook bash)"; locus enter <alias> || { locus leave --force && locus enter <alias>; }
//!   ```
//!
//! - Everything else that enter still refuses (expired TTL, invalid or legacy
//!   seal, unreadable session file): forced teardown first:
//!
//!   ```text
//!   eval "$(locus hook bash)"; locus leave --force; locus enter <alias>
//!   ```
//!
//! These are message helpers only: they never touch session state.

use crate::LocusError;

/// Shell used for the `locus hook` step when `$SHELL` names none we support.
const DEFAULT_HOOK_SHELL: &str = "bash";

/// Hook shell for `shell_path` (a `$SHELL` value): zsh, fish, else bash.
pub fn hook_shell(shell_path: Option<&str>) -> &'static str {
    let name = shell_path
        .and_then(|p| p.rsplit('/').next())
        .unwrap_or_default();
    match name {
        "zsh" => "zsh",
        "fish" => "fish",
        _ => DEFAULT_HOOK_SHELL,
    }
}

/// The command that exports `LOCUS_CONTROL_CAPABILITY` in `shell`.
pub fn hook_eval_command(shell: &str) -> String {
    if shell == "fish" {
        "locus hook fish | source".to_string()
    } else {
        format!("eval \"$(locus hook {shell})\"")
    }
}

/// The command that exports `LOCUS_CONTROL_CAPABILITY` in the user's `$SHELL`
/// (zsh / fish / else bash). Every next-step hint names this, not a fixed shell.
pub fn user_hook_eval_command() -> String {
    let shell = std::env::var("SHELL").ok();
    hook_eval_command(hook_shell(shell.as_deref()))
}

/// One-line recovery for a stale or unverifiable pin, for an explicit shell.
///
/// `alias` usually comes from a session file that failed verification, so it
/// is used only when it is a plain alias-shaped name; anything else (empty,
/// overlong, shell metacharacters) falls back to `<alias>`.
pub fn stale_pin_recovery_for_shell(alias: Option<&str>, shell: &str) -> String {
    format!(
        "{}; locus leave --force; locus enter {}",
        hook_eval_command(shell),
        alias.filter(|a| is_plain_alias(a)).unwrap_or("<alias>")
    )
}

/// One-line recovery for a pin whose broker died (authority anchor
/// unavailable or mismatched) while its seal still verifies. Since #156,
/// `locus enter <alias>` replaces such a pin on its own (audited as
/// `session.replace_stale`); `locus leave --force` runs only if enter is
/// refused. The line is valid shell for `shell`, so it can be run verbatim.
pub fn dead_broker_recovery_for_shell(alias: Option<&str>, shell: &str) -> String {
    let alias = alias.filter(|a| is_plain_alias(a)).unwrap_or("<alias>");
    let hook = hook_eval_command(shell);
    if shell == "fish" {
        format!(
            "{hook}; locus enter {alias}; or begin; locus leave --force; and locus enter {alias}; end"
        )
    } else {
        format!("{hook}; locus enter {alias} || {{ locus leave --force && locus enter {alias}; }}")
    }
}

/// [`dead_broker_recovery_for_shell`] for the user's `$SHELL`.
pub fn dead_broker_recovery(alias: Option<&str>) -> String {
    let shell = std::env::var("SHELL").ok();
    dead_broker_recovery_for_shell(alias, hook_shell(shell.as_deref()))
}

/// True when `locus enter` can replace the active pin despite `err`: only a
/// dead or stale broker generation (anchor unavailable or mismatched) is
/// tolerated by `require_local_control_to_replace_active`. Expiry, a bad or
/// legacy seal and an unreadable session file are still refused by enter.
pub fn enter_replaces_stale_pin(err: &LocusError) -> bool {
    matches!(
        err,
        LocusError::AuthorityAnchorUnavailable(_) | LocusError::AuthorityAnchorMismatch
    )
}

/// The recovery line for a stale-pin `err`: [`dead_broker_recovery`] when
/// enter can replace the pin, else [`stale_pin_recovery`] (force first).
pub fn recovery_for_error(err: &LocusError, alias: Option<&str>) -> String {
    if enter_replaces_stale_pin(err) {
        dead_broker_recovery(alias)
    } else {
        stale_pin_recovery(alias)
    }
}

fn is_plain_alias(alias: &str) -> bool {
    !alias.is_empty()
        && alias.len() <= 64
        && alias
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// One-line recovery for a stale or unverifiable pin, for the user's `$SHELL`.
pub fn stale_pin_recovery(alias: Option<&str>) -> String {
    let shell = std::env::var("SHELL").ok();
    stale_pin_recovery_for_shell(alias, hook_shell(shell.as_deref()))
}

/// True for errors that mean the active pin exists but can no longer be
/// verified or used (dead broker, stale generation, bad or legacy seal,
/// unreadable session file, expired TTL) — the cases [`stale_pin_recovery`]
/// fixes. An expired pin refuses a plain `leave` and a new `enter` alike, so
/// it needs the same forced teardown.
pub fn is_stale_pin_error(err: &LocusError) -> bool {
    matches!(
        err,
        LocusError::InvalidSeal
            | LocusError::LegacySessionSeal
            | LocusError::AuthorityAnchorUnavailable(_)
            | LocusError::AuthorityAnchorMismatch
            | LocusError::SessionExpired(_)
            | LocusError::Json(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_shell_from_shell_path() {
        assert_eq!(hook_shell(Some("/bin/zsh")), "zsh");
        assert_eq!(hook_shell(Some("/usr/local/bin/fish")), "fish");
        assert_eq!(hook_shell(Some("/usr/bin/bash")), "bash");
        assert_eq!(hook_shell(Some("/bin/sh")), "bash");
        assert_eq!(hook_shell(None), "bash");
    }

    #[test]
    fn recovery_names_hook_force_leave_and_enter() {
        assert_eq!(
            stale_pin_recovery_for_shell(Some("personal"), "bash"),
            "eval \"$(locus hook bash)\"; locus leave --force; locus enter personal"
        );
        assert_eq!(
            stale_pin_recovery_for_shell(None, "fish"),
            "locus hook fish | source; locus leave --force; locus enter <alias>"
        );
    }

    #[test]
    fn dead_broker_recovery_names_enter_first() {
        assert_eq!(
            dead_broker_recovery_for_shell(Some("personal"), "bash"),
            "eval \"$(locus hook bash)\"; locus enter personal || \
             { locus leave --force && locus enter personal; }"
        );
        assert_eq!(
            dead_broker_recovery_for_shell(None, "fish"),
            "locus hook fish | source; locus enter <alias>; or begin; \
             locus leave --force; and locus enter <alias>; end"
        );
        assert_eq!(
            dead_broker_recovery_for_shell(Some("$(id)"), "zsh"),
            "eval \"$(locus hook zsh)\"; locus enter <alias> || \
             { locus leave --force && locus enter <alias>; }"
        );
    }

    #[test]
    fn only_dead_broker_errors_skip_force_first() {
        assert!(enter_replaces_stale_pin(
            &LocusError::AuthorityAnchorUnavailable("x".into())
        ));
        assert!(enter_replaces_stale_pin(
            &LocusError::AuthorityAnchorMismatch
        ));
        for e in [
            LocusError::InvalidSeal,
            LocusError::LegacySessionSeal,
            LocusError::SessionExpired("2026-10-16T00:00:00Z".into()),
        ] {
            assert!(!enter_replaces_stale_pin(&e), "{e:?}");
            assert!(
                recovery_for_error(&e, Some("a")).contains("locus leave --force; locus enter a")
            );
        }
    }

    #[test]
    fn untrusted_alias_never_reaches_the_recipe() {
        for bad in ["", "a; rm -rf ~", "$(id)", "a b", &"x".repeat(65)] {
            assert!(
                stale_pin_recovery_for_shell(Some(bad), "bash").ends_with("locus enter <alias>"),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn stale_pin_errors_are_classified() {
        assert!(is_stale_pin_error(&LocusError::AuthorityAnchorUnavailable(
            "x".into()
        )));
        assert!(is_stale_pin_error(&LocusError::AuthorityAnchorMismatch));
        assert!(is_stale_pin_error(&LocusError::InvalidSeal));
        assert!(is_stale_pin_error(&LocusError::SessionExpired(
            "2026-10-16T00:00:00Z".into()
        )));
        assert!(!is_stale_pin_error(&LocusError::NotPinned));
        assert!(!is_stale_pin_error(
            &LocusError::ExecutorAuthorityUnavailable("no capability".into())
        ));
    }
}

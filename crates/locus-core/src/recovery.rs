//! Operator recovery recipes for a pin that can no longer be verified.
//!
//! A pin's authority broker is supervised by the process that pinned, so a
//! new shell (or a crashed one) leaves a sealed session behind that no
//! command can validate. Every surface that detects this (`enter`, `whoami`,
//! `status`, `agent report`) names the same human-only recovery:
//!
//! ```text
//! eval "$(locus hook bash)"; locus leave --force; locus enter <alias>
//! ```
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

/// One-line recovery for a stale or unverifiable pin, for an explicit shell.
pub fn stale_pin_recovery_for_shell(alias: Option<&str>, shell: &str) -> String {
    format!(
        "{}; locus leave --force; locus enter {}",
        hook_eval_command(shell),
        alias.unwrap_or("<alias>")
    )
}

/// One-line recovery for a stale or unverifiable pin, for the user's `$SHELL`.
pub fn stale_pin_recovery(alias: Option<&str>) -> String {
    let shell = std::env::var("SHELL").ok();
    stale_pin_recovery_for_shell(alias, hook_shell(shell.as_deref()))
}

/// True for errors that mean the active pin exists but can no longer be
/// verified (dead broker, stale generation, bad or legacy seal, unreadable
/// session file) — the cases [`stale_pin_recovery`] fixes.
pub fn is_stale_pin_error(err: &LocusError) -> bool {
    matches!(
        err,
        LocusError::InvalidSeal
            | LocusError::LegacySessionSeal
            | LocusError::AuthorityAnchorUnavailable(_)
            | LocusError::AuthorityAnchorMismatch
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
    fn stale_pin_errors_are_classified() {
        assert!(is_stale_pin_error(&LocusError::AuthorityAnchorUnavailable(
            "x".into()
        )));
        assert!(is_stale_pin_error(&LocusError::AuthorityAnchorMismatch));
        assert!(is_stale_pin_error(&LocusError::InvalidSeal));
        assert!(!is_stale_pin_error(&LocusError::NotPinned));
        assert!(!is_stale_pin_error(
            &LocusError::ExecutorAuthorityUnavailable("no capability".into())
        ));
    }
}

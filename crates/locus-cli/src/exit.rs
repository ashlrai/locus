//! Process exit codes: the one table every `locus` command maps to.
//!
//! Documented in `docs/agent-contract.md` and `docs/exit-codes.md` (tests keep
//! the tables in sync). Values are stable; a new meaning gets a new variant, never a
//! reused number. `exec`, `run`, `ci run` and `mcp --supervised` pass the
//! child's own exit code through instead (`128 + signal` when a signal killed
//! it), so for those commands only 126/127 come from this table.

use locus_core::{AgentStatus, DoctorVerdict};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExitCode {
    /// Success. Check commands: healthy (doctor SAFE, agent `ready`,
    /// session ok). A `--dry-run` plan whose preconditions hold.
    Success,
    /// Command error or refusal (bad seal, unknown alias, policy deny,
    /// missing control capability, IO). Check commands: degraded (doctor
    /// WARN, agent `protected`, `verify session` / `watch` not ok).
    Failure,
    /// Check commands: unsafe (doctor UNSAFE, agent `unsafe`, `status` pin
    /// invalid, frozen or expired).
    Unsafe,
    /// Usage error: unknown flag or subcommand, bad value, conflicting flags,
    /// `--dry-run` on a command that cannot plan. Shares 2 with [`Self::Unsafe`];
    /// a usage error prints no report on stdout, only the error on stderr.
    Usage,
    /// The child program exists but could not be executed.
    CannotExecute,
    /// The child program was not found.
    NotFound,
    /// The stdout reader went away (`locus doctor | head -1`): 128 + SIGPIPE.
    StdoutClosed,
    /// An action was explicitly refused by policy or an authority gate.
    Refused,
    /// The operator must establish a fresh pin before retrying.
    RepinRequired,
    /// The relevant pin, credential, or authorization has expired.
    Expired,
    /// Local Locus configuration is invalid or incomplete.
    Misconfig,
}

impl ExitCode {
    /// Every variant, in table order (docs + tests iterate this).
    pub(crate) const ALL: [ExitCode; 11] = [
        Self::Success,
        Self::Failure,
        Self::Unsafe,
        Self::Usage,
        Self::Refused,
        Self::RepinRequired,
        Self::Expired,
        Self::Misconfig,
        Self::CannotExecute,
        Self::NotFound,
        Self::StdoutClosed,
    ];

    pub(crate) const fn code(self) -> i32 {
        match self {
            Self::Success => 0,
            Self::Failure => 1,
            Self::Unsafe | Self::Usage => 2,
            Self::CannotExecute => 126,
            Self::NotFound => 127,
            Self::StdoutClosed => 141,
            Self::Refused => 3,
            Self::RepinRequired => 4,
            Self::Expired => 5,
            Self::Misconfig => 6,
        }
    }

    /// Stable snake_case name, as printed in `exit_code_name` JSON fields.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Unsafe => "unsafe",
            Self::Usage => "usage",
            Self::CannotExecute => "cannot_execute",
            Self::NotFound => "not_found",
            Self::StdoutClosed => "stdout_closed",
            Self::Refused => "refused",
            Self::RepinRequired => "repin_required",
            Self::Expired => "expired",
            Self::Misconfig => "misconfig",
        }
    }

    pub(crate) const fn meaning(self) -> &'static str {
        match self {
            Self::Success => "success; check commands: healthy (doctor SAFE, agent ready, session ok)",
            Self::Failure => {
                "command error or refusal; check commands: degraded (doctor WARN, agent protected, session not ok)"
            }
            Self::Unsafe => "check commands: unsafe (doctor UNSAFE, agent unsafe, status pin invalid/frozen/expired)",
            Self::Usage => "usage error (bad flag/value, --dry-run on a command that cannot plan); no report on stdout",
            Self::CannotExecute => "child program exists but is not executable (exec/run/ci run)",
            Self::NotFound => "child program not found (exec/run/ci run)",
            Self::StdoutClosed => "stdout closed by the reader (128 + SIGPIPE)",
            Self::Refused => "action refused by policy or an authority gate",
            Self::RepinRequired => "human must establish a fresh pin before retrying",
            Self::Expired => "pin, credential, or authorization expired",
            Self::Misconfig => "local configuration is invalid or incomplete",
        }
    }

    pub(crate) fn exit(self) -> ! {
        std::process::exit(self.code())
    }

    /// Exit with this code unless it is [`Self::Success`].
    pub(crate) fn exit_unless_success(self) {
        if self != Self::Success {
            self.exit();
        }
    }

    pub(crate) const fn from_doctor(verdict: DoctorVerdict) -> Self {
        match verdict {
            DoctorVerdict::Safe => Self::Success,
            DoctorVerdict::Warn => Self::Failure,
            DoctorVerdict::Unsafe => Self::Unsafe,
        }
    }

    pub(crate) const fn from_agent(status: AgentStatus) -> Self {
        match status {
            AgentStatus::Ready => Self::Success,
            AgentStatus::Protected => Self::Failure,
            AgentStatus::Unsafe => Self::Unsafe,
        }
    }

    /// Markdown table for `locus topic exit-codes` and the docs section.
    pub(crate) fn markdown_table() -> String {
        let mut out = String::from("| Code | Name | Meaning |\n| --- | --- | --- |\n");
        for c in Self::ALL {
            out.push_str(&format!(
                "| {} | `{}` | {} |\n",
                c.code(),
                c.name(),
                c.meaning()
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_match_core_verdict_codes() {
        for v in [
            DoctorVerdict::Safe,
            DoctorVerdict::Warn,
            DoctorVerdict::Unsafe,
        ] {
            assert_eq!(ExitCode::from_doctor(v).code(), v.exit_code());
        }
        for s in [
            AgentStatus::Ready,
            AgentStatus::Protected,
            AgentStatus::Unsafe,
        ] {
            assert_eq!(ExitCode::from_agent(s).code(), s.exit_code());
        }
    }

    #[test]
    fn only_unsafe_and_usage_share_a_number() {
        for a in ExitCode::ALL {
            for b in ExitCode::ALL {
                if a != b && a.code() == b.code() {
                    let mut pair = [a.name(), b.name()];
                    pair.sort();
                    assert_eq!(pair, ["unsafe", "usage"]);
                }
            }
        }
    }

    #[test]
    fn names_are_unique_snake_case() {
        let mut names: Vec<_> = ExitCode::ALL.iter().map(|c| c.name()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), ExitCode::ALL.len());
        assert!(names
            .iter()
            .all(|n| n.chars().all(|c| c.is_ascii_lowercase() || c == '_')));
    }

    #[test]
    fn named_agent_outcomes_have_fixed_codes_and_names() {
        assert_eq!(
            [
                (ExitCode::Refused.code(), ExitCode::Refused.name()),
                (
                    ExitCode::RepinRequired.code(),
                    ExitCode::RepinRequired.name()
                ),
                (ExitCode::Expired.code(), ExitCode::Expired.name()),
                (ExitCode::Misconfig.code(), ExitCode::Misconfig.name()),
            ],
            [
                (3, "refused"),
                (4, "repin_required"),
                (5, "expired"),
                (6, "misconfig")
            ]
        );
    }

    #[test]
    fn clap_usage_errors_use_the_usage_code() {
        use clap::Parser;
        #[derive(Parser, Debug)]
        struct T {
            #[arg(long)]
            n: u8,
        }
        let e = T::try_parse_from(["t", "--n", "x"]).unwrap_err();
        assert_eq!(e.exit_code(), ExitCode::Usage.code());
    }
}

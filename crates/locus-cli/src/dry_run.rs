//! `--dry-run` plans and the non-interactive gate.
//!
//! A mutating command run with `--dry-run` performs only its read-only
//! validation (alias resolution, workspace allowlist, file existence) and
//! prints a [`Plan`] instead of writing. A plan never contains a secret or a
//! capability value: changes name paths, aliases and record ids only.
//!
//! A plan does not authenticate the control capability: that would start the
//! authority broker, which is itself a side effect. It reports whether the
//! real run needs the capability and whether one is present in the env, so a
//! plan that exits 0 can still be refused by the real run's control gate.

use crate::json_out;
use anyhow::{anyhow, Result};
use colored::Colorize;
use locus_core::Store;
use serde::Serialize;
use std::io::IsTerminal;
use std::path::Path;

/// Set `LOCUS_NON_INTERACTIVE=1` to refuse prompts even on a TTY.
pub(crate) const NON_INTERACTIVE_ENV: &str = "LOCUS_NON_INTERACTIVE";

/// Whether a command may prompt: not `--json`, `LOCUS_NON_INTERACTIVE` is
/// unset, and stdin is a terminal. Only stdin decides, as before the agent
/// contract: `locus binding add | tee log` on a terminal still prompts.
pub(crate) fn interactive(json: bool) -> bool {
    may_prompt(json, non_interactive_env(), std::io::stdin().is_terminal())
}

/// The prompt rule with its inputs made explicit. stdout is deliberately
/// not an input: piping or capturing output must not disable prompts.
pub(crate) fn may_prompt(json: bool, non_interactive_env: bool, stdin_is_tty: bool) -> bool {
    !json && !non_interactive_env && stdin_is_tty
}

fn non_interactive_env() -> bool {
    std::env::var(NON_INTERACTIVE_ENV)
        .ok()
        .is_some_and(|v| !v.is_empty() && v != "0")
}

/// One planned effect.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct Change {
    /// `create` | `overwrite` | `update` | `delete` | `pin` | `unpin` |
    /// `mint` | `revoke` | `grant` | `deny` | `register` | `send` | `print`
    pub action: &'static str,
    /// Path, alias, session or record id the change touches.
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Fields every plan carries; [`Plan::with`] cannot shadow them.
const STANDARD_KEYS: &[&str] = &[
    "schema",
    "ok",
    "dry_run",
    "command",
    "changes",
    "requires_control_capability",
    "control_capability_in_env",
    "notes",
];

#[derive(Serialize)]
struct PlanBody<'a> {
    ok: bool,
    dry_run: bool,
    command: &'a str,
    changes: &'a [Change],
    requires_control_capability: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    control_capability_in_env: Option<bool>,
    notes: &'a [String],
    #[serde(flatten)]
    extra: &'a serde_json::Map<String, serde_json::Value>,
}

/// The planned changes of one `--dry-run` invocation.
#[derive(Debug)]
pub(crate) struct Plan {
    command: String,
    changes: Vec<Change>,
    requires_control_capability: bool,
    notes: Vec<String>,
    extra: serde_json::Map<String, serde_json::Value>,
}

impl Plan {
    pub(crate) fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            changes: Vec::new(),
            requires_control_capability: false,
            notes: Vec::new(),
            extra: serde_json::Map::new(),
        }
    }

    pub(crate) fn change(
        &mut self,
        action: &'static str,
        target: impl Into<String>,
        detail: Option<String>,
    ) -> &mut Self {
        self.changes.push(Change {
            action,
            target: target.into(),
            detail,
        });
        self
    }

    /// Create `path` when missing, overwrite it otherwise.
    pub(crate) fn write_file(&mut self, path: &Path, detail: Option<String>) -> &mut Self {
        let action = if path.exists() { "overwrite" } else { "create" };
        self.change(action, path.display().to_string(), detail)
    }

    pub(crate) fn delete_file(&mut self, path: &Path, detail: Option<String>) -> &mut Self {
        self.change("delete", path.display().to_string(), detail)
    }

    /// The real run passes the control gate (`LOCUS_CONTROL_CAPABILITY`).
    pub(crate) fn needs_control(&mut self) -> &mut Self {
        self.requires_control_capability = true;
        self
    }

    pub(crate) fn note(&mut self, note: impl Into<String>) -> &mut Self {
        self.notes.push(note.into());
        self
    }

    /// Command-specific field carried after the standard plan fields. A key
    /// that names a standard field is ignored: extras never override them.
    pub(crate) fn with(&mut self, key: &str, value: impl Serialize) -> &mut Self {
        if !STANDARD_KEYS.contains(&key) {
            let value = serde_json::to_value(value).unwrap_or(serde_json::Value::Null);
            self.extra.insert(key.to_string(), value);
        }
        self
    }

    #[cfg(test)]
    pub(crate) fn changes(&self) -> &[Change] {
        &self.changes
    }

    /// The `locus.plan.v1` body (unstamped): standard fields first, in a
    /// fixed order, then the command-specific extras.
    pub(crate) fn body(&self) -> impl Serialize + '_ {
        PlanBody {
            ok: true,
            dry_run: true,
            command: &self.command,
            changes: &self.changes,
            requires_control_capability: self.requires_control_capability,
            // Presence only — never the value.
            control_capability_in_env: self.requires_control_capability.then(|| {
                std::env::var_os(locus_core::CONTROL_CAPABILITY_ENV).is_some_and(|v| !v.is_empty())
            }),
            notes: &self.notes,
            extra: &self.extra,
        }
    }

    /// Print the plan (JSON with `--json`, else a human list). Writes nothing.
    pub(crate) fn emit(&self, json: bool) -> Result<()> {
        if json {
            return json_out::print(json_out::PLAN, &self.body());
        }
        let n = self.changes.len();
        println!(
            "{} {} — {} planned change{}, nothing written",
            "dry-run:".cyan().bold(),
            self.command,
            n,
            if n == 1 { "" } else { "s" }
        );
        let width = self
            .changes
            .iter()
            .map(|c| c.action.len())
            .max()
            .unwrap_or(0);
        for c in &self.changes {
            match &c.detail {
                Some(d) => println!(
                    "  {:<width$}  {}  {}",
                    c.action,
                    c.target,
                    d.dimmed(),
                    width = width
                ),
                None => println!("  {:<width$}  {}", c.action, c.target, width = width),
            }
        }
        if self.requires_control_capability {
            let present =
                std::env::var_os(locus_core::CONTROL_CAPABILITY_ENV).is_some_and(|v| !v.is_empty());
            println!(
                "  {}",
                format!(
                    "the real run needs {} ({}; not authenticated by --dry-run)",
                    locus_core::CONTROL_CAPABILITY_ENV,
                    if present { "set" } else { "NOT set" }
                )
                .dimmed()
            );
        }
        for note in &self.notes {
            println!("  {} {note}", "note:".yellow());
        }
        Ok(())
    }
}

/// Open the store for planning without creating it: `Store::open` makes the
/// home, its subdirectories and the seal key, which a dry run must not do.
pub(crate) fn existing_store() -> Result<Store> {
    let home = locus_core::locus_home()?;
    Store::open_existing(&home).map_err(|_| {
        anyhow!(
            "{} is not initialized — nothing to plan against (preview setup with `locus init --dry-run`)",
            home.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 0.5.1 contract: only stdin decides whether a prompt may appear, so
    /// `locus binding add | tee log` on a terminal still prompts.
    #[test]
    fn legacy_prompt_rule_is_stdin_only() {
        assert!(may_prompt(false, false, true));
        assert!(!may_prompt(false, false, false));
        assert!(!may_prompt(true, false, true));
        assert!(!may_prompt(false, true, true));
    }

    #[test]
    fn plan_json_is_stamped_and_value_free() {
        let mut plan = Plan::new("locus binding rm acme");
        plan.delete_file(Path::new("/nonexistent/acme.toml"), None)
            .needs_control()
            .with("alias", "acme");
        let v = json_out::stamp(json_out::PLAN, &plan.body()).unwrap();
        assert_eq!(v["schema"], "locus.plan.v1");
        assert_eq!(v["dry_run"], true);
        assert_eq!(v["changes"][0]["action"], "delete");
        assert_eq!(v["requires_control_capability"], true);
        assert!(v["control_capability_in_env"].is_boolean());
        assert_eq!(v["alias"], "acme");
    }

    #[test]
    fn extra_fields_never_override_standard_ones() {
        let mut plan = Plan::new("x");
        plan.with("dry_run", false)
            .with("ok", false)
            .with("schema", "x");
        let v = json_out::stamp(json_out::PLAN, &plan.body()).unwrap();
        assert_eq!(v["schema"], "locus.plan.v1");
        assert_eq!(v["dry_run"], true);
        assert_eq!(v["ok"], true);
    }

    #[test]
    fn write_file_distinguishes_create_from_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().join("a");
        std::fs::write(&existing, "x").unwrap();
        let mut plan = Plan::new("x");
        plan.write_file(&existing, None)
            .write_file(&dir.path().join("b"), None);
        assert_eq!(plan.changes()[0].action, "overwrite");
        assert_eq!(plan.changes()[1].action, "create");
    }
}

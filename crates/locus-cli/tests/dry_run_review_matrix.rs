//! `docs/dry-run-review-matrix.md` records, for every command form, whether
//! `locus <form> --help` documents `--dry-run`. Re-derive that surface from
//! the built binary so the page cannot drift.

use super::doc_facts::{help_commands, locus_output};
use std::collections::{BTreeMap, BTreeSet};

const DOC: &str = include_str!("../../../docs/dry-run-review-matrix.md");

/// Every `Usage:` line of a help text, including wrapped `locus ...`
/// continuation lines (`locus mcp` has two).
fn usage_lines(help: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut in_usage = false;
    for line in help.lines() {
        if line.trim_start().starts_with("Usage:") {
            in_usage = true;
            lines.push(line.trim_start().to_string());
            continue;
        }
        if in_usage {
            if line.starts_with(' ') && line.trim_start().starts_with("locus") {
                lines.push(line.trim_start().to_string());
            } else {
                break;
            }
        }
    }
    lines
}

/// The command path a usage line displays: `locus a b [OPTIONS]` → `a b`.
/// Aliased forms display their canonical path (`locus approve pending`
/// shows `locus approve list [OPTIONS]` → `approve list`).
fn usage_path(usage: &str) -> String {
    let mut tokens = usage.split_whitespace().peekable();
    if tokens.peek() == Some(&"Usage:") {
        tokens.next();
    }
    tokens.next(); // the binary name
    tokens
        .take_while(|t| !t.starts_with('[') && !t.starts_with('<'))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A `--dry-run` option line, not a mention inside another option's
/// description (`agent setup --apply` names the flag in prose).
fn advertises_dry_run(help: &str) -> bool {
    help.lines()
        .any(|line| line.trim_start().starts_with("--dry-run"))
}

#[derive(Default)]
struct Surface {
    /// Invocable forms whose help lists `--dry-run`.
    documented: BTreeSet<String>,
    /// Invocable forms whose help hides it.
    hidden: BTreeSet<String>,
    /// Forms that only group subcommands (usage requires `<COMMAND>`).
    grouping: BTreeSet<String>,
    /// Alias → canonical form, detected by usage-path mismatch.
    aliases: BTreeMap<String, String>,
}

/// Walk every `locus <form> --help` reachable from `locus --help`.
fn inspect() -> Surface {
    let mut surface = Surface::default();
    let mut stack: Vec<Vec<String>> = vec![Vec::new()];
    while let Some(path) = stack.pop() {
        let mut args: Vec<&str> = path.iter().map(String::as_str).collect();
        args.push("--help");
        let help = locus_output(&args);
        assert!(help.contains("Usage:"), "no help for {}", path.join(" "));
        let children: Vec<Vec<String>> = help_commands(&help)
            .into_iter()
            .map(|name| {
                let mut child = path.clone();
                child.push(name);
                child
            })
            .collect();
        if path.is_empty() {
            stack.extend(children);
            continue;
        }
        let form = path.join(" ");
        let usages = usage_lines(&help);
        let canonical = usages.first().map(|u| usage_path(u)).unwrap_or_default();
        if canonical != form {
            surface.aliases.insert(form, canonical);
            continue;
        }
        let bare_invocable = children.is_empty() || usages.iter().any(|u| !u.contains("<COMMAND>"));
        if !bare_invocable {
            surface.grouping.insert(form);
        } else if advertises_dry_run(&help) {
            surface.documented.insert(form);
        } else {
            surface.hidden.insert(form);
        }
        stack.extend(children);
    }
    surface
}

/// Backticked command forms in the first cell of each table row under
/// `heading`.
fn forms_under(heading: &str) -> BTreeSet<String> {
    let section = DOC
        .split(heading)
        .nth(1)
        .unwrap_or_else(|| panic!("doc is missing `{heading}`"))
        .split("\n## ")
        .next()
        .unwrap();
    let mut forms = BTreeSet::new();
    for line in section.lines().filter(|line| line.starts_with("| `")) {
        let cell = line.split('|').nth(1).unwrap();
        for token in cell.split('`').skip(1).step_by(2) {
            forms.insert(token.trim().to_string());
        }
    }
    assert!(!forms.is_empty(), "no table rows under `{heading}`");
    forms
}

#[test]
fn review_matrix_tables_match_help() {
    let surface = inspect();
    assert_eq!(
        surface.documented,
        forms_under("## Help documents `--dry-run`"),
        "documented forms in the review matrix do not match `--help`"
    );
    assert_eq!(
        surface.hidden,
        forms_under("## Help does not document `--dry-run`"),
        "hidden forms in the review matrix do not match `--help`"
    );
    let grouping = forms_under("## Grouping help pages");
    assert_eq!(
        surface.grouping, grouping,
        "grouping pages in the review matrix do not match `--help`"
    );
    // Every grouping page still lists the global flag, even where a child
    // hides it (`locus ci --help` vs `locus ci run --help`).
    for form in &grouping {
        let args: Vec<&str> = form.split_whitespace().chain(["--help"]).collect();
        let help = locus_output(&args);
        assert!(
            advertises_dry_run(&help),
            "grouping page {form} does not list --dry-run"
        );
    }
}

#[test]
fn help_subcommand_renders_the_same_flag_surface() {
    // `locus help <form>` must show exactly what `locus <form> --help`
    // shows, including the forms that hide `--dry-run`.
    let surface = inspect();
    for form in surface
        .documented
        .union(&surface.hidden)
        .chain(&surface.grouping)
    {
        let mut help_args = vec!["help"];
        help_args.extend(form.split_whitespace());
        let rendered = locus_output(&help_args);
        if !rendered.contains("Options:") {
            // Not a clap help render: `locus help topic` is intercepted in
            // `run()` and prints the extended-help topic index instead.
            assert_eq!(form, "topic", "unexpected `locus help` intercept");
            continue;
        }
        let args: Vec<&str> = form.split_whitespace().chain(["--help"]).collect();
        let expected = advertises_dry_run(&locus_output(&args));
        let actual = advertises_dry_run(&rendered);
        assert_eq!(
            actual, expected,
            "`locus help {form}` does not match `locus {form} --help`"
        );
    }
}

#[test]
fn hidden_forms_reject_the_flag_as_a_usage_error() {
    for form in forms_under("## Help does not document `--dry-run`") {
        let mut args = vec!["--dry-run"];
        args.extend(form.split_whitespace());
        let out = locus_output(&args);
        assert!(
            out.contains("unexpected argument '--dry-run' found"),
            "`locus --dry-run {form}` was not a usage error:\n{out}"
        );
    }
}

#[test]
fn review_matrix_cites_the_inspected_help() {
    let surface = inspect();
    // The flag's help text is quoted from the binary itself.
    let top = locus_output(&["--help"]);
    let description = "Print the planned changes of a mutating command";
    assert!(top.contains(description), "flag text moved out of --help");
    assert!(
        DOC.contains(description),
        "doc must cite the flag's help text"
    );
    assert!(
        DOC.contains("issues/171"),
        "doc must reference the audited issue"
    );
    // Documented aliases name their canonical target.
    for (alias, canonical) in &surface.aliases {
        assert!(
            DOC.contains(&format!("`{alias}`")),
            "doc omits alias {alias}"
        );
        assert!(
            DOC.contains(&format!("`{canonical}`")),
            "doc omits canonical form {canonical} of alias {alias}"
        );
    }
}

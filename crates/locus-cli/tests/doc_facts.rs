//! Doc facts: agent-facing docs name CLI commands, MCP tools and setup
//! clients that are derived from code. These checks re-derive each list from
//! its source of truth and fail naming the doc line to fix.
//!
//! Sources of truth:
//! - CLI commands: the built `locus` binary's `--help` (clap catalog).
//! - MCP control tools: `locus_core::adapters::control_tools` (the catalog
//!   `locus-mcp` serves before provider tools).
//! - Setup clients: the `unknown client` refusal of `locus setup` and
//!   `locus agent setup` (the match arms that accept a client).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Docs an agent reads to learn the CLI and MCP surface.
const DOCS: &[&str] = &[
    "CLAUDE.md",
    "AGENTS.md",
    "README.md",
    "CONTRIBUTING.md",
    "docs/mcp.md",
    "docs/onboarding.md",
    "docs/architecture.md",
    "docs/policy.md",
    "docs/firm-mode.md",
    "docs/agency-starter.md",
    "docs/workers.md",
    "docs/adapters.md",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read_doc(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

pub(super) fn locus_output(args: &[&str]) -> String {
    let home = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_locus"))
        .args(args)
        .env("LOCUS_HOME", home.path())
        .env("HOME", home.path())
        .env("NO_COLOR", "1")
        .env_remove("LOCUS_CONTROL_CAPABILITY")
        .current_dir(home.path())
        .output()
        .expect("locus binary runs");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// Names (and `[alias: x]` aliases) in the `Commands:` section of a help text,
/// without clap's implicit `help`.
pub(super) fn help_commands(help: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut in_section = false;
    for line in help.lines() {
        if line.trim_end() == "Commands:" {
            in_section = true;
            continue;
        }
        if !in_section {
            continue;
        }
        if line.trim().is_empty() || !line.starts_with("  ") {
            break;
        }
        let trimmed = line.trim_start();
        if line.len() - trimmed.len() > 2 {
            continue; // wrapped description line
        }
        if let Some(name) = trimmed.split_whitespace().next() {
            if name != "help" {
                names.insert(name.to_string());
            }
        }
        if let Some(rest) = trimmed.split("[alias: ").nth(1) {
            if let Some(alias) = rest.split(']').next() {
                names.insert(alias.trim().to_string());
            }
        }
    }
    names
}

/// The `Daily use` / `Setup` group rows of the top-level long help.
fn help_group(help: &str, group: &str) -> Vec<String> {
    let line = help
        .lines()
        .find(|l| l.trim_start().starts_with(group))
        .unwrap_or_else(|| panic!("`locus --help` has no `{group}` group row"));
    line.trim_start()[group.len()..]
        .split('·')
        .map(|w| w.trim().to_string())
        .filter(|w| !w.is_empty())
        .collect()
}

/// Text an agent would copy: fenced code blocks and inline backtick spans,
/// with the 1-based line number each came from.
pub(super) fn code_spans(doc: &str) -> Vec<(usize, String)> {
    let mut spans = Vec::new();
    let mut fenced = false;
    for (i, line) in doc.lines().enumerate() {
        let n = i + 1;
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            spans.push((n, line.to_string()));
            continue;
        }
        let mut parts = line.split('`');
        parts.next();
        while let (Some(inside), Some(_)) = (parts.next(), parts.next()) {
            spans.push((n, inside.to_string()));
        }
    }
    spans
}

fn is_word(t: &str) -> bool {
    t.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && t.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Every `locus <cmd> [<sub>]` invocation in a span.
fn invocations(span: &str) -> Vec<(String, Option<String>)> {
    let tokens: Vec<&str> = span
        .split(|c: char| c.is_whitespace() || "\"'$()|&;`".contains(c))
        .filter(|t| !t.is_empty())
        .collect();
    let mut found = Vec::new();
    for (i, t) in tokens.iter().enumerate() {
        if *t != "locus" {
            continue;
        }
        // `which locus locus-mcp`: a binary name, not a subcommand.
        let Some(cmd) = tokens
            .get(i + 1)
            .filter(|c| is_word(c) && !c.starts_with("locus"))
        else {
            continue;
        };
        let sub = tokens
            .get(i + 2)
            .filter(|s| is_word(s))
            .map(|s| s.to_string());
        found.push((cmd.to_string(), sub));
    }
    found
}

#[test]
fn every_documented_locus_command_is_a_real_subcommand() {
    let top_help = locus_output(&["--help"]);
    let top = help_commands(&top_help);
    assert!(
        top.contains("enter"),
        "could not parse `locus --help`:\n{top_help}"
    );

    let mut subs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut failures = Vec::new();
    for doc in DOCS {
        for (line, span) in code_spans(&read_doc(doc)) {
            for (cmd, sub) in invocations(&span) {
                if !top.contains(&cmd) {
                    failures.push(format!(
                        "{doc}:{line}: `locus {cmd}` is not a subcommand of `locus --help`"
                    ));
                    continue;
                }
                let known = subs
                    .entry(cmd.clone())
                    .or_insert_with(|| help_commands(&locus_output(&[cmd.as_str(), "--help"])));
                if let Some(sub) = sub {
                    if !known.is_empty() && !known.contains(&sub) {
                        failures.push(format!(
                            "{doc}:{line}: `locus {cmd} {sub}` is not a subcommand of \
                             `locus {cmd} --help` (have: {known:?})"
                        ));
                    }
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "stale CLI commands in docs:\n{}",
        failures.join("\n")
    );
}

#[test]
fn claude_md_command_groups_match_help() {
    let help = locus_output(&["--help"]);
    let top = help_commands(&help);
    let daily = help_group(&help, "Daily use");
    let setup = help_group(&help, "Setup");
    let claude = read_doc("CLAUDE.md");
    let (n, row) = claude
        .lines()
        .enumerate()
        .find(|(_, l)| l.contains("**locus-cli**"))
        .expect("CLAUDE.md has a **locus-cli** architecture row");
    let n = n + 1;
    let backticked = |s: &str| -> Vec<String> {
        s.split('`')
            .skip(1)
            .step_by(2)
            .map(str::to_string)
            .collect()
    };
    let after_daily = row.split("Daily use:").nth(1).expect("row names Daily use");
    let (daily_part, setup_part) = after_daily.split_once("Setup:").expect("row names Setup");
    assert_eq!(
        backticked(daily_part),
        daily,
        "CLAUDE.md:{n}: Daily use list must equal the `Daily use` group of `locus --help`"
    );
    let setup_doc = backticked(setup_part.split("and more").next().unwrap());
    for c in &setup_doc {
        assert!(
            setup.contains(c),
            "CLAUDE.md:{n}: `{c}` is listed under Setup but is not in the `Setup` group of `locus --help` ({setup:?})"
        );
    }

    // AGENTS.md layout row: `locus` — enter, pin, ... , …
    let agents = read_doc("AGENTS.md");
    let (n, row) = agents
        .lines()
        .enumerate()
        .find(|(_, l)| l.starts_with("| `crates/locus-cli`"))
        .expect("AGENTS.md has a crates/locus-cli layout row");
    let list = row
        .split('—')
        .nth(1)
        .and_then(|s| s.split('(').next())
        .expect("row lists commands after an em dash");
    for c in list.split(',').map(str::trim).filter(|c| is_word(c)) {
        assert!(
            top.contains(c),
            "AGENTS.md:{}: `{c}` is not a subcommand of `locus --help`",
            n + 1
        );
    }
}

fn control_tool_names(pinned: bool) -> Vec<String> {
    locus_core::adapters::control_tools(pinned)
        .into_iter()
        .map(|t| t.name)
        .collect()
}

/// Backticked first-column names of the markdown table under `heading`.
fn table_tools(doc: &str, heading: &str) -> (usize, Vec<String>) {
    let lines: Vec<&str> = doc.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.trim() == heading)
        .unwrap_or_else(|| panic!("docs/mcp.md has no `{heading}` heading"));
    let mut names = Vec::new();
    for l in &lines[start + 1..] {
        if l.starts_with('#') {
            break;
        }
        if let Some(cell) = l.strip_prefix("| `") {
            if let Some(name) = cell.split('`').next() {
                names.push(name.to_string());
            }
        }
    }
    (start + 1, names)
}

#[test]
fn docs_mcp_tool_lists_match_the_control_catalog() {
    let unpinned = control_tool_names(false);
    let pinned = control_tool_names(true);
    let pinned_only: Vec<String> = pinned
        .iter()
        .filter(|t| !unpinned.contains(t))
        .cloned()
        .collect();
    let doc = read_doc("docs/mcp.md");

    let (n, always) = table_tools(&doc, "### Always (control plane)");
    assert_eq!(
        always, unpinned,
        "docs/mcp.md:{n}: the `Always (control plane)` table must list control_tools(false) in catalog order"
    );
    let (n, when_pinned) = table_tools(&doc, "### When pinned");
    let when_pinned: Vec<String> = when_pinned
        .into_iter()
        .filter(|t| t.starts_with("locus_"))
        .collect();
    assert_eq!(
        when_pinned, pinned_only,
        "docs/mcp.md:{n}: the `When pinned` table must list the locus_* tools control_tools(true) adds"
    );

    // "Until a valid pin exists, `tools/list` returns control tools only (...)"
    let (n, line) = doc
        .lines()
        .enumerate()
        .find(|(_, l)| l.contains("returns **control tools only**"))
        .expect("docs/mcp.md states the unpinned catalog");
    let inline: Vec<String> = line
        .split("control tools only**")
        .nth(1)
        .and_then(|s| s.split(')').next())
        .unwrap()
        .split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect();
    assert_eq!(
        inline,
        unpinned,
        "docs/mcp.md:{}: the unpinned control-tool list must equal control_tools(false)",
        n + 1
    );

    // "control tools plus `locus_providers` (N tools for the sample `personal` binding)"
    let flat = doc.replace('\n', " ");
    let count: usize = flat
        .split("tools for the sample `personal`")
        .next()
        .and_then(|s| s.rsplit('(').next())
        .and_then(|s| s.trim().parse().ok())
        .expect("docs/mcp.md states `(N tools for the sample `personal` binding)`");
    assert_eq!(
        count,
        pinned.len(),
        "docs/mcp.md: `({count} tools for the sample personal binding)` must equal control_tools(true).len()"
    );

    // Every locus_* name in agent-facing docs is a tool or the locus_context prompt.
    let mut known: BTreeSet<String> = pinned.iter().cloned().collect();
    known.insert("locus_context".into());
    // Rust crate paths, not tools.
    known.extend(["locus_core", "locus_cli", "locus_mcp"].map(String::from));
    let mut failures = Vec::new();
    for rel in DOCS {
        for (i, line) in read_doc(rel).lines().enumerate() {
            for tok in line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
                if tok.len() > "locus_".len() && tok.starts_with("locus_") && !known.contains(tok) {
                    failures.push(format!(
                        "{rel}:{}: `{tok}` is not an MCP control tool or prompt",
                        i + 1
                    ));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "stale MCP tool names:\n{}",
        failures.join("\n")
    );
}

/// Clients named in an `unknown client ... (a|b|c)` refusal.
fn accepted_clients(args: &[&str]) -> BTreeSet<String> {
    let out = locus_output(args);
    let list = out
        .rsplit('(')
        .next()
        .and_then(|s| s.split(')').next())
        .unwrap_or_else(|| panic!("no client list in `locus {}`: {out}", args.join(" ")));
    list.trim_start_matches("use ")
        .split('|')
        .map(|s| s.trim().to_string())
        .collect()
}

/// Values after `<prefix> --client ` in docs/mcp.md code spans.
fn documented_clients(doc: &str, prefix: &str) -> Vec<(usize, String)> {
    let needle = format!("{prefix} --client ");
    let mut found = Vec::new();
    for (n, span) in code_spans(doc) {
        for rest in span.split(&needle).skip(1) {
            let value: String = rest
                .chars()
                .take_while(|c| c.is_ascii_lowercase())
                .collect();
            if !value.is_empty() {
                found.push((n, value));
            }
        }
    }
    found
}

#[test]
fn docs_mcp_setup_clients_match_the_cli() {
    let doc = read_doc("docs/mcp.md");
    for (prefix, args) in [
        ("locus setup", &["setup", "--client", "not-a-client"][..]),
        (
            "locus agent setup --apply",
            &["agent", "setup", "--dry-run", "--client", "not-a-client"][..],
        ),
    ] {
        let accepted = accepted_clients(args);
        assert!(accepted.contains("claude"), "parsed {accepted:?}");
        let documented = documented_clients(&doc, prefix);
        for (n, c) in &documented {
            assert!(
                accepted.contains(c),
                "docs/mcp.md:{n}: `{prefix} --client {c}` is refused by the CLI (accepts {accepted:?})"
            );
        }
        if prefix == "locus setup" {
            let named: BTreeSet<String> = documented.into_iter().map(|(_, c)| c).collect();
            let missing: Vec<&String> = accepted.difference(&named).collect();
            assert!(
                missing.is_empty(),
                "docs/mcp.md: no `locus setup --client <c>` example for accepted clients {missing:?}"
            );
        }
    }
}

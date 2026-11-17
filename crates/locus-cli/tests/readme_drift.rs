//! README drift. The README must name every command path the binary's
//! `--help` tree has, and no command path the tree lacks. It must name every
//! MCP tool in `locus_core::adapters::control_tools` (plus the `locus_context`
//! prompt), and no other `locus_*` name. Live tests read the real binary and
//! catalog; mutation tests prove each direction reports drift.

use super::doc_facts::{help_commands, locus_output};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::OnceLock;

/// Parent command path -> child names, one entry per `--help` we read. The top
/// level (empty path) includes clap aliases such as `help-topic`.
type Tree = BTreeMap<Vec<String>, BTreeSet<String>>;

/// Deepest level read. The CLI has no path deeper than `adapter registry export`.
const MAX_DEPTH: usize = 4;

fn readme() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../README.md");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read README.md: {e}"))
}

fn help_tree() -> &'static Tree {
    static TREE: OnceLock<Tree> = OnceLock::new();
    TREE.get_or_init(|| {
        let mut tree = Tree::new();
        let mut queue = vec![Vec::<String>::new()];
        while let Some(parent) = queue.pop() {
            let mut args: Vec<&str> = parent.iter().map(String::as_str).collect();
            args.push("--help");
            let children = help_commands(&locus_output(&args));
            if parent.len() < MAX_DEPTH {
                for child in &children {
                    let mut path = parent.clone();
                    path.push(child.clone());
                    queue.push(path);
                }
            }
            tree.insert(parent, children);
        }
        assert!(
            tree.get(&Vec::new())
                .is_some_and(|top| top.contains("enter")),
            "could not parse `locus --help`"
        );
        tree
    })
}

/// Body of the first fenced block under `## CLI`: the command list the README promises.
fn cli_block(doc: &str) -> &str {
    let heading = "\n## CLI\n";
    let start = doc.find(heading).expect("README.md has a `## CLI` heading") + heading.len();
    let rest = &doc[start..];
    let open = rest.find("```").expect("`## CLI` has a fenced block");
    let after_fence = &rest[open + 3..];
    let body = &after_fence[after_fence.find('\n').expect("fence line ends") + 1..];
    &body[..body.find("```").expect("CLI block closes")]
}

/// Command paths the CLI block names, plus every prefix (`binding`, then
/// `binding list`). `a|b` names both alternatives; `·` starts a new command; a
/// `# comment` is prose, except `(alias: x)`, which names a top-level alias.
/// A subcommand alias is written as an alternative (`list|pending`). Each
/// command's walk stops at its first flag, placeholder, or bracket.
fn documented_paths(block: &str) -> BTreeSet<Vec<String>> {
    let mut found = BTreeSet::new();
    for line in block.lines() {
        let (code, comment) = line.split_once('#').unwrap_or((line, ""));
        if let Some(alias) = comment
            .split("alias: ")
            .nth(1)
            .and_then(|r| r.split(')').next())
        {
            found.insert(vec![alias.trim().to_string()]);
        }
        for segment in code.split('·') {
            let mut tokens = segment.split_whitespace().peekable();
            if tokens.peek() == Some(&"locus") {
                tokens.next();
            }
            let mut paths: Vec<Vec<String>> = vec![Vec::new()];
            for token in tokens {
                if token.starts_with(['-', '[', '<', '(']) {
                    break;
                }
                let mut next = Vec::new();
                for path in &paths {
                    for alternative in token.split('|') {
                        let mut extended = path.clone();
                        extended.push(alternative.to_string());
                        next.push(extended);
                    }
                }
                paths = next;
                found.extend(paths.iter().cloned());
            }
        }
    }
    found
}

/// Forward: every command path in the tree is documented. Reverse: every
/// documented path whose parent has subcommands is one of them.
fn command_drift(doc: &str, tree: &Tree) -> Vec<String> {
    let documented = documented_paths(cli_block(doc));
    let mut problems = Vec::new();
    for (parent, children) in tree {
        for child in children {
            let mut path = parent.clone();
            path.push(child.clone());
            if !documented.contains(&path) {
                problems.push(format!(
                    "README.md CLI block does not list `locus {}`",
                    path.join(" ")
                ));
            }
        }
    }
    for path in &documented {
        let (last, parent) = path.split_last().expect("documented paths are non-empty");
        // A parent without subcommands takes a value (`hook zsh`), so its child is not a command.
        let Some(children) = tree.get(parent) else {
            continue;
        };
        if !children.is_empty() && !children.contains(last) {
            problems.push(format!(
                "README.md CLI block lists `locus {}`, which `locus --help` does not have",
                path.join(" ")
            ));
        }
    }
    problems
}

/// The MCP names the server serves: the control tools plus the `locus_context` prompt.
fn served_mcp_names() -> BTreeSet<String> {
    let mut names: BTreeSet<String> = locus_core::adapters::control_tools(true)
        .into_iter()
        .map(|tool| tool.name)
        .collect();
    names.insert("locus_context".into());
    names
}

/// Forward: every served name appears in backticks. Reverse: every `locus_*`
/// word in the README is a served name.
fn tool_drift(doc: &str, served: &BTreeSet<String>) -> Vec<String> {
    let mut problems = Vec::new();
    for name in served {
        if !doc.contains(&format!("`{name}`")) {
            problems.push(format!("README.md does not name MCP tool `{name}`"));
        }
    }
    let named: BTreeSet<&str> = doc
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|word| word.starts_with("locus_") && word.len() > "locus_".len())
        .collect();
    for word in named {
        if !served.contains(word) {
            problems.push(format!(
                "README.md names `{word}`, which the MCP server does not serve"
            ));
        }
    }
    problems
}

#[test]
fn readme_cli_block_lists_every_command_and_no_stale_one() {
    let problems = command_drift(&readme(), help_tree());
    assert!(
        problems.is_empty(),
        "README drift:\n{}",
        problems.join("\n")
    );
}

#[test]
fn readme_names_every_mcp_tool_and_no_other_locus_name() {
    let problems = tool_drift(&readme(), &served_mcp_names());
    assert!(
        problems.is_empty(),
        "README drift:\n{}",
        problems.join("\n")
    );
}

#[test]
fn drift_fails_when_the_binary_has_a_command_the_readme_lacks() {
    let mut tree = help_tree().clone();
    tree.entry(Vec::new())
        .or_default()
        .insert("frobnicate".into());
    let problems = command_drift(&readme(), &tree);
    assert!(
        problems.iter().any(|p| p.contains("`locus frobnicate`")),
        "{problems:?}"
    );
}

#[test]
fn drift_fails_when_the_readme_drops_a_command() {
    let doc = readme().replace("locus leave · renew", "locus leave");
    assert_ne!(doc, readme(), "fixture no longer has `locus leave · renew`");
    let problems = command_drift(&doc, help_tree());
    assert!(
        problems.iter().any(|p| p.contains("`locus renew`")),
        "{problems:?}"
    );
}

#[test]
fn drift_fails_when_the_readme_names_a_subcommand_the_binary_lacks() {
    let old = "locus binding list|show|add|rm|migrate-credential-refs";
    let doc = readme().replace(old, &format!("{old}|frobnicate"));
    assert_ne!(doc, readme(), "fixture no longer has `{old}`");
    let problems = command_drift(&doc, help_tree());
    assert!(
        problems
            .iter()
            .any(|p| p.contains("`locus binding frobnicate`")),
        "{problems:?}"
    );
}

#[test]
fn drift_fails_when_a_served_tool_is_missing_from_the_readme() {
    let doc = readme().replace("| `locus_providers` |", "| `locus_providers_old` |");
    assert_ne!(
        doc,
        readme(),
        "fixture no longer has the `locus_providers` row"
    );
    let problems = tool_drift(&doc, &served_mcp_names());
    assert!(
        problems.iter().any(|p| p.contains("`locus_providers`")),
        "{problems:?}"
    );
    assert!(
        problems.iter().any(|p| p.contains("`locus_providers_old`")),
        "{problems:?}"
    );
}

#[test]
fn drift_fails_when_the_server_gains_a_tool_the_readme_lacks() {
    let mut served = served_mcp_names();
    served.insert("locus_new_tool".into());
    let problems = tool_drift(&readme(), &served);
    assert!(
        problems.iter().any(|p| p.contains("`locus_new_tool`")),
        "{problems:?}"
    );
}

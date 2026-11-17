//! Validate documentation against help only: never execute a documented command.
use super::doc_facts::{code_spans, help_commands, locus_output};
use std::collections::BTreeMap;
#[derive(Default)]
struct Catalog(BTreeMap<Vec<String>, String>);
impl Catalog {
    fn help(&mut self, path: &[String]) -> String {
        self.0
            .entry(path.to_vec())
            .or_insert_with(|| {
                let mut args: Vec<_> = path.iter().map(String::as_str).collect();
                args.push("--help");
                let help = locus_output(&args);
                assert!(help.contains("Usage:"), "cannot read help for {path:?}");
                help
            })
            .clone()
    }
    fn check(&mut self, text: &str) -> Result<Vec<String>, String> {
        let mut path = Vec::new();
        let tokens: Vec<_> = text.split_whitespace().collect();
        let mut i = 0;
        while i < tokens.len() {
            let token = tokens[i].trim_matches(|c| "[](),`\"'".contains(c));
            i += 1;
            if token == "--" || token == "…" {
                break; // child arguments and shell comments belong to another parser
            }
            let help = self.help(&path);
            if token.starts_with('-') {
                let flag = token.split('=').next().unwrap();
                // Exec deliberately hides --dry-run: it rejects execution previews.
                let help = if flag == "--dry-run" {
                    self.help(&[])
                } else {
                    help
                };
                let option = help.lines().find(|line| {
                    line.trim_start().starts_with('-')
                        && line
                            .split_whitespace()
                            .any(|word| word.trim_end_matches(',') == flag)
                });
                let Some(option) = option else {
                    return Err(format!("unknown flag {flag} on locus {}", path.join(" ")));
                };
                // Clap renders value-taking options with <VALUE> (or [VALUE]).
                let definition = option.split("  ").find(|s| !s.is_empty()).unwrap();
                if !token.contains('=') && (definition.contains('<') || definition.contains('[')) {
                    i += 1;
                }
            } else {
                let children = help_commands(&help);
                if children.is_empty() {
                    continue; // positional binding, filename, shell, etc.
                }
                if !children.contains(token) {
                    return Err(format!(
                        "unknown subcommand {token} on locus {}",
                        path.join(" ")
                    ));
                }
                path.push(token.to_string());
            }
        }
        Ok(path)
    }
}
/// Expand the README's compact `list|show` and `session · claim` notation.
fn expand(text: &str) -> Vec<String> {
    let mut forms = vec![String::new()];
    for token in text.split_whitespace() {
        forms = forms
            .into_iter()
            .flat_map(|prefix| {
                token
                    .split('|')
                    .map(move |choice| format!("{prefix} {choice}"))
            })
            .collect();
    }
    forms
}
fn audit(doc: &str) -> Vec<String> {
    let mut catalog = Catalog::default();
    let root = help_commands(&catalog.help(&[]));
    let mut failures = Vec::new();
    let mut count = 0;
    let doc = doc.replace("./target/release/locus ", "locus ");
    let mut spans: Vec<(usize, String)> = Vec::new();
    for (line, span) in code_spans(&doc) {
        let span = if span.trim_start().starts_with("\"$locus\" ") {
            span.replacen("\"$locus\"", "locus", 1)
        } else {
            span
        };
        if let Some((_, previous)) = spans.last_mut().filter(|(_, s)| s.ends_with('\\')) {
            previous.pop();
            previous.push_str(&span);
        } else {
            spans.push((line, span));
        }
    }
    for (line, span) in spans {
        let span = if span.contains("\"locus\"") && span.contains("\"args\":") {
            let args = span.split_once("\"args\":").unwrap().1.trim_start();
            let end = args.find(']').expect("MCP args array closes");
            let args: Vec<String> = serde_json::from_str(&args[..=end]).unwrap();
            format!("locus {}", args.join(" "))
        } else {
            span
        };
        let shell = span.split('#').next().unwrap();
        for fragment in shell.split(['$', '(', ')', ';', '&']) {
            let fragment = fragment.trim();
            let command = if let Some((prefix, rest)) = fragment.rsplit_once("locus ") {
                if !prefix.is_empty() && !prefix.ends_with(char::is_whitespace) {
                    continue;
                }
                rest
            } else if root.contains(fragment.split_whitespace().next().unwrap_or("")) {
                fragment
            } else {
                continue;
            };
            let mut parent = Vec::new();
            for (index, part) in command.split('·').enumerate() {
                let part = if index == 0 {
                    part.to_string()
                } else {
                    format!("{} {part}", parent.join(" "))
                };
                for form in expand(&part) {
                    count += 1;
                    match catalog.check(&form) {
                        Ok(mut path) => {
                            path.pop();
                            parent = path;
                        }
                        Err(error) => failures.push(format!("README.md:{line}: {error} ({form})")),
                    }
                }
            }
        }
    }
    assert!(count > 0, "no documented invocations found");
    failures
}
/// The README's "For agents" block must run as written on a host with no pin,
/// and its promises must hold: the JSON `schema` key leads stdout, errors go
/// to stderr as `locus.error.v1`, `--print` writes no client config, and
/// `init --dry-run` creates no home. Pinned behaviour is in readme_quickstart.rs.
#[test]
fn readme_for_agents_read_only_commands_run_as_written() {
    use std::process::{Command, Output};

    // Each run gets a fresh HOME and LOCUS_HOME so the no-write checks below
    // are not defeated by a sibling command that creates the store.
    let run = |args: &[&str]| -> (Output, tempfile::TempDir) {
        let home = tempfile::tempdir().unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_locus"))
            .args(args)
            .current_dir(home.path())
            .env("HOME", home.path())
            .env("LOCUS_HOME", home.path().join(".locus"))
            .env("NO_COLOR", "1")
            .env_remove("LOCUS_CONTROL_CAPABILITY")
            .env_remove("LOCUS_SESSION_ID")
            .env_remove("LOCUS_SEAL")
            .output()
            .expect("locus runs");
        (out, home)
    };
    let text = |out: &Output| {
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    };

    // `--json` output leads with the `schema` key, as docs/agent-contract.md
    // and the README promise. Parse the whole object so the stamp is real JSON.
    for (args, schema) in [
        (&["version", "--json"][..], "locus.version.v1"),
        (
            &["topic", "agent-contract", "--json"][..],
            "locus.contract.v1",
        ),
        (&["status", "--json"][..], "locus.status.v1"),
        (&["init", "--dry-run", "--json"][..], "locus.plan.v1"),
    ] {
        let (out, home) = run(args);
        assert!(out.status.success(), "{args:?} failed:\n{}", text(&out));
        let stdout = String::from_utf8_lossy(&out.stdout);
        let value: serde_json::Value = serde_json::from_str(&stdout)
            .unwrap_or_else(|e| panic!("{args:?} stdout is not JSON ({e}):\n{stdout}"));
        assert_eq!(value["schema"], schema, "{args:?}");
        assert_eq!(
            stdout.split('"').nth(1),
            Some("schema"),
            "{args:?}: schema must be the first key"
        );
        if args.contains(&"--dry-run") {
            assert!(
                !home.path().join(".locus").exists(),
                "init --dry-run must not create the Locus home"
            );
        }
    }

    let (oneline, _) = run(&["status", "--oneline"]);
    assert!(oneline.status.success(), "{}", text(&oneline));
    assert_eq!(String::from_utf8_lossy(&oneline.stdout).trim(), "unpinned");

    // With no pin, whoami exits 1 and the error on stderr is machine-readable.
    let (whoami, _) = run(&["whoami", "--json"]);
    assert_eq!(whoami.status.code(), Some(1), "{}", text(&whoami));
    let error: serde_json::Value = serde_json::from_slice(&whoami.stderr)
        .unwrap_or_else(|e| panic!("whoami stderr is not JSON ({e}): {}", text(&whoami)));
    assert_eq!(error["schema"], "locus.error.v1");

    // `setup --print` prints the entry and writes no client config.
    for client in ["claude", "cursor", "codex"] {
        let (out, home) = run(&["setup", "--client", client, "--print"]);
        assert!(
            out.status.success(),
            "setup {client} --print:\n{}",
            text(&out)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("locus-mcp"),
            "{client}: no MCP entry:\n{stdout}"
        );
        assert!(
            !home.path().join(".mcp.json").exists(),
            "{client}: wrote .mcp.json"
        );
        assert!(
            !home.path().join(".cursor").exists(),
            "{client}: wrote ~/.cursor"
        );
        assert!(
            !home.path().join(".codex").exists(),
            "{client}: wrote ~/.codex"
        );
        assert!(
            !home.path().join(".locus").exists(),
            "{client}: setup --print created the Locus home"
        );
    }
}

#[test]
fn readme_commands_and_flags_exist() {
    let failures = audit(include_str!("../../../README.md"));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
#[test]
fn rejects_stale_commands_flags_and_compact_alternatives() {
    for sample in [
        "`locus nonexistent`",
        "```sh\nclaude mcp add locus -- locus mcp --nonexistent\n```",
        r#"`{"command":"locus", "args":["mcp", "--nonexistent"]}`"#,
        "`locus adapter registry index nonexistent`",
        "`locus whoami --nonexistent`",
        "`locus binding list|nonexistent`",
        "`locus adapter search|install|uninstall · registry export|verify-manifest`",
        "`locus verify session · nonexistent`",
        "`locus --json adapter --dry-run registry index nonexistent`",
    ] {
        assert!(!audit(sample).is_empty(), "accepted {sample}");
    }
    assert!(audit(concat!(
        "`locus exec -- child --nonexistent`\n`locus exec --dry-run -- child`\n",
        "```sh\ncargo install --git https://github.com/ashlrai/locus locus-cli --locked\n",
        "locus engagement init|close # client (binding + workspace)\n",
        "python3 - \"$locus\" arg\n```",
    ))
    .is_empty());
}

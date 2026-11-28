//! Syntax-only repository cards. No provider credentials or code execution.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fs, path::Path, process::Command};
use tree_sitter::{Node, Parser};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Symbol {
    name: String,
    kind: String,
    signature: String,
    line: usize,
    calls: Vec<String>,
    body_hash: String,
    body: String,
}
#[derive(Serialize)]
struct Card {
    trimmed: bool,
    #[serde(flatten)]
    symbol: Symbol,
    file: String,
    callers_count: usize,
    callers: Vec<String>,
    tests: Vec<String>,
    last_change: Option<String>,
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}
fn language(ext: &str) -> Option<tree_sitter::Language> {
    Some(match ext {
        "ts" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
        "py" => tree_sitter_python::LANGUAGE.into(),
        "rs" => tree_sitter_rust::LANGUAGE.into(),
        "go" => tree_sitter_go::LANGUAGE.into(),
        _ => return None,
    })
}
fn is_symbol(kind: &str) -> bool {
    matches!(
        kind,
        "function_declaration"
            | "function_definition"
            | "function_item"
            | "method_definition"
            | "method_declaration"
            | "class_declaration"
            | "class_definition"
            | "struct_item"
            | "enum_item"
            | "trait_item"
            | "interface_declaration"
            | "type_alias_declaration"
            | "type_spec"
    )
}
fn definition(node: Node<'_>) -> bool {
    is_symbol(node.kind())
        || (node.kind() == "variable_declarator"
            && node.child_by_field_name("value").is_some_and(|value| {
                matches!(value.kind(), "arrow_function" | "function_expression")
            }))
}
fn calls(node: Node<'_>, source: &str, out: &mut BTreeSet<String>) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if definition(child) {
            continue;
        }
        if matches!(child.kind(), "call_expression" | "call") {
            if let Some(function) = child.child_by_field_name("function") {
                out.insert(text(function, source).to_owned());
            }
        }
        calls(child, source, out);
    }
}
fn extract(node: Node<'_>, source: &str, out: &mut Vec<Symbol>) {
    if definition(node) {
        if let Some(name) = node.child_by_field_name("name") {
            let body = node.child_by_field_name("body").or_else(|| {
                node.child_by_field_name("value")
                    .and_then(|v| v.child_by_field_name("body"))
            });
            let end = body.map_or(node.end_byte(), |b| b.start_byte());
            let mut direct = BTreeSet::new();
            calls(node, source, &mut direct);
            let code = text(node, source);
            out.push(Symbol {
                name: text(name, source).into(),
                kind: node.kind().into(),
                signature: source[node.start_byte()..end].trim().into(),
                line: node.start_position().row + 1,
                calls: direct.into_iter().collect(),
                body_hash: hash(code.as_bytes()),
                body: code.into(),
            });
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        extract(child, source, out);
    }
}
fn parse(source: &str, ext: &str) -> Result<Vec<Symbol>> {
    let mut parser = Parser::new();
    parser.set_language(&language(ext).context("unsupported language")?)?;
    let tree = parser
        .parse(source, None)
        .context("tree-sitter parse cancelled")?;
    let mut symbols = Vec::new();
    extract(tree.root_node(), source, &mut symbols);
    Ok(symbols)
}
fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git").current_dir(root).args(args).output()?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args[0],
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(output.stdout)
}
fn terminal_name(call: &str) -> &str {
    call.rsplit(['.', ':']).next().unwrap_or(call)
}
fn is_test(file: &str, name: &str, body: &str) -> bool {
    file.split('/')
        .any(|p| matches!(p, "test" | "tests" | "__tests__"))
        || file.contains("test_")
        || file.contains("_test.")
        || file.contains(".test.")
        || file.contains(".spec.")
        || name.starts_with("test")
        || name.starts_with("Test")
        || body.contains("#[test]")
}
fn cards(root: &Path, target: &str, include_body: bool, cache: bool) -> Result<Vec<Card>> {
    let files = git(
        root,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
    )?;
    let cache_dir = root.join(".git");
    // Worktrees have a .git file: resolve the private git directory through git.
    let git_dir = String::from_utf8(git(root, &["rev-parse", "--absolute-git-dir"])?)?;
    let cache_dir = if cache_dir.is_dir() {
        cache_dir
    } else {
        git_dir.trim().into()
    }
    .join("locus-card-v1");
    let mut index = Vec::new();
    for file in files.split(|b| *b == 0).filter(|b| !b.is_empty()) {
        let file = std::str::from_utf8(file).context("non-UTF-8 repository path")?;
        let path = root.join(file);
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        if language(ext).is_none() || fs::symlink_metadata(&path).map_or(true, |m| !m.is_file()) {
            continue;
        }
        let bytes = fs::read(&path).with_context(|| format!("read {file}"))?;
        let source = std::str::from_utf8(&bytes).with_context(|| format!("UTF-8 source {file}"))?;
        let key = cache_dir.join(format!("{}-{}.json", ext, hash(&bytes)));
        let cached = fs::read(&key)
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<Symbol>>(&b).ok());
        let symbols = match cached {
            Some(s) => s,
            None => {
                let s = parse(source, ext)?;
                if cache {
                    fs::create_dir_all(&cache_dir)?;
                    let mut tmp = tempfile::NamedTempFile::new_in(&cache_dir)?;
                    serde_json::to_writer(&mut tmp, &s)?;
                    tmp.persist(&key).map_err(|e| e.error)?;
                }
                s
            }
        };
        index.push((file.to_owned(), symbols, source.to_owned()));
    }
    let target = target.strip_prefix("./").unwrap_or(target);
    let mut result = Vec::new();
    for (file, symbols, _) in &index {
        for symbol in symbols {
            if symbol.name != target && file != target {
                continue;
            }
            let mut callers = BTreeSet::new();
            let mut tests = BTreeSet::new();
            for (other_file, others, source) in &index {
                for other in others {
                    let location = format!("{}:{}:{}", other_file, other.line, other.name);
                    if other.calls.iter().any(|c| terminal_name(c) == symbol.name) {
                        callers.insert(location.clone());
                    }
                    if (is_test(other_file, &other.name, &other.body)
                        || (other.line > 1
                            && source
                                .lines()
                                .nth(other.line - 2)
                                .is_some_and(|l| l.trim() == "#[test]")))
                        && (other.calls.iter().any(|c| terminal_name(c) == symbol.name)
                            || other.name.contains(&symbol.name)
                            || source.lines().any(|l| {
                                (l.trim_start().starts_with("import ")
                                    || l.trim_start().starts_with("from ")
                                    || l.trim_start().starts_with("use "))
                                    && l.contains(&symbol.name)
                            }))
                    {
                        tests.insert(location);
                    }
                }
            }
            let commit = git(root, &["log", "-1", "--format=%H", "--", file])
                .ok()
                .and_then(|b| String::from_utf8(b).ok())
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.trim().to_owned());
            let mut symbol = symbol.clone();
            if !include_body {
                symbol.body.clear();
            }
            result.push(Card {
                trimmed: false,
                symbol,
                file: file.clone(),
                callers_count: callers.len(),
                callers: callers.into_iter().take(5).collect(),
                tests: tests.into_iter().collect(),
                last_change: commit,
            });
        }
    }
    if result.is_empty() {
        bail!("no supported symbol or path matches {target:?}");
    }
    Ok(result)
}
fn render(cards: &[Card], json: bool) -> Result<String> {
    if json {
        return Ok(serde_json::to_string(cards)?);
    }
    let mut rendered = Vec::new();
    for card in cards {
        let symbol = &card.symbol;
        let mut output = format!(
            "{}{} [{}]\n{}\n{}:{}\ncalls: {}\ncallers: {} ({})\ntests: {}\nlast-change: {}\nbody-hash: {}",
            symbol.name,
            if card.trimmed { " (trimmed)" } else { "" },
            symbol.kind,
            symbol.signature,
            card.file,
            symbol.line,
            symbol.calls.join(", "),
            card.callers_count,
            card.callers.join(", "),
            card.tests.join(", "),
            card.last_change.as_deref().unwrap_or("uncommitted"),
            symbol.body_hash,
        );
        if !symbol.body.is_empty() {
            output.push('\n');
            output.push_str(&symbol.body);
        }
        rendered.push(output);
    }
    Ok(rendered.join("\n\n"))
}
fn bounded(mut cards: Vec<Card>, budget: Option<usize>, json: bool) -> Result<String> {
    // A byte is a conservative upper bound on tokens, independent of model tokenizer.
    loop {
        let output = render(&cards, json)?;
        if budget.is_none_or(|n| output.len() < n) {
            return Ok(output);
        }
        for card in &mut cards {
            card.trimmed = true;
        }
        if cards.iter().any(|c| !c.symbol.body.is_empty()) {
            for c in &mut cards {
                c.symbol.body.clear();
            }
        } else if cards
            .iter()
            .any(|c| !c.tests.is_empty() || !c.callers.is_empty())
        {
            for c in &mut cards {
                c.tests.clear();
                c.callers.clear();
            }
        } else if cards
            .iter()
            .any(|c| !c.symbol.signature.is_empty() || !c.symbol.calls.is_empty())
        {
            for c in &mut cards {
                c.symbol.signature.clear();
                c.symbol.calls.clear();
            }
        } else if cards.len() > 1 {
            cards.pop();
        } else {
            bail!(
                "budget too small for card identity and hashes (need at least {} tokens)",
                output.len() + 1
            );
        }
    }
}
pub fn run(
    target: &str,
    budget: Option<usize>,
    body: bool,
    json: bool,
    dry_run: bool,
) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let root = String::from_utf8(git(&cwd, &["rev-parse", "--show-toplevel"])?)?;
    let root = Path::new(root.trim());
    let target_path = cwd.join(target);
    let target_path = if target_path.is_file() {
        target_path.canonicalize()?
    } else {
        std::path::PathBuf::from(target)
    };
    let target = target_path
        .strip_prefix(root)
        .ok()
        .and_then(|p| p.to_str())
        .unwrap_or(target);
    println!(
        "{}",
        bounded(cards(root, target, body, !dry_run)?, budget, json)?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]).unwrap();
        for (ext, source) in [
            ("ts", include_str!("../tests/fixtures/card/sample.ts")),
            ("py", include_str!("../tests/fixtures/card/sample.py")),
            ("rs", include_str!("../tests/fixtures/card/sample.rs")),
            ("go", include_str!("../tests/fixtures/card/sample.go")),
        ] {
            fs::write(dir.path().join(format!("sample.{ext}")), source).unwrap();
        }
        dir
    }
    #[test]
    fn languages_relationships_cache_and_invalidation() {
        let dir = fixture();
        let first = cards(dir.path(), "target", true, true).unwrap();
        assert_eq!(first.len(), 4);
        for card in &first {
            assert_eq!(card.symbol.calls, ["helper"]);
            assert_eq!(card.callers_count, 4);
            assert_eq!(card.tests.len(), 4);
            assert!(card.symbol.signature.contains("target"));
            assert_eq!(card.symbol.body_hash.len(), 64);
            assert!(card.last_change.is_none());
        }
        let cache = dir.path().join(".git/locus-card-v1");
        let entries: Vec<_> = fs::read_dir(&cache)
            .unwrap()
            .map(|e| {
                let p = e.unwrap().path();
                let modified = fs::metadata(&p).unwrap().modified().unwrap();
                (p, modified)
            })
            .collect();
        assert_eq!(entries.len(), 4);
        assert_eq!(
            render(&first, true).unwrap(),
            render(&cards(dir.path(), "target", true, true).unwrap(), true).unwrap()
        );
        for (path, modified) in entries {
            assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), modified);
        }
        fs::write(
            dir.path().join("sample.py"),
            "def target(x):\n    return changed(x)\n",
        )
        .unwrap();
        let next = cards(dir.path(), "sample.py", false, true).unwrap();
        assert_eq!(next[0].symbol.calls, ["changed"]);
        assert_ne!(
            next[0].symbol.body_hash,
            first
                .iter()
                .find(|c| c.file == "sample.py")
                .unwrap()
                .symbol
                .body_hash
        );
        assert!(cards(dir.path(), "absent", false, true).is_err());
    }
    #[test]
    fn budget_preserves_json_and_identity() {
        let dir = fixture();
        let all = cards(dir.path(), "target", true, false).unwrap();
        let full = render(&all, true).unwrap().len();
        let output = bounded(all, Some(full - 1), true).unwrap();
        assert!(output.len() < full - 1);
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value[0]["name"], "target");
        assert_eq!(value[0]["body"], "");
        assert!(bounded(
            cards(dir.path(), "target", true, false).unwrap(),
            Some(1),
            true
        )
        .is_err());
        assert!(!dir.path().join(".git/locus-card-v1").exists());
    }
    #[test]
    fn nested_calls_are_owned_by_their_function() {
        let parsed = parse(
            "def outer():\n    def inner():\n        hidden()\n    visible()\n",
            "py",
        )
        .unwrap();
        assert_eq!(parsed[0].calls, ["visible"]);
        assert_eq!(parsed[1].calls, ["hidden"]);
    }
}

#[cfg(test)]
mod additional_tests {
    use super::*;
    #[test]
    fn methods_arrow_functions_and_tsx() {
        for (ext, source, expected) in [
            ("ts", "const arrow = (x: number) => helper(x);", "arrow"),
            (
                "tsx",
                "function View() { return <div>{helper()}</div>; }",
                "View",
            ),
            (
                "go",
                "package p\ntype T struct {}\nfunc (t T) Method() { helper() }",
                "Method",
            ),
            (
                "rs",
                "struct T; impl T { fn method(&self) { helper(); } }",
                "method",
            ),
            (
                "py",
                "class T:\n    def method(self):\n        helper()\n",
                "method",
            ),
        ] {
            let parsed = parse(source, ext).unwrap();
            let symbol = parsed.iter().find(|s| s.name == expected).unwrap();
            assert_eq!(symbol.calls, ["helper"]);
        }
    }
    #[test]
    fn cli_flags_and_read_only_classification() {
        use clap::Parser;
        let cli = crate::Cli::try_parse_from([
            "locus", "card", "symbol", "--body", "--json", "--budget", "500",
        ])
        .unwrap();
        assert!(cli.json);
        assert!(matches!(
            cli.command,
            crate::Commands::Card {
                body: true,
                budget: Some(500),
                ..
            }
        ));
    }
    #[test]
    fn imports_and_top_five_callers_refresh() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]).unwrap();
        fs::write(dir.path().join("main.py"), "def target():\n    pass\n").unwrap();
        fs::write(
            dir.path().join("test_import.py"),
            "from main import target\ndef test_alias():\n    pass\n",
        )
        .unwrap();
        let first = cards(dir.path(), "target", false, true).unwrap();
        assert_eq!(first[0].tests.len(), 1);
        let source: String = (0..7)
            .map(|n| format!("def caller{n}():\n    target()\n"))
            .collect();
        fs::write(dir.path().join("callers.py"), source).unwrap();
        let next = cards(dir.path(), "target", false, true).unwrap();
        assert_eq!(next[0].callers_count, 7);
        assert_eq!(next[0].callers.len(), 5);
        let full = render(&next, false).unwrap().len();
        let trimmed = bounded(next, Some(full), false).unwrap();
        assert!(trimmed.len() < full);
        assert!(trimmed.contains("target"));
    }
}

#[cfg(test)]
mod call_ownership_tests {
    use super::*;
    #[test]
    fn local_initializer_calls_belong_to_the_function() {
        let symbols = parse("function outer() { const value = helper(); const inner = () => hidden(); return value; }", "ts").unwrap();
        assert_eq!(symbols.len(), 2);
        assert_eq!(symbols[0].calls, ["helper"]);
        assert_eq!(symbols[1].calls, ["hidden"]);
    }
}

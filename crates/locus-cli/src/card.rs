//! Syntax-only repository cards. No provider credentials or code execution.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
};
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
    let mut command = Command::new("git");
    // Cards describe this checkout, even when launched from a shell carrying
    // another repository's Git context. Signature display can invoke gpg, so
    // explicitly disable it on this syntax-only surface.
    for variable in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_CEILING_DIRECTORIES",
        "GIT_DISCOVERY_ACROSS_FILESYSTEM",
        "GIT_NAMESPACE",
    ] {
        command.env_remove(variable);
    }
    let output = command
        .current_dir(root)
        .args([
            "-c",
            "log.showSignature=false",
            "-c",
            "core.fsmonitor=false",
        ])
        .args(args)
        .output()?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args[0],
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(output.stdout)
}
fn source_path(root: &Path, file: &str) -> Option<PathBuf> {
    let mut path = root.to_path_buf();
    // Checking only the final file would still follow a replaced parent
    // directory into another checkout. Refuse symlinks at every component.
    for component in Path::new(file).components() {
        let Component::Normal(part) = component else {
            return None;
        };
        path.push(part);
        if fs::symlink_metadata(&path).ok()?.file_type().is_symlink() {
            return None;
        }
    }
    let canonical = path.canonicalize().ok()?;
    (canonical.starts_with(root) && fs::metadata(&canonical).ok()?.is_file()).then_some(canonical)
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
    let canonical_root = root.canonicalize()?;
    let root = canonical_root.as_path();
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
        git_dir.strip_suffix('\n').unwrap_or(&git_dir).into()
    }
    .join("locus-card-v1");
    let mut index = Vec::new();
    // An unmerged index lists the same path once for each conflict stage.
    let paths: BTreeSet<_> = files.split(|b| *b == 0).filter(|b| !b.is_empty()).collect();
    for file in paths {
        let file = std::str::from_utf8(file).context("non-UTF-8 repository path")?;
        let Some(path) = source_path(root, file) else {
            continue;
        };
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        if language(ext).is_none() {
            continue;
        }
        let bytes = fs::read(&path).with_context(|| format!("read {file}"))?;
        let source = std::str::from_utf8(&bytes).with_context(|| format!("UTF-8 source {file}"))?;
        // Parser and extraction changes shipped in another CLI version must
        // not reuse otherwise-valid facts from an older implementation.
        let key = cache_dir.join(format!(
            "{}-{}-{}.json",
            env!("CARGO_PKG_VERSION"),
            ext,
            hash(&bytes)
        ));
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
    let root = Path::new(root.strip_suffix('\n').unwrap_or(&root));
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
    fn unmerged_index_stages_produce_one_card_per_symbol() {
        use std::{io::Write, process::Stdio};
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]).unwrap();
        fs::write(
            dir.path().join("sample.py"),
            "def target():\n    helper()\n",
        )
        .unwrap();
        let object =
            String::from_utf8(git(dir.path(), &["hash-object", "-w", "sample.py"]).unwrap())
                .unwrap();
        let mut index = Command::new("git")
            .current_dir(dir.path())
            .args(["update-index", "--index-info"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let stages: String = (1..=3)
            .map(|stage| format!("100644 {} {stage}\tsample.py\n", object.trim()))
            .collect();
        index
            .stdin
            .take()
            .unwrap()
            .write_all(stages.as_bytes())
            .unwrap();
        assert!(index.wait().unwrap().success());
        let files = git(dir.path(), &["ls-files", "-z", "--cached"]).unwrap();
        assert_eq!(
            files.split(|b| *b == 0).filter(|b| !b.is_empty()).count(),
            3
        );
        let result = cards(dir.path(), "target", true, false).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].file, "sample.py");
        assert_eq!(result[0].symbol.calls, ["helper"]);
    }

    #[test]
    fn legacy_and_other_version_caches_do_not_override_parser_facts() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]).unwrap();
        let source = "def target():\n    helper()\n";
        fs::write(dir.path().join("sample.py"), source).unwrap();
        let mut stale = parse(source, "py").unwrap();
        stale[0].calls = vec!["stale_parser_fact".into()];
        stale[0].body_hash = "stale-body-hash".into();
        let cache = dir.path().join(".git/locus-card-v1");
        fs::create_dir(&cache).unwrap();
        let digest = hash(source.as_bytes());
        for name in [
            format!("py-{digest}.json"),
            format!("{}-other-py-{digest}.json", env!("CARGO_PKG_VERSION")),
        ] {
            fs::write(cache.join(name), serde_json::to_vec(&stale).unwrap()).unwrap();
        }
        let result = cards(dir.path(), "target", true, true).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].symbol.calls, ["helper"]);
        assert_eq!(
            result[0].symbol.body_hash,
            hash(source.trim_end().as_bytes())
        );
        let current = cache.join(format!("{}-py-{digest}.json", env!("CARGO_PKG_VERSION")));
        let cached: Vec<Symbol> = serde_json::from_slice(&fs::read(current).unwrap()).unwrap();
        assert_eq!(cached[0].calls, ["helper"]);
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

#[cfg(test)]
mod isolation_tests {
    use super::*;

    #[test]
    fn git_location_overrides_do_not_escape_repository() {
        const CHILD: &str = "LOCUS_CARD_GIT_ISOLATION_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let dir = tempfile::tempdir().unwrap();
            git(dir.path(), &["init", "--quiet"]).unwrap();
            fs::write(dir.path().join("local.py"), "def local():\n    pass\n").unwrap();
            let root =
                String::from_utf8(git(dir.path(), &["rev-parse", "--show-toplevel"]).unwrap())
                    .unwrap();
            assert_eq!(
                Path::new(root.trim()).canonicalize().unwrap(),
                dir.path().canonicalize().unwrap()
            );
            assert_eq!(
                cards(dir.path(), "local", false, false).unwrap()[0].file,
                "local.py"
            );
            return;
        }
        let other = tempfile::tempdir().unwrap();
        git(other.path(), &["init", "--quiet"]).unwrap();
        fs::write(
            other.path().join("external.py"),
            "def external():\n    pass\n",
        )
        .unwrap();
        let out = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "card::isolation_tests::git_location_overrides_do_not_escape_repository",
            ])
            .env(CHILD, "1")
            .env("GIT_DIR", other.path().join(".git"))
            .env("GIT_COMMON_DIR", other.path().join(".git"))
            .env("GIT_WORK_TREE", other.path())
            .env("GIT_INDEX_FILE", other.path().join(".git/index"))
            .env("GIT_OBJECT_DIRECTORY", other.path().join(".git/objects"))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(String::from_utf8_lossy(&out.stdout).contains("1 passed"));
    }

    #[cfg(unix)]
    #[test]
    fn source_symlinks_and_replaced_parent_directories_are_not_read() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]).unwrap();
        fs::write(dir.path().join("local.py"), "def local():\n    pass\n").unwrap();
        fs::create_dir(dir.path().join("nested")).unwrap();
        fs::write(
            dir.path().join("nested/external.py"),
            "def external():\n    pass\n",
        )
        .unwrap();
        git(dir.path(), &["add", "nested/external.py"]).unwrap();
        fs::remove_dir_all(dir.path().join("nested")).unwrap();
        // Invalid source must never be read through either kind of symlink.
        fs::write(outside.path().join("external.py"), [0xff]).unwrap();
        symlink(outside.path(), dir.path().join("nested")).unwrap();
        symlink(
            outside.path().join("external.py"),
            dir.path().join("direct.py"),
        )
        .unwrap();
        let root = dir.path().canonicalize().unwrap();
        assert!(source_path(&root, "nested/external.py").is_none());
        assert!(source_path(&root, "direct.py").is_none());
        assert!(source_path(&root, "../external.py").is_none());
        assert_eq!(cards(dir.path(), "local", false, false).unwrap().len(), 1);
        assert!(cards(dir.path(), "external", false, false).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn history_does_not_execute_configured_signature_program() {
        use std::{io::Write, os::unix::fs::PermissionsExt, process::Stdio};
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]).unwrap();
        fs::write(dir.path().join("local.py"), "def local():\n    pass\n").unwrap();
        git(dir.path(), &["add", "local.py"]).unwrap();
        let tree = String::from_utf8(git(dir.path(), &["write-tree"]).unwrap()).unwrap();
        let signed = format!("tree {}\nauthor Fixture <fixture@example.invalid> 1 +0000\ncommitter Fixture <fixture@example.invalid> 1 +0000\ngpgsig -----BEGIN PGP SIGNATURE-----\n synthetic\n -----END PGP SIGNATURE-----\n\nfixture\n", tree.trim());
        let mut writer = Command::new("git")
            .current_dir(dir.path())
            .args(["hash-object", "-t", "commit", "-w", "--stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        writer
            .stdin
            .take()
            .unwrap()
            .write_all(signed.as_bytes())
            .unwrap();
        let output = writer.wait_with_output().unwrap();
        assert!(output.status.success());
        let commit = String::from_utf8(output.stdout).unwrap();
        git(dir.path(), &["update-ref", "HEAD", commit.trim()]).unwrap();
        let program = dir.path().join("signature-check");
        fs::write(
            &program,
            "#!/bin/sh\ntouch signature-was-executed\nexit 1\n",
        )
        .unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
        git(
            dir.path(),
            &["config", "gpg.program", program.to_str().unwrap()],
        )
        .unwrap();
        git(dir.path(), &["config", "log.showSignature", "true"]).unwrap();
        let result = cards(dir.path(), "local", false, false).unwrap();
        assert_eq!(result[0].last_change.as_deref(), Some(commit.trim()));
        assert!(!dir.path().join("signature-was-executed").exists());
    }
}

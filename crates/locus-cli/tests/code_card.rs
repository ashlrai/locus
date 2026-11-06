//! Exercise code cards through the public CLI from a nested repository cwd.
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

struct Fixture {
    home: tempfile::TempDir,
    repo: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        // Preserve Git output path whitespace as well as NUL-delimited filenames.
        let repo = home.path().join("repo with space\n");
        fs::create_dir_all(repo.join("nested")).unwrap();
        fs::create_dir(repo.join("src")).unwrap();
        let out = Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&repo)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", home.path())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        fs::write(repo.join("src/module\nfixture.py"),
            "def helper(x):\n    return x + 1\n\ndef target(x):\n    return helper(x)\n\ndef test_target():\n    return target(1)\n").unwrap();
        Self { home, repo }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_locus"))
            .arg("card")
            .args(args)
            .current_dir(self.repo.join("nested"))
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.home.path())
            .env("LOCUS_HOME", self.home.path().join("locus"))
            .env("NO_COLOR", "1")
            .output()
            .unwrap()
    }

    fn json(&self, args: &[&str]) -> Value {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.stderr.is_empty());
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn cache(&self) -> PathBuf {
        self.repo.join(".git/locus-card-v1")
    }
}

#[test]
fn nested_symbol_and_newline_file_targets_return_code_and_relationships() {
    let fixture = Fixture::new();
    let cards = fixture.json(&["target", "--body", "--json"]);
    assert_eq!(cards.as_array().unwrap().len(), 1);
    let card = &cards[0];
    assert_eq!(card["name"], "target");
    assert_eq!(card["file"], "src/module\nfixture.py");
    assert_eq!(card["calls"], serde_json::json!(["helper"]));
    assert_eq!(card["callers_count"], 1);
    assert_eq!(card["tests"].as_array().unwrap().len(), 1);
    assert!(card["body"].as_str().unwrap().contains("return helper(x)"));
    assert_eq!(card["body_hash"].as_str().unwrap().len(), 64);
    assert!(card["last_change"].is_null());
    let file = fixture.json(&["../src/module\nfixture.py", "--json"]);
    assert_eq!(file.as_array().unwrap().len(), 3);
    assert!(file.as_array().unwrap().iter().all(|c| c["body"] == ""));
    let text = fixture.run(&["target", "--body"]);
    assert!(text.status.success());
    assert!(String::from_utf8_lossy(&text.stdout).contains("return helper(x)"));
}

#[test]
fn dry_run_avoids_cache_and_normal_run_creates_it() {
    let fixture = Fixture::new();
    let preview = fixture.json(&["target", "--json", "--dry-run"]);
    assert_eq!(preview[0]["name"], "target");
    assert!(!fixture.cache().exists());
    assert!(!fixture.home.path().join("locus").exists());
    let normal = fixture.json(&["target", "--json"]);
    assert_eq!(preview, normal);
    assert!(fs::read_dir(fixture.cache()).unwrap().next().is_some());
    assert!(!fixture.home.path().join("locus").exists());
}

#[test]
fn budget_counts_final_newline_and_errors_stay_machine_readable() {
    let fixture = Fixture::new();
    let full = fixture.run(&["target", "--body", "--json"]);
    assert!(full.status.success());
    let budget = full.stdout.len() - 1;
    let out = fixture.run(&[
        "target",
        "--body",
        "--json",
        "--budget",
        &budget.to_string(),
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.stdout.last(), Some(&b'\n'));
    assert!(
        out.stdout.len() <= budget,
        "final newline exceeds byte budget"
    );
    let trimmed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(trimmed[0]["trimmed"], true);
    assert_eq!(trimmed[0]["name"], "target");
    assert_eq!(
        trimmed[0]["body_hash"],
        serde_json::from_slice::<Value>(&full.stdout).unwrap()[0]["body_hash"]
    );
    for args in [
        vec!["absent", "--json"],
        vec!["target", "--json", "--budget", "1"],
    ] {
        let out = fixture.run(&args);
        assert!(!out.status.success());
        assert_eq!(out.stdout, out.stderr);
        let error: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(error["ok"], false);
        assert!(error["error"].as_str().is_some_and(|s| !s.is_empty()));
    }
}

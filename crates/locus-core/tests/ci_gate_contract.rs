//! CI Gate merge-queue contract.
//!
//! Reworks https://github.com/ashlrai/locus/pull/256: the original added a
//! `merge_group` trigger to conformance.yml because a required check that
//! never reports hangs every queue entry. The current design keeps the heavy
//! suites schedule-only and makes the `gate` job in ci.yml the one required
//! check, so the invariant is pinned here instead of on one trigger line:
//! ci.yml must report on `merge_group`, the gate must aggregate every sibling
//! job, and nothing else may join the queue or run as an unscoped PR check.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const CI: &str = include_str!("../../../.github/workflows/ci.yml");

fn workflow_files() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.github/workflows");
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("yml" | "yaml")
            )
        })
        .collect();
    files.sort();
    files
}

/// Lines under a column-0 `key:` section, up to the next column-0 key.
/// Column-0 comments and blanks are kept; callers skip what they do not read.
fn section<'a>(text: &'a str, key: &str) -> Vec<&'a str> {
    let header = format!("{key}:");
    let mut inside = false;
    let mut lines = Vec::new();
    for line in text.lines() {
        if inside {
            if !line.is_empty() && !line.starts_with(' ') && !line.starts_with('#') {
                break;
            }
            lines.push(line);
        } else if line == header {
            inside = true;
        }
    }
    assert!(inside, "workflow is missing the `{key}:` section");
    lines
}

/// The mapping key of a line at exactly `indent` spaces, or None for deeper
/// lines, list items, blanks and comments.
fn child_key(line: &str, indent: usize) -> Option<&str> {
    if line.len() <= indent || !line.as_bytes()[..indent].iter().all(|b| *b == b' ') {
        return None;
    }
    let (key, _) = line[indent..].split_once(':')?;
    if key.is_empty()
        || !key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    Some(key)
}

/// The keys a workflow's `on:` section subscribes to.
fn triggers(text: &str) -> BTreeSet<String> {
    section(text, "on")
        .iter()
        .filter_map(|line| child_key(line, 2))
        .map(str::to_owned)
        .collect()
}

/// Lines owned by `key` inside `lines` (a section), up to the next sibling
/// key at the same indent.
fn child_block<'a>(lines: &[&'a str], key: &str, indent: usize) -> Vec<&'a str> {
    let mut inside = false;
    let mut block = Vec::new();
    for line in lines {
        match child_key(line, indent) {
            Some(_) if inside => break,
            Some(k) if k == key => inside = true,
            _ if inside => block.push(*line),
            _ => {}
        }
    }
    block
}

/// An inline `[a, b]` list on the line introducing `key` at `indent`.
fn inline_list(lines: &[&str], key: &str, indent: usize) -> BTreeSet<String> {
    let line = lines
        .iter()
        .find(|line| child_key(line, indent) == Some(key))
        .unwrap_or_else(|| panic!("missing `{key}:` at indent {indent}"));
    let list = line.split_once(':').unwrap().1.trim();
    assert!(
        list.starts_with('[') && list.ends_with(']'),
        "`{key}:` must stay an inline list, got: {list}"
    );
    list[1..list.len() - 1]
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The dependency inventory the gate step hardcodes in its jq assertion.
/// `needs:` and this list must never drift: the gate fails closed on any job
/// that is not in both, which reads as an unexplained required-check red.
fn gate_inventory(gate: &[&str]) -> BTreeSet<String> {
    let line = gate
        .iter()
        .find(|line| line.contains("--argjson exp"))
        .expect("gate must keep an exact dependency inventory assertion");
    let anchor = line.find("--argjson exp").unwrap();
    let start = anchor
        + line[anchor..]
            .find('[')
            .expect("inventory must be a JSON array");
    let end = start
        + line[start..]
            .find(']')
            .expect("inventory must be a JSON array");
    let json = line[start..=end].replace("\\\"", "\"");
    serde_json::from_str::<Vec<String>>(&json)
        .expect("gate inventory must parse as JSON")
        .into_iter()
        .collect()
}

#[test]
fn required_check_reports_in_the_merge_queue() {
    let on = triggers(CI);
    assert!(on.contains("pull_request"), "CI Gate must report on PRs");
    assert!(
        on.contains("merge_group"),
        "CI Gate is the required check: without merge_group every queue \
         entry hangs waiting on a status that never reports (ashlrai/locus#256)"
    );
}

#[test]
fn required_check_pull_request_trigger_is_unfiltered() {
    // A paths filter or a narrowed `types:` skips the required check on the
    // PRs it excludes; those PRs hang the same way a missing merge_group
    // trigger hangs the queue.
    let on = section(CI, "on");
    let pr = child_block(&on, "pull_request", 2);
    for key in ["paths", "paths-ignore"] {
        assert!(
            !pr.iter().any(|line| child_key(line, 4) == Some(key)),
            "the required check cannot be {key}-filtered on pull_request"
        );
    }
    if let Some(line) = pr.iter().find(|line| child_key(line, 4) == Some("types")) {
        let types = inline_list(&pr, "types", 4);
        for lifecycle in ["opened", "synchronize", "reopened"] {
            assert!(
                types.contains(lifecycle),
                "pull_request `types:` dropped `{lifecycle}`: {line}"
            );
        }
    }
}

#[test]
fn gate_aggregates_every_ci_job() {
    let jobs = section(CI, "jobs");
    let names: BTreeSet<String> = jobs
        .iter()
        .filter_map(|line| child_key(line, 2))
        .map(str::to_owned)
        .collect();
    let gate = child_block(&jobs, "gate", 2);
    assert!(
        gate.iter().any(|line| line.trim() == "name: CI Gate"),
        "the required status is named `CI Gate`: renaming it hangs every PR \
         and queue entry that waits on it"
    );
    assert!(
        gate.iter().any(|line| line.trim() == "if: always()"),
        "the gate must report even when a dependency fails or is cancelled"
    );

    let needs = inline_list(&gate, "needs", 4);
    let expected: BTreeSet<String> = names
        .iter()
        .filter(|name| *name != "gate" && *name != "fixup-queue")
        .cloned()
        .collect();
    assert_eq!(
        needs, expected,
        "gate `needs:` must cover every ci.yml job except itself and \
         fixup-queue (which follows the gate)"
    );
    assert_eq!(
        needs,
        gate_inventory(&gate),
        "gate `needs:` drifted from the jq inventory the gate step asserts"
    );

    let fixup = child_block(&jobs, "fixup-queue", 2);
    let mut want = expected;
    want.insert("gate".to_owned());
    assert_eq!(
        inline_list(&fixup, "needs", 4),
        want,
        "fixup-queue must trail the gate and every job the gate watches"
    );
}

#[test]
fn merge_group_runs_never_cancel() {
    let cancel = section(CI, "concurrency")
        .iter()
        .map(|line| line.trim())
        .find(|line| line.starts_with("cancel-in-progress:"))
        .expect("ci.yml must set concurrency cancel-in-progress");
    assert!(
        cancel.contains("github.event_name == 'pull_request'"),
        "only superseded PR runs may cancel: a cancelled merge_group check \
         ejects the queue entry"
    );
}

#[test]
fn the_queue_waits_on_ci_gate_alone() {
    for path in workflow_files() {
        if path.file_name().unwrap() == "ci.yml" {
            continue;
        }
        let on = triggers(&fs::read_to_string(&path).unwrap());
        assert!(
            !on.contains("merge_group"),
            "{}: non-required workflows stay off the merge queue; entries \
             wait on CI Gate alone",
            path.display()
        );
    }
}

#[test]
fn non_gate_pull_request_workflows_do_not_run_on_every_pr() {
    // An unconditional PR workflow that ever becomes required hangs the queue
    // the same way #256 found. Non-gate PR triggers must stay scoped so they
    // remain opt-in evidence, not candidate checks.
    let lifecycle: BTreeSet<String> = ["opened", "synchronize", "reopened"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    for path in workflow_files() {
        if path.file_name().unwrap() == "ci.yml" {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap();
        let on = section(&text, "on");
        if !on
            .iter()
            .any(|line| child_key(line, 2) == Some("pull_request"))
        {
            continue;
        }
        let pr = child_block(&on, "pull_request", 2);
        let scoped = pr
            .iter()
            .any(|line| matches!(child_key(line, 4), Some("paths" | "paths-ignore")));
        let every_event = match pr.iter().find(|line| child_key(line, 4) == Some("types")) {
            // No `types:` means GitHub's default set: every PR lifecycle event.
            None => true,
            Some(_) => {
                let types = inline_list(&pr, "types", 4);
                lifecycle.iter().all(|event| types.contains(event))
            }
        };
        assert!(
            scoped || !every_event,
            "{}: a pull_request workflow that runs on every PR must be \
             paths-scoped (or type-limited) unless it joins the merge queue",
            path.display()
        );
    }
}

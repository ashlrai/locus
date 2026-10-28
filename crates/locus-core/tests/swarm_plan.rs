//! The queue handoff is data: reject plans that cannot be dispatched safely.
use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::{Component, Path};

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    version: u32,
    parent: String,
    baseline: String,
    jobs: Vec<Job>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Job {
    id: String,
    title: String,
    repo: String,
    pool: String,
    tier: String,
    priority: String,
    depends_on: Vec<String>,
    external_dependencies: Vec<String>,
    evidence: Vec<String>,
    gap: String,
    deliverable: String,
    acceptance: Vec<String>,
    validation: Vec<String>,
}

const DATA: &str = include_str!("../../../docs/swarm-child-jobs.json");
const INDEX: &str = include_str!("../../../docs/swarm-child-jobs.md");

fn plan() -> Plan {
    serde_json::from_str(DATA).unwrap()
}

fn validate(plan: &Plan) -> Result<(), &'static str> {
    if plan.version != 1 || plan.parent != "RQ-PLAN-LOCUS" || plan.baseline.is_empty() {
        return Err("invalid plan header");
    }
    if plan.jobs.is_empty() {
        return Err("empty backlog");
    }
    let ids: BTreeSet<_> = plan.jobs.iter().map(|j| j.id.as_str()).collect();
    if ids.len() != plan.jobs.len() {
        return Err("duplicate job");
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for job in &plan.jobs {
        if !job.id.starts_with("RQ-LOCUS-")
            || job.repo != "locus"
            || job.pool != "codex-personal"
            || job.tier != "frontier"
            || !["P0", "P1"].contains(&job.priority.as_str())
        {
            return Err("invalid routing");
        }
        if [&job.title, &job.gap, &job.deliverable]
            .iter()
            .any(|s| s.trim().is_empty())
            || job.acceptance.is_empty()
            || job.validation.is_empty()
            || job.evidence.is_empty()
            || job
                .acceptance
                .iter()
                .chain(&job.validation)
                .chain(&job.external_dependencies)
                .any(|s| s.trim().is_empty())
        {
            return Err("incomplete job");
        }
        let dependencies: BTreeSet<_> = job.depends_on.iter().collect();
        if dependencies.len() != job.depends_on.len()
            || dependencies.iter().any(|d| !ids.contains(d.as_str()))
        {
            return Err("invalid dependency");
        }
        for evidence in &job.evidence {
            let path = Path::new(evidence);
            if path
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
                || !root.join(path).is_file()
            {
                return Err("invalid evidence path");
            }
        }
    }
    // Topological elimination accepts unordered records but rejects cycles.
    let mut completed = BTreeSet::new();
    loop {
        let before = completed.len();
        for job in &plan.jobs {
            if job.depends_on.iter().all(|d| completed.contains(d)) {
                completed.insert(job.id.clone());
            }
        }
        if completed.len() == plan.jobs.len() {
            return Ok(());
        }
        if completed.len() == before {
            return Err("dependency cycle");
        }
    }
}

#[test]
fn backlog_is_complete_and_dispatchable() {
    validate(&plan()).unwrap();
    let mut reordered = plan();
    reordered.jobs.reverse();
    validate(&reordered).unwrap();
}

#[test]
fn dispatch_index_matches_canonical_jobs() {
    let indexed: BTreeSet<_> = INDEX
        .lines()
        .filter(|line| line.starts_with("| RQ-LOCUS-"))
        .map(|line| line.split('|').nth(1).unwrap().trim())
        .collect();
    let plan = plan();
    let ids: BTreeSet<_> = plan.jobs.iter().map(|j| j.id.as_str()).collect();
    assert_eq!(indexed, ids);
}

#[test]
fn rejects_unknown_fields_and_versions() {
    let mut value: serde_json::Value = serde_json::from_str(DATA).unwrap();
    value["jobs"][0]["approval"] = true.into();
    assert!(serde_json::from_value::<Plan>(value).is_err());
    let mut p = plan();
    p.version = 2;
    assert_eq!(validate(&p), Err("invalid plan header"));
}

#[test]
fn rejects_duplicate_missing_and_cyclic_dependencies() {
    let mut p = plan();
    p.jobs.push(p.jobs[0].clone());
    assert_eq!(validate(&p), Err("duplicate job"));
    let mut p = plan();
    p.jobs[0].depends_on.push("RQ-LOCUS-MISSING".into());
    assert_eq!(validate(&p), Err("invalid dependency"));
    let mut p = plan();
    let dependency = p.jobs[1].id.clone();
    p.jobs[0].depends_on.push(dependency);
    assert_eq!(validate(&p), Err("dependency cycle"));
    let mut p = plan();
    let dependency = p.jobs[0].id.clone();
    p.jobs[0].depends_on.push(dependency);
    assert_eq!(validate(&p), Err("dependency cycle"));
}

#[test]
fn rejects_incomplete_contracts_and_bad_routing() {
    let mut p = plan();
    p.jobs[0].acceptance.clear();
    assert_eq!(validate(&p), Err("incomplete job"));
    let mut p = plan();
    p.jobs[0].validation.clear();
    assert_eq!(validate(&p), Err("incomplete job"));
    let mut p = plan();
    p.jobs[0].priority = "urgent-ish".into();
    assert_eq!(validate(&p), Err("invalid routing"));
}

#[test]
fn rejects_missing_or_escaping_evidence() {
    for path in ["does-not-exist.rs", "../PLAN.md", "/tmp/PLAN.md"] {
        let mut p = plan();
        p.jobs[0].evidence = vec![path.into()];
        assert_eq!(validate(&p), Err("invalid evidence path"));
    }
}

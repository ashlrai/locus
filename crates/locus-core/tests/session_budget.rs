//! Integration: opt-in session budget caps at the policy gate.
//!
//! With no `[budget]` configured the gate behaves exactly as before; with a cap
//! the Nth+1 call in a session is refused with `session_budget_exhausted`.

use locus_core::adapters::ApprovalGate;
use locus_core::budget::{charge, peek, BudgetVerdict};
use locus_core::config::{BudgetConfig, LocusConfig};
use locus_core::{enforce_policy, Binding, BindingBody, Policy, ProviderBinding, Scope, Store};
use serde_json::json;
use std::fs;
use tempfile::tempdir;

fn binding() -> Binding {
    Binding::from_body(BindingBody {
        id: "bnd_budget".into(),
        alias: "budget".into(),
        tenant: "example".into(),
        principal: None,
        description: None,
        policy: Policy::default(),
        providers: vec![ProviderBinding {
            provider: "github".into(),
            account: "example-gh".into(),
            credential_ref: "env:EXAMPLE_GH_TOKEN".into(),
            scope: Scope {
                orgs: vec!["example".into()],
                ..Scope::default()
            },
            upstream: None,
        }],
    })
}

fn call(store: &Store, b: &Binding, session: &str) -> Option<serde_json::Value> {
    let gate = ApprovalGate {
        store,
        session_id: session,
        principal: None,
    };
    enforce_policy(b, "github.list_prs", &json!({}), Some(gate))
        .unwrap()
        .map(|r| r.content)
}

#[test]
fn unconfigured_budget_never_blocks_and_writes_nothing() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let b = binding();
    for _ in 0..100 {
        assert!(call(&store, &b, "ses_one").is_none());
    }
    assert!(!dir.path().join("budgets").exists());
}

#[test]
fn action_cap_refuses_after_n_calls_per_session() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store
        .save_config(&LocusConfig {
            budget: BudgetConfig {
                max_actions: Some(3),
                max_minutes: None,
            },
            ..Default::default()
        })
        .unwrap();
    let b = binding();
    for _ in 0..3 {
        assert!(call(&store, &b, "ses_one").is_none());
    }
    let blocked = call(&store, &b, "ses_one").expect("fourth call refused");
    assert_eq!(blocked["error"], "session_budget_exhausted");
    assert!(blocked["detail"].as_str().unwrap().contains("3 tool calls"));
    // Another session has its own budget.
    assert!(call(&store, &b, "ses_two").is_none());
}

#[test]
fn removing_the_cap_restores_unlimited_calls_for_new_sessions() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store
        .save_config(&LocusConfig {
            budget: BudgetConfig {
                max_actions: Some(1),
                max_minutes: None,
            },
            ..Default::default()
        })
        .unwrap();
    let b = binding();
    assert!(call(&store, &b, "ses_one").is_none());
    assert!(call(&store, &b, "ses_one").is_some());
    store.save_config(&LocusConfig::default()).unwrap();
    for _ in 0..10 {
        assert!(call(&store, &b, "ses_fresh").is_none());
    }
}

#[test]
fn config_round_trips_and_defaults_to_off() {
    let cfg = LocusConfig::parse("").unwrap();
    assert!(!cfg.budget.is_enabled());
    let cfg = LocusConfig::parse("[budget]\nmax_actions = 5\nmax_minutes = 30\n").unwrap();
    assert_eq!(cfg.budget.actions_cap(), Some(5));
    assert_eq!(cfg.budget.minutes_cap(), Some(30));
    let again = LocusConfig::parse(&cfg.to_toml().unwrap()).unwrap();
    assert_eq!(again, cfg);
}

fn enable_action_cap(store: &Store, cap: u64) {
    store
        .save_config(&LocusConfig {
            budget: BudgetConfig {
                max_actions: Some(cap),
                max_minutes: None,
            },
            ..Default::default()
        })
        .unwrap();
}

#[test]
fn invalid_session_ids_are_refused_before_any_budget_io() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    enable_action_cap(&store, 1);
    let b = binding();
    for id in ["", "../escape", "a/b", "ses a", "séance", &"a".repeat(81)] {
        let blocked = call(&store, &b, id).expect("invalid session refused");
        assert_eq!(blocked["error"], "session_budget_exhausted");
        assert!(peek(&store, id).is_none());
    }
    assert!(!dir.path().join("budgets").exists());
    assert!(!dir.path().join("escape.json").exists());
    // The inclusive length boundary and both permitted separators remain usable.
    let valid = format!("_-{}", "a".repeat(78));
    assert!(call(&store, &b, &valid).is_none());
    assert_eq!(peek(&store, &valid).unwrap().actions, 1);
}

#[test]
fn unreadable_budget_storage_propagates_an_error_through_the_policy_gate() {
    for fault in [
        "parent-file",
        "state-directory",
        "invalid-json",
        "invalid-utf8",
    ] {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        enable_action_cap(&store, 1);
        let budgets = dir.path().join("budgets");
        let state = budgets.join("ses_one.json");
        if fault == "parent-file" {
            fs::write(&budgets, "occupied").unwrap();
        } else {
            fs::create_dir(&budgets).unwrap();
            match fault {
                "state-directory" => fs::create_dir(&state).unwrap(),
                "invalid-json" => fs::write(&state, "{broken").unwrap(),
                "invalid-utf8" => fs::write(&state, [0xff]).unwrap(),
                _ => unreachable!(),
            }
        }
        let before = fs::read(&state).ok();
        assert!(peek(&store, "ses_one").is_none());
        let b = binding();
        let gate = ApprovalGate {
            store: &store,
            session_id: "ses_one",
            principal: None,
        };
        assert!(enforce_policy(&b, "github.list_prs", &json!({}), Some(gate)).is_err());
        assert_eq!(fs::read(&state).ok(), before, "{fault} must not be reset");
        assert!(peek(&store, "ses_one").is_none());
    }
}

#[test]
fn denied_and_approval_pending_calls_consume_budget_without_minting_more_approvals() {
    for expected in ["denied_by_policy", "requires_approval"] {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        enable_action_cap(&store, 1);
        let mut b = binding();
        if expected == "denied_by_policy" {
            b.policy.default = "deny".into();
        } else {
            b.policy.require_approval = vec!["github.*".into()];
        }
        assert_eq!(call(&store, &b, "ses_one").unwrap()["error"], expected);
        let pending = store.list_approvals().unwrap();
        assert_eq!(pending.len(), usize::from(expected == "requires_approval"));
        for _ in 0..2 {
            let blocked = call(&store, &b, "ses_one").unwrap();
            assert_eq!(blocked["error"], "session_budget_exhausted");
        }
        assert_eq!(peek(&store, "ses_one").unwrap().actions, 1);
        assert_eq!(store.list_approvals().unwrap().len(), pending.len());
    }
}

#[test]
fn audit_write_failure_cannot_release_an_exhausted_session() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    enable_action_cap(&store, 1);
    let b = binding();
    assert!(call(&store, &b, "ses_one").is_none());
    fs::create_dir(store.audit_path()).unwrap();
    assert_eq!(
        call(&store, &b, "ses_one").unwrap()["error"],
        "session_budget_exhausted"
    );
    let stopped = peek(&store, "ses_one").unwrap();
    assert!(stopped.stopped.is_some());
    fs::remove_dir(store.audit_path()).unwrap();
    assert_eq!(
        call(&store, &b, "ses_one").unwrap()["error"],
        "session_budget_exhausted"
    );
    assert_eq!(peek(&store, "ses_one").unwrap(), stopped);
    assert!(
        !store.audit_path().exists(),
        "sticky stops are not re-audited"
    );
}

#[test]
fn concurrent_calls_share_one_persisted_action_cap() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let cfg = BudgetConfig {
        max_actions: Some(3),
        max_minutes: None,
    };
    let barrier = std::sync::Barrier::new(12);
    let allowed = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..12)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    charge(&store, &cfg, "ses_shared", chrono::Utc::now()).unwrap()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(|verdict| *verdict == BudgetVerdict::Allowed)
            .count()
    });
    assert_eq!(allowed, 3);
    let state = peek(&store, "ses_shared").unwrap();
    assert_eq!(state.actions, 3);
    assert!(state.stopped.is_some());
    let audit = fs::read_to_string(store.audit_path()).unwrap();
    assert_eq!(audit.lines().count(), 1);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&audit).unwrap()["op"],
        "budget.exhausted"
    );
}

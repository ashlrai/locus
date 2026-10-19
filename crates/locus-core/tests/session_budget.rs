//! Integration: opt-in session budget caps at the policy gate.
//!
//! With no `[budget]` configured the gate behaves exactly as before; with a cap
//! the Nth+1 call in a session is refused with `session_budget_exhausted`.

use locus_core::adapters::ApprovalGate;
use locus_core::config::{BudgetConfig, LocusConfig};
use locus_core::{enforce_policy, Binding, BindingBody, Policy, ProviderBinding, Scope, Store};
use serde_json::json;
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

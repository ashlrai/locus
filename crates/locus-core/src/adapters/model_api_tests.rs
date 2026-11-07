//! Shared public contract for the two model API adapters. Exercise the adapter
//! boundary directly as well as dispatch, so a gate cannot mask a missing
//! provider-level freeze. All operations are local synthetic stubs.

use super::{adapter_for, call_tool};
use crate::binding::{Binding, BindingBody, Policy, PolicyRule, ProviderBinding, Scope};
use serde_json::{json, Value};

struct Case {
    provider: &'static str,
    selector: &'static str,
    aliases: [&'static str; 3],
}

const CASES: [Case; 2] = [
    Case {
        provider: "openai",
        selector: "project_id",
        aliases: ["project_id", "project", "project_ref"],
    },
    Case {
        provider: "anthropic",
        selector: "workspace_id",
        aliases: ["workspace_id", "workspace", "project_ref"],
    },
];

fn binding(case: &Case, frozen: bool) -> Binding {
    Binding::from_body(BindingBody {
        id: "bnd_model_contract".into(),
        alias: "model-contract".into(),
        tenant: "tenant-fixture".into(),
        principal: None,
        description: None,
        policy: Policy::default(),
        providers: vec![ProviderBinding {
            provider: case.provider.into(),
            account: "account-fixture".into(),
            // Never resolved. Responses must omit even this locator.
            credential_ref: "env:MODEL_CONTRACT_LOCATOR".into(),
            scope: Scope {
                account_id: frozen.then(|| "org-fixture".into()),
                project_ref: frozen.then(|| "partition-fixture".into()),
                read_only: Some(false),
                ..Scope::default()
            },
            upstream: None,
        }],
    })
}

#[test]
fn every_operation_preserves_the_pinned_spend_boundary() {
    for case in &CASES {
        let binding = binding(case, true);
        for suffix in ["scope", "whoami", "usage", "keys.list", "keys.create"] {
            let tool = format!("{}.{suffix}", case.provider);
            for args in [
                json!({}),
                json!({"org_id": "org-fixture", case.selector: "partition-fixture"}),
            ] {
                // whoami takes no selectors at dispatch, even matching ones.
                if suffix == "whoami" && args != json!({}) {
                    let error = call_tool(&binding, &tool, &args).unwrap_err();
                    assert!(error.to_string().contains("undeclared argument"));
                    continue;
                }
                let result = call_tool(&binding, &tool, &args).unwrap();
                assert!(result.ok, "{tool}");
                assert_eq!(result.content["org_id"], "org-fixture", "{tool}");
                assert_eq!(result.content[case.selector], "partition-fixture", "{tool}");
                let encoded = result.content.to_string();
                assert!(!encoded.contains("MODEL_CONTRACT_LOCATOR"), "{tool}");
                assert!(!encoded.contains("credential_ref"), "{tool}");
                if matches!(suffix, "scope" | "whoami") {
                    assert_eq!(result.content["tenant"], binding.tenant);
                    assert_eq!(result.content["binding"], binding.alias);
                    assert_eq!(result.content["credential"]["source"], "environment");
                    assert_eq!(result.content["credential"]["present"], false);
                    assert_eq!(
                        result.content["identity"],
                        format!("{}:org-fixture:partition-fixture", case.provider)
                    );
                } else {
                    assert_eq!(result.content["stub"], true, "{tool}");
                    assert_eq!(result.content["action"], suffix, "{tool}");
                }
                if suffix == "usage" {
                    assert_eq!(
                        result.content["spend_boundary"],
                        format!("{}:org-fixture:partition-fixture", case.provider)
                    );
                }
                if suffix == "keys.list" {
                    assert_eq!(result.content["keys"], json!([]));
                }
                if suffix == "keys.create" {
                    assert!(result.content["name"].is_null());
                    assert!(result.content.get("key").is_none());
                }
            }
        }
    }
}

#[test]
fn key_creation_only_echoes_the_label_and_remains_synthetic() {
    for case in &CASES {
        let binding = binding(case, true);
        let result = call_tool(
            &binding,
            &format!("{}.keys.create", case.provider),
            &json!({"confirm": true, "name": "fixture-label"}),
        )
        .unwrap();
        assert!(result.ok);
        assert_eq!(result.content["name"], "fixture-label");
        assert_eq!(result.content["stub"], true);
        assert!(result.content.get("key").is_none());
        assert!(result.content.get("credential").is_none());
    }
}

#[test]
fn adapters_refuse_every_conflicting_selector_alias_on_every_operation() {
    for case in &CASES {
        let binding = binding(case, true);
        let provider = &binding.providers[0];
        let adapter = adapter_for(case.provider).unwrap();
        for suffix in ["scope", "whoami", "usage", "keys.list", "keys.create"] {
            let tool = format!("{}.{suffix}", case.provider);
            for key in ["org_id", "account_id", "org", "organization"]
                .into_iter()
                .chain(case.aliases)
            {
                // Correct canonical values must not conceal a conflicting alias.
                let mut args = json!({
                    "org_id": "org-fixture",
                    case.selector: "partition-fixture",
                    "confirm": true
                });
                args[key] = json!("other-tenant");
                let error = adapter.call(&tool, &args, provider, &binding).unwrap_err();
                let message = error.to_string();
                assert!(message.contains("scope freeze"), "{tool}: {key}");
                assert!(message.contains(key), "{tool}: {key}");
            }
        }
    }
}

#[test]
fn matching_aliases_are_accepted_without_changing_identity() {
    for case in &CASES {
        let binding = binding(case, true);
        let adapter = adapter_for(case.provider).unwrap();
        let mut args = json!({
            "org_id": "org-fixture", "account_id": "org-fixture",
            "org": "org-fixture", "organization": "org-fixture"
        });
        for key in case.aliases {
            args[key] = json!("partition-fixture");
        }
        let result = adapter
            .call(
                &format!("{}.whoami", case.provider),
                &args,
                &binding.providers[0],
                &binding,
            )
            .unwrap();
        assert!(result.ok);
        assert_eq!(result.content["org_id"], "org-fixture");
        assert_eq!(result.content[case.selector], "partition-fixture");
    }
}

#[test]
fn unset_scope_reports_account_fallback_without_inventing_selectors() {
    for case in &CASES {
        let binding = binding(case, false);
        let adapter = adapter_for(case.provider).unwrap();
        assert_eq!(adapter.name(), case.provider);
        let tools = adapter.tools(&binding.providers[0], &binding);
        assert_eq!(tools.len(), 5);
        for tool in tools {
            assert_eq!(tool.provider, case.provider);
            assert_eq!(tool.destructive, tool.name.ends_with("keys.create"));
            assert_eq!(tool.input_schema["additionalProperties"], false);
        }
        for suffix in ["whoami", "usage"] {
            let result =
                call_tool(&binding, &format!("{}.{suffix}", case.provider), &json!({})).unwrap();
            assert!(result.ok);
            assert_eq!(result.content["org_id"], Value::Null);
            assert_eq!(result.content[case.selector], Value::Null);
            let identity_field = if suffix == "whoami" {
                "identity"
            } else {
                "spend_boundary"
            };
            assert_eq!(
                result.content[identity_field],
                format!("{}:account-fixture:*", case.provider)
            );
        }
    }
}

#[test]
fn unknown_tools_return_an_explicit_unsuccessful_result() {
    for case in &CASES {
        let binding = binding(case, true);
        let tool = format!("{}.missing", case.provider);
        let result = adapter_for(case.provider)
            .unwrap()
            .call(&tool, &json!({}), &binding.providers[0], &binding)
            .unwrap();
        assert!(!result.ok);
        assert_eq!(
            result.content["error"],
            format!("unknown {} tool: {tool}", case.provider)
        );
        assert!(result.policy.is_none());
    }
}

#[test]
fn key_creation_is_denied_by_policy_and_requires_external_approval_when_configured() {
    for case in &CASES {
        let mut binding = binding(case, true);
        let tool = format!("{}.keys.create", case.provider);
        binding.policy.rules = vec![PolicyRule::new("*.keys.create", "deny")];
        let denied = call_tool(&binding, &tool, &json!({"confirm": true})).unwrap();
        assert!(!denied.ok);
        assert_eq!(denied.content["error"], "denied_by_policy");

        binding.policy.rules.clear();
        binding.policy.require_approval = vec!["*.keys.create".into()];
        let blocked = call_tool(&binding, &tool, &json!({"confirm": true})).unwrap();
        assert!(!blocked.ok);
        assert_eq!(blocked.content["error"], "requires_approval");
        assert!(blocked.content.get("stub").is_none());
    }
}

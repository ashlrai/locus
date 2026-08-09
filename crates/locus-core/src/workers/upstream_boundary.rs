//! Closed request boundary for host-executed upstream MCP tools.

use crate::binding::{ProviderBinding, UpstreamToolCapability};
use crate::error::{LocusError, Result};
use serde_json::{Map, Value};

pub fn apply_capability(provider: &ProviderBinding, tool: &str, args: &Value) -> Result<Value> {
    let upstream = provider
        .upstream
        .as_ref()
        .ok_or_else(|| LocusError::msg("provider has no upstream capability manifest"))?;
    let capability = upstream.capabilities.get(tool).ok_or_else(|| {
        LocusError::msg(format!(
            "upstream capability denied: tool `{tool}` is not declared for provider `{}`",
            provider.provider
        ))
    })?;
    let supplied = args.as_object().ok_or_else(|| {
        LocusError::msg(format!(
            "upstream capability denied: arguments for `{tool}` must be an object"
        ))
    })?;

    for argument in supplied.keys() {
        if matches!(argument.as_str(), "confirm" | "approval_id") {
            continue;
        }
        if !capability.arguments.contains_key(argument) {
            return Err(LocusError::msg(format!(
                "upstream capability denied: argument `{argument}` has no declared semantics for `{tool}`"
            )));
        }
    }

    let mut enforced = Map::new();
    for (argument, semantics) in &capability.arguments {
        if semantics == "passthrough" {
            if let Some(value) = supplied.get(argument) {
                enforced.insert(argument.clone(), value.clone());
            }
            continue;
        }

        let allowed = selector_values(provider, semantics)?;
        let selected = match supplied.get(argument) {
            Some(Value::String(value)) if allowed.iter().any(|candidate| candidate == value) => {
                value.clone()
            }
            Some(Value::String(value)) => {
                return Err(LocusError::msg(format!(
                    "upstream scope freeze denied `{argument}={value}` for `{tool}`"
                )));
            }
            Some(_) => {
                return Err(LocusError::msg(format!(
                    "upstream capability denied: selector `{argument}` for `{tool}` must be a string"
                )));
            }
            None if allowed.len() == 1 => allowed[0].clone(),
            None => {
                return Err(LocusError::msg(format!(
                    "upstream capability denied: selector `{argument}` for `{tool}` must choose one frozen binding value"
                )));
            }
        };
        enforced.insert(argument.clone(), Value::String(selected));
    }

    Ok(Value::Object(enforced))
}

fn selector_values(provider: &ProviderBinding, semantics: &str) -> Result<Vec<String>> {
    let values = match semantics {
        "account" => vec![provider.account.clone()],
        "scope.project_ref" => provider.scope.project_ref.iter().cloned().collect(),
        "scope.team_id" => provider.scope.team_id.iter().cloned().collect(),
        "scope.account_id" => provider.scope.account_id.iter().cloned().collect(),
        "scope.orgs" => provider.scope.orgs.clone(),
        "scope.repos" => provider.scope.repos.clone(),
        "scope.projects" => provider.scope.projects.clone(),
        "scope.env" => provider.scope.env.clone(),
        _ => {
            return Err(LocusError::msg(format!(
                "upstream capability denied: unknown selector semantics `{semantics}`"
            )));
        }
    };
    if values.is_empty() {
        return Err(LocusError::msg(format!(
            "upstream capability denied: binding has no frozen value for `{semantics}`"
        )));
    }
    Ok(values)
}

pub fn constrain_input_schema(schema: &Value, capability: &UpstreamToolCapability) -> Value {
    let mut constrained = schema.as_object().cloned().unwrap_or_default();
    constrained.insert("type".into(), Value::String("object".into()));
    constrained.insert("additionalProperties".into(), Value::Bool(false));

    let upstream_properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut properties = Map::new();
    for (argument, semantics) in &capability.arguments {
        let property = upstream_properties
            .get(argument)
            .cloned()
            .unwrap_or_else(|| {
                if semantics == "passthrough" {
                    Value::Object(Map::new())
                } else {
                    serde_json::json!({ "type": "string" })
                }
            });
        properties.insert(argument.clone(), property);
    }
    constrained.insert("properties".into(), Value::Object(properties));

    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        let required: Vec<Value> = required
            .iter()
            .filter(|name| {
                name.as_str().is_some_and(|name| {
                    capability.arguments.get(name).map(String::as_str) == Some("passthrough")
                })
            })
            .cloned()
            .collect();
        constrained.insert("required".into(), Value::Array(required));
    }

    Value::Object(constrained)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::{Scope, UpstreamSpec};

    fn provider() -> ProviderBinding {
        ProviderBinding::new("github", "acme-account", "env:TOKEN")
            .with_scope(Scope {
                project_ref: Some("project-acme".into()),
                team_id: Some("team-acme".into()),
                orgs: vec!["org-acme".into()],
                ..Scope::default()
            })
            .with_upstream(
                UpstreamSpec::new("mock")
                    .unsafe_host_execution(true)
                    .with_capability(
                        "inspect",
                        UpstreamToolCapability::new()
                            .with_argument("message", "passthrough")
                            .with_argument("account", "account")
                            .with_argument("org", "scope.orgs")
                            .with_argument("project", "scope.project_ref")
                            .with_argument("team", "scope.team_id"),
                    ),
            )
    }

    #[test]
    fn injects_frozen_selectors_and_preserves_declared_passthrough() {
        let result = apply_capability(
            &provider(),
            "inspect",
            &serde_json::json!({
                "message": "hello",
                "confirm": true,
                "approval_id": "appr_aabbccddeeff001122334455"
            }),
        )
        .unwrap();
        assert_eq!(result["message"], "hello");
        assert_eq!(result["account"], "acme-account");
        assert_eq!(result["org"], "org-acme");
        assert_eq!(result["project"], "project-acme");
        assert_eq!(result["team"], "team-acme");
        assert!(result.get("confirm").is_none());
        assert!(result.get("approval_id").is_none());
    }

    #[test]
    fn denies_alternate_unknown_and_undeclared_selector_semantics() {
        let provider = provider();
        for args in [
            serde_json::json!({ "account": "other" }),
            serde_json::json!({ "org": "other" }),
            serde_json::json!({ "project": "other" }),
            serde_json::json!({ "team": "other" }),
            serde_json::json!({ "organization": "other" }),
        ] {
            assert!(apply_capability(&provider, "inspect", &args).is_err());
        }
        assert!(apply_capability(&provider, "undeclared", &serde_json::json!({})).is_err());
    }
}

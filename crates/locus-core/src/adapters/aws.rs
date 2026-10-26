use super::{freeze_string_arg, AdapterTool, ProviderAdapter, ToolCallResult};
use crate::binding::{Binding, ProviderBinding};
use crate::error::{LocusError, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use zeroize::Zeroizing;

pub struct AwsAdapter;

pub(crate) const CREDENTIAL_ENV_KEYS: &[&str] = &[
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
];

/// Decode one pinned credential reference, never combining it with ambient
/// AWS identity. Scalars retain the legacy secret-key-only mapping; the CLI
/// route requires the complete pair and refuses that incomplete legacy form.
pub(crate) fn credential_env(value: &str) -> Result<BTreeMap<String, Zeroizing<String>>> {
    let invalid = || LocusError::msg("invalid AWS credential bundle");
    if !value.trim_start().starts_with('{') {
        return Ok(BTreeMap::from([(
            "AWS_SECRET_ACCESS_KEY".into(),
            Zeroizing::new(value.to_owned()),
        )]));
    }

    // Zeroize decoded strings on both success and partial-deserialization
    // failure. Parser diagnostics can contain input, so only a closed error
    // leaves this adapter.
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Bundle {
        #[serde(rename = "AWS_ACCESS_KEY_ID", deserialize_with = "secret")]
        access_key_id: Zeroizing<String>,
        #[serde(rename = "AWS_SECRET_ACCESS_KEY", deserialize_with = "secret")]
        secret_access_key: Zeroizing<String>,
        #[serde(
            rename = "AWS_SESSION_TOKEN",
            default,
            deserialize_with = "optional_secret"
        )]
        session_token: Option<Zeroizing<String>>,
    }

    fn secret<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Zeroizing<String>, D::Error> {
        String::deserialize(deserializer).map(Zeroizing::new)
    }

    fn optional_secret<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Option<Zeroizing<String>>, D::Error> {
        // Missing is allowed; an explicitly supplied null is not a credential.
        secret(deserializer).map(Some)
    }

    let bundle: Bundle = serde_json::from_str(value).map_err(|_| invalid())?;
    let mut env = BTreeMap::from([
        ("AWS_ACCESS_KEY_ID".into(), bundle.access_key_id),
        ("AWS_SECRET_ACCESS_KEY".into(), bundle.secret_access_key),
    ]);
    if let Some(token) = bundle.session_token {
        env.insert("AWS_SESSION_TOKEN".into(), token);
    }
    if env
        .values()
        .any(|value| value.trim().is_empty() || value.chars().any(char::is_control))
    {
        return Err(invalid());
    }
    Ok(env)
}

fn extra_str(provider: &ProviderBinding, key: &str) -> Option<String> {
    provider
        .scope
        .extra
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

impl ProviderAdapter for AwsAdapter {
    fn name(&self) -> &'static str {
        "aws"
    }

    fn cli_name(&self) -> Option<&'static str> {
        Some("aws")
    }

    fn cli_required_credentials(&self) -> &'static [&'static str] {
        &["AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY"]
    }

    fn configure_cli_env(&self, provider: &ProviderBinding, vars: &mut BTreeMap<String, String>) {
        vars.insert("AWS_EC2_METADATA_DISABLED".into(), "true".into());
        vars.insert("AWS_IGNORE_CONFIGURED_ENDPOINT_URLS".into(), "true".into());
        if let Some(region) = extra_str(provider, "region") {
            vars.insert("AWS_REGION".into(), region.clone());
            vars.insert("AWS_DEFAULT_REGION".into(), region);
        }
    }

    fn tools(&self, provider: &ProviderBinding, binding: &Binding) -> Vec<AdapterTool> {
        let account_id = provider.scope.account_id.as_deref().unwrap_or("<unset>");
        let profile = extra_str(provider, "profile").unwrap_or_else(|| "<unset>".into());
        let role = extra_str(provider, "role_arn").unwrap_or_else(|| "<unset>".into());
        let region = extra_str(provider, "region").unwrap_or_else(|| "<unset>".into());
        vec![
            AdapterTool {
                name: "aws.scope".into(),
                description: format!(
                    "Frozen AWS scope for tenant `{}`: account_id={account_id}, profile={profile}, role={role}, region={region}.",
                    binding.tenant
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "account_id": {
                            "type": "string",
                            "description": "Ignored if binding freezes account_id; mismatch is denied."
                        },
                        "profile": {
                            "type": "string",
                            "description": "Ignored if binding freezes profile; mismatch is denied."
                        },
                        "region": {
                            "type": "string",
                            "description": "Ignored if binding freezes region; mismatch is denied."
                        }
                    },
                    "additionalProperties": false
                }),
                provider: "aws".into(),
                destructive: false,
            },
            AdapterTool {
                name: "aws.whoami".into(),
                description: format!(
                    "Show which AWS identity this pin uses: account_id={account_id}, profile={profile}, region={region}. Private AWS config dir — no STS in phase 2 scaffolding."
                ),
                input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
                provider: "aws".into(),
                destructive: false,
            },
        ]
    }

    fn call(
        &self,
        tool: &str,
        args: &Value,
        provider: &ProviderBinding,
        binding: &Binding,
    ) -> Result<ToolCallResult> {
        let frozen_account = provider.scope.account_id.as_deref();
        let account_id = freeze_string_arg(args, "account_id", frozen_account)?;

        let frozen_profile = provider.scope.extra.get("profile").and_then(|v| v.as_str());
        let profile = freeze_string_arg(args, "profile", frozen_profile)?;

        let frozen_region = provider.scope.extra.get("region").and_then(|v| v.as_str());
        let region = freeze_string_arg(args, "region", frozen_region)?;

        let role_arn = extra_str(provider, "role_arn");
        let resolved_account = account_id
            .clone()
            .or_else(|| provider.scope.account_id.clone());
        let resolved_profile = profile
            .clone()
            .or_else(|| frozen_profile.map(str::to_string));
        let resolved_region = region.clone().or_else(|| frozen_region.map(str::to_string));

        match tool {
            "aws.scope" | "aws.whoami" => Ok(ToolCallResult {
                ok: true,
                content: json!({
                    "provider": "aws",
                    "account": provider.account,
                    "account_id": resolved_account,
                    "profile": resolved_profile,
                    "role_arn": role_arn,
                    "region": resolved_region,
                    "credential": crate::credential::credential_metadata(&provider.credential_ref),
                    "tenant": binding.tenant,
                    "binding": binding.alias,
                    "frozen_selectors": ["account_id", "profile", "region", "role_arn"],
                    "identity": format!(
                        "aws:{}:{}",
                        resolved_account.as_deref().unwrap_or(&provider.account),
                        resolved_profile.as_deref().unwrap_or("default")
                    ),
                    "worker_hint": "locus exec sets AWS_CONFIG_FILE / AWS_SHARED_CREDENTIALS_FILE to private session dirs — never mutates global ~/.aws",
                    "note": "Phase 2 identity tool — real AWS CLI/MCP fan-out uses private config + keys from resolved credential_ref."
                }),
                policy: None,
            }),
            other => Ok(ToolCallResult {
                ok: false,
                content: json!({"error": format!("unknown aws tool: {other}")}),
                policy: None,
            }),
        }
    }
}

#[cfg(test)]
mod credential_tests {
    use super::{credential_env, AwsAdapter, ProviderAdapter};

    #[test]
    fn bundle_maps_each_credential_to_its_own_key() {
        let env = credential_env(
            r#"{"AWS_ACCESS_KEY_ID":"fake-id","AWS_SECRET_ACCESS_KEY":"fake-secret","AWS_SESSION_TOKEN":"fake-session"}"#,
        )
        .unwrap();
        assert!(env.len() == 3);
        assert!(env["AWS_ACCESS_KEY_ID"].as_str() == "fake-id");
        assert!(env["AWS_SECRET_ACCESS_KEY"].as_str() == "fake-secret");
        assert!(env["AWS_SESSION_TOKEN"].as_str() == "fake-session");
    }

    #[test]
    fn long_lived_bundle_omits_session_token() {
        let env = credential_env(
            r#" {"AWS_ACCESS_KEY_ID":"fake-id","AWS_SECRET_ACCESS_KEY":"fake-secret"} "#,
        )
        .unwrap();
        assert!(env.len() == 2);
        assert!(!env.contains_key("AWS_SESSION_TOKEN"));
    }

    #[test]
    fn legacy_scalar_never_becomes_a_key_id_or_session_token() {
        let env = credential_env("fake-legacy-secret").unwrap();
        assert!(env.len() == 1);
        assert!(env["AWS_SECRET_ACCESS_KEY"].as_str() == "fake-legacy-secret");
        assert!(!env.contains_key("AWS_ACCESS_KEY_ID"));
        assert!(!env.contains_key("AWS_SESSION_TOKEN"));
    }

    #[test]
    fn invalid_bundles_fail_with_closed_error() {
        for value in [
            "{not-json",
            "{}",
            r#"{"AWS_ACCESS_KEY_ID":"fake-id"}"#,
            r#"{"AWS_ACCESS_KEY_ID":"fake-id","AWS_SECRET_ACCESS_KEY":""}"#,
            r#"{"AWS_ACCESS_KEY_ID":" ","AWS_SECRET_ACCESS_KEY":"fake-secret"}"#,
            r#"{"AWS_ACCESS_KEY_ID":123,"AWS_SECRET_ACCESS_KEY":"fake-secret"}"#,
            r#"{"AWS_ACCESS_KEY_ID":"fake-id","AWS_SECRET_ACCESS_KEY":"fake-secret","AWS_SESSION_TOKEN":null}"#,
            r#"{"AWS_ACCESS_KEY_ID":"fake-id","AWS_SECRET_ACCESS_KEY":"fake-secret","AWS_SESSION_TOKEN":""}"#,
            r#"{"AWS_ACCESS_KEY_ID":"fake-id","AWS_SECRET_ACCESS_KEY":"fake-secret","AWS_SESSION_TOKEN":"fake\nsession"}"#,
            r#"{"AWS_ACCESS_KEY_ID":"fake-id","AWS_SECRET_ACCESS_KEY":"fake\u0000secret"}"#,
            r#"{"AWS_ACCESS_KEY_ID":"fake-id","AWS_SECRET_ACCESS_KEY":"fake-secret","AWS_PROFILE":"foreign"}"#,
            r#"{"AWS_ACCESS_KEY_ID":"fake-id","AWS_ACCESS_KEY_ID":"other-id","AWS_SECRET_ACCESS_KEY":"fake-secret"}"#,
            r#"{"AWS_ACCESS_KEY_ID":"fake-id","AWS_SECRET_ACCESS_KEY":"fake-secret"} trailing"#,
        ] {
            let result = credential_env(value);
            assert!(result.is_err());
            assert!(result.err().unwrap().to_string() == "invalid AWS credential bundle");
        }
    }

    #[test]
    fn cli_configuration_uses_pinned_region_and_disables_fallback_identity() {
        let binding = crate::binding::Binding::parse_toml(
            r#"
id = "bnd_aws"
alias = "aws-test"
tenant = "synthetic"
[[providers]]
provider = "aws"
account = "synthetic"
credential_ref = "env:AWS_TEST_BUNDLE"
scope = { region = "us-east-2" }
"#,
        )
        .unwrap();
        let mut vars = std::collections::BTreeMap::new();
        AwsAdapter.configure_cli_env(&binding.providers[0], &mut vars);
        assert_eq!(vars["AWS_REGION"], "us-east-2");
        assert_eq!(vars["AWS_DEFAULT_REGION"], "us-east-2");
        assert_eq!(vars["AWS_EC2_METADATA_DISABLED"], "true");
        assert_eq!(vars["AWS_IGNORE_CONFIGURED_ENDPOINT_URLS"], "true");
        assert!(!vars.contains_key("AWS_PROFILE"));
    }
}

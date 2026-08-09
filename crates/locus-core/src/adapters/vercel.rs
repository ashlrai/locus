use super::{freeze_string_arg, AdapterTool, ProviderAdapter, ToolCallResult};
use crate::binding::{Binding, ProviderBinding};
use crate::error::{LocusError, Result};
use serde_json::{json, Value};

pub struct VercelAdapter;

/// Freeze env target (production / preview / development / …) against scope.env allowlist.
///
/// Accepts `env` or `target` arg keys. Empty allowlist ⇒ no restriction.
fn freeze_env_target(args: &Value, allowlist: &[String]) -> Result<Option<String>> {
    let model = args
        .get("env")
        .or_else(|| args.get("target"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());

    match model {
        Some(m) if !allowlist.is_empty() => {
            let ok = allowlist
                .iter()
                .any(|a| a.eq_ignore_ascii_case(m) || a == "*");
            if !ok {
                return Err(LocusError::msg(format!(
                    "scope freeze: refusing env/target={m:?}; binding freezes env={allowlist:?}"
                )));
            }
            Ok(Some(m.to_string()))
        }
        Some(m) => Ok(Some(m.to_string())),
        None => Ok(None),
    }
}

impl ProviderAdapter for VercelAdapter {
    fn name(&self) -> &'static str {
        "vercel"
    }

    fn tools(&self, provider: &ProviderBinding, binding: &Binding) -> Vec<AdapterTool> {
        let team = provider.scope.team_id.as_deref().unwrap_or("<unset>");
        let env_hint = if provider.scope.env.is_empty() {
            "<any>".into()
        } else {
            provider.scope.env.join(",")
        };
        vec![
            AdapterTool {
                name: "vercel.scope".into(),
                description: format!(
                    "Frozen Vercel scope for tenant `{}`: team_id={team}, projects={:?}, env=[{env_hint}]. team_id and env target are frozen.",
                    binding.tenant, provider.scope.projects
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "team_id": {
                            "type": "string",
                            "description": "Ignored if binding freezes team_id; mismatch is denied."
                        },
                        "env": {
                            "type": "string",
                            "description": "Env target (production|preview|development). Denied if outside binding scope.env allowlist."
                        },
                        "target": {
                            "type": "string",
                            "description": "Alias of env (production|preview|development)."
                        }
                    },
                    "additionalProperties": false
                }),
                provider: "vercel".into(),
                destructive: false,
            },
            AdapterTool {
                name: "vercel.deploy.prod".into(),
                description: format!(
                    "SYNTHETIC prod deploy stub — policy-gated. Frozen team_id={team}, env=[{env_hint}]. Does not deploy in phase 1."
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "confirm": { "type": "boolean" },
                        "team_id": {
                            "type": "string",
                            "description": "Ignored if binding freezes team_id; mismatch is denied."
                        },
                        "env": {
                            "type": "string",
                            "description": "Env target; denied if outside binding scope.env allowlist."
                        },
                        "target": {
                            "type": "string",
                            "description": "Alias of env."
                        }
                    },
                    "additionalProperties": false
                }),
                provider: "vercel".into(),
                destructive: true,
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
        let frozen_team = provider.scope.team_id.as_deref();
        let team_id = freeze_string_arg(args, "team_id", frozen_team)?;
        // Also reject smuggled `team` key when team_id is frozen.
        let _ = freeze_string_arg(args, "team", frozen_team)?;
        let env_target = freeze_env_target(args, &provider.scope.env)?;

        match tool {
            "vercel.scope" => Ok(ToolCallResult {
                ok: true,
                content: json!({
                    "provider": "vercel",
                    "account": provider.account,
                    "team_id": team_id.or_else(|| provider.scope.team_id.clone()),
                    "projects": provider.scope.projects,
                    "env": provider.scope.env,
                    "env_target": env_target,
                    "credential": crate::credential::credential_metadata(&provider.credential_ref),
                    "tenant": binding.tenant,
                    "binding": binding.alias,
                    "frozen_selectors": ["team_id", "env", "projects"],
                    "note": "Phase 1 identity tool — remote Vercel MCP/API fan-out lands next."
                }),
                policy: None,
            }),
            "vercel.deploy.prod" => Ok(ToolCallResult {
                ok: true,
                content: json!({
                    "stub": true,
                    "action": "deploy.prod",
                    "team_id": team_id.or_else(|| provider.scope.team_id.clone()),
                    "projects": provider.scope.projects,
                    "env": provider.scope.env,
                    "env_target": env_target,
                    "message": "Synthetic tool — no deployment created."
                }),
                policy: None,
            }),
            other => Ok(ToolCallResult {
                ok: false,
                content: json!({"error": format!("unknown vercel tool: {other}")}),
                policy: None,
            }),
        }
    }
}

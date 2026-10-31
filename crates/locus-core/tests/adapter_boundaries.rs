use locus_core::adapters::{CliRoute, ProviderAdapter, ResendAdapter};
use locus_core::credential::CredentialResolutionIssue;
use locus_core::isolation::IsolatedEnv;
use locus_core::{Binding, BindingBody, Policy, ProviderBinding, Scope};
use serde_json::json;

fn binding(providers: &[&str]) -> Binding {
    Binding::from_body(BindingBody {
        id: "bnd_boundary".into(),
        alias: "boundary".into(),
        tenant: "example".into(),
        principal: None,
        description: None,
        policy: Policy::default(),
        providers: providers
            .iter()
            .map(|name| ProviderBinding {
                provider: (*name).into(),
                account: "example-account".into(),
                credential_ref: "env:BOUNDARY_FIXTURE".into(),
                scope: Scope::default(),
                upstream: None,
            })
            .collect(),
    })
}

fn isolated() -> IsolatedEnv {
    IsolatedEnv {
        vars: Default::default(),
        scrubbed: vec![],
        secrets_resolved: 0,
        secrets_failed: vec![],
    }
}

#[test]
fn cli_routes_exactly_one_pinned_provider_without_mutating_binding() {
    let b = binding(&["GitHub", "vercel", "supabase", "aws"]);
    for (cli, provider) in [
        ("gh", "github"),
        ("vercel", "vercel"),
        ("supabase", "supabase"),
        ("aws", "aws"),
    ] {
        let route = CliRoute::resolve(&b, cli, &format!("/trusted/bin/{cli}")).unwrap();
        assert_eq!(route.provider_name(), provider);
        assert_eq!(route.binding().providers.len(), 1);
        assert!(route.binding().providers[0]
            .provider
            .eq_ignore_ascii_case(provider));
        assert_eq!(route.binding().tenant, b.tenant);
        assert_eq!(route.binding().alias, b.alias);
    }
    assert_eq!(b.providers.len(), 4);
}

#[test]
fn cli_route_refuses_unsupported_mismatched_missing_and_ambiguous_identity() {
    for (providers, cli, program, message) in [
        (vec!["github"], "curl", "curl", "unsupported CLI"),
        (vec!["github"], "gh", "/bin/sh", "must match"),
        (vec!["vercel"], "gh", "gh", "not pinned"),
        (vec!["github", "GITHUB"], "gh", "gh", "ambiguous CLI route"),
    ] {
        let Err(error) = CliRoute::resolve(&binding(&providers), cli, program) else {
            panic!("invalid route accepted");
        };
        assert!(error.to_string().contains(message));
    }
}

#[test]
fn routed_aws_requires_both_keys_before_applying_pinned_defaults() {
    let mut b = binding(&["aws"]);
    b.providers[0]
        .scope
        .extra
        .insert("region".into(), "us-west-2".into());
    let route = CliRoute::resolve(&b, "aws", "aws").unwrap();
    for invalid in [
        None,
        Some(""),
        Some(" \t"),
        Some("fixture\0"),
        Some("fixture\r"),
        Some("fixture\n"),
    ] {
        for key in ["AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY"] {
            let mut env = isolated();
            env.vars
                .insert("AWS_ACCESS_KEY_ID".into(), "fixture-id".into());
            env.vars
                .insert("AWS_SECRET_ACCESS_KEY".into(), "fixture-key".into());
            env.vars.remove(key);
            if let Some(value) = invalid {
                env.vars.insert(key.into(), value.into());
            }
            let before = env.vars.clone();
            assert!(route.prepare_env(&mut env).is_err());
            assert_eq!(env.vars, before);
        }
    }
    let mut env = isolated();
    env.vars
        .insert("AWS_ACCESS_KEY_ID".into(), "fixture-id".into());
    env.vars
        .insert("AWS_SECRET_ACCESS_KEY".into(), "fixture-key".into());
    env.secrets_failed.push(CredentialResolutionIssue {
        provider: "aws".into(),
        source: "env".into(),
        code: "missing".into(),
    });
    assert!(route.prepare_env(&mut env).is_err());
    env.secrets_failed.clear();
    route.prepare_env(&mut env).unwrap();
    assert_eq!(env.vars["AWS_REGION"], "us-west-2");
    assert_eq!(env.vars["AWS_DEFAULT_REGION"], "us-west-2");
    assert_eq!(env.vars["AWS_EC2_METADATA_DISABLED"], "true");
    assert_eq!(env.vars["AWS_IGNORE_CONFIGURED_ENDPOINT_URLS"], "true");
}

#[test]
fn resend_domain_sources_preserve_allowlist_precedence_and_wildcards() {
    let b = binding(&["resend"]);
    for (projects, extra, allowed, denied, domains) in [
        (
            vec!["primary.example"],
            json!(["other.example"]),
            "primary.example",
            Some("other.example"),
            json!(["primary.example"]),
        ),
        (
            vec![],
            json!([7, "extra.example", false]),
            "extra.example",
            Some("other.example"),
            json!(["extra.example"]),
        ),
        (vec![], json!(["*"]), "any.example", None, json!(["*"])),
        (vec![], json!(false), "any.example", None, json!([])),
    ] {
        let mut p = b.providers[0].clone();
        p.scope.projects = projects.into_iter().map(str::to_string).collect();
        p.scope
            .extra
            .insert("domains".into(), toml::Value::try_from(extra).unwrap());
        let tools = ResendAdapter.tools(&p, &b);
        assert_eq!(tools.len(), 2);
        assert!(tools
            .iter()
            .all(|t| !t.destructive && t.provider == "resend"));
        for tool in ["resend.scope", "resend.whoami"] {
            let result = ResendAdapter
                .call(tool, &json!({"domain": allowed}), &p, &b)
                .unwrap();
            assert!(result.ok);
            assert_eq!(result.content["domain"], allowed);
            assert_eq!(result.content["domains"], domains);
            assert_eq!(result.content["tenant"], "example");
            assert_eq!(result.content["account"], "example-account");
        }
        if let Some(domain) = denied {
            assert!(ResendAdapter
                .call("resend.scope", &json!({"domain": domain}), &p, &b)
                .unwrap_err()
                .to_string()
                .contains("scope freeze"));
        }
        let unknown = ResendAdapter
            .call("resend.unknown", &json!({}), &p, &b)
            .unwrap();
        assert!(!unknown.ok);
        assert!(unknown.content["error"]
            .as_str()
            .unwrap()
            .contains("unknown resend tool"));
    }
}

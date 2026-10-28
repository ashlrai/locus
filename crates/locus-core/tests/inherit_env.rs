//! Integration: opt-in `[exec] inherit_env` at the isolated env builder.
//!
//! One test function, because it sets LOCUS_HOME and parent variables for this
//! test binary. The default-path pin in `tests/isolation.rs`
//! (`gateway_and_proxy_variables_from_the_parent_never_reach_the_child`) is a
//! separate binary and is unchanged.

use chrono::Duration;
use locus_core::config::{ExecConfig, GatewayConfig, LocusConfig};
use locus_core::seal::SealKey;
use locus_core::session::{PinSource, Session};
use locus_core::{
    build_isolated_env_opts, Binding, BindingBody, Policy, ProviderBinding, Scope, Store,
};
use tempfile::tempdir;

const BOUND: &str = "inherit-test-bound-credential-789";
const REAL: &str = "sk-inherit-test-REALVALUE-0123";
const PLACEHOLDER: &str = "phm_inheritsession01";
const PROXY: &str = "http://127.0.0.1:41000/anthropic/_phantom/tok123/";

fn binding() -> Binding {
    Binding::from_body(BindingBody {
        id: "bnd_inherit".into(),
        alias: "inherit".into(),
        tenant: "inherit".into(),
        principal: None,
        description: None,
        policy: Policy::default(),
        providers: vec![ProviderBinding {
            provider: "supabase".into(),
            account: "inherit".into(),
            credential_ref: "env:LOCUS_INHERIT_TEST_TOKEN".into(),
            scope: Scope::default(),
            upstream: None,
        }],
    })
}

fn session(b: &Binding) -> Session {
    let key = SealKey::generate();
    let session = Session::new(
        &b.id,
        &b.alias,
        &b.tenant,
        None,
        PinSource::Explicit,
        None,
        Duration::hours(1),
        "/tmp/locus-inherit-test".into(),
        &key,
    );
    session.verify_seal(&key).unwrap();
    session
}

fn names(n: &[&str]) -> Vec<String> {
    n.iter().map(|s| s.to_string()).collect()
}

fn cfg(inherit: &[&str], gateway: GatewayConfig) -> LocusConfig {
    LocusConfig {
        exec: ExecConfig {
            inherit_env: names(inherit),
        },
        gateway,
        ..Default::default()
    }
}

#[test]
fn inherit_env_end_to_end() {
    let home = tempdir().unwrap();
    std::env::set_var("LOCUS_HOME", home.path());
    std::env::set_var("LOCUS_INHERIT_TEST_TOKEN", BOUND);
    std::env::set_var("ANTHROPIC_API_KEY", PLACEHOLDER);
    std::env::set_var("ANTHROPIC_BASE_URL", PROXY);
    std::env::set_var("PHANTOM_PROXY_PORT", "41000");
    std::env::set_var("OPENAI_API_KEY", REAL);
    std::env::set_var("SUPABASE_ACCESS_TOKEN", "phm_parentsupabase");
    let store = Store::open(home.path()).unwrap();
    let b = binding();
    let s = session(&b);

    // 1. Default (no config): nothing is inherited, exactly as before.
    let iso = build_isolated_env_opts(&s, &b, true);
    for k in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_BASE_URL",
        "PHANTOM_PROXY_PORT",
        "OPENAI_API_KEY",
    ] {
        assert!(!iso.vars.contains_key(k), "{k}");
        assert!(iso.scrubbed.iter().any(|s| s == k), "{k} scrubbed");
    }
    assert_eq!(
        iso.vars.get("SUPABASE_ACCESS_TOKEN").map(String::as_str),
        Some(BOUND)
    );
    assert!(iso.secrets_failed.is_empty());
    let baseline: Vec<String> = iso.vars.keys().cloned().collect();

    // 2. An empty list is the same as no config.
    store
        .save_config(&cfg(&[], GatewayConfig::default()))
        .unwrap();
    let iso = build_isolated_env_opts(&s, &b, true);
    assert_eq!(iso.vars.keys().cloned().collect::<Vec<_>>(), baseline);
    assert!(iso.secrets_failed.is_empty());

    // 3. The phantom exec route: placeholder, proxy URL and port cross. The real
    //    OPENAI_API_KEY does not, and the refusal carries no name or value.
    //    The bound credential still wins over the parent's placeholder.
    store
        .save_config(&cfg(
            &[
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_BASE_URL",
                "PHANTOM_PROXY_PORT",
                "OPENAI_API_KEY",
                "SUPABASE_ACCESS_TOKEN",
            ],
            GatewayConfig::default(),
        ))
        .unwrap();
    let iso = build_isolated_env_opts(&s, &b, true);
    assert_eq!(
        iso.vars.get("ANTHROPIC_API_KEY").map(String::as_str),
        Some(PLACEHOLDER)
    );
    assert_eq!(
        iso.vars.get("ANTHROPIC_BASE_URL").map(String::as_str),
        Some(PROXY)
    );
    assert_eq!(
        iso.vars.get("PHANTOM_PROXY_PORT").map(String::as_str),
        Some("41000")
    );
    assert!(!iso.vars.contains_key("OPENAI_API_KEY"));
    assert!(iso.vars.values().all(|v| !v.contains("REALVALUE")));
    assert_eq!(
        iso.vars.get("SUPABASE_ACCESS_TOKEN").map(String::as_str),
        Some(BOUND)
    );
    assert!(!iso.scrubbed.iter().any(|s| s == "ANTHROPIC_API_KEY"));
    assert!(iso.scrubbed.iter().any(|s| s == "OPENAI_API_KEY"));
    assert_eq!(iso.secrets_failed.len(), 1);
    assert_eq!(iso.secrets_failed[0].code, "inherit-env-secret-refused");
    let report = format!("{:?}", iso.secrets_failed);
    assert!(
        !report.contains("REALVALUE") && !report.contains("OPENAI_API_KEY"),
        "{report}"
    );

    // 4. The [gateway] URL wins over an inherited base URL of the same name.
    store
        .save_config(&cfg(
            &["ANTHROPIC_BASE_URL"],
            GatewayConfig {
                enabled: true,
                base_url: Some("http://127.0.0.1:4000".into()),
                child_env: vec!["ANTHROPIC_BASE_URL".into()],
                allowed_hosts: vec![],
            },
        ))
        .unwrap();
    let iso = build_isolated_env_opts(&s, &b, true);
    assert_eq!(
        iso.vars.get("ANTHROPIC_BASE_URL").map(String::as_str),
        Some("http://127.0.0.1:4000")
    );
    assert!(iso.secrets_failed.is_empty());

    // 5. A parent base URL that fails the gateway rules does not cross.
    std::env::set_var("ANTHROPIC_BASE_URL", "http://gw.example.com/v1");
    store
        .save_config(&cfg(&["ANTHROPIC_BASE_URL"], GatewayConfig::default()))
        .unwrap();
    let iso = build_isolated_env_opts(&s, &b, true);
    assert!(!iso.vars.contains_key("ANTHROPIC_BASE_URL"));
    assert_eq!(iso.secrets_failed[0].code, "inherit-env-base-url-refused");

    // 6. A glob (or any invalid list) inherits nothing; credentials still resolve.
    store
        .save_config(&cfg(
            &["ANTHROPIC_*", "PHANTOM_PROXY_PORT"],
            GatewayConfig::default(),
        ))
        .unwrap();
    let iso = build_isolated_env_opts(&s, &b, true);
    assert!(!iso.vars.contains_key("PHANTOM_PROXY_PORT"));
    assert!(!iso.vars.contains_key("ANTHROPIC_API_KEY"));
    assert_eq!(
        iso.vars.get("SUPABASE_ACCESS_TOKEN").map(String::as_str),
        Some(BOUND)
    );
    assert_eq!(iso.secrets_failed.len(), 1);
    assert_eq!(iso.secrets_failed[0].code, "inherit-env-invalid");

    // 7. Every subset of provider names is checked against a sealed session.
    // Safe placeholders may cross only under their exact configured name;
    // the pinned Supabase credential always takes precedence.
    let ambient = [
        "GH_TOKEN",
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
        "AWS_PROFILE",
        "AWS_REGION",
        "SUPABASE_ACCESS_TOKEN",
        "SUPABASE_PROJECT_REF",
        "VERCEL_TOKEN",
    ];
    for name in ambient {
        std::env::set_var(name, "phm_ambient_provider_fixture");
    }
    for mask in 0..(1usize << ambient.len()) {
        let allowlist: Vec<_> = ambient
            .iter()
            .enumerate()
            .filter(|(i, _)| mask & (1 << i) != 0)
            .map(|(_, name)| *name)
            .collect();
        store
            .save_config(&cfg(&allowlist, GatewayConfig::default()))
            .unwrap();
        let iso = build_isolated_env_opts(&s, &b, true);
        assert!(iso.secrets_failed.is_empty());
        for (i, name) in ambient.iter().enumerate() {
            let expected = if *name == "SUPABASE_ACCESS_TOKEN" {
                Some(BOUND)
            } else if mask & (1 << i) != 0 {
                Some("phm_ambient_provider_fixture")
            } else {
                None
            };
            assert_eq!(
                iso.vars.get(*name).map(String::as_str),
                expected,
                "{name}, subset {mask}"
            );
        }
    }
    // Even an exact allowlist cannot authorize ambient raw credentials.
    let secret_names = [
        "GH_TOKEN",
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
        "SUPABASE_ACCESS_TOKEN",
        "VERCEL_TOKEN",
    ];
    for name in secret_names {
        std::env::set_var(name, "AKIA_SYNTHETIC_AMBIENT_CANARY");
    }
    store
        .save_config(&cfg(&secret_names, GatewayConfig::default()))
        .unwrap();
    let iso = build_isolated_env_opts(&s, &b, true);
    assert_eq!(iso.secrets_failed.len(), secret_names.len() - 1);
    for name in secret_names {
        if name == "SUPABASE_ACCESS_TOKEN" {
            assert_eq!(iso.vars.get(name).map(String::as_str), Some(BOUND));
        } else {
            assert!(!iso.vars.contains_key(name), "{name}");
        }
    }
    assert!(!iso.vars.values().any(|v| v.contains("AMBIENT_CANARY")));
    assert!(!format!("{:?}", iso.secrets_failed).contains("AMBIENT_CANARY"));
    for name in ambient {
        std::env::remove_var(name);
    }

    for k in [
        "LOCUS_HOME",
        "LOCUS_INHERIT_TEST_TOKEN",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_BASE_URL",
        "PHANTOM_PROXY_PORT",
        "OPENAI_API_KEY",
        "SUPABASE_ACCESS_TOKEN",
    ] {
        std::env::remove_var(k);
    }
}

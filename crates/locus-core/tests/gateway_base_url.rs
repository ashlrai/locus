//! Integration: the opt-in `[gateway]` base URL at the isolated env builder.
//!
//! One test function, because it sets LOCUS_HOME and parent variables for
//! this test binary. The default-path pin in `tests/isolation.rs`
//! (`gateway_and_proxy_variables_from_the_parent_never_reach_the_child`) is a
//! separate binary and is unchanged.

use chrono::Duration;
use locus_core::config::{GatewayConfig, LocusConfig};
use locus_core::seal::SealKey;
use locus_core::session::{PinSource, Session};
use locus_core::{
    build_isolated_env_opts, Binding, BindingBody, Policy, ProviderBinding, Scope, Store,
};
use tempfile::tempdir;

const VALUE: &str = "gateway-test-credential-value-456";
const PARENT_URL: &str = "https://parent-shell.example.com/v1";
const CONFIG_URL: &str = "http://127.0.0.1:4000/v1";

fn binding() -> Binding {
    Binding::from_body(BindingBody {
        id: "bnd_example".into(),
        alias: "example".into(),
        tenant: "example".into(),
        principal: None,
        description: None,
        policy: Policy::default(),
        providers: vec![ProviderBinding {
            provider: "supabase".into(),
            account: "example".into(),
            credential_ref: "env:LOCUS_GATEWAY_TEST_TOKEN".into(),
            scope: Scope::default(),
            upstream: None,
        }],
    })
}

fn session(b: &Binding) -> Session {
    Session::new(
        &b.id,
        &b.alias,
        &b.tenant,
        None,
        PinSource::Explicit,
        None,
        Duration::hours(1),
        "/tmp/locus-gateway-test".into(),
        &SealKey::generate(),
    )
}

fn gateway(cfg: GatewayConfig) -> LocusConfig {
    LocusConfig {
        gateway: cfg,
        ..Default::default()
    }
}

fn no_value_carries(vars: &std::collections::BTreeMap<String, String>, needle: &str) -> bool {
    vars.values().all(|v| !v.contains(needle))
}

#[test]
fn gateway_base_url_end_to_end() {
    let home = tempdir().unwrap();
    std::env::set_var("LOCUS_HOME", home.path());
    std::env::set_var("LOCUS_GATEWAY_TEST_TOKEN", VALUE);
    std::env::set_var("OPENAI_BASE_URL", PARENT_URL);
    std::env::set_var("ANTHROPIC_BASE_URL", PARENT_URL);
    let store = Store::open(home.path()).unwrap();
    let b = binding();
    let s = session(&b);

    // 1. Unset (no config file): no behavior change. Parent URLs are dropped,
    //    the credential is injected as before, nothing is reported.
    let iso = build_isolated_env_opts(&s, &b, true);
    assert!(!iso.vars.contains_key("OPENAI_BASE_URL"));
    assert!(!iso.vars.contains_key("ANTHROPIC_BASE_URL"));
    assert!(iso.scrubbed.iter().any(|k| k == "OPENAI_BASE_URL"));
    assert_eq!(
        iso.vars.get("SUPABASE_ACCESS_TOKEN").map(String::as_str),
        Some(VALUE)
    );
    assert!(iso.secrets_failed.is_empty());
    let baseline: Vec<String> = iso.vars.keys().cloned().collect();

    // 2. Present but `enabled` not true: still no change.
    store
        .save_config(&gateway(GatewayConfig {
            enabled: false,
            base_url: Some(CONFIG_URL.into()),
            child_env: vec!["OPENAI_BASE_URL".into()],
            allowed_hosts: vec![],
        }))
        .unwrap();
    let iso = build_isolated_env_opts(&s, &b, true);
    assert_eq!(iso.vars.keys().cloned().collect::<Vec<_>>(), baseline);
    assert!(iso.secrets_failed.is_empty());

    // 3. Enabled loopback: only the listed variable gets the configured URL.
    //    The parent's value never crosses, and the credential is unchanged.
    store
        .save_config(&gateway(GatewayConfig {
            enabled: true,
            base_url: Some(CONFIG_URL.into()),
            child_env: vec!["OPENAI_BASE_URL".into()],
            allowed_hosts: vec![],
        }))
        .unwrap();
    let iso = build_isolated_env_opts(&s, &b, true);
    assert_eq!(
        iso.vars.get("OPENAI_BASE_URL").map(String::as_str),
        Some(CONFIG_URL)
    );
    assert!(!iso.vars.contains_key("ANTHROPIC_BASE_URL"));
    assert!(no_value_carries(&iso.vars, "parent-shell.example.com"));
    assert_eq!(
        iso.vars
            .iter()
            .filter(|(_, v)| v.as_str() == CONFIG_URL)
            .count(),
        1
    );
    assert_eq!(
        iso.vars.get("SUPABASE_ACCESS_TOKEN").map(String::as_str),
        Some(VALUE)
    );
    assert!(iso.secrets_failed.is_empty());
    let mut expected = baseline.clone();
    expected.push("OPENAI_BASE_URL".into());
    expected.sort();
    assert_eq!(iso.vars.keys().cloned().collect::<Vec<_>>(), expected);

    // 4. Enabled https host on the allowlist.
    store
        .save_config(&gateway(GatewayConfig {
            enabled: true,
            base_url: Some("https://gw.example.com/anthropic".into()),
            child_env: vec!["ANTHROPIC_BASE_URL".into()],
            allowed_hosts: vec!["gw.example.com:443".into()],
        }))
        .unwrap();
    let iso = build_isolated_env_opts(&s, &b, true);
    assert_eq!(
        iso.vars.get("ANTHROPIC_BASE_URL").map(String::as_str),
        Some("https://gw.example.com/anthropic")
    );
    assert!(iso.secrets_failed.is_empty());

    // 5. Opted in but refused: fail closed. No URL is set, no credential is
    //    resolved, and the closed code is reported without the input.
    for (url, hosts, env, code) in [
        (
            "http://localhost:4000",
            vec![],
            "OPENAI_BASE_URL",
            "loopback-name-not-allowed",
        ),
        (
            "https://gw.example.com",
            vec![],
            "OPENAI_BASE_URL",
            "host-not-allowlisted",
        ),
        (
            "http://gw.example.com",
            vec!["gw.example.com:80".to_string()],
            "OPENAI_BASE_URL",
            "http-requires-loopback",
        ),
        (
            "https://u:p@gw.example.com",
            vec!["gw.example.com:443".to_string()],
            "OPENAI_BASE_URL",
            "userinfo-not-allowed",
        ),
        (CONFIG_URL, vec![], "OPENAI_API_KEY", "child-env-invalid"),
    ] {
        store
            .save_config(&gateway(GatewayConfig {
                enabled: true,
                base_url: Some(url.into()),
                child_env: vec![env.into()],
                allowed_hosts: hosts,
            }))
            .unwrap();
        let iso = build_isolated_env_opts(&s, &b, true);
        assert!(!iso.vars.contains_key(env), "{code}");
        assert!(!iso.vars.contains_key("SUPABASE_ACCESS_TOKEN"), "{code}");
        assert!(no_value_carries(&iso.vars, VALUE), "{code}");
        assert_eq!(iso.secrets_resolved, 0, "{code}");
        assert_eq!(iso.secrets_failed.len(), 1, "{code}");
        let issue = &iso.secrets_failed[0];
        assert_eq!(
            (issue.source.as_str(), issue.code.as_str()),
            ("gateway", code)
        );
        let shown = issue.to_string();
        assert!(
            !shown.contains("gw.example.com") && !shown.contains("localhost"),
            "{shown}"
        );
        assert_eq!(
            iso.vars
                .get("LOCUS_SUPABASE_CREDENTIAL_RESOLVED")
                .map(String::as_str),
            Some("0")
        );
    }

    for k in [
        "LOCUS_HOME",
        "LOCUS_GATEWAY_TEST_TOKEN",
        "OPENAI_BASE_URL",
        "ANTHROPIC_BASE_URL",
    ] {
        std::env::remove_var(k);
    }
}

/// Join a request path onto a base URL the way a child client does: the
/// separator is supplied once, whether or not the base already ends in `/`.
fn join(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

/// Count `//` runs after the scheme's own `://`.
fn doubled_separators(url: &str) -> usize {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    after_scheme.matches("//").count()
}

/// Pure (no env, no store), so it can share the binary with the end-to-end
/// test above.
#[test]
fn trailing_slash_base_url_joins_with_exactly_one_separator() {
    use locus_core::gateway::{resolve, GatewayChild};

    for (with_slash, without_slash) in [
        ("http://127.0.0.1:4000/v1/", "http://127.0.0.1:4000/v1"),
        ("http://127.0.0.1:4000/", "http://127.0.0.1:4000"),
    ] {
        // A trailing slash is accepted and handed to the child verbatim.
        let GatewayChild::Set { target, vars } = resolve(&GatewayConfig {
            enabled: true,
            base_url: Some(with_slash.into()),
            child_env: vec!["OPENAI_BASE_URL".into()],
            allowed_hosts: vec![],
        }) else {
            panic!("trailing-slash base URL {with_slash} was refused");
        };
        assert_eq!(target.base_url, with_slash);
        assert_eq!(vars, vec!["OPENAI_BASE_URL".to_string()]);

        for path in ["chat/completions", "/chat/completions"] {
            let slashed = join(&target.base_url, path);
            let plain = join(without_slash, path);
            assert_eq!(slashed, plain, "{with_slash} + {path}");
            assert!(slashed.ends_with("/chat/completions"), "{slashed}");
            assert_eq!(doubled_separators(&slashed), 0, "{slashed}");
            // The join is exactly one `/` between base and path.
            let base = target.base_url.trim_end_matches('/');
            assert_eq!(&slashed[base.len()..], "/chat/completions");
        }
    }

    // A doubled trailing slash is refused, not silently collapsed.
    let refused = resolve(&GatewayConfig {
        enabled: true,
        base_url: Some("http://127.0.0.1:4000/v1//".into()),
        child_env: vec!["OPENAI_BASE_URL".into()],
        allowed_hosts: vec![],
    });
    assert!(matches!(refused, GatewayChild::Refused("path-not-allowed")));
}

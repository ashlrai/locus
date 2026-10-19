//! LiteLLM compatibility, hermetic (same method as ashlr-plugin #207).
//!
//! A fake LiteLLM-shaped proxy listens on 127.0.0.1 (port 0, no network).
//! `[gateway]` points the isolated child's OPENAI_BASE_URL at it and
//! `[exec] inherit_env` passes OPENAI_API_KEY. A real child process (this
//! test binary re-run as an OpenAI-style client) is spawned with exactly the
//! isolated env, calls `<OPENAI_BASE_URL>/chat/completions`, and sends its
//! whole environment in the body. Asserts:
//!   (a) the configured base URL is honored (the request reached the proxy),
//!   (b) the path is LiteLLM's `/v1/chat/completions`,
//!   (c) only the configured key (a phm_ placeholder) is sent; canary keys,
//!       an OAuth token, and a real OPENAI_API_KEY never reach the proxy.
//! The leak check is proven red with an injected canary.
//!
//! One test function drives the parent side, because it sets LOCUS_HOME and
//! parent variables for this test binary.

use chrono::Duration;
use locus_core::config::{ExecConfig, GatewayConfig, LocusConfig};
use locus_core::seal::SealKey;
use locus_core::session::{PinSource, Session};
use locus_core::{
    build_isolated_env_opts, Binding, BindingBody, Policy, ProviderBinding, Scope, Store,
};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use tempfile::tempdir;

const CHILD_FLAG: &str = "LOCUS_LITELLM_COMPAT_CHILD";
const PLACEHOLDER: &str = "phm_litellmsession01";
const BOUND: &str = "litellm-test-bound-credential-321";
const CANARIES: [(&str, &str); 5] = [
    ("ANTHROPIC_API_KEY", "sk-ant-CANARY-anthropic-0001"),
    ("GEMINI_API_KEY", "CANARY-gemini-0002"),
    ("CLAUDE_CODE_OAUTH_TOKEN", "CANARY-claude-oauth-0003"),
    ("GITHUB_TOKEN", "CANARY-github-0004"),
    ("LITELLM_MASTER_KEY", "sk-CANARY-litellm-master-0005"),
];
const REAL_OPENAI: &str = "sk-CANARY-real-openai-0006";

fn binding() -> Binding {
    Binding::from_body(BindingBody {
        id: "bnd_litellm".into(),
        alias: "litellm".into(),
        tenant: "litellm".into(),
        principal: None,
        description: None,
        policy: Policy::default(),
        providers: vec![ProviderBinding {
            provider: "supabase".into(),
            account: "litellm".into(),
            credential_ref: "env:LOCUS_LITELLM_TEST_TOKEN".into(),
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
        "/tmp/locus-litellm-test".into(),
        &SealKey::generate(),
    )
}

/// Child side: an OpenAI-style client. A no-op unless spawned by the parent.
#[test]
fn child_client() {
    if std::env::var(CHILD_FLAG).is_err() {
        return;
    }
    let base = std::env::var("OPENAI_BASE_URL").expect("OPENAI_BASE_URL in child");
    let key = std::env::var("OPENAI_API_KEY").unwrap_or_default();
    let rest = base.strip_prefix("http://").expect("http loopback base");
    let (authority, path) = rest.split_once('/').map_or((rest, ""), |(a, p)| (a, p));
    let env: BTreeMap<String, String> = std::env::vars().collect();
    let body = serde_json::json!({ "model": "m", "messages": [], "env": env }).to_string();
    let mut s = TcpStream::connect(authority).expect("proxy reachable");
    write!(
        s,
        "POST /{path}/chat/completions HTTP/1.1\r\nhost: {authority}\r\nauthorization: Bearer {key}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    assert!(out.starts_with("HTTP/1.1 200"), "{out}");
}

/// One-shot fake LiteLLM proxy; returns (base URL, handle yielding the raw request).
fn litellm_stub() -> (String, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let h = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let n = s.read(&mut chunk).unwrap_or(0);
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            let text = String::from_utf8_lossy(&buf).to_string();
            if let Some(end) = text.find("\r\n\r\n") {
                let len = text[..end]
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.eq_ignore_ascii_case("content-length")
                            .then(|| v.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                if buf.len() >= end + 4 + len {
                    break;
                }
            }
        }
        let body = r#"{"choices":[{"message":{"content":"ok"}}]}"#;
        let _ = write!(
            s,
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        String::from_utf8_lossy(&buf).to_string()
    });
    (base, h)
}

/// Err naming the variable when any non-configured secret is in the request.
fn only_configured(raw: &str) -> Result<(), String> {
    let mut others: Vec<(&str, &str)> = CANARIES.to_vec();
    others.push(("OPENAI_API_KEY (real)", REAL_OPENAI));
    match others.iter().find(|(_, v)| raw.contains(*v)) {
        Some((name, _)) => Err(format!("leak: {name} reached the proxy")),
        None => Ok(()),
    }
}

/// Spawn the child client with exactly `vars` and return the raw request.
fn run_child(
    vars: &BTreeMap<String, String>,
    base: &str,
    stub: std::thread::JoinHandle<String>,
) -> String {
    let _ = base;
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child_client", "--nocapture", "--test-threads=1"])
        .env_clear()
        .envs(vars)
        .env(CHILD_FLAG, "1")
        .status()
        .expect("child runs");
    assert!(status.success(), "child client failed");
    stub.join().unwrap()
}

#[test]
fn litellm_proxy_end_to_end() {
    let home = tempdir().unwrap();
    std::env::set_var("LOCUS_HOME", home.path());
    std::env::set_var("LOCUS_LITELLM_TEST_TOKEN", BOUND);
    for (k, v) in CANARIES {
        std::env::set_var(k, v);
    }
    let store = Store::open(home.path()).unwrap();
    let b = binding();
    let s = session(&b);
    let (base, stub) = litellm_stub();
    let config = |inherit: &[&str]| LocusConfig {
        exec: ExecConfig {
            inherit_env: inherit.iter().map(|s| s.to_string()).collect(),
        },
        gateway: GatewayConfig {
            enabled: true,
            base_url: Some(base.clone()),
            child_env: vec!["OPENAI_BASE_URL".into()],
            allowed_hosts: vec![],
        },
        ..Default::default()
    };

    // 1. phm_ placeholder inherited: base URL honored, LiteLLM path, only the placeholder.
    std::env::set_var("OPENAI_API_KEY", PLACEHOLDER);
    store.save_config(&config(&["OPENAI_API_KEY"])).unwrap();
    let iso = build_isolated_env_opts(&s, &b, true);
    assert_eq!(
        iso.vars.get("OPENAI_BASE_URL"),
        Some(&base),
        "(a) configured base URL"
    );
    let raw = run_child(&iso.vars, &base, stub);
    let line = raw.lines().next().unwrap_or("");
    assert!(
        line.starts_with("POST /v1/chat/completions "),
        "(b) path: {line}"
    );
    assert!(
        raw.contains(&format!("authorization: Bearer {PLACEHOLDER}")),
        "(c) placeholder sent"
    );
    only_configured(&raw).unwrap();

    // 2. A real (non-placeholder) OPENAI_API_KEY in the parent never crosses.
    let (base2, stub2) = litellm_stub();
    let mut cfg2 = config(&["OPENAI_API_KEY"]);
    cfg2.gateway.base_url = Some(base2.clone());
    store.save_config(&cfg2).unwrap();
    std::env::set_var("OPENAI_API_KEY", REAL_OPENAI);
    let iso = build_isolated_env_opts(&s, &b, true);
    let raw = run_child(&iso.vars, &base2, stub2);
    only_configured(&raw).unwrap();

    // 3. The check is red with an injected leak.
    let (base3, stub3) = litellm_stub();
    let mut cfg3 = config(&["OPENAI_API_KEY"]);
    cfg3.gateway.base_url = Some(base3.clone());
    store.save_config(&cfg3).unwrap();
    std::env::set_var("OPENAI_API_KEY", PLACEHOLDER);
    let mut leaky = build_isolated_env_opts(&s, &b, true).vars;
    leaky.insert("CLAUDE_CODE_OAUTH_TOKEN".into(), CANARIES[2].1.into());
    let raw = run_child(&leaky, &base3, stub3);
    assert_eq!(
        only_configured(&raw),
        Err("leak: CLAUDE_CODE_OAUTH_TOKEN reached the proxy".to_string())
    );
}

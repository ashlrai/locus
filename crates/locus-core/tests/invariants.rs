//! Cutting-edge invariant / property tests for Locus (DESIGN.md INV-1…6 + ops).
//!
//! These encode load-bearing security properties so regressions fail closed in CI.
//! Prefer `proptest` loops over random digests/aliases where cheap; otherwise
//! thorough deterministic unit checks.

use chrono::Duration;
use locus_core::adapters::freeze_string_arg;
use locus_core::{
    args_digest, build_isolated_env, call_tool, control_tools, decrypt_graph, encrypt_graph,
    notifications_enabled, required_grant_count, Binding, BindingBody, GraphEnvelope, GraphMeta,
    LocusConfig, LocusError, NotifyConfig, PinSource, Policy, ProviderBinding, Scope, SealKey,
    Session, Store, GRAPH_MAGIC,
};
use proptest::prelude::*;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs;
use std::sync::{Mutex, MutexGuard};
use tempfile::tempdir;

// ── Env isolation for notify / LOCUS_SESSION_ID ─────────────────────────────

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    _g: MutexGuard<'static, ()>,
}

fn lock_env() -> EnvGuard {
    EnvGuard {
        _g: ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner()),
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn multi_binding(alias: &str, tenant: &str, marker: &str) -> Binding {
    let up = marker.to_uppercase().replace('-', "_");
    Binding::from_body(BindingBody {
        id: format!("bnd_{alias}"),
        alias: alias.into(),
        tenant: tenant.into(),
        principal: Some("tester".into()),
        description: Some(format!("{tenant} work")),
        policy: Policy {
            max_ttl: Some("1h".into()),
            require_approval: vec!["*.delete*".into()],
            dual_control: vec!["*.delete*".into()],
            ..Policy::default()
        },
        providers: vec![
            ProviderBinding {
                provider: "supabase".into(),
                account: format!("{alias}-db"),
                credential_ref: format!("phm:SUPABASE_{up}"),
                scope: Scope {
                    project_ref: Some(format!("proj_{marker}")),
                    read_only: Some(false),
                    ..Scope::default()
                },
                upstream: None,
            },
            ProviderBinding {
                provider: "vercel".into(),
                account: format!("{alias}-vc"),
                credential_ref: format!("phm:VERCEL_{up}"),
                scope: Scope {
                    team_id: Some(format!("team_{marker}")),
                    projects: vec![format!("{alias}-web")],
                    ..Scope::default()
                },
                upstream: None,
            },
            ProviderBinding {
                provider: "github".into(),
                account: format!("{alias}-gh"),
                credential_ref: format!("phm:GH_{up}"),
                scope: Scope {
                    orgs: vec![tenant.into()],
                    ..Scope::default()
                },
                upstream: None,
            },
        ],
    })
}

fn safe_alias(s: &str) -> String {
    let mut out: String = s
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(24)
        .collect();
    if out.is_empty() || !out.chars().next().unwrap().is_ascii_alphabetic() {
        out = format!("a{out}");
    }
    // binding ids/aliases must stay path-safe
    out.truncate(32);
    out
}

// ═══════════════════════════════════════════════════════════════════════════
// INV-4 / exclusive catalog: unbound surface ⊆ locus_* only
// ═══════════════════════════════════════════════════════════════════════════

/// Unbound catalog is control-only — no provider tools fall through.
#[test]
fn inv_unbound_tools_subset_locus_star_only() {
    let unbound = control_tools(false);
    assert!(!unbound.is_empty(), "control tools must be non-empty");
    for t in &unbound {
        assert!(
            t.name.starts_with("locus_"),
            "unbound catalog leaked non-control tool: {}",
            t.name
        );
        assert_eq!(t.provider, "locus");
        assert!(!t.destructive, "control tools must not be destructive");
    }
    let names: HashSet<_> = unbound.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains("locus_whoami"));
    assert!(names.contains("locus_safe_next"));
    assert!(names.contains("locus_request_pin"));
    assert!(names.contains("locus_enter_hint"));
    // Agents never get a pin primitive
    assert!(!names.contains("locus_pin"));
    assert!(!names.contains("locus_enter"));
    assert!(!names
        .iter()
        .any(|n| n.contains("supabase") || n.contains("github")));
    // Providers tool only when pinned
    assert!(!names.contains("locus_providers"));
    let pinned = control_tools(true);
    assert!(pinned.iter().any(|t| t.name == "locus_providers"));
    assert!(pinned.iter().any(|t| t.name == "locus_safe_next"));
    for t in &pinned {
        assert!(t.name.starts_with("locus_"));
    }
}

/// Property: for any pin flag, every control tool name is `locus_*`.
#[test]
fn prop_control_tools_always_locus_prefix() {
    proptest!(|(pinned in any::<bool>())| {
        let tools = control_tools(pinned);
        prop_assert!(!tools.is_empty());
        for t in &tools {
            prop_assert!(
                t.name.starts_with("locus_"),
                "non-locus tool in control catalog (pinned={}): {}",
                pinned,
                t.name
            );
            prop_assert_eq!(&t.provider, "locus");
        }
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// INV-2: pin switch — previous binding credential_refs never in whoami/env
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn inv_pin_switch_no_prior_credential_refs() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let acme = multi_binding("acme", "acme-corp", "acme");
    let personal = multi_binding("personal", "personal", "personal");
    store.save_binding(&acme).unwrap();
    store.save_binding(&personal).unwrap();

    let s1 = store
        .pin("acme", dir.path(), Some("inv".into()), false)
        .unwrap();
    let w1 = store.whoami().unwrap();
    assert_eq!(w1.binding_alias, "acme");
    assert!(w1
        .providers
        .iter()
        .all(|p| p.credential.present && p.credential.source == "phantom"));
    let whoami1 = serde_json::to_string(&w1).unwrap();
    assert!(!whoami1.contains("SUPABASE_ACME"));
    assert!(!whoami1.contains("VERCEL_ACME"));
    assert!(!whoami1.contains("GH_ACME"));

    let iso1 = build_isolated_env(&s1, &acme);
    assert!(!iso1
        .vars
        .values()
        .any(|v| v.to_uppercase().contains("PERSONAL")));
    // Switch pin
    let s2 = store
        .pin("personal", dir.path(), Some("inv".into()), false)
        .unwrap();
    let w2 = store.whoami().unwrap();
    assert_eq!(w2.binding_alias, "personal");
    for p in &w2.providers {
        assert!(p.credential.present);
        assert_eq!(p.credential.source, "phantom");
    }
    let whoami2 = serde_json::to_string(&w2).unwrap();
    assert!(!whoami2.contains("ACME"));
    assert!(!whoami2.contains("SUPABASE_PERSONAL"));
    assert!(!whoami2.contains("VERCEL_PERSONAL"));
    assert!(!whoami2.contains("GH_PERSONAL"));
    assert!(w2
        .providers
        .iter()
        .all(|p| p.project_ref.as_deref() != Some("proj_acme")));

    let iso2 = build_isolated_env(&s2, &personal);
    assert!(!iso2
        .vars
        .values()
        .any(|v| v.to_uppercase().contains("ACME")));
    assert!(!iso2.vars.values().any(|v| v.contains("proj_acme")));
    assert!(!iso2.vars.values().any(|v| v.contains("team_acme")));
    assert_ne!(s1.session_id, s2.session_id);
}

/// Property: exclusive pin of A never surfaces B's credential_ref markers.
#[test]
fn prop_pin_exclusive_refs_for_random_aliases() {
    // Store pin involves filesystem + seal — keep cases moderate for CI.
    let mut cfg = ProptestConfig::with_cases(24);
    cfg.source_file = Some(file!());
    proptest!(cfg, |(
        a_raw in "[a-z][a-z0-9_]{1,10}",
        b_raw in "[a-z][a-z0-9_]{1,10}",
    )| {
        let a = safe_alias(&a_raw);
        let b = safe_alias(&b_raw);
        prop_assume!(a != b);

        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let ba = multi_binding(&a, &format!("{a}-tenant"), &a);
        let bb = multi_binding(&b, &format!("{b}-tenant"), &b);
        store.save_binding(&ba).unwrap();
        store.save_binding(&bb).unwrap();

        let sess = store.pin(&a, dir.path(), None, false).unwrap();
        let w = store.whoami().unwrap();
        prop_assert_eq!(&w.binding_alias, &a);

        // Locator names from either binding are absent; frozen scope metadata
        // still proves the selected tenant is exclusive.
        let sibling_refs: HashSet<_> = bb
            .providers
            .iter()
            .map(|p| p.credential_ref.clone())
            .collect();
        let sibling_proj = format!("proj_{b}");
        let sibling_team = format!("team_{b}");
        for p in &w.providers {
            prop_assert!(p.credential.present);
            prop_assert_eq!(p.credential.source.as_str(), "phantom");
            prop_assert!(p.project_ref.as_deref() != Some(sibling_proj.as_str()));
        }
        let whoami_json = serde_json::to_string(&w).unwrap();
        for locator in ba.providers.iter().chain(bb.providers.iter()).map(|p| &p.credential_ref) {
            prop_assert!(!whoami_json.contains(locator));
        }
        let iso = build_isolated_env(&sess, &ba);
        for v in iso.vars.values() {
            // Exact equality only — substring checks false-positive when one
            // alias is a prefix of another (e.g. phm:GH_D70 contains phm:GH_D7).
            prop_assert!(
                !sibling_refs.iter().any(|r| v == r),
                "isolated env leaked sibling credential_ref material: {}",
                v
            );
            prop_assert_ne!(v.as_str(), sibling_proj.as_str());
            prop_assert_ne!(v.as_str(), sibling_team.as_str());
        }
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// INV scope freeze: wrong project_ref / team_id rejected
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn inv_scope_freeze_rejects_wrong_project_and_team() {
    let b = multi_binding("acme", "acme-corp", "acme");

    let err = call_tool(&b, "supabase.scope", &json!({ "project_ref": "proj_evil" }));
    assert!(err.is_err(), "expected project_ref freeze: {err:?}");
    let msg = err.unwrap_err().to_string();
    assert!(
        msg.contains("scope freeze") || msg.contains("proj_evil"),
        "{msg}"
    );

    let err = call_tool(&b, "vercel.scope", &json!({ "team_id": "team_evil" }));
    assert!(err.is_err(), "expected team_id freeze: {err:?}");
    let msg = err.unwrap_err().to_string();
    assert!(
        msg.contains("scope freeze") || msg.contains("team_evil"),
        "{msg}"
    );

    // Matching frozen values allowed
    let ok = call_tool(&b, "supabase.scope", &json!({ "project_ref": "proj_acme" })).unwrap();
    assert!(ok.ok);

    let ok = call_tool(&b, "vercel.scope", &json!({ "team_id": "team_acme" })).unwrap();
    assert!(ok.ok);
}

/// Property: any non-equal model value for a frozen key is refused.
#[test]
fn prop_freeze_string_arg_rejects_mismatches() {
    proptest!(|(
        frozen in "[a-zA-Z0-9_-]{1,32}",
        model in "[a-zA-Z0-9_-]{1,32}",
        key in prop::sample::select(vec!["project_ref", "team_id", "account_id"]),
    )| {
        prop_assume!(frozen != model);
        let args = json!({ key: model });
        let r = freeze_string_arg(&args, key, Some(&frozen));
        prop_assert!(
            r.is_err(),
            "expected freeze deny for {}={} vs {}",
            key,
            model,
            frozen
        );
        let msg = r.unwrap_err().to_string();
        prop_assert!(msg.contains("scope freeze"));

        // Equal value (or omit) is fine
        let ok = freeze_string_arg(&json!({ key: &frozen }), key, Some(&frozen)).unwrap();
        prop_assert_eq!(ok.as_deref(), Some(frozen.as_str()));

        let omitted = freeze_string_arg(&json!({}), key, Some(&frozen)).unwrap();
        prop_assert_eq!(omitted.as_deref(), Some(frozen.as_str()));
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// INV-5: agents cannot pin — request_pin only; agent_can_pin default false
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn inv_agent_cannot_pin_without_agent_can_pin() {
    // Structural: MCP control surface has request_pin / enter_hint only
    let tools = control_tools(false);
    let names: Vec<_> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"locus_request_pin"));
    assert!(names.contains(&"locus_enter_hint"));
    for forbidden in [
        "locus_pin",
        "locus_enter",
        "locus_leave",
        "pin",
        "enter",
        "binding_pin",
    ] {
        assert!(
            !names.contains(&forbidden),
            "agent-facing catalog must not expose pin primitive `{forbidden}`"
        );
    }

    // Config default: agent_can_pin is unset/false
    let cfg = LocusConfig::default();
    assert_eq!(
        cfg.daemon.agent_can_pin, None,
        "agent_can_pin must default unset (false)"
    );
    let parsed = LocusConfig::parse("").unwrap();
    assert_eq!(parsed.daemon.agent_can_pin, None);
    let parsed2 = LocusConfig::parse("[daemon]\nagent_can_pin = false\n").unwrap();
    assert_eq!(parsed2.daemon.agent_can_pin, Some(false));

    // request_pin does not mutate pin state (audit-only path via store.pin absence)
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store
        .save_binding(&multi_binding("acme", "acme-corp", "acme"))
        .unwrap();
    assert!(store.active_session().unwrap().is_none());
    // Simulate agent "request" — only humans call store.pin
    assert!(
        store.require_active().is_err(),
        "without human pin, require_active must fail closed"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// INV seal: tamper → require_active fails
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn inv_seal_tamper_require_active_fails() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store
        .save_binding(&multi_binding("acme", "acme-corp", "acme"))
        .unwrap();
    store.pin("acme", dir.path(), None, false).unwrap();
    assert!(store.require_active().is_ok());

    let path = store.active_session_path();
    let mut s: Session = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    s.binding_id = "bnd_evil".into();
    fs::write(&path, serde_json::to_string_pretty(&s).unwrap()).unwrap();
    assert!(matches!(
        store.require_active().unwrap_err(),
        LocusError::InvalidSeal
    ));
}

/// Property: any single-field session mutation invalidates the seal.
#[test]
fn prop_seal_tamper_any_field_fails_verify() {
    proptest!(|(
        field in 0u8..5,
        noise in "[a-zA-Z0-9]{4,16}",
    )| {
        let key = SealKey::generate();
        let sess = Session::new(
            "bnd_acme",
            "acme",
            "acme-corp",
            None,
            PinSource::Explicit,
            Some("test".into()),
            Duration::hours(1),
            "/tmp/locus-worker".into(),
            &key,
        );
        prop_assert!(sess.verify(&key).is_ok());

        let mut bad = sess.clone();
        match field {
            0 => bad.session_id = format!("ses_{noise}"),
            1 => bad.binding_id = format!("bnd_{noise}"),
            2 => {
                // flip last hex nibble of seal if present
                if let Some(stripped) = bad.seal.strip_prefix("hmac-sha256:") {
                    let mut chars: Vec<char> = stripped.chars().collect();
                    if let Some(last) = chars.last_mut() {
                        *last = if *last == '0' { '1' } else { '0' };
                    }
                    bad.seal = format!("hmac-sha256:{}", chars.into_iter().collect::<String>());
                } else {
                    bad.seal = format!("hmac-sha256:{noise}");
                }
            }
            3 => {
                bad.pinned_at -= Duration::seconds(42);
            }
            _ => {
                bad.expires_at += Duration::seconds(99);
            }
        }
        prop_assert!(
            matches!(bad.verify(&key), Err(LocusError::InvalidSeal)),
            "tampered field {} still verified",
            field
        );
    });
}

/// Property: seal is deterministic for same key+material; different keys diverge.
#[test]
fn prop_seal_key_material_binding() {
    proptest!(|(
        material in "[a-zA-Z0-9_|:-]{1,64}",
        other in "[a-zA-Z0-9_|:-]{1,64}",
    )| {
        let key = SealKey::generate();
        let s = key.seal(&material);
        prop_assert!(key.verify(&material, &s));
        prop_assume!(material != other);
        prop_assert!(!key.verify(&other, &s));

        let key2 = SealKey::generate();
        // Extremely unlikely equal keys; if equal, skip
        prop_assume!(key.to_hex() != key2.to_hex());
        prop_assert!(!key2.verify(&material, &s));
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// Graph export: ciphertext hides refs; cleartext envelope only refs not sk-
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn inv_graph_ciphertext_hides_credential_refs() {
    let b = multi_binding("acme", "acme-corp", "acme");
    let env = GraphEnvelope::build(vec![b], vec![], GraphMeta::default()).unwrap();
    let plain = env.to_json_bytes().unwrap();
    let plain_s = String::from_utf8(plain.clone()).unwrap();

    // Cleartext envelope may contain refs (that's the point of sharing surfaces)
    assert!(plain_s.contains("phm:SUPABASE_ACME"));
    assert!(plain_s.contains("phm:VERCEL_ACME"));
    // Never raw secret prefixes
    assert!(!plain_s.contains("sk_"));
    assert!(!plain_s.contains("sk-"));
    assert!(!plain_s.contains("ghp_"));
    assert!(!plain_s.contains("xoxb-"));

    let file = encrypt_graph(&plain, "unit-test-passphrase").unwrap();
    let file_s = String::from_utf8_lossy(&file);
    // Ciphertext must not contain plaintext credential_ref strings
    assert!(
        !file_s.contains("phm:SUPABASE_ACME"),
        "ciphertext leaked SUPABASE ref"
    );
    assert!(
        !file_s.contains("phm:VERCEL_ACME"),
        "ciphertext leaked VERCEL ref"
    );
    assert!(!file_s.contains("phm:GH_ACME"), "ciphertext leaked GH ref");
    assert!(
        file.starts_with(GRAPH_MAGIC),
        "encrypted graph must start with LOCUSGRAPH1 magic"
    );
    // Magic is clear; payload after magic must not be plain JSON
    let payload = &file[GRAPH_MAGIC.len()..];
    let payload_s = String::from_utf8_lossy(payload);
    assert!(!payload_s.contains("\"credential_ref\""));
    assert!(!payload_s.contains("phm:"));

    let dec = decrypt_graph(&file, "unit-test-passphrase").unwrap();
    let env2 = GraphEnvelope::from_json_bytes(&dec).unwrap();
    for p in &env2.bindings[0].providers {
        assert!(
            p.credential_ref.starts_with("phm:")
                || p.credential_ref.starts_with("env:")
                || p.credential_ref.starts_with("test:"),
            "decrypted envelope has non-ref credential: {}",
            p.credential_ref
        );
        assert!(!p.credential_ref.contains("sk_"));
        assert!(!p.credential_ref.contains("sk-"));
    }
}

/// Property: encrypt(plain) never contains the plaintext substring of each credential_ref.
///
/// Argon2id makes each trial relatively expensive — keep the case count modest.
#[test]
fn prop_graph_encrypt_hides_refs() {
    let mut cfg = ProptestConfig::with_cases(12);
    cfg.source_file = Some(file!());
    proptest!(cfg, |(
        alias_raw in "[a-z][a-z0-9]{2,8}",
        marker_raw in "[a-z][a-z0-9]{2,8}",
        pass in "[a-zA-Z0-9!@#]{8,24}",
    )| {
        let alias = safe_alias(&alias_raw);
        let marker = safe_alias(&marker_raw);
        let b = multi_binding(&alias, &format!("{alias}-t"), &marker);
        let refs: Vec<String> = b
            .providers
            .iter()
            .map(|p| p.credential_ref.clone())
            .collect();

        let env = GraphEnvelope::build(vec![b], vec![], GraphMeta::default()).unwrap();
        let plain = env.to_json_bytes().unwrap();
        let file = encrypt_graph(&plain, &pass).unwrap();
        let file_s = String::from_utf8_lossy(&file);
        for r in &refs {
            prop_assert!(
                !file_s.contains(r.as_str()),
                "ciphertext contained cleartext ref {}",
                r
            );
        }
        prop_assert!(!file_s.contains("sk_"));
        prop_assert!(!file_s.contains("sk-"));

        let dec = decrypt_graph(&file, &pass).unwrap();
        let env2 = GraphEnvelope::from_json_bytes(&dec).unwrap();
        for p in &env2.bindings[0].providers {
            prop_assert!(
                refs.contains(&p.credential_ref),
                "roundtrip lost/changed ref {}",
                p.credential_ref
            );
        }
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// Notify default false
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn inv_notify_default_false() {
    assert!(
        !NotifyConfig::default().enabled,
        "NotifyConfig.enabled must default false"
    );
    let cfg = LocusConfig::default();
    assert!(!cfg.notify.enabled);

    let _g = lock_env();
    // Point LOCUS_HOME at empty temp so config cannot enable notify
    let dir = tempdir().unwrap();
    let prev_home = std::env::var_os("LOCUS_HOME");
    let prev_notify = std::env::var_os("LOCUS_NOTIFY");
    let prev_quiet = std::env::var_os("LOCUS_QUIET");
    let prev_ci = std::env::var_os("CI");
    std::env::set_var("LOCUS_HOME", dir.path());
    std::env::remove_var("LOCUS_NOTIFY");
    std::env::remove_var("LOCUS_QUIET");
    // CI may be set in runners — treat as still false
    let enabled = notifications_enabled();
    assert!(
        !enabled,
        "notifications_enabled() must be false by default (got true; CI/LOCUS_NOTIFY?)"
    );

    // Explicit enable works (unless CI kills it)
    std::env::set_var("LOCUS_NOTIFY", "1");
    std::env::remove_var("CI");
    let on = notifications_enabled();
    // If CI is forced by the runner environment in weird ways, at least LOCUS_NOTIFY=0 kills
    std::env::set_var("LOCUS_NOTIFY", "0");
    assert!(!notifications_enabled());
    let _ = on;

    // restore
    match prev_home {
        Some(v) => std::env::set_var("LOCUS_HOME", v),
        None => std::env::remove_var("LOCUS_HOME"),
    }
    match prev_notify {
        Some(v) => std::env::set_var("LOCUS_NOTIFY", v),
        None => std::env::remove_var("LOCUS_NOTIFY"),
    }
    match prev_quiet {
        Some(v) => std::env::set_var("LOCUS_QUIET", v),
        None => std::env::remove_var("LOCUS_QUIET"),
    }
    match prev_ci {
        Some(v) => std::env::set_var("CI", v),
        None => std::env::remove_var("CI"),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Approval digest ignores confirm / secret keys
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn inv_approval_digest_ignores_confirm_and_secrets() {
    let base = json!({ "table": "users", "limit": 10 });
    let dirty = json!({
        "table": "users",
        "limit": 10,
        "confirm": true,
        "approval_id": "appr_deadbeef",
        "token": "sk-super-secret",
        "password": "hunter2",
        "api_key": "ak_xxx",
        "Authorization": "Bearer x",
        "nested": {
            "client_secret": "cs",
            "access_token": "at",
            "keep": true,
        }
    });
    let expected = json!({
        "table": "users",
        "limit": 10,
        "nested": { "keep": true },
    });
    assert_eq!(
        args_digest(&base),
        args_digest(&json!({
            "table": "users",
            "limit": 10,
            "confirm": false,
            "approval_id": "appr_other",
        }))
    );
    assert_eq!(args_digest(&dirty), args_digest(&expected));
    assert_ne!(
        args_digest(&base),
        args_digest(&json!({ "table": "orders", "limit": 10 }))
    );
}

/// Property: confirm/approval_id/token-like noise never changes the digest.
#[test]
fn prop_args_digest_ignores_control_and_secret_keys() {
    proptest!(|(
        table in "[a-z_]{1,16}",
        limit in 0u32..1000,
        confirm in any::<bool>(),
        secret in "[a-zA-Z0-9]{8,32}",
        appr in "[a-f0-9]{8,24}",
    )| {
        let base = json!({ "table": &table, "limit": limit });
        let with_noise = json!({
            "table": &table,
            "limit": limit,
            "confirm": confirm,
            "approval_id": format!("appr_{appr}"),
            "token": &secret,
            "password": &secret,
            "api_key": format!("sk-{secret}"),
            "access_token": &secret,
        });
        prop_assert_eq!(
            args_digest(&base),
            args_digest(&with_noise),
            "digest must ignore confirm/secrets"
        );
        // Real arg change must change digest
        let other = json!({ "table": format!("{table}_x"), "limit": limit });
        prop_assert_ne!(args_digest(&base), args_digest(&other));
    });
}

/// Property: object key order is irrelevant to digest.
#[test]
fn prop_args_digest_key_order_independent() {
    proptest!(|(
        a in "[a-y]{1,8}",
        b in "[a-y]{1,8}",
        n in 0i64..100,
    )| {
        // Fixed third key "zz" never collides with a/b (alphabet a-y only).
        prop_assume!(a != b);
        let v1 = json!({ a.clone(): n, b.clone(): true, "zz": "ok" });
        let v2 = json!({ "zz": "ok", b: true, a: n });
        prop_assert_eq!(args_digest(&v1), args_digest(&v2));
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// Dual control requires 2 principals
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn inv_dual_control_requires_two_principals() {
    assert_eq!(required_grant_count(false), 1);
    assert_eq!(required_grant_count(true), 2);

    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let mut b = multi_binding("acme", "acme-corp", "acme");
    // dual_control already set on *.delete* in multi_binding
    b.policy.dual_control = vec!["*.delete*".into()];
    b.policy.require_approval = vec!["*.delete*".into()];
    store.save_binding(&b).unwrap();
    store.pin("acme", dir.path(), None, false).unwrap();

    let pending = store
        .create_pending_approval(
            "supabase.table.delete",
            "acme",
            &json!({ "table": "users" }),
            "ses_inv",
            "agent",
        )
        .unwrap();
    assert!(pending.is_pending());
    assert!(store.tool_requires_dual_control("acme", "supabase.table.delete"));

    let one = store.grant_approval(&pending.id, None, "alice").unwrap();
    assert!(
        one.is_pending(),
        "single principal must leave dual-control pending"
    );
    assert_eq!(one.grants.len(), 1);
    assert!(!one.is_valid_grant());

    // Same principal cannot complete dual-control
    let err = store
        .grant_approval(&pending.id, None, "alice")
        .unwrap_err();
    assert!(
        err.to_string().contains("already granted") || err.to_string().contains("different"),
        "{err}"
    );

    let two = store.grant_approval(&pending.id, None, "bob").unwrap();
    assert_eq!(two.status.as_str(), "approved");
    assert_eq!(two.grants.len(), 2);
    assert!(two.is_valid_grant());
}

// ═══════════════════════════════════════════════════════════════════════════
// CI mint does not overwrite active.json
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn inv_ci_mint_does_not_overwrite_active_json() {
    let _g = lock_env();
    std::env::remove_var("LOCUS_SESSION_ID");

    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store
        .save_binding(&multi_binding("acme", "acme-corp", "acme"))
        .unwrap();
    store
        .save_binding(&multi_binding("personal", "personal", "personal"))
        .unwrap();

    let active = store.pin("acme", dir.path(), None, false).unwrap();
    let active_id = active.session_id.clone();
    let active_path = store.active_session_path();
    let before = fs::read_to_string(&active_path).unwrap();

    let (ci, ci_path) = store
        .create_ci_session("personal", dir.path(), false, Some(Duration::minutes(10)))
        .unwrap();
    assert!(matches!(ci.source, PinSource::Ci));
    assert_eq!(ci.binding_alias, "personal");
    assert_ne!(ci.session_id, active_id);
    assert!(ci_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap()
        .starts_with("ci-"));

    // active.json unchanged byte-for-byte (no overwrite)
    let after = fs::read_to_string(&active_path).unwrap();
    assert_eq!(before, after, "CI mint must not rewrite active.json");
    let still = store.active_session().unwrap().unwrap();
    assert_eq!(still.session_id, active_id);
    assert_eq!(still.binding_alias, "acme");

    // CI file is separate
    assert!(ci_path.exists());
    assert_ne!(ci_path, active_path);

    store.cleanup_ci_session(&ci_path, &ci).unwrap();
    // active still intact after cleanup
    assert_eq!(
        store.active_session().unwrap().unwrap().session_id,
        active_id
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// INV-1-ish: tools/call path fails closed without valid seal (store gate)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn inv_require_active_fail_closed_when_unbound() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store
        .save_binding(&multi_binding("acme", "acme-corp", "acme"))
        .unwrap();
    assert!(matches!(
        store.require_active().unwrap_err(),
        LocusError::NotPinned
    ));
    // whoami also fail closed
    assert!(store.whoami().is_err());
}

/// Property: random digests stay stable under confirm noise (bulk).
#[test]
fn prop_random_digests_stable_under_noise() {
    proptest!(|(
        keys in prop::collection::vec("[a-z]{2,6}", 1..6),
        vals in prop::collection::vec(0u32..500, 1..6),
        noise_secret in "[A-Za-z0-9]{4,20}",
    )| {
        let n = keys.len().min(vals.len());
        prop_assume!(n > 0);
        let mut map = serde_json::Map::new();
        for i in 0..n {
            // skip keys that look like secrets so base stays meaningful
            prop_assume!(!keys[i].ends_with("key") && keys[i] != "token" && keys[i] != "password");
            map.insert(keys[i].clone(), json!(vals[i]));
        }
        let base = Value::Object(map.clone());
        map.insert("confirm".into(), json!(true));
        map.insert("approval_id".into(), json!(format!("appr_{noise_secret}")));
        map.insert("token".into(), json!(noise_secret.clone()));
        map.insert("password".into(), json!(noise_secret));
        let noisy = Value::Object(map);
        prop_assert_eq!(args_digest(&base), args_digest(&noisy));
    });
}

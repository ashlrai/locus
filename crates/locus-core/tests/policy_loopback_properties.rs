//! Security properties for ordered policies and exact-loopback HTTP exceptions.
//! Structured rules intentionally use first-match semantics. Only permutations
//! with equivalent matching decisions are invariant; conflicting rules are not.

use locus_core::binding::{Policy, PolicyRule};
use locus_core::gateway::validate_base_url;
use locus_core::loopback::{is_exact_loopback_host, is_exact_loopback_ip};
use locus_core::marketplace::validate_index_url;
use locus_core::policy::{evaluate, glob_match, Decision};
use proptest::prelude::*;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

fn pattern() -> impl Strategy<Value = String> {
    "[a-c.*?]{0,20}"
}

// Independent dynamic-programming oracle, without the production backtracking.
fn reference_glob(pattern: &str, text: &str) -> bool {
    let mut row = vec![false; text.len() + 1];
    row[0] = true;
    for p in pattern.bytes() {
        let mut next = vec![false; row.len()];
        next[0] = p == b'*' && row[0];
        for (i, t) in text.bytes().enumerate() {
            next[i + 1] = if p == b'*' {
                row[i + 1] || next[i]
            } else {
                row[i] && (p == b'?' || p == t)
            };
        }
        row = next;
    }
    row[text.len()]
}

fn action() -> impl Strategy<Value = &'static str> {
    prop_oneof![
        Just("allow"),
        Just("deny"),
        Just("require_approval"),
        Just("dual_control"),
        Just("unknown"),
        Just(" ALLOW "),
        Just(" DENY ")
    ]
}

fn decision(action: &str) -> Decision {
    match action.trim().to_ascii_lowercase().as_str() {
        "allow" => Decision::Allow,
        "require_approval" | "dual_control" => Decision::RequireApproval,
        _ => Decision::Deny,
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn glob_matches_independent_oracle(p in pattern(), text in "[a-c.]{0,24}") {
        prop_assert_eq!(glob_match(&p, &text), reference_glob(&p, &text));
    }

    #[test]
    fn random_policies_obey_first_match_and_fail_closed(
        rules in prop::collection::vec((pattern(), action()), 0..32),
        approval in prop::collection::vec(pattern(), 0..16),
        dual in prop::collection::vec(pattern(), 0..16),
        default in action(), tool in "[a-c.]{0,24}",
    ) {
        let expected = rules.iter().find(|(p, _)| reference_glob(p, &tool))
            .map(|(_, a)| decision(a))
            .unwrap_or_else(|| {
                if approval.iter().chain(&dual).any(|p| reference_glob(p, &tool)) {
                    Decision::RequireApproval
                } else if default.trim().eq_ignore_ascii_case("allow") {
                    Decision::Allow
                } else {
                    Decision::Deny
                }
            });
        let policy = Policy {
            rules: rules.iter().map(|(p, a)| PolicyRule::new(p, *a)).collect(),
            require_approval: approval, dual_control: dual,
            default: default.into(), ..Policy::default()
        };
        prop_assert_eq!(evaluate(&policy, &tool).decision, expected);
    }

    #[test]
    fn leading_deny_cannot_be_overridden_by_random_grants(
        tool in "[a-c.]{1,24}",
        grants in prop::collection::vec((pattern(), any::<u64>()), 0..32),
    ) {
        let mut grants = grants;
        grants.push(("*".into(), 0)); // Always include an overlapping grant.
        grants.sort_by_key(|(_, rank)| *rank);
        let mut policy = Policy {
            rules: vec![PolicyRule::new(&tool, "deny")], ..Policy::default()
        };
        policy.rules.extend(grants.iter().map(|(p, _)| PolicyRule::new(p, "allow")));
        prop_assert_eq!(evaluate(&policy, &tool).decision, Decision::Deny);
        policy.rules[1..].reverse();
        prop_assert_eq!(evaluate(&policy, &tool).decision, Decision::Deny);
    }

    #[test]
    fn disjoint_grants_never_override_deny_in_any_order(
        suffix in "[a-z_]{1,20}",
        grants in prop::collection::vec((pattern(), any::<u64>()), 0..32),
        deny_rank in any::<u64>(),
    ) {
        let tool = format!("denied.{suffix}");
        let mut ranked: Vec<_> = grants.into_iter().map(|(p, rank)|
            (rank, PolicyRule::new(format!("granted.{p}"), "allow"))).collect();
        ranked.push((deny_rank, PolicyRule::new("denied.*", "deny")));
        ranked.sort_by_key(|(rank, _)| *rank);
        let mut policy = Policy {
            rules: ranked.into_iter().map(|(_, rule)| rule).collect(),
            ..Policy::default()
        };
        prop_assert_eq!(evaluate(&policy, &tool).decision, Decision::Deny);
        policy.rules.reverse();
        prop_assert_eq!(evaluate(&policy, &tool).decision, Decision::Deny);
    }

    #[test]
    fn equivalent_rules_and_legacy_globs_are_order_independent(
        patterns in prop::collection::vec((pattern(), any::<u64>()), 0..32),
        approval in prop::collection::vec(pattern(), 0..16),
        dual in prop::collection::vec(pattern(), 0..16),
        action in action(), tool in "[a-c.]{0,24}",
    ) {
        let mut patterns = patterns;
        let mut policy = Policy {
            rules: patterns.iter().map(|(p, _)| PolicyRule::new(p, action)).collect(),
            require_approval: approval, dual_control: dual,
            default: "deny".into(), ..Policy::default()
        };
        let expected = evaluate(&policy, &tool).decision;
        patterns.sort_by_key(|(_, rank)| *rank);
        policy.rules = patterns.iter().map(|(p, _)| PolicyRule::new(p, action)).collect();
        policy.require_approval.reverse();
        policy.dual_control.reverse();
        prop_assert_eq!(evaluate(&policy, &tool).decision, expected);
    }

    #[test]
    fn exact_ip_and_parsed_http_host_agree(
        v4 in any::<[u8; 4]>(), v6 in any::<[u16; 8]>(), port in 1u16..=65535,
    ) {
        // Include positive controls and near-loopback cases on every run.
        for ip in [IpAddr::V4(Ipv4Addr::from(v4)), IpAddr::V6(Ipv6Addr::from(v6)),
            IpAddr::V4(Ipv4Addr::new(127, v4[1], v4[2], v4[3])),
            IpAddr::V4(Ipv4Addr::LOCALHOST), IpAddr::V6(Ipv6Addr::LOCALHOST),
            IpAddr::V6(Ipv4Addr::LOCALHOST.to_ipv6_mapped())] {
            let expected = ip == IpAddr::V4(Ipv4Addr::LOCALHOST)
                || ip == IpAddr::V6(Ipv6Addr::LOCALHOST);
            let host = match ip {
                IpAddr::V4(ip) => ip.to_string(), IpAddr::V6(ip) => format!("[{ip}]"),
            };
            let raw = format!("http://{host}:{port}/index.json");
            let url = url::Url::parse(&raw).unwrap();
            prop_assert_eq!(is_exact_loopback_ip(ip), expected);
            prop_assert_eq!(is_exact_loopback_host(&url.host().unwrap()), expected);
            prop_assert_eq!(validate_index_url(&raw).is_ok(), expected);
            prop_assert_eq!(validate_base_url(&raw, &[]).is_ok(), expected);
        }
    }

    #[test]
    fn random_host_grants_never_authorize_non_loopback_http(
        label in "[a-z]{1,20}", port in 1u16..=65535,
        grants in prop::collection::vec(("[a-z]{1,20}", 1u16..=65535), 0..24),
    ) {
        let host = format!("{label}.example.com");
        let mut allowed: Vec<_> = grants.into_iter()
            .map(|(h, p)| format!("{h}.example.com:{p}")).collect();
        allowed.push(format!("{host}:{port}"));
        let http = format!("http://{host}:{port}/v1");
        let https = format!("https://{host}:{port}/v1");
        // The matching allowlist really is valid, but cannot grant plain HTTP.
        prop_assert!(validate_base_url(&https, &allowed).is_ok());
        prop_assert!(validate_base_url(&http, &allowed).is_err());
        allowed.reverse();
        prop_assert!(validate_base_url(&https, &allowed).is_ok());
        prop_assert!(validate_base_url(&http, &allowed).is_err());
        for host in [host, "localhost".into(), format!("{label}.localhost")] {
            prop_assert!(!is_exact_loopback_host(&url::Host::Domain(&host)));
            let target = format!("http://{host}:{port}/");
            prop_assert!(validate_index_url(&target).is_err());
            prop_assert!(validate_base_url(&target, &allowed).is_err());
        }
    }
}

#[test]
fn conflicting_rules_are_intentionally_order_sensitive() {
    // Counterexample to blanket order independence: changing this contract
    // requires a policy migration, not a property-test-only hardening change.
    let mut policy = Policy {
        rules: vec![PolicyRule::new("*", "deny"), PolicyRule::new("*", "allow")],
        ..Policy::default()
    };
    assert_eq!(
        evaluate(&policy, "github.list_prs").decision,
        Decision::Deny
    );
    policy.rules.reverse();
    assert_eq!(
        evaluate(&policy, "github.list_prs").decision,
        Decision::Allow
    );
}

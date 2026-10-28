//! Opt-in `[exec] inherit_env`: exact parent variable names the isolated
//! `locus exec` child may inherit. Empty (the default) inherits nothing, so the
//! child environment is exactly what it was before.
//!
//! The supported route for model keys is `phantom exec`: the parent holds
//! `phm_` session placeholders, and only those may cross under a key-shaped
//! name. A real key never crosses this list:
//! - a key-shaped name (`KEY`, `TOKEN`, `SECRET`, `PASSWORD`, `PASSWD`,
//!   `CREDENTIAL`, `AUTH`, `PRIVATE`, `COOKIE`) is inherited only when its value
//!   is a `phm_` placeholder;
//! - any other value that starts like a well-known key, or is a URL with a user
//!   name or password, is refused;
//! - a `*_BASE_URL` or `*_API_BASE` value must pass the `[gateway]` URL rules
//!   (`https`, or `http` only to `127.0.0.1` or `[::1]`; other hosts need
//!   `[gateway] allowed_hosts`), so the parent cannot pick an arbitrary target.
//!
//! Names Locus sets itself, the `[gateway]` variables and injected credentials
//! always win; an inherited value never overwrites them. Refusals are closed
//! codes that carry neither the name nor the value.

use crate::credential::CredentialResolutionIssue;
use std::collections::BTreeMap;

/// Most names `inherit_env` may list.
pub const MAX_INHERIT_ENV: usize = 32;

/// Name fragments that mark a variable as able to hold a secret.
const SECRET_WORDS: [&str; 9] = [
    "KEY",
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "PASSWD",
    "CREDENTIAL",
    "AUTH",
    "PRIVATE",
    "COOKIE",
];

/// Value prefixes of common real credentials (best effort, case-sensitive).
const KEY_PREFIXES: [&str; 14] = [
    "sk-",
    "sk_live_",
    "sk_test_",
    "rk_live_",
    "ghp_",
    "gho_",
    "ghs_",
    "ghu_",
    "github_pat_",
    "glpat-",
    "xoxb-",
    "xoxp-",
    "AKIA",
    "ASIA",
];

/// Names Locus sets for the child itself; listing them is a config error.
const RESERVED: [&str; 5] = [
    "HOME",
    "USERPROFILE",
    "GH_CONFIG_DIR",
    "AWS_CONFIG_FILE",
    "AWS_SHARED_CREDENTIALS_FILE",
];

/// An exact environment variable name: `[A-Za-z_][A-Za-z0-9_]*`, at most 64
/// bytes. No globs, prefixes or patterns.
pub fn is_exact_name(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && (b[0].is_ascii_alphabetic() || b[0] == b'_')
        && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
}

/// True when the name looks like it holds a key, token or password.
pub fn is_secret_name(name: &str) -> bool {
    let up = name.to_ascii_uppercase();
    SECRET_WORDS.iter().any(|w| up.contains(w))
}

fn is_base_url_name(name: &str) -> bool {
    let up = name.to_ascii_uppercase();
    up.ends_with("_BASE_URL") || up.ends_with("_API_BASE")
}

/// A `phm_` session placeholder from `phantom exec`: `phm_` then 1 to 256
/// characters of `[A-Za-z0-9_-]`.
pub fn is_phm_placeholder(value: &str) -> bool {
    value.strip_prefix("phm_").is_some_and(|rest| {
        !rest.is_empty()
            && rest.len() <= 256
            && rest
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    })
}

fn url_has_userinfo(value: &str) -> bool {
    match value.find("://") {
        Some(i) => {
            let rest = &value[i + 3..];
            let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
            authority.contains('@')
        }
        None => false,
    }
}

/// Check the configured list. `Err` carries a closed code.
pub fn validate_names(names: &[String]) -> Result<Vec<String>, &'static str> {
    if names.len() > MAX_INHERIT_ENV {
        return Err("inherit-env-invalid");
    }
    let mut out: Vec<String> = Vec::new();
    for n in names {
        if !is_exact_name(n)
            || n.to_ascii_uppercase().starts_with("LOCUS_")
            || RESERVED.iter().any(|r| r.eq_ignore_ascii_case(n))
        {
            return Err("inherit-env-invalid");
        }
        if !out.contains(n) {
            out.push(n.clone());
        }
    }
    Ok(out)
}

/// Decide whether one parent value may cross. `Err` is a closed code.
pub fn check_value(name: &str, value: &str, allowed_hosts: &[String]) -> Result<(), &'static str> {
    if value.contains(['\0', '\n', '\r']) {
        return Err("inherit-env-value-refused");
    }
    if is_secret_name(name) {
        return if is_phm_placeholder(value) {
            Ok(())
        } else {
            Err("inherit-env-secret-refused")
        };
    }
    if KEY_PREFIXES.iter().any(|p| value.starts_with(p)) {
        return Err("inherit-env-secret-refused");
    }
    if url_has_userinfo(value) {
        return Err("inherit-env-userinfo-refused");
    }
    if is_base_url_name(name) {
        crate::gateway::validate_base_url(value, allowed_hosts)
            .map_err(|_| "inherit-env-base-url-refused")?;
    }
    Ok(())
}

fn issue(code: &str) -> CredentialResolutionIssue {
    CredentialResolutionIssue {
        provider: "inherit_env".into(),
        source: "inherit_env".into(),
        code: code.into(),
    }
}

/// Add the allowed parent values to `vars`, never overwriting a key already
/// there. `parent` is the parent environment; `scrubbed` loses each inherited
/// name. Returns value-free issues for refused names.
pub fn apply(
    names: &[String],
    allowed_hosts: &[String],
    parent: &BTreeMap<String, String>,
    vars: &mut BTreeMap<String, String>,
    scrubbed: &mut Vec<String>,
) -> Vec<CredentialResolutionIssue> {
    let names = match validate_names(names) {
        Ok(n) => n,
        Err(code) => return vec![issue(code)],
    };
    let mut issues = Vec::new();
    for name in names {
        let Some(value) = parent.get(&name) else {
            continue;
        };
        if vars.contains_key(&name) {
            continue;
        }
        match check_value(&name, value, allowed_hosts) {
            Ok(()) => {
                vars.insert(name.clone(), value.clone());
                scrubbed.retain(|s| s != &name);
            }
            Err(code) => issues.push(issue(code)),
        }
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    use proptest::prelude::*;

    proptest! {
        // Exercise arbitrary subsets, rather than only an all-or-nothing list.
        // All values are synthetic; no process environment or real credentials
        // are read by these properties.
        #[test]
        fn ambient_provider_values_require_exact_allowlisting(
            suffix in "[A-Z][A-Z0-9_]{0,20}".prop_filter(
                "private AWS config paths are reserved, not inheritable names",
                |s| s != "CONFIG_FILE" && s != "SHARED_CREDENTIALS_FILE",
            ),
            placeholder in "[a-zA-Z0-9_-]{1,64}",
            allowed in prop::array::uniform4(any::<bool>()),
        ) {
            let names = [
                "GH_TOKEN".to_string(),
                format!("AWS_{suffix}"),
                format!("SUPABASE_{suffix}"),
                "VERCEL_TOKEN".to_string(),
            ];
            let value = format!("phm_{placeholder}");
            let parent = names.iter().map(|n| (n.clone(), value.clone())).collect();
            let allowlist = names.iter().zip(allowed).filter(|(_, yes)| *yes)
                .map(|(n, _)| n.clone()).collect::<Vec<_>>();
            let mut vars = BTreeMap::new();
            let mut scrubbed = names.to_vec();
            let issues = apply(&allowlist, &[], &parent, &mut vars, &mut scrubbed);
            prop_assert!(issues.is_empty());
            for (name, yes) in names.iter().zip(allowed) {
                prop_assert_eq!(vars.get(name), yes.then_some(&value));
                prop_assert_eq!(scrubbed.contains(name), !yes);
            }
        }

        #[test]
        fn allowlisting_never_replaces_pinned_credentials_or_admits_raw_secrets(
            raw in "[a-zA-Z0-9]{1,64}",
            pinned in "[a-zA-Z0-9]{1,64}",
            selected in 0usize..4,
        ) {
            let names = ["GH_TOKEN", "AWS_SECRET_ACCESS_KEY", "SUPABASE_ACCESS_TOKEN", "VERCEL_TOKEN"]
                .map(str::to_string);
            let raw = format!("synthetic_{raw}");
            let pinned = format!("pinned_{pinned}");
            let parent = names.iter().map(|n| (n.clone(), raw.clone())).collect();
            let mut vars = BTreeMap::from([(names[selected].clone(), pinned.clone())]);
            let mut scrubbed = names.to_vec();
            let issues = apply(&names, &[], &parent, &mut vars, &mut scrubbed);
            prop_assert_eq!(vars.len(), 1);
            prop_assert_eq!(vars.get(&names[selected]), Some(&pinned));
            prop_assert_eq!(issues.len(), names.len() - 1);
            for issue in issues {
                prop_assert_eq!(issue.code, "inherit-env-secret-refused");
            }
            prop_assert_eq!(scrubbed, names.to_vec());
        }
    }

    #[test]
    fn names_are_exact() {
        for ok in ["PHANTOM_PROXY_PORT", "OPENAI_API_KEY", "_X", "a1"] {
            assert!(is_exact_name(ok), "{ok}");
        }
        for bad in [
            "", "OPENAI_*", "OPENAI_?", "1ABC", "A-B", "A B", "A=B", "PHANTOM*",
        ] {
            assert!(!is_exact_name(bad), "{bad}");
        }
        assert_eq!(
            validate_names(&["OPENAI_*".into()]),
            Err("inherit-env-invalid")
        );
        assert_eq!(
            validate_names(&["LOCUS_SEAL".into()]),
            Err("inherit-env-invalid")
        );
        assert_eq!(validate_names(&["HOME".into()]), Err("inherit-env-invalid"));
        let many: Vec<String> = (0..=MAX_INHERIT_ENV).map(|i| format!("V{i}")).collect();
        assert_eq!(validate_names(&many), Err("inherit-env-invalid"));
        assert_eq!(
            validate_names(&["A".into(), "A".into()]),
            Ok(vec!["A".to_string()])
        );
    }

    #[test]
    fn key_names_need_a_placeholder() {
        for name in [
            "OPENAI_API_KEY",
            "GH_TOKEN",
            "AWS_SECRET_ACCESS_KEY",
            "DB_PASSWORD",
            "my_auth_header",
        ] {
            assert_eq!(check_value(name, "phm_abc-123_X", &[]), Ok(()), "{name}");
            for real in [
                "sk-live-0123",
                "plainvalue",
                "phm_",
                "phm:OPENAI",
                "phm_a b",
                "xphm_abc",
            ] {
                assert_eq!(
                    check_value(name, real, &[]),
                    Err("inherit-env-secret-refused"),
                    "{name}={real}"
                );
            }
        }
    }

    #[test]
    fn other_names_refuse_key_like_values_and_userinfo() {
        assert_eq!(check_value("PHANTOM_PROXY_PORT", "41000", &[]), Ok(()));
        assert_eq!(
            check_value("MY_VAR", "ghp_0123456789", &[]),
            Err("inherit-env-secret-refused")
        );
        assert_eq!(
            check_value("DATABASE_URL", "postgres://u:p@db.example.com/x", &[]),
            Err("inherit-env-userinfo-refused")
        );
        assert_eq!(
            check_value("HTTPS_PROXY", "http://u:p@proxy:3128", &[]),
            Err("inherit-env-userinfo-refused")
        );
        assert_eq!(
            check_value("X", "a\nb", &[]),
            Err("inherit-env-value-refused")
        );
    }

    #[test]
    fn base_urls_follow_the_gateway_rules() {
        let proxy = "http://127.0.0.1:41000/openai/_phantom/tok123/";
        assert_eq!(check_value("OPENAI_BASE_URL", proxy, &[]), Ok(()));
        assert_eq!(
            check_value("OPENAI_BASE_URL", "http://localhost:4000/v1", &[]),
            Err("inherit-env-base-url-refused")
        );
        assert_eq!(
            check_value("ANTHROPIC_BASE_URL", "https://gw.example.com", &[]),
            Err("inherit-env-base-url-refused")
        );
        assert_eq!(
            check_value(
                "ANTHROPIC_BASE_URL",
                "https://gw.example.com",
                &["gw.example.com:443".into()]
            ),
            Ok(())
        );
    }

    #[test]
    fn apply_never_overwrites_and_reports_without_values() {
        let parent: BTreeMap<String, String> = [
            ("OPENAI_API_KEY", "sk-real-SECRETVALUE"),
            ("ANTHROPIC_API_KEY", "phm_session1"),
            ("PHANTOM_PROXY_PORT", "41000"),
            ("HOME_LIKE", "x"),
            ("LOCUS_SESSION_ID", "parent"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let mut vars: BTreeMap<String, String> =
            [("PHANTOM_PROXY_PORT".to_string(), "set-by-locus".to_string())].into();
        let mut scrubbed: Vec<String> = parent.keys().cloned().collect();
        let names: Vec<String> = [
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "PHANTOM_PROXY_PORT",
            "UNSET_VAR",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let issues = apply(&names, &[], &parent, &mut vars, &mut scrubbed);
        assert_eq!(
            vars.get("ANTHROPIC_API_KEY").map(String::as_str),
            Some("phm_session1")
        );
        assert!(!vars.contains_key("OPENAI_API_KEY"));
        assert_eq!(
            vars.get("PHANTOM_PROXY_PORT").map(String::as_str),
            Some("set-by-locus")
        );
        assert!(!vars.contains_key("UNSET_VAR"));
        assert!(!scrubbed.contains(&"ANTHROPIC_API_KEY".to_string()));
        assert!(scrubbed.contains(&"OPENAI_API_KEY".to_string()));
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, "inherit-env-secret-refused");
        let dbg = format!("{issues:?}");
        assert!(
            !dbg.contains("SECRETVALUE") && !dbg.contains("OPENAI_API_KEY"),
            "{dbg}"
        );
        // An invalid list inherits nothing.
        let mut vars2 = BTreeMap::new();
        let issues = apply(
            &["OPENAI_*".into()],
            &[],
            &parent,
            &mut vars2,
            &mut Vec::new(),
        );
        assert!(vars2.is_empty());
        assert_eq!(issues[0].code, "inherit-env-invalid");
    }
}

//! Opt-in gateway base URL for the isolated child (`[gateway]`, off by default).
//!
//! `locus exec` rebuilds the child environment from a runtime allowlist, so a
//! base URL exported in the parent shell never reaches the child (pinned by the
//! `gateway_and_proxy_variables_from_the_parent_never_reach_the_child` test).
//! This module lets a user name a gateway in `config.toml` instead. The value
//! comes from config only; the parent environment never selects a target.
//!
//! Rules match phantom's governed-client gateway validator
//! (`src/core/run/gateway-target.ts`):
//! - `enabled` must be `true`; anything else changes nothing.
//! - Schemes: `https` always, `http` only for loopback.
//! - Loopback means the IP literals `127.0.0.1` and `[::1]` only. `localhost`
//!   and `*.localhost` are refused, as is every other IP literal.
//! - A non-loopback host needs an exact `host:port` entry in `allowed_hosts`.
//! - Refused: userinfo, query, fragment, non-canonical hosts (trailing dot,
//!   numeric shorthands, percent-encoding, non-ASCII), dot or empty path
//!   segments, backslashes, percent-encoding and whitespace.
//! - Refusal reasons are closed codes and never contain the input.
//!
//! Only the URL is placed in the child, under base-URL variable names the user
//! lists in `child_env`. No key, token or header is ever forwarded.

use crate::config::GatewayConfig;
use url::Url;

const MAX_URL_LENGTH: usize = 2048;
const MAX_ALLOWED_HOSTS: usize = 64;
const MAX_CHILD_ENV: usize = 8;
const LOOPBACK_HOSTS: [&str; 2] = ["127.0.0.1", "[::1]"];

/// A validated gateway target. `host` is canonical (IPv6 in brackets).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayTarget {
    /// The URL exactly as written in config (already canonical when accepted).
    pub base_url: String,
    pub host: String,
    pub port: u16,
    pub loopback: bool,
}

/// What the isolated child should get for `[gateway]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayChild {
    /// Not configured or `enabled` is not `true`: no change.
    Off,
    /// Set each variable to the validated base URL.
    Set {
        target: GatewayTarget,
        vars: Vec<String>,
    },
    /// Opted in but invalid. Callers fail closed. The code never contains input.
    Refused(&'static str),
}

fn printable_ascii(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

fn is_ipv4_literal(host: &str) -> bool {
    let parts: Vec<&str> = host.split('.').collect();
    parts.len() == 4
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 3 && p.bytes().all(|b| b.is_ascii_digit()))
}

fn is_ip_literal(host: &str) -> bool {
    host.starts_with('[') || is_ipv4_literal(host)
}

fn is_loopback_name(host: &str) -> bool {
    host == "localhost" || host.ends_with(".localhost")
}

/// Split a raw authority (no userinfo) into host and optional port text.
fn split_authority(raw: &str) -> Option<(&str, Option<&str>)> {
    if raw.starts_with('[') {
        let close = raw.find(']')?;
        let rest = &raw[close + 1..];
        if !rest.is_empty() && !rest.starts_with(':') {
            return None;
        }
        let port = if rest.is_empty() {
            None
        } else {
            Some(&rest[1..])
        };
        return Some((&raw[..=close], port));
    }
    match raw.rfind(':') {
        None => Some((raw, None)),
        Some(i) => Some((&raw[..i], Some(&raw[i + 1..]))),
    }
}

fn digits_1_to_5(s: &str) -> bool {
    !s.is_empty() && s.len() <= 5 && s.bytes().all(|b| b.is_ascii_digit())
}

fn safe_path(path: &str) -> bool {
    path.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'-' | b'/'))
}

/// Normalize one `allowed_hosts` entry to `host:port`. The port is required and
/// the host must already be canonical (case is folded).
fn normalize_allow_entry(entry: &str) -> Option<String> {
    if entry.is_empty() || entry.len() > 300 || !printable_ascii(entry) {
        return None;
    }
    if entry.contains(['/', '?', '#', '@', '\\', '%', '*']) {
        return None;
    }
    let (host_raw, port_raw) = split_authority(entry)?;
    let port_raw = port_raw?;
    if !digits_1_to_5(port_raw) {
        return None;
    }
    let parsed = Url::parse(&format!("https://{entry}/")).ok()?;
    let host = host_raw.to_ascii_lowercase();
    if parsed.host_str() != Some(host.as_str()) || host.is_empty() || host.ends_with('.') {
        return None;
    }
    if is_ip_literal(&host) || is_loopback_name(&host) {
        return None;
    }
    let port: u32 = port_raw.parse().ok()?;
    if !(1..=65535).contains(&port) {
        return None;
    }
    Some(format!("{host}:{port}"))
}

/// Validate a gateway base URL and its allowlist. Pure: no I/O, env or DNS.
pub fn validate_base_url(
    base_url: &str,
    allowed_hosts: &[String],
) -> Result<GatewayTarget, &'static str> {
    if base_url.is_empty() || base_url.len() > MAX_URL_LENGTH {
        return Err("base-url-invalid");
    }
    if !printable_ascii(base_url) || base_url.contains('\\') {
        return Err("base-url-invalid");
    }
    let colon = base_url.find(':').ok_or("base-url-invalid")?;
    let scheme_raw = &base_url[..colon];
    let mut sb = scheme_raw.bytes();
    let scheme_ok = matches!(sb.next(), Some(b) if b.is_ascii_alphabetic())
        && sb.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-'));
    if !scheme_ok {
        return Err("base-url-invalid");
    }
    let scheme = scheme_raw.to_ascii_lowercase();
    if scheme != "https" && scheme != "http" {
        return Err("scheme-not-allowed");
    }
    let rest = &base_url[colon + 1..];
    let Some(after) = rest.strip_prefix("//") else {
        return Err("base-url-invalid");
    };
    if base_url.contains('#') {
        return Err("fragment-not-allowed");
    }
    if base_url.contains('?') {
        return Err("query-not-allowed");
    }
    let (raw_authority, raw_path) = match after.find('/') {
        None => (after, ""),
        Some(i) => (&after[..i], &after[i..]),
    };
    if raw_authority.contains('@') {
        return Err("userinfo-not-allowed");
    }
    let parsed = Url::parse(base_url).map_err(|_| "base-url-invalid")?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("userinfo-not-allowed");
    }
    if parsed.query().is_some() {
        return Err("query-not-allowed");
    }
    if parsed.fragment().is_some() {
        return Err("fragment-not-allowed");
    }

    let (host_raw, port_raw) = split_authority(raw_authority).ok_or("host-not-canonical")?;
    let host = host_raw.to_ascii_lowercase();
    // The parser silently rewrites trailing dots, numeric shorthands
    // (0x7f.1, 2130706433), percent-encoding and IDNA. Require the raw text to
    // already be canonical so the allowlist compares what the user wrote.
    if host.is_empty() || parsed.host_str() != Some(host.as_str()) || host.ends_with('.') {
        return Err("host-not-canonical");
    }
    if let Some(p) = port_raw {
        if !digits_1_to_5(p) {
            return Err("port-invalid");
        }
    }
    let default_port: u16 = if scheme == "https" { 443 } else { 80 };
    let port = parsed.port().unwrap_or(default_port);
    if port == 0 {
        return Err("port-invalid");
    }
    if let Some(p) = port_raw {
        if p.parse::<u32>().ok() != Some(u32::from(port)) {
            return Err("port-invalid");
        }
    }

    let loopback = LOOPBACK_HOSTS.contains(&host.as_str());
    if !loopback {
        if is_loopback_name(&host) {
            return Err("loopback-name-not-allowed");
        }
        if is_ip_literal(&host) {
            return Err("ip-literal-not-allowed");
        }
        if scheme != "https" {
            return Err("http-requires-loopback");
        }
    }

    if !safe_path(raw_path) {
        return Err("path-not-allowed");
    }
    let trimmed = raw_path.trim_end_matches('/');
    if !trimmed.is_empty()
        && trimmed[1..]
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..")
    {
        return Err("path-not-allowed");
    }
    if raw_path.len() > 1 && raw_path.ends_with("//") {
        return Err("path-not-allowed");
    }
    if parsed.path().trim_end_matches('/') != trimmed {
        return Err("path-not-allowed");
    }

    if allowed_hosts.len() > MAX_ALLOWED_HOSTS {
        return Err("allowlist-invalid");
    }
    let mut normalized = Vec::with_capacity(allowed_hosts.len());
    for entry in allowed_hosts {
        normalized.push(normalize_allow_entry(entry).ok_or("allowlist-invalid")?);
    }
    if !loopback && !normalized.contains(&format!("{host}:{port}")) {
        return Err("host-not-allowlisted");
    }

    Ok(GatewayTarget {
        base_url: base_url.to_string(),
        host,
        port,
        loopback,
    })
}

/// A child variable name that can only carry a base URL: upper-case ASCII,
/// ending in `_BASE_URL` or `_API_BASE`, and naming nothing secret-shaped.
pub fn is_base_url_var(name: &str) -> bool {
    let shape = !name.is_empty()
        && name.len() <= 64
        && name.as_bytes()[0].is_ascii_uppercase()
        && name
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
    let suffix = name.ends_with("_BASE_URL") || name.ends_with("_API_BASE");
    let secretish = ["KEY", "TOKEN", "SECRET", "PASSWORD", "CREDENTIAL", "AUTH"]
        .iter()
        .any(|w| name.contains(w));
    shape && suffix && !secretish && !name.starts_with("LOCUS_")
}

/// Resolve `[gateway]` into what the child should receive.
pub fn resolve(cfg: &GatewayConfig) -> GatewayChild {
    if !cfg.enabled {
        return GatewayChild::Off;
    }
    let Some(base_url) = cfg.base_url.as_deref() else {
        return GatewayChild::Refused("base-url-invalid");
    };
    if cfg.child_env.is_empty() || cfg.child_env.len() > MAX_CHILD_ENV {
        return GatewayChild::Refused("child-env-invalid");
    }
    let mut vars: Vec<String> = Vec::new();
    for name in &cfg.child_env {
        if !is_base_url_var(name) {
            return GatewayChild::Refused("child-env-invalid");
        }
        if !vars.contains(name) {
            vars.push(name.clone());
        }
    }
    match validate_base_url(base_url, &cfg.allowed_hosts) {
        Ok(target) => GatewayChild::Set { target, vars },
        Err(code) => GatewayChild::Refused(code),
    }
}

/// `[gateway]` from `$LOCUS_HOME/config.toml` (or `~/.locus`). Unreadable home
/// or config means defaults, which is off.
pub fn resolve_from_home() -> GatewayChild {
    match crate::store::locus_home() {
        Ok(home) => resolve(&crate::config::load_config(&home).gateway),
        Err(_) => GatewayChild::Off,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(url: &str, hosts: &[&str]) -> GatewayTarget {
        let hosts: Vec<String> = hosts.iter().map(|s| s.to_string()).collect();
        validate_base_url(url, &hosts).unwrap_or_else(|e| panic!("{url}: {e}"))
    }

    fn err(url: &str, hosts: &[&str]) -> &'static str {
        let hosts: Vec<String> = hosts.iter().map(|s| s.to_string()).collect();
        validate_base_url(url, &hosts).expect_err(url)
    }

    #[test]
    fn loopback_ip_literals_are_accepted_over_http() {
        let t = ok("http://127.0.0.1:4000/v1", &[]);
        assert!(t.loopback);
        assert_eq!((t.host.as_str(), t.port), ("127.0.0.1", 4000));
        let t = ok("http://[::1]:4000", &[]);
        assert_eq!(t.host, "[::1]");
        assert!(ok("https://127.0.0.1/", &[]).loopback);
    }

    #[test]
    fn loopback_names_and_other_ip_literals_are_refused() {
        assert_eq!(
            err("http://localhost:4000", &[]),
            "loopback-name-not-allowed"
        );
        assert_eq!(
            err("https://gw.localhost", &[]),
            "loopback-name-not-allowed"
        );
        assert_eq!(err("http://10.0.0.5:4000", &[]), "ip-literal-not-allowed");
        assert_eq!(err("https://127.0.0.2", &[]), "ip-literal-not-allowed");
        assert_eq!(err("http://[::2]:4000", &[]), "ip-literal-not-allowed");
    }

    #[test]
    fn non_loopback_needs_https_and_an_exact_allowlist_entry() {
        assert_eq!(
            err("http://gw.example.com", &["gw.example.com:80"]),
            "http-requires-loopback"
        );
        assert_eq!(err("https://gw.example.com", &[]), "host-not-allowlisted");
        assert_eq!(
            err("https://gw.example.com", &["gw.example.com:8443"]),
            "host-not-allowlisted"
        );
        assert_eq!(
            err("https://gw.example.com", &["other.example.com:443"]),
            "host-not-allowlisted"
        );
        let t = ok("https://gw.example.com/anthropic", &["Gw.Example.com:443"]);
        assert_eq!(
            (t.host.as_str(), t.port, t.loopback),
            ("gw.example.com", 443, false)
        );
        ok("https://gw.example.com:8443/v1", &["gw.example.com:8443"]);
    }

    #[test]
    fn bad_allowlist_entries_are_refused() {
        for bad in [
            "gw.example.com",
            "*.example.com:443",
            "gw.example.com.:443",
            "10.0.0.1:443",
            "localhost:443",
            "gw.example.com:443/x",
            "u@gw.example.com:443",
            "gw.example.com:0",
        ] {
            assert_eq!(
                err("https://gw.example.com", &[bad]),
                "allowlist-invalid",
                "{bad}"
            );
        }
    }

    #[test]
    fn unsafe_urls_are_refused_with_closed_codes() {
        let host = ["gw.example.com:443"];
        assert_eq!(err("ftp://gw.example.com", &host), "scheme-not-allowed");
        assert_eq!(
            err("https://user:pw@gw.example.com", &host),
            "userinfo-not-allowed"
        );
        assert_eq!(
            err("https://gw.example.com/?k=v", &host),
            "query-not-allowed"
        );
        assert_eq!(
            err("https://gw.example.com/#f", &host),
            "fragment-not-allowed"
        );
        assert_eq!(err("https://gw.example.com./", &host), "host-not-canonical");
        // Case is folded, as in phantom.
        assert_eq!(ok("https://GW.example.com/", &host).host, "gw.example.com");
        assert_eq!(err("http://0x7f.1:4000", &[]), "host-not-canonical");
        assert_eq!(err("http://2130706433:4000", &[]), "host-not-canonical");
        assert_eq!(err("https://g%77.example.com", &host), "host-not-canonical");
        assert_eq!(
            err("https://gw.example.com/a/../b", &host),
            "path-not-allowed"
        );
        assert_eq!(
            err("https://gw.example.com/a//b", &host),
            "path-not-allowed"
        );
        assert_eq!(
            err("https://gw.example.com/a%2fb", &host),
            "path-not-allowed"
        );
        assert_eq!(err("https://gw.example.com/a b", &host), "base-url-invalid");
        assert_eq!(err("https:\\\\gw.example.com", &host), "base-url-invalid");
        assert_eq!(err("http://127.0.0.1:0", &[]), "port-invalid");
        assert_eq!(err("http://127.0.0.1:99999", &[]), "base-url-invalid");
        assert_eq!(err("", &[]), "base-url-invalid");
        assert_eq!(err("gw.example.com", &host), "base-url-invalid");
    }

    #[test]
    fn refusal_codes_never_echo_input() {
        let secretish = "https://sk-live-abc123@gw.example.com/?token=sk-live-abc123";
        let code = err(secretish, &[]);
        assert!(!code.contains("sk-live"));
    }

    #[test]
    fn child_env_names_can_only_be_base_url_variables() {
        for good in ["OPENAI_BASE_URL", "ANTHROPIC_BASE_URL", "OPENAI_API_BASE"] {
            assert!(is_base_url_var(good), "{good}");
        }
        for bad in [
            "OPENAI_API_KEY",
            "ANTHROPIC_AUTH_TOKEN_BASE_URL",
            "PATH",
            "HTTPS_PROXY",
            "openai_base_url",
            "LOCUS_GATEWAY_BASE_URL",
            "",
            "X_SECRET_BASE_URL",
        ] {
            assert!(!is_base_url_var(bad), "{bad}");
        }
    }

    #[test]
    fn resolve_is_off_unless_enabled_is_true() {
        assert_eq!(resolve(&GatewayConfig::default()), GatewayChild::Off);
        let cfg = GatewayConfig {
            enabled: false,
            base_url: Some("http://127.0.0.1:4000".into()),
            child_env: vec!["OPENAI_BASE_URL".into()],
            allowed_hosts: vec![],
        };
        assert_eq!(resolve(&cfg), GatewayChild::Off);
    }

    #[test]
    fn resolve_refuses_an_opted_in_but_invalid_block() {
        let base = GatewayConfig {
            enabled: true,
            base_url: Some("http://127.0.0.1:4000".into()),
            child_env: vec!["OPENAI_BASE_URL".into()],
            allowed_hosts: vec![],
        };
        let mut c = base.clone();
        c.base_url = None;
        assert_eq!(resolve(&c), GatewayChild::Refused("base-url-invalid"));
        let mut c = base.clone();
        c.child_env = vec![];
        assert_eq!(resolve(&c), GatewayChild::Refused("child-env-invalid"));
        let mut c = base.clone();
        c.child_env = vec!["OPENAI_API_KEY".into()];
        assert_eq!(resolve(&c), GatewayChild::Refused("child-env-invalid"));
        let mut c = base.clone();
        c.base_url = Some("http://localhost:4000".into());
        assert_eq!(
            resolve(&c),
            GatewayChild::Refused("loopback-name-not-allowed")
        );
        match resolve(&base) {
            GatewayChild::Set { target, vars } => {
                assert_eq!(target.base_url, "http://127.0.0.1:4000");
                assert_eq!(vars, vec!["OPENAI_BASE_URL".to_string()]);
            }
            other => panic!("{other:?}"),
        }
    }
}

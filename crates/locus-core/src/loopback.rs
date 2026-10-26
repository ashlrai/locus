//! The single loopback rule for locus: loopback means exactly `127.0.0.1`
//! and `::1`. The hostname `localhost` (and `*.localhost`) is never treated
//! as loopback, and neither is the rest of `127.0.0.0/8`.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// True only for the IP literals `127.0.0.1` and `::1`.
pub fn is_exact_loopback_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4 == Ipv4Addr::LOCALHOST,
        IpAddr::V6(v6) => v6 == Ipv6Addr::LOCALHOST,
    }
}

/// True only for a parsed URL host that is `127.0.0.1` or `::1`. Domain
/// names, including `localhost`, are never loopback.
pub fn is_exact_loopback_host(host: &url::Host<&str>) -> bool {
    match host {
        url::Host::Ipv4(ip) => is_exact_loopback_ip(IpAddr::V4(*ip)),
        url::Host::Ipv6(ip) => is_exact_loopback_ip(IpAddr::V6(*ip)),
        url::Host::Domain(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_loopback_only() {
        assert!(is_exact_loopback_ip("127.0.0.1".parse().unwrap()));
        assert!(is_exact_loopback_ip("::1".parse().unwrap()));
        for ip in [
            "127.0.0.2",
            "127.1.2.3",
            "0.0.0.0",
            "::",
            "10.0.0.1",
            "::ffff:127.0.0.1",
        ] {
            assert!(!is_exact_loopback_ip(ip.parse().unwrap()), "{ip}");
        }
        assert!(!is_exact_loopback_host(&url::Host::Domain("localhost")));
    }
}

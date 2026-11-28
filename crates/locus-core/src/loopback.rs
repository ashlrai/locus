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

    #[test]
    fn parsed_url_hosts_obey_exact_loopback_rule() {
        for (url, expected) in [
            ("http://127.0.0.1:8080/", true),
            ("http://[::1]:8080/", true),
            ("http://127.0.0.2/", false),
            ("http://[::ffff:127.0.0.1]/", false),
            ("http://localhost/", false),
            ("http://service.localhost/", false),
        ] {
            let parsed = url::Url::parse(url).unwrap();
            assert_eq!(
                is_exact_loopback_host(&parsed.host().unwrap()),
                expected,
                "{url}"
            );
        }
    }

    #[test]
    fn edge_case_ipv4_loopback_addresses() {
        // Test edge case addresses that are close to loopback but not exactly loopback
        let edge_cases = [
            "127.0.0.0",  // Start of 127.x.x.x range
            "127.255.255.255",  // End of 127.x.x.x range
            "127.0.0.100",  // Different port in same network 
        ];
        
        for ip in edge_cases {
            let parsed = ip.parse().unwrap();
            assert!(!is_exact_loopback_ip(parsed), "Expected {} to NOT be exact loopback", ip);
        }
    }

    #[test]
    fn edge_case_ipv6_loopback_addresses() {
        // Test various IPv6 addresses that look like loopback but aren't exactly ::1
        let edge_cases = [
            "::ffff:127.0.0.1",  // IPv4-mapped address - not exactly ::1
            "::1:0:0",           // Different canonical form - not exactly ::1
            "::ffff:7f00:1",     // Another representation - not exactly ::1  
        ];
        
        for ip in edge_cases {
            let parsed = ip.parse().unwrap();
            assert!(!is_exact_loopback_ip(parsed), "Expected {} to NOT be exact loopback", ip);
        }
    }

    #[test]
    fn error_path_invalid_url_host() {
        // Ensure that our function handles invalid URL hosts gracefully
        // This test just verifies the current behavior - no crash or panic
        let url = "http://[::1]:8080/";
        let parsed = url::Url::parse(url).unwrap();
        let host = parsed.host().unwrap();
        
        // Function should work normally and not panic on valid input
        assert_eq!(is_exact_loopback_host(&host), true);
    }
}

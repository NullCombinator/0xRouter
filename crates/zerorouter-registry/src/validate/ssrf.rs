//! Endpoint URL rules for plugin-declared upstreams (research R18).
//!
//! The gate rejects hosts that reach the machine or its network unless the operator sets
//! `allow_private_endpoints`. The engine's connector re-checks the resolved address with
//! [`is_private_ip`], so a public name that resolves privately is caught too.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use super::secrets::check_url;

/// Placeholders an endpoint URL may carry, in its path only.
pub const URL_PLACEHOLDERS: &[&str] = &["model", "voice"];

/// Checks a plugin endpoint URL: http(s), no credentials, placeholders only in the path,
/// no `.`/`..` segments, and (unless `allow_private`) a public host.
pub fn check_endpoint_url(url: &str, allow_private: bool) -> Result<(), String> {
    check_url(url).map_err(str::to_owned)?;
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    let path_end = tail.find(['?', '#']).unwrap_or(tail.len());
    let (path, after) = tail.split_at(path_end);

    if authority.contains(['{', '}']) || after.contains(['{', '}']) {
        return Err("placeholders are allowed only in the URL path".into());
    }
    check_path_placeholders(path)?;
    if path.split('/').any(|seg| seg == "." || seg == "..") {
        return Err("URL path must not contain `.` or `..` segments".into());
    }

    let host = host_of(authority)?;
    if allow_private {
        return Ok(());
    }
    match host_kind(&host) {
        Host::Ip(ip) if is_private_ip(ip) => Err(format!(
            "host {host} is loopback, private, link-local or metadata; set allow_private_endpoints = true to allow it"
        )),
        Host::Ip(_) => Ok(()),
        Host::AmbiguousNumeric => Err(format!("host {host} is not a canonical address")),
        Host::Name if is_private_name(&host) => Err(format!(
            "host {host} is a local or internal name; set allow_private_endpoints = true to allow it"
        )),
        Host::Name => Ok(()),
    }
}

fn check_path_placeholders(path: &str) -> Result<(), String> {
    let mut rest = path;
    while let Some(open) = rest.find('{') {
        let close = rest[open..].find('}').ok_or("unclosed `{` in URL path")? + open;
        let name = &rest[open + 1..close];
        if !URL_PLACEHOLDERS.contains(&name) {
            return Err(format!("unknown URL placeholder {{{name}}}; allowed: {{model}}, {{voice}}"));
        }
        rest = &rest[close + 1..];
    }
    if rest.contains('}') { Err("unmatched `}` in URL path".into()) } else { Ok(()) }
}

/// The host part of an authority, lowercased, without port, brackets or a trailing dot.
fn host_of(authority: &str) -> Result<String, String> {
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split_once(']').ok_or("unclosed `[` in URL host")?.0
    } else {
        authority.rsplit_once(':').map_or(authority, |(h, port)| {
            if port.chars().all(|c| c.is_ascii_digit()) { h } else { authority }
        })
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() { Err("URL has no host".into()) } else { Ok(host) }
}

enum Host {
    Ip(IpAddr),
    /// Digits-and-dots or hex forms (`2130706433`, `0x7f.1`) that resolvers may read as
    /// an address but aren't dotted-quad.
    AmbiguousNumeric,
    Name,
}

fn host_kind(host: &str) -> Host {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Host::Ip(ip);
    }
    let last = host.rsplit('.').next().unwrap_or(host);
    let numeric_label = |l: &str| {
        !l.is_empty()
            && (l.chars().all(|c| c.is_ascii_digit())
                || l.strip_prefix("0x").is_some_and(|h| h.chars().all(|c| c.is_ascii_hexdigit())))
    };
    if numeric_label(last) { Host::AmbiguousNumeric } else { Host::Name }
}

fn is_private_name(host: &str) -> bool {
    host == "localhost"
        || [".localhost", ".local", ".internal"].iter().any(|s| host.ends_with(s))
        || host == "metadata.google.internal"
}

/// True for loopback, unspecified, private, shared (100.64/10), link-local, multicast,
/// broadcast and ULA addresses, including IPv4-mapped IPv6 forms. Cloud metadata
/// addresses (169.254.169.254, fd00:ec2::254) fall in these ranges.
pub fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => private_v4(v4),
        IpAddr::V6(v6) => private_v6(v6),
    }
}

fn private_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || a == 0
        || (a == 100 && (64..128).contains(&b))
}

fn private_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return private_v4(v4);
    }
    let first = ip.segments()[0];
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (first & 0xfe00) == 0xfc00
        || (first & 0xffc0) == 0xfe80
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_urls_pass() {
        for url in [
            "https://api.openai.com/v1/chat/completions",
            "https://generativelanguage.googleapis.com/v1beta/models/{model}:streamGenerateContent?alt=sse",
            "https://api.elevenlabs.io/v1/text-to-speech/{voice}",
            "https://8.8.8.8:443/x",
            "https://[2001:4860::8888]/x",
        ] {
            assert_eq!(check_endpoint_url(url, false), Ok(()), "{url}");
        }
    }

    #[test]
    fn private_hosts_rejected_unless_allowed() {
        for url in [
            "http://localhost:11434/api/chat",
            "http://LOCALHOST./x",
            "http://127.0.0.1/x",
            "http://10.1.2.3/x",
            "http://172.16.0.1/x",
            "http://192.168.1.1/x",
            "http://100.64.0.1/x",
            "http://169.254.169.254/latest/meta-data",
            "http://[::1]:8080/x",
            "http://[fd00:ec2::254]/x",
            "http://[fe80::1]/x",
            "http://[::ffff:127.0.0.1]/x",
            "http://metadata.google.internal/x",
            "http://printer.local/x",
            "http://svc.corp.internal/x",
            "http://0.0.0.0/x",
            "http://2130706433/x",
            "http://0x7f.1/x",
        ] {
            assert!(check_endpoint_url(url, false).is_err(), "{url}");
        }
        assert_eq!(check_endpoint_url("http://localhost:11434/api/chat", true), Ok(()));
    }

    #[test]
    fn placeholders_and_segments() {
        let err = |u: &str| check_endpoint_url(u, false).unwrap_err();
        assert!(err("https://{model}.example.com/x").contains("only in the URL path"));
        assert!(err("https://example.com/x?m={model}").contains("only in the URL path"));
        assert!(err("https://example.com/{account}/x").contains("unknown URL placeholder {account}"));
        assert!(err("https://example.com/a/../b").contains("`..`"));
        assert!(err("https://example.com/a/./b").contains("`..`"));
        assert!(err("https://user:pw@example.com/").contains("credentials"));
        assert!(err("ftp://example.com/").contains("http(s)"));
    }

    #[test]
    fn resolved_ip_check() {
        assert!(is_private_ip("100.127.255.255".parse().unwrap()));
        assert!(!is_private_ip("100.128.0.1".parse().unwrap()));
        assert!(!is_private_ip("1.1.1.1".parse().unwrap()));
        assert!(!is_private_ip("2606:4700::1111".parse().unwrap()));
    }
}

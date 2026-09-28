//! Which client headers reach the upstream (research R18, R27).
//!
//! A same-style attempt passes every client header; a cross-style one passes only the
//! names the provider declares in `forwarding.to_upstream`. Either way the security floor
//! applies first, then values with CR/LF or a secret in them are left out. A declared rule
//! whose `from_styles` admits the client's style still sets how a header merges with the
//! endpoint's static value.

use std::borrow::Cow;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use zerorouter_registry::floor::Floor;
use zerorouter_registry::schema::{ForwardHeader, ForwardMerge};

use crate::redact::Redactor;

/// `pattern` is a header name, or a prefix ending in `*`. Case-insensitive.
pub fn name_matches(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => name.len() >= prefix.len() && name[..prefix.len()].eq_ignore_ascii_case(prefix),
        None => pattern.eq_ignore_ascii_case(name),
    }
}

/// True if the header may leave 0router at all.
fn passes_floor(name: &HeaderName, value: &HeaderValue, floor: &Floor, redactor: &Redactor) -> bool {
    if floor.blocks(name.as_str()) || value.as_bytes().iter().any(|b| matches!(b, b'\r' | b'\n')) {
        return false;
    }
    value.to_str().is_ok_and(|text| matches!(redactor.redact(text), Cow::Borrowed(_)))
}

/// The client headers one attempt forwards, each with how it merges with a static header.
pub fn client_headers(
    rules: &[ForwardHeader],
    same_style: bool,
    client_style: &str,
    headers: &HeaderMap,
    floor: &Floor,
    redactor: &Redactor,
) -> Vec<(HeaderName, HeaderValue, ForwardMerge)> {
    let rules: Vec<&ForwardHeader> = rules
        .iter()
        .filter(|r| r.from_styles.is_empty() || r.from_styles.iter().any(|s| s == client_style))
        .collect();
    let safe = headers.iter().filter(|(n, v)| passes_floor(n, v, floor, redactor));
    if same_style {
        safe.map(|(n, v)| {
            let merge = rules.iter().find(|r| name_matches(&r.name, n.as_str())).map_or(ForwardMerge::Replace, |r| r.merge);
            (n.clone(), v.clone(), merge)
        })
        .collect()
    } else {
        let safe: Vec<_> = safe.collect();
        rules
            .iter()
            .flat_map(|r| safe.iter().filter(|(n, _)| name_matches(&r.name, n.as_str())).map(|(n, v)| ((*n).clone(), (*v).clone(), r.merge)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerorouter_registry::SecretString;

    fn rule(name: &str, merge: ForwardMerge, from: &[&str]) -> ForwardHeader {
        ForwardHeader { name: name.into(), merge, from_styles: from.iter().map(|s| (*s).to_owned()).collect() }
    }

    #[test]
    fn same_style_passes_every_header_but_the_floor_and_secrets() {
        let secret = SecretString::new("sk-acme-SECRET-1");
        let redactor = Redactor::new([&secret]);
        let floor = Floor::default();
        let mut h = HeaderMap::new();
        for (k, v) in [
            ("anthropic-beta", "context-management-2025-06-27"),
            ("x-headroom-session", "s1"),
            ("user-agent", "claude-cli/2.0"),
            ("connection", "keep-alive"),
            ("accept-encoding", "gzip"),
            ("host", "127.0.0.1"),
            ("x-0router-agent", "a1"),
            ("x-api-key", "0r-client-key"),
            ("x-leak", "has sk-acme-SECRET-1 inside"),
        ] {
            h.insert(k, HeaderValue::from_static(v));
        }
        let rules = [rule("anthropic-beta", ForwardMerge::AppendCsv, &["anthropic-messages"])];

        let same = client_headers(&rules, true, "anthropic-messages", &h, &floor, &redactor);
        let names: Vec<(&str, ForwardMerge)> = same.iter().map(|(n, _, m)| (n.as_str(), *m)).collect();
        assert_eq!(
            names,
            [
                ("anthropic-beta", ForwardMerge::AppendCsv),
                ("x-headroom-session", ForwardMerge::Replace),
                ("user-agent", ForwardMerge::Replace),
            ]
        );

        let cross = client_headers(&rules, false, "anthropic-messages", &h, &floor, &redactor);
        assert_eq!(cross.len(), 1, "cross-style keeps the declared list only");
        assert_eq!(cross[0].0, "anthropic-beta");
        assert!(client_headers(&rules, false, "openai-chat", &h, &floor, &redactor).is_empty(), "from_styles limits the rule");
    }
}

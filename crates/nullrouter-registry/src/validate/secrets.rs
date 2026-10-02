//! Keeps credentials out of plugin files (research R5).
//!
//! The key denylist applies to open maps only (`headers`, model `params`). Closed structs
//! are already guarded by their key sets.

/// Terms that mark a key as credential-bearing, matched case-insensitively on the key
/// with `-` read as `_`.
const DENYLIST: &[&str] =
    &["secret", "password", "passwd", "api_key", "apikey", "access_token", "refresh_token", "cookie", "authorization"];

/// Returns the denylisted term `key` hits, if any. Any key containing `token` is also
/// rejected unless it names a URL or endpoint (`token_url`, `tokenEndpoint`).
pub fn check_map_key(key: &str) -> Option<&'static str> {
    let k = key.to_ascii_lowercase().replace('-', "_");
    if let Some(term) = DENYLIST.iter().find(|t| k.contains(**t)) {
        return Some(term);
    }
    let names_location = k.ends_with("url") || k.ends_with("endpoint");
    (k.contains("token") && !names_location).then_some("token")
}

/// Rejects userinfo and secret-like query keys in an absolute http(s) URL.
pub fn check_url(url: &str) -> Result<(), &'static str> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).ok_or("must be an http(s) URL")?;
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    if rest[..authority_end].contains('@') {
        return Err("URL must not carry credentials");
    }
    check_query(rest)
}

/// Rejects secret-like query keys in a URL, path, or suffix such as `?beta=true`.
pub fn check_query(s: &str) -> Result<(), &'static str> {
    let Some(q) = s.split('#').next().and_then(|s| s.split_once('?')).map(|(_, q)| q) else {
        return Ok(());
    };
    let secret = q.split('&').map(|kv| kv.split_once('=').map_or(kv, |(k, _)| k)).any(|k| check_map_key(k).is_some());
    if secret { Err("URL must not carry credentials") } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denylisted_keys() {
        for k in [
            "secret",
            "client_secret",
            "Password",
            "passwd",
            "api_key",
            "apiKey",
            "access_token",
            "refresh_token",
            "Cookie",
            "Authorization",
            "X-Api-Key",
            "Proxy-Authorization",
            "x-goog-api-key",
            "session-token",
        ] {
            assert!(check_map_key(k).is_some(), "{k} should be rejected");
        }
    }

    #[test]
    fn allowed_keys() {
        for k in ["User-Agent", "Anthropic-Version", "token_url", "tokenEndpoint", "X-Title", "size"] {
            assert_eq!(check_map_key(k), None, "{k} should be allowed");
        }
    }

    #[test]
    fn urls() {
        assert!(check_url("https://api.example/v1?beta=true").is_ok());
        assert!(check_url("https://{region}.example/v1").is_ok());
        assert_eq!(check_url("https://u:p@x.example"), Err("URL must not carry credentials"));
        assert_eq!(check_url("https://x.example/?api_key=1"), Err("URL must not carry credentials"));
        assert_eq!(check_url("https://x.example/p?a=1&Token=2#f"), Err("URL must not carry credentials"));
        assert!(check_url("https://x.example/a@b").is_ok());
        assert_eq!(check_url("ftp://x.example"), Err("must be an http(s) URL"));
        assert!(check_query("?beta=true").is_ok());
    }

    #[test]
    fn no_bundled_header_is_rejected() {
        let mut names = std::collections::BTreeSet::new();
        for (_, src) in crate::load::BUNDLED.iter().chain(crate::community::COMMUNITY) {
            let file: crate::schema::PluginFile = toml::from_str(src).unwrap();
            let sections = file.capabilities.values().filter_map(|s| s.endpoint.as_ref());
            for t in file.transport.iter().chain(&file.transports) {
                names.extend(t.headers.iter().flat_map(|h| h.keys()).cloned());
            }
            for e in sections {
                names.extend(e.headers.keys().cloned());
            }
        }
        assert!(names.len() >= 39, "only {} header names", names.len());
        for n in &names {
            assert_eq!(check_map_key(n), None, "bundled header {n} rejected");
        }
    }
}

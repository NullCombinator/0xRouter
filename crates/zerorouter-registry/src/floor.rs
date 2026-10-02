//! The security floor: header names never forwarded in either direction, whatever a
//! plugin declares (contracts/provider-schema-v2.md § Forwarding, research R18).

use std::collections::BTreeSet;

use crate::validate::check_map_key;

/// Credential carriers.
const CREDENTIALS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "x-api-key",
    "api-key",
    "x-goog-api-key",
    "xi-api-key",
    "cookie",
    "set-cookie",
    "set-cookie2",
    "www-authenticate",
    "proxy-authenticate",
    "x-amz-security-token",
    "x-auth-token",
];

/// Hop-by-hop and core-owned headers.
const CORE_OWNED: &[&str] = &[
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "keep-alive",
    "proxy-connection",
    "te",
    "trailer",
    "upgrade",
    "content-encoding",
    "accept-encoding",
];

/// Headers under this prefix belong to 0router.
const CORE_PREFIX: &str = "x-0router-";

/// The header names no forwarding rule may pass.
#[derive(Debug, Clone)]
pub struct Floor {
    /// Lowercased static and computed names.
    names: BTreeSet<String>,
}

/// How dangerous a forwarding pattern is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatternRisk {
    Ok,
    /// The pattern could match a floor name; matching names are still blocked at runtime.
    Warning(String),
    Error(String),
}

impl Default for Floor {
    fn default() -> Self {
        Self { names: CREDENTIALS.iter().chain(CORE_OWNED).map(|s| (*s).to_owned()).collect() }
    }
}

impl Floor {
    /// The static floor plus every loaded style's key-carrier headers and every loaded
    /// provider's auth header.
    pub fn computed<'a>(
        style_carriers: impl IntoIterator<Item = &'a str>,
        provider_auth_headers: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        let mut floor = Self::default();
        floor.names.extend(style_carriers.into_iter().chain(provider_auth_headers).map(str::to_ascii_lowercase));
        floor
    }

    /// True if `name` may never be forwarded. Case-insensitive.
    pub fn blocks(&self, name: &str) -> bool {
        let n = name.to_ascii_lowercase();
        self.names.contains(&n) || n.starts_with(CORE_PREFIX) || check_map_key(&n).is_some()
    }

    /// Rates a forwarding list entry. Only a trailing `*` is a wildcard; exact names are
    /// always `Ok` here and are checked with [`Floor::blocks`].
    pub fn pattern_risk(&self, pattern: &str) -> PatternRisk {
        let Some(star) = pattern.find('*') else { return PatternRisk::Ok };
        let prefix = pattern[..star].to_ascii_lowercase();
        if star != pattern.len() - 1 {
            return PatternRisk::Error(format!("`{pattern}`: only a trailing `*` is supported"));
        }
        if prefix.len() < 3 {
            return PatternRisk::Error(format!("`{pattern}`: a wildcard needs a prefix of at least 3 characters"));
        }
        let hit = self
            .names
            .iter()
            .find(|n| n.starts_with(&prefix))
            .cloned()
            .or_else(|| {
                (CORE_PREFIX.starts_with(&prefix) || prefix.starts_with(CORE_PREFIX)).then(|| CORE_PREFIX.into())
            })
            .or_else(|| check_map_key(&prefix).map(|t| format!("names containing `{t}`")));
        match hit {
            Some(n) => PatternRisk::Warning(format!("`{pattern}` could match floor header {n}; it is never forwarded")),
            None => PatternRisk::Ok,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_floor_blocks_case_insensitively() {
        let f = Floor::default();
        for n in ["Authorization", "X-API-KEY", "set-cookie2", "Host", "Accept-Encoding", "x-0router-request-id"] {
            assert!(f.blocks(n), "{n}");
        }
        assert!(f.blocks("x-refresh-token"), "secret-name pattern");
        assert!(!f.blocks("anthropic-beta"));
        assert!(!f.blocks("retry-after"));
    }

    #[test]
    fn computed_adds_carriers() {
        let f = Floor::computed(["X-Custom-Key"], ["x-provider-auth"]);
        assert!(f.blocks("x-custom-key"));
        assert!(f.blocks("X-Provider-Auth"));
    }

    #[test]
    fn pattern_risks() {
        let f = Floor::default();
        assert_eq!(f.pattern_risk("anthropic-ratelimit-*"), PatternRisk::Ok);
        assert_eq!(f.pattern_risk("request-id"), PatternRisk::Ok);
        assert!(matches!(f.pattern_risk("*"), PatternRisk::Error(_)));
        assert!(matches!(f.pattern_risk("x-*"), PatternRisk::Error(_)));
        assert!(matches!(f.pattern_risk("a*b"), PatternRisk::Error(_)));
        assert!(matches!(f.pattern_risk("x-api-*"), PatternRisk::Warning(_)));
        assert!(matches!(f.pattern_risk("x-0r*"), PatternRisk::Warning(_)));
        assert!(matches!(f.pattern_risk("x-session-token*"), PatternRisk::Warning(_)));
    }
}

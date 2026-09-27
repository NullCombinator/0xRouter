//! The core credential table (FR-012, FR-012a).
//!
//! 9router ships four public "installed-app" OAuth client secrets in its source. They
//! live here, never in a plugin, and are released to a provider only while the active
//! plugin for that id sends OAuth traffic to the hosts the secret was issued for.
//!
//! The value cannot be read from outside the crate:
//!
//! ```compile_fail
//! fn leak(s: &zerorouter_registry::SecretString) -> &str {
//!     s.expose()
//! }
//! ```
//!
//! and the table itself is not nameable:
//!
//! ```compile_fail
//! let _ = &zerorouter_registry::credentials::BUNDLED_CREDENTIALS;
//! ```

use std::collections::BTreeSet;
use std::fmt;
use std::sync::LazyLock;

use url::Url;

use crate::schema::{ProviderEntity, oauth_urls};

/// A secret whose value never leaves the crate. `Debug`/`Display` print `***`.
pub struct SecretString(Box<str>);

impl SecretString {
    /// Compares against `candidate` without revealing the value.
    pub fn matches(&self, candidate: &str) -> bool {
        let (a, b) = (self.0.as_bytes(), candidate.as_bytes());
        a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
    }

    #[allow(dead_code)] // read by the execution slice
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

/// Whether a provider's bundled client secret is released in this snapshot.
#[derive(Debug, Clone)]
pub enum ResolvedCredential {
    Available(&'static SecretString),
    /// The active plugin sends OAuth traffic to `offending_url`, outside the bound hosts.
    Withheld { offending_url: Url },
}

struct RawCredential {
    provider_id: &'static str,
    client_secret: &'static str,
    bound_hosts: &'static [&'static str],
}

include!("bundled.rs");

pub(crate) struct CredentialEntry {
    pub(crate) provider_id: &'static str,
    secret: SecretString,
    bound_hosts: BTreeSet<&'static str>,
}

pub(crate) static TABLE: LazyLock<Vec<CredentialEntry>> = LazyLock::new(|| {
    BUNDLED_CREDENTIALS
        .iter()
        .map(|c| CredentialEntry {
            provider_id: c.provider_id,
            secret: SecretString(c.client_secret.into()),
            bound_hosts: c.bound_hosts.iter().copied().collect(),
        })
        .collect()
});

/// Releases `entry` to `active` only if every OAuth URL it declares is on a bound host.
/// A plugin that declares no OAuth URLs (gemini) matches.
pub(crate) fn bind(entry: &'static CredentialEntry, active: &ProviderEntity) -> ResolvedCredential {
    let transports = active.all_transports();
    let offending = oauth_urls(active.oauth.as_ref(), &transports).find(|u| {
        let host = Url::parse(u).ok().and_then(|u| u.host_str().map(str::to_owned));
        !host.is_some_and(|h| entry.bound_hosts.contains(h.as_str()))
    });
    match offending {
        None => ResolvedCredential::Available(&entry.secret),
        Some(u) => ResolvedCredential::Withheld {
            offending_url: Url::parse(u).unwrap_or_else(|_| Url::parse("invalid:").expect("static URL")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_is_opaque() {
        let s = SecretString("abc".into());
        assert_eq!(format!("{s:?} {s}"), "*** ***");
        assert!(s.matches("abc"));
        assert!(!s.matches("abd") && !s.matches("ab"));
    }

    #[test]
    fn table_has_four_bound_entries() {
        assert_eq!(TABLE.len(), 4);
        assert!(TABLE.iter().all(|e| !e.bound_hosts.is_empty()));
    }
}

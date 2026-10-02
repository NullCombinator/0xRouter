//! The access-key check (contracts/client-surface.md § Access key). Runs on headers and
//! query alone, before the body is read.

use axum::http::HeaderMap;
use nullrouter_engine::keys::{AgentKey, Keys};
use nullrouter_registry::schema::{KeyCarrier, KeyScheme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Missing,
    /// Unknown or revoked: the client isn't told which.
    Unknown,
}

impl Refusal {
    pub fn message(self) -> &'static str {
        match self {
            Self::Missing => "0router: no access key. Send an agent key from `nullrouter keys issue`.",
            Self::Unknown => "0router: the access key is unknown or revoked.",
        }
    }
}

fn query_value(query: &str, name: &str) -> Option<String> {
    url::form_urlencoded::parse(query.as_bytes()).find(|(k, _)| k == name).map(|(_, v)| v.into_owned())
}

fn bearer(v: &str) -> Option<&str> {
    let (scheme, rest) = v.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| rest.trim())
}

/// The key from the first carrier present, in the style's order.
pub fn presented(carriers: &[KeyCarrier], headers: &HeaderMap, query: Option<&str>) -> Option<String> {
    carriers.iter().find_map(|c| {
        let v = if let Some(h) = &c.header {
            let raw = headers.get(h.as_str())?.to_str().ok()?;
            match c.scheme {
                KeyScheme::Bearer => bearer(raw)?.to_owned(),
                KeyScheme::Raw => raw.trim().to_owned(),
            }
        } else {
            query_value(query?, c.query.as_deref()?)?
        };
        (!v.is_empty()).then_some(v)
    })
}

/// The agent key the request presents, or why it's refused.
pub fn check<'k>(
    keys: &'k Keys,
    carriers: &[KeyCarrier],
    headers: &HeaderMap,
    query: Option<&str>,
) -> Result<&'k AgentKey, Refusal> {
    let key = presented(carriers, headers, query).ok_or(Refusal::Missing)?;
    keys.lookup(&key).ok_or(Refusal::Unknown)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn carriers() -> Vec<KeyCarrier> {
        vec![
            KeyCarrier { header: Some("x-goog-api-key".into()), query: None, scheme: KeyScheme::Raw },
            KeyCarrier { header: None, query: Some("key".into()), scheme: KeyScheme::Raw },
            KeyCarrier { header: Some("authorization".into()), query: None, scheme: KeyScheme::Bearer },
        ]
    }

    #[test]
    fn carriers_are_read_in_order() {
        let mut h = HeaderMap::new();
        assert_eq!(presented(&carriers(), &h, None), None);
        h.insert("authorization", "Basic abc".parse().unwrap());
        assert_eq!(presented(&carriers(), &h, None), None, "not a bearer value");
        h.insert("authorization", "bearer 0r-c".parse().unwrap());
        assert_eq!(presented(&carriers(), &h, None).as_deref(), Some("0r-c"));
        assert_eq!(presented(&carriers(), &h, Some("alt=sse&key=0r-b")).as_deref(), Some("0r-b"));
        h.insert("x-goog-api-key", "0r-a".parse().unwrap());
        assert_eq!(presented(&carriers(), &h, Some("key=0r-b")).as_deref(), Some("0r-a"));
    }

    #[test]
    fn unknown_and_revoked_keys_are_refused() {
        let mut keys = Keys::default();
        let (good, _) = keys.issue("a", None).unwrap();
        let (gone, k) = keys.issue("b", None).unwrap();
        let gone_id = k.id.clone();
        keys.revoke(&gone_id).unwrap();
        let c = &carriers()[2..];
        let h = |k: &str| {
            let mut h = HeaderMap::new();
            h.insert("authorization", format!("Bearer {k}").parse().unwrap());
            h
        };
        assert_eq!(check(&keys, c, &HeaderMap::new(), None).err(), Some(Refusal::Missing));
        assert_eq!(check(&keys, c, &h("0r-nope"), None).err(), Some(Refusal::Unknown));
        assert_eq!(check(&keys, c, &h(&gone), None).err(), Some(Refusal::Unknown));
        assert_eq!(check(&keys, c, &h(&good), None).unwrap().name, "a");
    }
}

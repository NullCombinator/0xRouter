//! `[identity]`: headers a sign-in account's requests carry (research R7, spec
//! Clarifications Q5). A value is a fixed string or exactly one core placeholder; the core
//! fills placeholders with information it owns. A plugin can't compute a value.

use std::fmt;

use indexmap::IndexMap;
use serde::de::{Deserializer, Error as _};
use serde::{Deserialize, Serialize, Serializer};

use super::enums::closed_enum;

closed_enum!(
    /// The closed set of values the core fills in an `[identity]` header: the agent's
    /// session id (else a random id kept per agent), a fresh id per request, the user turns
    /// in the conversation, the upstream model id, the signed-in account's email and user
    /// id, and a random id generated once per installation.
    Placeholder, "placeholder" {
        SessionId = "session.id",
        RequestId = "request.id",
        SessionTurn = "session.turn",
        ModelUpstream = "model.upstream",
        AccountEmail = "account.email",
        AccountUserId = "account.user_id",
        InstallId = "install.id",
    }
);

/// One `[identity]` header value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderValue {
    /// Sent as written.
    Fixed(String),
    /// Filled by the core per request.
    Core(Placeholder),
}

impl HeaderValue {
    /// Parses `"text"` (no braces) or `"{placeholder}"`.
    pub fn parse(s: &str) -> Result<Self, String> {
        if !s.contains(['{', '}']) {
            return Ok(Self::Fixed(s.to_owned()));
        }
        let inner = s.strip_prefix('{').and_then(|r| r.strip_suffix('}')).filter(|i| !i.contains(['{', '}']));
        let Some(inner) = inner else {
            return Err(format!("{s:?}: a header value is a fixed string or exactly one placeholder"));
        };
        Placeholder::parse(inner).map(Self::Core).ok_or_else(|| {
            let allowed: Vec<String> = Placeholder::ALLOWED.iter().map(|p| format!("{{{p}}}")).collect();
            format!("unknown placeholder {{{inner}}}; allowed: {}", allowed.join(", "))
        })
    }
}

impl fmt::Display for HeaderValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fixed(s) => f.write_str(s),
            Self::Core(p) => write!(f, "{{{p}}}"),
        }
    }
}

impl<'de> Deserialize<'de> for HeaderValue {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::parse(&String::deserialize(d)?).map_err(D::Error::custom)
    }
}

/// `[identity]`. Applied to every request, poll and live-model call of a sign-in account.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityDecl {
    #[serde(default)]
    pub headers: IndexMap<String, HeaderValue>,
}

impl IdentityDecl {
    /// The placeholders this declaration uses.
    pub fn placeholders(&self) -> impl Iterator<Item = Placeholder> + '_ {
        self.headers.values().filter_map(|v| match v {
            HeaderValue::Core(p) => Some(*p),
            HeaderValue::Fixed(_) => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_or_one_placeholder() {
        assert_eq!(
            HeaderValue::parse("grok-shell/0.2.99 (linux)"),
            Ok(HeaderValue::Fixed("grok-shell/0.2.99 (linux)".into()))
        );
        assert_eq!(HeaderValue::parse("{install.id}"), Ok(HeaderValue::Core(Placeholder::InstallId)));
        assert_eq!(Placeholder::ALL.len(), 7);
        for p in Placeholder::ALL {
            let v = HeaderValue::Core(*p);
            assert_eq!(HeaderValue::parse(&v.to_string()), Ok(v));
        }
        assert!(HeaderValue::parse("{machine.id}").unwrap_err().contains("unknown placeholder {machine.id}"));
        for bad in ["id-{session.id}", "{session.id}{request.id}", "{session.id", "}", "{{session.id}}"] {
            assert!(HeaderValue::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn parses_a_table() {
        let d: IdentityDecl =
            toml::from_str("[headers]\nUser-Agent = \"x/1\"\nx-grok-req-id = \"{request.id}\"\n").unwrap();
        assert_eq!(d.placeholders().collect::<Vec<_>>(), [Placeholder::RequestId]);
        assert!(toml::from_str::<IdentityDecl>("other = 1\n").is_err());
    }
}

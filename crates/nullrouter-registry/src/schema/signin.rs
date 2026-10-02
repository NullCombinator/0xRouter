//! `[signin]`: how an account signs in to a subscription (contracts/signin-quota-schema.md
//! § `[signin]`). Public client data only: no field holds a secret, and the core does all
//! sending. Tokens never appear here.

use std::fmt;
use std::time::Duration;

use indexmap::IndexMap;
use serde::de::{Deserializer, Error as _};
use serde::{Deserialize, Serialize, Serializer};

use super::duration::de_duration;
use super::endpoint::EndpointAuth;
use super::enums::{AuthScheme, closed_enum};
use super::quota::ValuePath;

closed_enum!(
    /// The OAuth flow.
    SignInFlow, "sign-in flow" {
        Pkce = "pkce",
        DeviceCode = "device_code",
    }
);

closed_enum!(
    /// Where the provider sends the browser after approval. `code_page` also allows
    /// pasting a bare code or `code#state`.
    RedirectKind, "redirect kind" {
        Loopback = "loopback",
        CodePage = "code_page",
    }
);

closed_enum!(
    /// How token requests are encoded.
    #[derive(Default)]
    TokenBody, "token body" {
        #[default]
        Form = "form",
        Json = "json",
    }
);

closed_enum!(
    /// The extra authorize/device parameters a plugin may set.
    SignInParam, "sign-in parameter" {
        Plan = "plan",
        Referrer = "referrer",
        Code = "code",
        Audience = "audience",
        Prompt = "prompt",
        Nonce = "nonce",
    }
);

/// A sign-in parameter value: fixed, or `{random.hex16}` (16 random bytes, hex) per sign-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignInParamValue {
    Fixed(String),
    RandomHex16,
}

impl<'de> Deserialize<'de> for SignInParamValue {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        match s.as_str() {
            "{random.hex16}" => Ok(Self::RandomHex16),
            v if !v.contains(['{', '}']) => Ok(Self::Fixed(s)),
            v => Err(D::Error::custom(format!(
                "unknown placeholder {v}; a parameter is a fixed string or {{random.hex16}}"
            ))),
        }
    }
}

/// One redirect, tried in declaration order.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Redirect {
    pub uri: String,
    pub kind: RedirectKind,
}

/// The optional post-sign-in profile read.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignInProfile {
    pub url: String,
    /// Static, non-secret headers.
    #[serde(default)]
    pub headers: IndexMap<String, String>,
    /// `id_token.<claim>` reads the id token's payload.
    pub email: Option<ValuePath>,
    pub user_id: Option<ValuePath>,
    pub tier: Option<ValuePath>,
}

/// An upstream error that marks the account `refused` rather than failing one request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefusedRule {
    #[serde(deserialize_with = "de_statuses")]
    pub status: Vec<u16>,
    pub body_contains: Option<String>,
}

impl RefusedRule {
    pub fn matches(&self, status: u16, body: &str) -> bool {
        self.status.contains(&status) && self.body_contains.as_deref().is_none_or(|t| body.contains(t))
    }
}

fn de_statuses<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u16>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(u16),
        Many(Vec<u16>),
    }
    match OneOrMany::deserialize(d) {
        Ok(OneOrMany::One(s)) => Ok(vec![s]),
        Ok(OneOrMany::Many(v)) => Ok(v),
        Err(_) => Err(D::Error::custom("a status or a list of statuses")),
    }
}

/// `[signin]`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignInDecl {
    pub flow: SignInFlow,
    /// The provider's public client id.
    pub client_id: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    /// OpenID discovery; its results must stay on the declared hosts.
    pub discovery_url: Option<String>,
    /// `pkce` only; the fallback when discovery fails.
    pub authorize_url: Option<String>,
    pub token_url: String,
    /// `device_code` only.
    pub device_url: Option<String>,
    /// `pkce` only, tried in order.
    #[serde(default)]
    pub redirect: Vec<Redirect>,
    #[serde(default)]
    pub params: IndexMap<SignInParam, SignInParamValue>,
    #[serde(default)]
    pub body: TokenBody,
    /// `pkce` only, 32–96; see [`SignInDecl::verifier_len`].
    pub verifier_bytes: Option<u32>,
    /// How long before expiry the core refreshes.
    #[serde(deserialize_with = "de_duration")]
    pub refresh_lead: Duration,
    /// Where the access token goes on requests; `Authorization: Bearer` by default.
    #[serde(default = "bearer")]
    pub auth: EndpointAuth,
    /// Print the terms warning (research R4) before every sign-in.
    #[serde(default)]
    pub terms_warning: bool,
    pub profile: Option<SignInProfile>,
    #[serde(default)]
    pub refused: Vec<RefusedRule>,
}

fn bearer() -> EndpointAuth {
    EndpointAuth { header: "Authorization".into(), scheme: AuthScheme::Bearer }
}

/// The PKCE verifier length when `verifier_bytes` is absent.
pub const DEFAULT_VERIFIER_BYTES: u32 = 32;

impl SignInDecl {
    /// PKCE verifier length in random bytes.
    pub fn verifier_len(&self) -> u32 {
        self.verifier_bytes.unwrap_or(DEFAULT_VERIFIER_BYTES)
    }

    /// The sign-in flow's own URLs, by field name: discovery, authorize, token and device.
    pub fn flow_urls(&self) -> impl Iterator<Item = (&'static str, &str)> {
        [
            ("discovery_url", self.discovery_url.as_deref()),
            ("authorize_url", self.authorize_url.as_deref()),
            ("token_url", Some(self.token_url.as_str())),
            ("device_url", self.device_url.as_deref()),
        ]
        .into_iter()
        .filter_map(|(k, v)| Some((k, v?)))
    }

    /// Whether an upstream error marks the account `refused`.
    pub fn refuses(&self, status: u16, body: &str) -> bool {
        self.refused.iter().any(|r| r.matches(status, body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Doc {
        signin: SignInDecl,
    }

    const XAI: &str = r#"
[signin]
flow = "pkce"
client_id = "b1a00492-073a-47ea-816f-4c329264a828"
scopes = ["openid", "offline_access"]
discovery_url = "https://auth.x.ai/.well-known/openid-configuration"
authorize_url = "https://auth.x.ai/oauth2/authorize"
token_url = "https://auth.x.ai/oauth2/token"
redirect = [{ uri = "http://127.0.0.1:56121/callback", kind = "loopback" }]
params = { plan = "generic", referrer = "cli-proxy-api", nonce = "{random.hex16}" }
verifier_bytes = 96
refresh_lead = "5m"

[signin.profile]
url = "https://cli-chat-proxy.grok.com/v1/user"
email = "email | id_token.email"

[[signin.refused]]
status = [400, 403]
body_contains = "only authorized for use with Claude Code"
"#;

    #[test]
    fn parses_the_contract_example() {
        let s = toml::from_str::<Doc>(XAI).unwrap().signin;
        assert_eq!(s.flow, SignInFlow::Pkce);
        assert_eq!(s.params[&SignInParam::Nonce], SignInParamValue::RandomHex16);
        assert_eq!(s.params[&SignInParam::Plan], SignInParamValue::Fixed("generic".into()));
        assert_eq!(s.refresh_lead, Duration::from_secs(300));
        assert_eq!(s.auth, bearer());
        assert_eq!(s.body, TokenBody::Form);
        assert_eq!(s.verifier_len(), 96);
        assert_eq!(s.flow_urls().count(), 3);
        assert!(s.refuses(403, "x only authorized for use with Claude Code y"));
        assert!(!s.refuses(401, "only authorized for use with Claude Code"));
    }

    #[test]
    fn closed_sets() {
        let err = |src: String| toml::from_str::<Doc>(&src).err().unwrap().to_string();
        assert!(err(XAI.replace("plan =", "scope =")).contains("allowed: plan, referrer"));
        assert!(err(XAI.replace("{random.hex16}", "{random.hex32}")).contains("unknown placeholder {random.hex32}"));
        assert!(err(XAI.replace("\"pkce\"", "\"implicit\"")).contains("allowed: pkce, device_code"));
        assert!(err(XAI.replace("\"5m\"", "\"5\"")).contains("not a duration"));
        assert!(err(XAI.replace("terms", "x").replace("verifier_bytes", "client_secret")).contains("client_secret"));
    }
}

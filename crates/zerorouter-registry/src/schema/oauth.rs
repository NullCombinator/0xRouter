//! Public OAuth endpoint data (data-model § OAuthDecl). Never a client secret.

use std::collections::BTreeSet;

use indexmap::IndexMap;
use serde::Deserialize;
use url::Url;

use super::transport::Transport;

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OAuthDecl {
    pub client_id: Option<String>,
    pub authorize_url: Option<String>,
    pub token_url: Option<String>,
    pub refresh_url: Option<String>,
    pub device_code_url: Option<String>,
    pub user_info_url: Option<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
    pub code_challenge_method: Option<String>,
    pub refresh_lead_ms: Option<u64>,
    /// Long-tail provider URLs. Never receive the client secret.
    #[serde(default)]
    pub endpoints: IndexMap<String, String>,
    /// Long-tail values; keys must be in [`KNOWN_OAUTH_PARAMS`](super::KNOWN_OAUTH_PARAMS).
    #[serde(default)]
    pub params: IndexMap<String, ParamValue>,
}

impl OAuthDecl {
    pub(crate) fn url_fields(&self) -> impl Iterator<Item = (String, &str)> {
        [
            ("authorize_url", &self.authorize_url),
            ("token_url", &self.token_url),
            ("refresh_url", &self.refresh_url),
            ("device_code_url", &self.device_code_url),
            ("user_info_url", &self.user_info_url),
        ]
        .into_iter()
        .filter_map(|(k, v)| v.as_deref().map(|v| (k.to_owned(), v)))
        .chain(self.endpoints.iter().map(|(k, v)| (format!("endpoints.{k}"), v.as_str())))
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    List(Vec<String>),
    Map(IndexMap<String, String>),
}

/// The OAuth host set (FR-012a): hosts of every URL that may receive the client secret.
/// `user_info_url`, `device_code_url`, and `oauth.endpoints` are excluded.
pub fn host_set(oauth: Option<&OAuthDecl>, transports: &[&Transport]) -> BTreeSet<String> {
    oauth_urls(oauth, transports)
        .filter_map(|u| Url::parse(u).ok()?.host_str().map(str::to_owned))
        .collect()
}

/// The URLs behind [`host_set`], in declaration order.
pub(crate) fn oauth_urls<'a>(
    oauth: Option<&'a OAuthDecl>,
    transports: &'a [&'a Transport],
) -> impl Iterator<Item = &'a str> {
    let declared = oauth
        .into_iter()
        .flat_map(|o| [&o.authorize_url, &o.token_url, &o.refresh_url]);
    let on_transports = transports
        .iter()
        .flat_map(|t| [&t.token_url, &t.refresh_url, &t.auth_url]);
    declared.chain(on_transports).filter_map(|u| u.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_set_excludes_user_info_and_endpoints() {
        let oauth = OAuthDecl {
            token_url: Some("https://auth.example/token".into()),
            user_info_url: Some("https://info.example/me".into()),
            endpoints: [("x".to_owned(), "https://other.example".to_owned())].into_iter().collect(),
            ..OAuthDecl::default()
        };
        let t = Transport { auth_url: Some("https://login.example/".into()), ..Transport::default() };
        let hosts: Vec<_> = host_set(Some(&oauth), &[&t]).into_iter().collect();
        assert_eq!(hosts, ["auth.example", "login.example"]);
    }
}

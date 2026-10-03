//! Sign-in accounts whose tokens come from the mock identity provider, over a mock upstream
//! that accepts only tokens the identity provider still holds valid: the refresh tests
//! (spec 005 US2).

// Each test binary uses only part of this module.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use nullrouter_engine::state::Engine;
use nullrouter_engine::testkit::mock_idp::{AUTHORIZE, CLIENT_ID, DEVICE, TOKEN};
use nullrouter_engine::testkit::{MockIdp, MockUpstream, Received, Step};
use nullrouter_engine::tokens::{self, Claims, TokenEntry, TokenStore};
use nullrouter_registry::{OperatorHome, SecretString};
use serde_json::json;

use crate::common::{Setup, chat_chunks, ok};

/// How a test plugin signs in and encodes its refresh body.
#[derive(Debug, Clone, Copy)]
pub enum Shape {
    /// grok-cli's: device code, form body.
    Device,
    /// xai's: PKCE with a loopback redirect, form body.
    PkceForm,
    /// anthropic's: PKCE with a code page, JSON body.
    PkceJson,
}

/// Provider `id` on the openai-chat wire at `/<id>/chat/completions` (model `m1`), signing
/// in at `idp` with `shape` and `refresh_lead = lead`.
pub fn plugin(mock: &MockUpstream, idp: &MockIdp, id: &str, shape: Shape, lead: &str) -> String {
    let flow = match shape {
        Shape::Device => format!("flow = \"device_code\"\ndevice_url = \"{}\"\n", idp.url(DEVICE)),
        Shape::PkceForm => format!(
            "flow = \"pkce\"\nauthorize_url = \"{}\"\nredirect = [{{ uri = \"http://127.0.0.1:56121/callback\", kind = \"loopback\" }}]\n",
            idp.url(AUTHORIZE)
        ),
        Shape::PkceJson => format!(
            "flow = \"pkce\"\nauthorize_url = \"{}\"\nbody = \"json\"\nredirect = [{{ uri = \"https://console.anthropic.com/oauth/code/callback\", kind = \"code_page\" }}]\n",
            idp.url(AUTHORIZE)
        ),
    };
    format!(
        "schema = 2\nid = \"{id}\"\ncategory = \"apikey\"\n[endpoints.text]\nurl = \"{url}\"\nwire = \"openai-chat\"\nretry = {{ 401 = {{ retries = 0 }}, 503 = {{ retries = 0 }} }}\n[signin]\n{flow}client_id = \"{CLIENT_ID}\"\ntoken_url = \"{token}\"\nrefresh_lead = \"{lead}\"\n[[models]]\nid = \"m1\"\n",
        url = mock.url(&format!("/{id}/chat/completions")),
        token = idp.url(TOKEN),
    )
}

pub struct Kit {
    pub s: Setup,
    pub idp: Arc<MockIdp>,
}

impl Kit {
    pub fn engine(&self) -> &Arc<Engine> {
        &self.s.engine
    }

    pub fn home(&self) -> &std::path::Path {
        self.s._dir.path()
    }

    /// `tokens.toml` as it is on disk.
    pub fn stored(&self, provider: &str, name: &str) -> TokenEntry {
        let store = TokenStore::load(self.home()).unwrap();
        tokens::dup_entry(store.get(provider, name).unwrap())
    }

    /// The access token in `provider/name`'s cell.
    pub fn access(&self, provider: &str, name: &str) -> String {
        self.engine().tokens.get(provider, name).unwrap().entry.access_token.with_exposed(str::to_owned)
    }

    /// Writes `provider/name`'s tokens: a fresh grant from the identity provider, with
    /// `expires_at` and `signed_in_at` adjusted by `edit`, then reloads.
    pub fn sign_in(&self, provider: &str, name: &str, edit: impl FnOnce(&mut TokenEntry)) {
        let (access, refresh, expires_in) = self.idp.grant();
        let hosts = self.engine().snapshot().registry.provider(provider).unwrap().token_hosts();
        let now = SystemTime::now();
        let mut e = TokenEntry {
            provider: provider.into(),
            name: name.into(),
            access_token: SecretString::new(access),
            refresh_token: Some(SecretString::new(refresh)),
            expires_at: now + expires_in,
            scope: "openid offline_access".into(),
            claims: Claims { email: Some(format!("{name}@example.com")), user_id: None, tier: None },
            hosts,
            signed_in_at: now,
            last_refresh_at: None,
            state: None,
            state_since: None,
            state_reason: None,
        };
        edit(&mut e);
        tokens::update(self.home(), provider, name, |slot| *slot = Some(e)).unwrap();
        self.engine().reload_blocking().unwrap();
    }
}

/// The bearer token a request carried.
pub fn bearer(r: &Received) -> String {
    r.headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").trim_start_matches("Bearer ").to_owned()
}

/// A 401 as a provider rejects an expired token.
pub fn expired() -> Step {
    Step::json(401, json!({"error": {"message": "token expired", "type": "authentication_error"}}))
}

/// `providers` (`(id, shape, lead)`), each with sign-in accounts `names` in order, over a
/// mock upstream that answers a live token with "hi" (a stream when asked) and anything
/// else with [`expired`]. Accounts start without tokens: call [`Kit::sign_in`].
pub async fn kit(providers: &[(&'static str, Shape, &str)], names: &[&str], lifetime: Duration) -> Kit {
    let mock = MockUpstream::start().await;
    let idp = Arc::new(MockIdp::start().await);
    idp.set_token_lifetime(lifetime);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), "allow_private_endpoints = true\n").unwrap();
    std::fs::create_dir(dir.path().join("plugins")).unwrap();
    let mut accounts = String::from("schema = 2\n");
    for (id, shape, lead) in providers {
        std::fs::write(dir.path().join(format!("plugins/{id}.toml")), plugin(&mock, &idp, id, *shape, lead)).unwrap();
        for (order, name) in names.iter().enumerate() {
            accounts +=
                &format!("[[account]]\nprovider = \"{id}\"\nname = \"{name}\"\nkind = \"signin\"\norder = {order}\n");
        }
    }
    nullrouter_engine::files::write_private(&dir.path().join(nullrouter_engine::accounts::FILE), &accounts).unwrap();
    let (engine, report) = Engine::open_parity(OperatorHome::new(dir.path())).unwrap();
    assert!(report.registry.unsupported.is_empty() && report.registry.skipped.is_empty(), "{report:#?}");
    let live = idp.clone();
    mock.respond(move |r| {
        if !live.token_valid(&bearer(r)) {
            return expired();
        }
        if r.json()["stream"] == true { chat_chunks() } else { ok() }
    });
    Kit { s: Setup { _dir: dir, engine: Arc::new(engine), mock }, idp }
}

/// Polls `f` every 20 ms for up to `limit`.
pub async fn eventually(limit: Duration, mut f: impl FnMut() -> bool) -> bool {
    let end = tokio::time::Instant::now() + limit;
    while tokio::time::Instant::now() < end {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    f()
}

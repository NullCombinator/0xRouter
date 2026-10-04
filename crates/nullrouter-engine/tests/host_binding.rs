//! Secrets stay bound to the hosts their provider used when the account was first seen
//! (T142): a hand-written account's binding is saved at load, and a replacing plugin that
//! sends token counts to another host gets no secret.

mod common;

use common::{SECRET, messages_plugin, request, setup};
use nullrouter_engine::accounts::FILE;
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn a_new_token_count_host_gets_no_secret() {
    let s = setup(|m| vec![("mockco", messages_plugin(m, "mockco"))], &[("mockco", "main")], "").await;
    let home = s._dir.path();
    let saved = std::fs::read_to_string(home.join(FILE)).unwrap();
    assert!(saved.contains("127.0.0.1"), "the first binding is saved:\n{saved}");

    // The same endpoint, plus a count URL on a name the account was never bound to.
    let count = s.mock.url("/count").replace("127.0.0.1", "localhost");
    let plugin = format!("{}[endpoints.text.token_count]\nurl = \"{count}\"\n", messages_plugin(&s.mock, "mockco"));
    std::fs::write(home.join("plugins/mockco.toml"), plugin).unwrap();
    s.engine.reload().await.unwrap();

    let body = json!({"model": "mockco/m1", "max_tokens": 8, "messages": [{"role": "user", "content": "hi"}]});
    let mut req = request(&s, "anthropic-messages", "mockco/m1", body, "ak_test", CancellationToken::new());
    req.count = true;
    let Err(f) = s.engine.text(s.engine.snapshot(), req).await else { panic!("the secret was released") };
    assert!(f.tried.iter().any(|t| t.reason.contains("localhost")), "{:?}", f.tried);
    assert!(s.mock.received().is_empty(), "nothing reaches either host");
    assert!(!f.message.contains(SECRET));
}

mod signin {
    use std::time::{Duration, SystemTime};

    use super::common::{SECRET, chat_plugin, ok, send, setup, trail};
    use nullrouter_engine::accounts::FILE;
    use nullrouter_engine::records::{AttemptKind, AttemptOutcome, ErrorClass};
    use nullrouter_engine::tokens::{self, AccountState, Claims, PersistedState, TokenEntry};
    use nullrouter_registry::SecretString;

    fn entry(name: &str, hosts: &[&str], expires_in: i64) -> TokenEntry {
        let now = SystemTime::now();
        let expires_at = if expires_in >= 0 {
            now + Duration::from_secs(expires_in.unsigned_abs())
        } else {
            now - Duration::from_secs(expires_in.unsigned_abs())
        };
        TokenEntry {
            provider: "mockco".into(),
            name: name.into(),
            access_token: SecretString::new(format!("tok-SENTINEL-{name}")),
            refresh_token: None,
            expires_at,
            scope: String::new(),
            claims: Claims::default(),
            hosts: hosts.iter().map(|h| (*h).to_owned()).collect(),
            signed_in_at: now,
            last_refresh_at: None,
            state: None,
            state_since: None,
            state_reason: None,
        }
    }

    fn put(home: &std::path::Path, e: TokenEntry) {
        let name = e.name.clone();
        tokens::update(home, "mockco", &name, |s| *s = Some(e)).unwrap();
    }

    /// `signin`: sign-in account names in order; then a key account `key`.
    fn accounts(home: &std::path::Path, signin: &[&str], disabled: &[&str]) {
        let mut file = String::from("schema = 2\n");
        for (i, n) in signin.iter().chain(disabled).enumerate() {
            let off = disabled.contains(n);
            file += &format!(
                "[[account]]\nprovider = \"mockco\"\nname = \"{n}\"\nkind = \"signin\"\norder = {i}\ndisabled = {off}\n"
            );
        }
        file += &format!(
            "[[account]]\nprovider = \"mockco\"\nname = \"key\"\nsecret = \"{SECRET}-mockco-key\"\norder = 99\n"
        );
        nullrouter_engine::files::write_private(&home.join(FILE), &file).unwrap();
    }

    fn auth_of(s: &super::common::Setup) -> Vec<String> {
        s.mock
            .received()
            .iter()
            .map(|r| r.headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_owned())
            .collect()
    }

    #[tokio::test]
    async fn the_token_goes_only_to_hosts_its_sign_in_covered() {
        let s = setup(|m| vec![("mockco", chat_plugin(m, "mockco", ""))], &[("mockco", "key")], "").await;
        let home = s._dir.path();
        accounts(home, &["sso"], &[]);
        put(home, entry("sso", &["127.0.0.1"], 3600));
        s.engine.reload().await.unwrap();

        s.mock.push([ok()]);
        let (_, res) = send(&s, "mockco/m1").await;
        assert!(res.is_ok());
        assert_eq!(auth_of(&s), ["Bearer tok-SENTINEL-sso"]);

        // Tokens bound elsewhere: withheld, and the key account serves. The two accounts share cold
        // work by deficit (spec 006), so two requests make sure one of them reaches `sso` first.
        put(home, entry("sso", &["api.mockco.example"], 3600));
        s.engine.reload().await.unwrap();
        let mut skipped = false;
        for _ in 0..2 {
            s.mock.push([ok()]);
            let (id, res) = send(&s, "mockco/m1").await;
            assert!(res.is_ok());
            let r = s.engine.records.get(&id).unwrap();
            if let Some(AttemptOutcome::Skipped { reason, .. }) = r.attempts.first().map(|a| &a.outcome).and_then(|o| o.as_ref()) {
                assert!(reason.contains("127.0.0.1") && reason.contains("accounts signin mockco sso"), "{reason}");
                skipped = true;
            }
            assert!(!format!("{r:?}").contains("tok-SENTINEL"));
        }
        assert!(skipped, "a request reached the withheld account first");
        let auth = auth_of(&s);
        let key = format!("Bearer {SECRET}-mockco-key");
        assert_eq!(auth[1..], [key.clone(), key]);
    }

    #[tokio::test]
    async fn out_of_service_accounts_are_recorded_skips() {
        let s = setup(|m| vec![("mockco", chat_plugin(m, "mockco", ""))], &[("mockco", "key")], "").await;
        let home = s._dir.path();
        accounts(home, &["none", "lost", "banned", "stale", "early"], &["off"]);
        let mut lost = entry("lost", &["127.0.0.1"], 3600);
        lost.state = Some(PersistedState::NeedsSignIn);
        lost.state_reason = Some("invalid_grant".into());
        put(home, lost);
        let mut banned = entry("banned", &["127.0.0.1"], 3600);
        banned.state = Some(PersistedState::Refused);
        banned.state_reason = Some("only authorized for use with Claude Code".into());
        put(home, banned);
        put(home, entry("stale", &["127.0.0.1"], -60));
        put(home, entry("early", &["127.0.0.1"], 3600));
        put(home, entry("off", &["127.0.0.1"], 3600));
        s.engine.reload().await.unwrap();
        let refreshing = AccountState::Refreshing { since: SystemTime::now(), attempts: 2 };
        assert!(s.engine.tokens.set_state("mockco", "stale", refreshing.clone()));
        // Still within its lifetime: a failed refresh doesn't stop it serving (R10).
        assert!(s.engine.tokens.set_state("mockco", "early", refreshing));

        s.mock.push([ok()]);
        let (id, res) = send(&s, "mockco/m1").await;
        assert!(res.is_ok());
        let r = s.engine.records.get(&id).unwrap();
        let skips: Vec<(String, Option<ErrorClass>)> = r
            .attempts
            .iter()
            .filter_map(|a| match &a.outcome {
                Some(AttemptOutcome::Skipped { reason, class }) => Some((reason.clone(), *class)),
                _ => None,
            })
            .collect();
        assert_eq!(
            skips,
            [
                ("needs sign-in: run nullrouter accounts signin mockco none".to_owned(), Some(ErrorClass::NeedsSignIn)),
                ("needs sign-in: run nullrouter accounts signin mockco lost".to_owned(), Some(ErrorClass::NeedsSignIn)),
                (
                    "refused by provider: only authorized for use with Claude Code; run nullrouter accounts signin mockco banned"
                        .to_owned(),
                    Some(ErrorClass::Refused)
                ),
                ("token expired, refresh retrying".to_owned(), Some(ErrorClass::TokenRefreshing)),
            ]
        );
        let names: Vec<_> = trail(&r).into_iter().map(|(_, a, k)| (a.unwrap_or_default(), k)).collect();
        assert!(!names.iter().any(|(a, _)| a == "off"), "disabled accounts stay silent: {names:?}");
        assert_eq!(names.last().unwrap(), &("early".to_owned(), AttemptKind::Initial), "{names:?}");
        assert_eq!(auth_of(&s), ["Bearer tok-SENTINEL-early"]);
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"class\":\"needs_sign_in\"") && json.contains("\"token_refreshing\""), "{json}");
    }
}

/// US6 (T088, FR-030, FR-031): a sign-in token goes only to the hosts its sign-in bound
/// it to. A redirect is never followed, a declared sign-in host the token wasn't bound to
/// withholds it, and a video download on another host is fetched bare.
///
/// Discovery naming another host is refused before any token exists: the flow falls back
/// to the declared URLs (`signin_flows.rs`,
/// `discovery_off_the_declared_hosts_falls_back_to_the_declared_urls`); the test below
/// checks the release side, that the issued token is bound to `token_hosts()` only.
mod us6 {
    use std::time::Duration;

    use super::common::{SECRET, chat_plugin, setup_signin};
    use nullrouter_engine::jobs::Job;
    use nullrouter_engine::signin::{self, Begun, SignInHttp};
    use nullrouter_engine::testkit::mock_idp::{AUTHORIZE, CLIENT_ID, DISCOVERY, TOKEN};
    use nullrouter_engine::testkit::{MockIdp, MockUpstream, Step};
    use nullrouter_registry::PluginSource;
    use nullrouter_registry::schema::TokenBody;
    use nullrouter_registry::validate::{GateCtx, validate_with};
    use serde_json::json;

    /// The token of `sso/main` in [`setup_signin`].
    const TOKEN_SSO: &str = "sk-mock-SENTINEL-0003-sso-main";

    /// A device-code `[signin]` whose flow URLs sit on `host` (the mock's port).
    fn signin_on(mock: &MockUpstream, host: &str) -> String {
        let at = |p: &str| mock.url(p).replace("127.0.0.1", host);
        format!(
            "[signin]\nflow = \"device_code\"\nclient_id = \"test-client\"\ndevice_url = \"{}\"\ntoken_url = \"{}\"\nrefresh_lead = \"5m\"\n[identity.headers]\nx-client = \"test\"\nx-email = \"{{account.email}}\"\n",
            at("/idp/device"),
            at("/idp/token"),
        )
    }

    fn auth_headers(mock: &MockUpstream) -> Vec<Option<String>> {
        mock.received().iter().map(|r| r.headers.get("authorization").map(|v| v.to_str().unwrap().to_owned())).collect()
    }

    #[tokio::test]
    async fn an_upstream_redirect_is_never_followed() {
        let s = setup_signin(
            |m| vec![("sso", format!("{}{}", chat_plugin(m, "sso", ""), signin_on(m, "127.0.0.1")))],
            &[("sso", "main")],
            &[("sso", "main")],
            "",
        )
        .await;
        let elsewhere = s.mock.url("/steal").replace("127.0.0.1", "localhost");
        s.mock.push([Step::json(302, json!({})).with_header("location", &elsewhere)]);
        let (_, res) = super::common::send(&s, "sso/m1").await;
        let Err(f) = res else { panic!("a redirect is not an answer") };
        let got = s.mock.received();
        assert_eq!(got.len(), 1, "{:?}", got.iter().map(|r| &r.path_and_query).collect::<Vec<_>>());
        assert_eq!(got[0].path_and_query, "/sso/chat/completions");
        assert_eq!(auth_headers(&s.mock), [Some(format!("Bearer {TOKEN_SSO}"))]);
        assert!(!f.message.contains(SECRET) && !format!("{:?}", f.tried).contains(SECRET));
    }

    #[tokio::test]
    async fn a_sign_in_token_endpoint_redirect_is_never_followed() {
        let mock = MockUpstream::start().await;
        let elsewhere = mock.url("/steal").replace("127.0.0.1", "localhost");
        mock.push([Step::json(307, json!({})).with_header("location", &elsewhere)]);
        let http = SignInHttp::new(true).with_timeout(Duration::from_secs(5));
        let body =
            signin::encode_body(TokenBody::Form, &[("grant_type", "refresh_token"), ("refresh_token", "rt-SENTINEL")]);
        let e = signin::token_request(&http, &mock.url("/idp/token"), Default::default(), body).await.unwrap_err();
        assert!(!e.to_string().contains("rt-SENTINEL"), "{e}");
        let got = mock.received();
        assert_eq!(got.iter().map(|r| r.path_and_query.as_str()).collect::<Vec<_>>(), ["/idp/token"]);
    }

    /// The token's hosts are every host the plugin sends it to: a sign-in URL on a host the
    /// stored token wasn't bound to withholds it, though the endpoint's host is bound.
    #[tokio::test]
    async fn a_sign_in_host_outside_the_binding_withholds_the_token() {
        let s = setup_signin(
            |m| vec![("sso", format!("{}{}", chat_plugin(m, "sso", ""), signin_on(m, "localhost")))],
            &[("sso", "main")],
            &[("sso", "main")],
            "",
        )
        .await;
        let (_, res) = super::common::send(&s, "sso/m1").await;
        let Err(f) = res else { panic!("the token was released") };
        assert!(f.tried.iter().any(|t| t.reason.contains("localhost")), "{:?}", f.tried);
        assert!(s.mock.received().is_empty(), "nothing is sent");
        assert!(!f.message.contains(SECRET) && !format!("{:?}", f.tried).contains(SECRET));
    }

    /// A discovery document naming another host never widens the binding: the token is
    /// bound to the plugin's `token_hosts()`, and the foreign host gets nothing.
    #[tokio::test]
    async fn discovery_naming_another_host_binds_nothing_there() {
        let idp = MockIdp::start().await;
        let foreign = |p: &str| idp.url(p).replace("127.0.0.1", "localhost");
        let src = format!(
            "schema = 2\nid = \"xai\"\ncategory = \"oauth\"\n[endpoints.text]\nurl = \"{}\"\nwire = \"openai-chat\"\n[[models]]\nid = \"m1\"\n[signin]\nflow = \"pkce\"\nclient_id = \"{CLIENT_ID}\"\ndiscovery_url = \"{}\"\nauthorize_url = \"{}\"\ntoken_url = \"{}\"\nredirect = [{{ uri = \"http://127.0.0.1:0/callback\", kind = \"loopback\" }}]\nrefresh_lead = \"5m\"\n",
            idp.url("/v1/chat/completions"),
            idp.url(DISCOVERY),
            idp.url(AUTHORIZE),
            idp.url(TOKEN),
        );
        let ctx = GateCtx { allow_private: true, ..GateCtx::default() };
        let p = validate_with(&src, PluginSource::Bundled, "xai.toml", &ctx).unwrap().entity;
        idp.set_discovery(Some(
            json!({ "authorization_endpoint": foreign(AUTHORIZE), "token_endpoint": foreign(TOKEN) }),
        ));
        let http = SignInHttp::new(true).with_timeout(Duration::from_secs(5));
        let Begun::Pkce(s) = signin::begin(&http, &p).await.unwrap() else { panic!("pkce") };
        assert!(s.authorize_url().starts_with(&idp.url(AUTHORIZE)), "{}", s.authorize_url());
        let location = idp.browse(s.authorize_url()).await;
        let e = s.complete(&http, "main", s.parse_paste(&location).unwrap()).await.unwrap();
        assert_eq!(e.hosts, p.token_hosts());
        assert_eq!(e.hosts.iter().map(String::as_str).collect::<Vec<_>>(), ["127.0.0.1"]);
        assert_eq!(idp.token_requests().pop().unwrap().path_and_query, TOKEN);
    }

    /// A video job's download URL on a host the token isn't bound to is fetched bare: no
    /// token, no identity headers. Polls on the bound host carry both.
    #[tokio::test]
    async fn a_video_download_on_a_foreign_host_gets_no_token() {
        let plugin = |m: &MockUpstream| {
            format!(
                "schema = 2\nid = \"vidso\"\ncategory = \"apikey\"\n[endpoints.video]\nurl = \"{}\"\nwire = \"openai-chat\"\npoll_url = \"{}\"\njob = {{ id = \"request_id\", status = \"status\", status_map = {{ done = \"completed\" }}, content_url = \"video.url\" }}\n[[models]]\nid = \"vid\"\nkind = \"video\"\n{}",
                m.url("/v1/videos/generations"),
                m.url("/v1/videos/{id}"),
                signin_on(m, "127.0.0.1"),
            )
        };
        let s = setup_signin(|m| vec![("vidso", plugin(m))], &[("vidso", "main")], &[("vidso", "main")], "").await;
        let foreign = s.mock.url("/files/out.mp4").replace("127.0.0.1", "localhost");
        s.mock.on("/files/out.mp4", [Step::binary("video/mp4", &b"MP4"[..])]);
        s.mock.on("/v1/videos/req-1", [Step::json(200, json!({"status": "done", "video": {"url": foreign}}))]);
        let id = s.engine.jobs.insert(Job {
            record: "rq_none".into(),
            provider: "vidso".into(),
            account: Some("main".into()),
            url: s.mock.url("/v1/videos/generations"),
            wire: "openai-chat".into(),
            model: "vid".into(),
            upstream_id: "req-1".into(),
            target: "vidso/vid".into(),
            agent: "ak_test".into(),
            content_url: None,
        });
        let (ct, mut rx) = s.engine.job_content(&s.engine.snapshot(), &id, "ak_test", "openai-chat").await.unwrap();
        assert_eq!(ct, "video/mp4");
        let mut bytes = Vec::new();
        while let Some(chunk) = rx.recv().await {
            bytes.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(bytes, b"MP4");

        let got = s.mock.received();
        assert_eq!(
            got.iter().map(|r| r.path_and_query.as_str()).collect::<Vec<_>>(),
            ["/v1/videos/req-1", "/files/out.mp4"]
        );
        let (poll, download) = (&got[0], &got[1]);
        assert_eq!(poll.headers["authorization"], format!("Bearer {SECRET}-vidso-main"));
        assert_eq!(poll.headers["x-email"], "main@example.com");
        assert!(download.headers["host"].to_str().unwrap().starts_with("localhost"));
        for h in ["authorization", "x-email", "x-client"] {
            assert!(download.headers.get(h).is_none(), "{h}: {:?}", download.headers);
        }
        assert!(!format!("{:?}", download.headers).contains(SECRET));
    }
}

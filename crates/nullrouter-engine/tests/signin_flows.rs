//! Sign-in flows against the mock identity provider (spec 005 T024; research R3, R4).

use std::time::Duration;

use nullrouter_engine::signin::pkce::PasteError;
use nullrouter_engine::signin::{Begun, DevicePoll as Polled, DeviceSignIn, PkceSignIn, SignInError, SignInHttp};
use nullrouter_engine::testkit::mock_idp::{AUTHORIZE, CLIENT_ID, DEVICE, DEVICE_GRANT, DISCOVERY, PROFILE, TOKEN};
use nullrouter_engine::testkit::{DevicePoll, Failure, MockIdp};
use nullrouter_registry::PluginSource;
use nullrouter_registry::ProviderEntity;
use nullrouter_registry::validate::{GateCtx, validate_with};
use serde_json::json;
use tokio_util::sync::CancellationToken;

fn provider(idp: &MockIdp, id: &str, signin: &str) -> ProviderEntity {
    let src = format!(
        "schema = 2\nid = \"{id}\"\ncategory = \"oauth\"\n[endpoints.text]\nurl = \"{}\"\nwire = \"openai-chat\"\n[[models]]\nid = \"m1\"\n{signin}",
        idp.url("/v1/chat/completions")
    );
    let ctx = GateCtx { allow_private: true, ..GateCtx::default() };
    validate_with(&src, PluginSource::Bundled, &format!("{id}.toml"), &ctx).unwrap_or_else(|e| panic!("{e:#?}")).entity
}

fn grok_cli(idp: &MockIdp) -> ProviderEntity {
    provider(
        idp,
        "grok-cli",
        &format!(
            "[signin]\nflow = \"device_code\"\nclient_id = \"{CLIENT_ID}\"\nscopes = [\"openid\", \"offline_access\"]\ndevice_url = \"{}\"\ntoken_url = \"{}\"\nparams = {{ referrer = \"grok-build\" }}\nrefresh_lead = \"5m\"\n",
            idp.url(DEVICE),
            idp.url(TOKEN)
        ),
    )
}

/// xai with a loopback redirect on `port` (0: ephemeral) and the discovery document.
fn xai(idp: &MockIdp, port: u16) -> ProviderEntity {
    provider(
        idp,
        "xai",
        &format!(
            "[signin]\nflow = \"pkce\"\nclient_id = \"{CLIENT_ID}\"\nscopes = [\"openid\", \"profile\", \"email\", \"offline_access\"]\ndiscovery_url = \"{}\"\nauthorize_url = \"{}\"\ntoken_url = \"{}\"\nredirect = [{{ uri = \"http://127.0.0.1:{port}/callback\", kind = \"loopback\" }}]\nparams = {{ plan = \"generic\", referrer = \"cli-proxy-api\", nonce = \"{{random.hex16}}\" }}\nverifier_bytes = 96\nrefresh_lead = \"5m\"\n[signin.profile]\nurl = \"{}\"\nemail = \"email | id_token.email\"\nuser_id = \"userId\"\ntier = \"subscriptionTier\"\n",
            idp.url(DISCOVERY),
            idp.url(AUTHORIZE),
            idp.url(TOKEN),
            idp.url(PROFILE)
        ),
    )
}

/// anthropic: hosted code page first, JSON bodies, `code=true`.
fn anthropic(idp: &MockIdp) -> ProviderEntity {
    provider(
        idp,
        "anthropic",
        &format!(
            "[signin]\nflow = \"pkce\"\nclient_id = \"{CLIENT_ID}\"\nscopes = [\"org:create_api_key\", \"user:profile\", \"user:inference\"]\nauthorize_url = \"{}\"\ntoken_url = \"{}\"\nredirect = [{{ uri = \"https://console.example.com/oauth/code/callback\", kind = \"code_page\" }}, {{ uri = \"http://localhost/callback\", kind = \"loopback\" }}]\nparams = {{ code = \"true\" }}\nbody = \"json\"\nrefresh_lead = \"4h\"\nterms_warning = true\n",
            idp.url(AUTHORIZE),
            idp.url(TOKEN)
        ),
    )
}

fn http() -> SignInHttp {
    SignInHttp::new(true).with_timeout(Duration::from_secs(5))
}

async fn device(idp: &MockIdp) -> (ProviderEntity, DeviceSignIn) {
    let p = grok_cli(idp);
    match nullrouter_engine::signin::begin(&http(), &p).await.unwrap() {
        Begun::Device(d) => (p, d),
        Begun::Pkce(_) => panic!("grok-cli is a device-code provider"),
    }
}

async fn pkce(p: &ProviderEntity) -> PkceSignIn {
    match nullrouter_engine::signin::begin(&http(), p).await.unwrap() {
        Begun::Pkce(s) => s,
        Begun::Device(_) => panic!("a pkce provider"),
    }
}

fn query(url: &str, key: &str) -> Option<String> {
    url::Url::parse(url).unwrap().query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned())
}

fn never() -> std::future::Pending<Option<String>> {
    std::future::pending()
}

#[tokio::test]
async fn device_code_pending_then_approved() {
    let idp = MockIdp::start().await;
    idp.set_id_claims(Some(json!({ "email": "grok@example.com", "sub": "s-1" })));
    idp.push_device([DevicePoll::Pending]);
    let (p, mut d) = device(&idp).await;
    assert!(d.user_code.starts_with("ABCD-"));
    assert!(d.link().contains("user_code="), "verification_uri_complete is shown");
    assert_eq!(d.interval(), Duration::from_secs(1));

    let sent = &idp.device_requests()[0];
    assert_eq!(sent.get("client_id"), Some(CLIENT_ID));
    assert_eq!(sent.get("scope"), Some("openid offline_access"));
    assert_eq!(sent.get("referrer"), Some("grok-build"));
    assert!(sent.get("code_challenge").is_none() && sent.get("code_verifier").is_none(), "no PKCE");

    let e = d.poll(&http(), "work", &CancellationToken::new()).await.unwrap();
    assert_eq!(idp.token_calls(), 2, "one pending poll, then the grant");
    let polls = idp.token_requests();
    assert!(polls.iter().all(|r| r.get("grant_type") == Some(DEVICE_GRANT) && !r.json));
    assert_eq!((e.provider.as_str(), e.name.as_str()), ("grok-cli", "work"));
    assert!(e.access_token.matches(idp.issued().last().unwrap()));
    assert!(e.refresh_token.is_some());
    assert_eq!(e.hosts, p.token_hosts());
    assert_eq!(e.claims.email.as_deref(), Some("grok@example.com"), "id token payload, display only");
    assert!(e.expires_at > std::time::SystemTime::now() + Duration::from_secs(3500));
    assert!(!format!("{e:?}").contains("SENTINEL"));
}

#[tokio::test]
async fn device_code_slow_down_adds_five_seconds() {
    let idp = MockIdp::start().await;
    idp.push_device([DevicePoll::SlowDown, DevicePoll::Pending]);
    let (_, mut d) = device(&idp).await;
    let h = http();
    assert!(matches!(d.poll_once(&h, "work").await.unwrap(), Polled::SlowDown));
    assert_eq!(d.interval(), Duration::from_secs(6));
    assert!(matches!(d.poll_once(&h, "work").await.unwrap(), Polled::Pending));
    assert_eq!(d.interval(), Duration::from_secs(6), "pending keeps the interval");
    assert!(matches!(d.poll_once(&h, "work").await.unwrap(), Polled::Done(_)));
}

#[tokio::test]
async fn device_code_expired_and_denied_end_with_that_reason() {
    let idp = MockIdp::start().await;
    idp.push_device([DevicePoll::Expired]);
    let (_, mut d) = device(&idp).await;
    let err = d.poll(&http(), "work", &CancellationToken::new()).await.unwrap_err();
    assert!(matches!(err, SignInError::Expired), "{err}");

    idp.push_device([DevicePoll::Denied]);
    let (_, mut d) = device(&idp).await;
    let err = d.poll(&http(), "work", &CancellationToken::new()).await.unwrap_err();
    assert!(matches!(err, SignInError::Denied(_)), "{err}");
    assert!(err.to_string().contains("access_denied"), "{err}");

    // The provider's own expiry ends the wait too, without another poll.
    idp.set_device_timing(1, 1);
    idp.push_device([DevicePoll::Pending; 5]);
    let (_, mut d) = device(&idp).await;
    let before = idp.token_calls();
    let err = d.poll(&http(), "work", &CancellationToken::new()).await.unwrap_err();
    assert!(matches!(err, SignInError::Expired), "{err}");
    assert!(idp.token_calls() - before <= 1);
}

#[tokio::test]
async fn pkce_with_the_loopback_redirect() {
    let idp = MockIdp::start().await;
    let p = xai(&idp, 0);
    let mut s = pkce(&p).await;
    assert!(s.listening() && s.port_busy().is_none());
    let redirect = s.redirect_uri().to_owned();
    assert!(redirect.starts_with("http://127.0.0.1:") && !redirect.contains(":0/"), "{redirect}");

    let url = s.authorize_url().to_owned();
    assert!(url.contains("scope=openid%20profile%20email%20offline_access"), "{url}");
    assert!(url.contains("code_challenge_method=S256") && url.contains("plan=generic"), "{url}");
    assert_eq!(query(&url, "nonce").unwrap().len(), 32);
    assert_eq!(query(&url, "redirect_uri").as_deref(), Some(redirect.as_str()));
    assert!(query(&url, "state").unwrap().len() >= 32);

    let browser = tokio::spawn({
        let location = idp.browse(&url).await;
        async move { reqwest::get(location).await.unwrap().text().await.unwrap() }
    });
    let code = s.wait_code(never(), &CancellationToken::new()).await.unwrap();
    assert!(browser.await.unwrap().contains("close this page"));
    let e = s.complete(&http(), "main", code).await.unwrap();

    let ex = idp.token_requests().pop().unwrap();
    assert!(!ex.json && ex.get("grant_type") == Some("authorization_code"));
    assert_eq!(ex.get("code_verifier").unwrap().len(), 128, "96 bytes, base64url");
    assert!(ex.get("state").is_none(), "form exchange carries no state");
    assert_eq!(e.claims.email.as_deref(), Some("user@example.com"));
    assert_eq!(e.claims.user_id.as_deref(), Some("user-0001"));
    assert_eq!(e.claims.tier.as_deref(), Some("SuperGrok"));
    assert_eq!(idp.profile_calls(), 1);
    assert_eq!(e.hosts, p.token_hosts());
}

#[tokio::test]
async fn pkce_paste_back_when_the_port_is_busy() {
    let idp = MockIdp::start().await;
    let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = busy.local_addr().unwrap().port();
    let p = xai(&idp, port);
    let mut s = pkce(&p).await;
    assert!(!s.listening());
    assert!(s.port_busy().unwrap().contains(&port.to_string()));
    assert_eq!(s.redirect_uri(), format!("http://127.0.0.1:{port}/callback"));

    let location = idp.browse(s.authorize_url()).await;
    // A bare code isn't accepted for a loopback redirect: the state check can't be skipped.
    let code = query(&location, "code").unwrap();
    assert_eq!(s.parse_paste(&code).unwrap_err(), PasteError::BareCode);
    let pasted = format!("  {location}\n");
    let code = s.wait_code(async { Some(pasted) }, &CancellationToken::new()).await.unwrap();
    s.complete(&http(), "main", code).await.unwrap();
}

#[tokio::test]
async fn pkce_code_page_takes_code_hash_state_or_a_bare_code() {
    let idp = MockIdp::start().await;
    let p = anthropic(&idp);
    let s = pkce(&p).await;
    assert!(!s.listening(), "the code page comes first");
    assert_eq!(s.redirect_uri(), "https://console.example.com/oauth/code/callback");
    assert!(s.authorize_url().contains("code=true"));
    assert!(s.authorize_url().contains("scope=org%3Acreate_api_key%20user%3Aprofile%20user%3Ainference"));

    let location = idp.browse(s.authorize_url()).await;
    let (code, state) = (query(&location, "code").unwrap(), query(&location, "state").unwrap());
    let got = s.parse_paste(&format!("{code}#{state}")).unwrap();
    let e = s.complete(&http(), "max", got).await.unwrap();
    assert!(e.access_token.matches(idp.issued().last().unwrap()));
    let ex = idp.token_requests().pop().unwrap();
    assert!(ex.json, "anthropic exchanges with a JSON body");
    assert_eq!(ex.get("state"), Some(state.as_str()), "and includes state");
    assert_eq!(ex.get("code"), Some(code.as_str()), "the code without #state");

    let s = pkce(&p).await;
    let location = idp.browse(s.authorize_url()).await;
    let got = s.parse_paste(&query(&location, "code").unwrap()).unwrap();
    s.complete(&http(), "max", got).await.unwrap();
}

#[tokio::test]
async fn state_mismatch_is_refused() {
    let idp = MockIdp::start().await;
    let s = pkce(&xai(&idp, 0)).await;
    let location = idp.browse(s.authorize_url()).await;
    let state = query(&location, "state").unwrap();
    let forged = location.replace(&state, "forged-state");
    assert_eq!(s.parse_paste(&forged).unwrap_err(), PasteError::StateMismatch);
    let no_state = location.split("&state=").next().unwrap().to_owned();
    assert_eq!(s.parse_paste(&no_state).unwrap_err(), PasteError::StateMismatch);

    let a = pkce(&anthropic(&idp)).await;
    let location = idp.browse(a.authorize_url()).await;
    let code = query(&location, "code").unwrap();
    assert_eq!(a.parse_paste(&format!("{code}#forged")).unwrap_err(), PasteError::StateMismatch);

    // A loopback callback with the wrong state ends the wait.
    let mut s = pkce(&xai(&idp, 0)).await;
    let location = idp.browse(s.authorize_url()).await;
    let state = query(&location, "state").unwrap();
    let forged = location.replace(&state, "forged-state");
    let hit = tokio::spawn(async move { reqwest::get(forged).await.unwrap().status() });
    let err = s.wait_code(never(), &CancellationToken::new()).await.unwrap_err();
    assert!(matches!(err, SignInError::Paste(PasteError::StateMismatch)), "{err}");
    assert_eq!(hit.await.unwrap(), 400);
}

#[tokio::test]
async fn a_denied_authorization_ends_the_sign_in() {
    let idp = MockIdp::start().await;
    idp.set_deny_authorize(true);
    let s = pkce(&xai(&idp, 0)).await;
    let location = idp.browse(s.authorize_url()).await;
    let err = s.parse_paste(&location).unwrap_err();
    assert!(matches!(err, PasteError::Provider(ref r) if r == "access_denied"), "{err}");
}

#[tokio::test]
async fn discovery_off_the_declared_hosts_falls_back_to_the_declared_urls() {
    let idp = MockIdp::start().await;
    let p = xai(&idp, 0);
    let via = |path: &str| format!("{}?via=discovery", idp.url(path));

    idp.set_discovery(Some(json!({ "authorization_endpoint": via(AUTHORIZE), "token_endpoint": via(TOKEN) })));
    let s = pkce(&p).await;
    assert!(s.authorize_url().starts_with(&format!("{}&", via(AUTHORIZE))), "{}", s.authorize_url());
    let location = idp.browse(s.authorize_url()).await;
    s.complete(&http(), "main", s.parse_paste(&location).unwrap()).await.unwrap();
    assert!(idp.token_requests().pop().unwrap().path_and_query.ends_with("?via=discovery"));

    for (auth, token) in [
        ("https://evil.example/oauth2/authorize".to_owned(), via(TOKEN)),
        (via(AUTHORIZE), "https://auth.evil.example/oauth2/token".to_owned()),
        (via(AUTHORIZE), format!("http://127.0.0.1:1{TOKEN}")),
        ("not a url".to_owned(), via(TOKEN)),
    ] {
        idp.set_discovery(Some(json!({ "authorization_endpoint": auth, "token_endpoint": token })));
        let s = pkce(&p).await;
        assert!(s.authorize_url().starts_with(&format!("{}?", idp.url(AUTHORIZE))), "{}", s.authorize_url());
        let location = idp.browse(s.authorize_url()).await;
        s.complete(&http(), "main", s.parse_paste(&location).unwrap()).await.unwrap();
        assert_eq!(idp.token_requests().pop().unwrap().path_and_query, TOKEN, "the declared token URL");
    }

    idp.set_discovery(Some(json!({ "nothing": true })));
    assert!(pkce(&p).await.authorize_url().starts_with(&idp.url(AUTHORIZE)));
}

#[tokio::test]
async fn the_browser_flow_times_out_and_can_be_cancelled() {
    let idp = MockIdp::start().await;
    let mut s = pkce(&xai(&idp, 0)).await.with_timeout(Duration::from_millis(300));
    let err = s.wait_code(never(), &CancellationToken::new()).await.unwrap_err();
    assert!(matches!(err, SignInError::TimedOut(_)), "{err}");
    assert_eq!(idp.token_calls(), 0, "nothing exchanged, nothing to write");

    let mut s = pkce(&xai(&idp, 0)).await;
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(s.wait_code(never(), &cancel).await.unwrap_err(), SignInError::Cancelled));

    let mut s = pkce(&anthropic(&idp)).await;
    let err = s.wait_code(async { None }, &CancellationToken::new()).await.unwrap_err();
    assert!(matches!(err, SignInError::NoInput), "no listener and stdin closed: {err}");
    assert_eq!(nullrouter_engine::signin::BROWSER_TIMEOUT, Duration::from_secs(600));
}

#[tokio::test]
async fn exchange_failures_name_the_reason_and_never_the_secrets() {
    let idp = MockIdp::start().await;
    let p = anthropic(&idp);
    for (failure, want) in [
        (Failure::permanent("invalid_grant"), "invalid_grant"),
        (Failure::Status(503), "503"),
        (Failure::Hang(Duration::from_secs(3)), "timed out"),
    ] {
        let s = pkce(&p).await;
        let location = idp.browse(s.authorize_url()).await;
        let code = s.parse_paste(&location).unwrap();
        idp.fail_token([failure]);
        let h = SignInHttp::new(true).with_timeout(Duration::from_millis(500));
        let err = s.complete(&h, "max", code).await.unwrap_err();
        let shown = format!("{err} {err:?}");
        assert!(shown.contains(want), "{shown}");
        assert!(!shown.contains("SENTINEL"), "no code, verifier or token: {shown}");
    }
    let s = pkce(&p).await;
    let location = idp.browse(s.authorize_url()).await;
    s.complete(&http(), "max", s.parse_paste(&location).unwrap()).await.unwrap();
    let again = s.complete(&http(), "max", s.parse_paste(&location).unwrap()).await.unwrap_err();
    assert!(
        matches!(&again, SignInError::Rejected { status: 400, code, .. } if code == "invalid_grant"),
        "a spent code: {again}"
    );
}

#[tokio::test]
async fn private_addresses_need_the_operator_allowance() {
    let idp = MockIdp::start().await;
    let p = grok_cli(&idp);
    let err = nullrouter_engine::signin::begin(&SignInHttp::new(false), &p).await.err().unwrap();
    assert!(matches!(err, SignInError::BadUrl { .. }), "{err}");
    assert_eq!(idp.device_calls(), 0);
}

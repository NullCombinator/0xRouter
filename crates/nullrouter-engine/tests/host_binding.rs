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

        // Tokens bound elsewhere: withheld, and the key account serves.
        put(home, entry("sso", &["api.mockco.example"], 3600));
        s.engine.reload().await.unwrap();
        s.mock.push([ok()]);
        let (id, res) = send(&s, "mockco/m1").await;
        assert!(res.is_ok());
        assert_eq!(auth_of(&s)[1], format!("Bearer {SECRET}-mockco-key"));
        let r = s.engine.records.get(&id).unwrap();
        let Some(AttemptOutcome::Skipped { reason, .. }) = &r.attempts[0].outcome else { panic!("{:?}", r.attempts) };
        assert!(reason.contains("127.0.0.1") && reason.contains("accounts signin mockco sso"), "{reason}");
        assert!(!format!("{r:?}").contains("tok-SENTINEL"));
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

//! No page carries a secret or a prompt (T069, SC-006, FR-043, FR-044). The slice 005/006
//! sentinel scan, extended to the dashboard: on the `dashboard()` home with a sentinel provider
//! key, its sign-in tokens, a freshly issued agent key, the dashboard token and a served request
//! whose prompt and answer are sentinels, every route is fetched (pages, `?notices`, windows,
//! filters, the sign-in page, assets, logos) and every response body and header is scanned. Only
//! a `…last4` form may appear.

mod common;

use std::fs;

use common::{Dash, TOKEN};
use nullrouter_dashboard::assets;
use nullrouter_dashboard::logos::Index;
use nullrouter_dashboard::pages::Id;
use nullrouter_engine::files::write_private;
use nullrouter_engine::keys::{self, Keys};
use nullrouter_engine::testkit::{MockUpstream, Step, homes};
use reqwest::header::COOKIE;
use reqwest::{Client, redirect};
use serde_json::json;

const KEY: &str = "sk-dash-SENTINEL-KEY-0001";
const PROMPT: &str = "PROMPT-SENTINEL-the-cat-sat";
const ANSWER: &str = "ANSWER-SENTINEL-on-the-mat";

/// The `dashboard()` home with one more provider at the mock (its account holds [`KEY`]) and one
/// more agent key, returned whole.
fn home(mock: &MockUpstream) -> (tempfile::TempDir, String) {
    let dir = homes::dashboard();
    let h = dir.path();
    fs::write(
        h.join("plugins/sentinel.toml"),
        format!(
            "schema = 2\nid = \"sentinel\"\ncategory = \"apikey\"\n\n[auth]\nkind = \"apikey\"\nheader = \"Authorization\"\n\
             scheme = \"bearer\"\n\n[endpoints.text]\nurl = \"{}\"\nwire = \"openai-chat\"\n\n[[models]]\nid = \"m1\"\n",
            mock.url("/sentinel/chat/completions")
        ),
    )
    .unwrap();
    let config = fs::read_to_string(h.join("config.toml")).unwrap();
    fs::write(h.join("config.toml"), format!("allow_private_endpoints = true\n{config}")).unwrap();
    let accounts = fs::read_to_string(h.join(nullrouter_engine::accounts::FILE)).unwrap();
    write_private(
        &h.join(nullrouter_engine::accounts::FILE),
        &format!("{accounts}\n[[account]]\nprovider = \"sentinel\"\nname = \"main\"\nsecret = \"{KEY}\"\norder = 9\n"),
    )
    .unwrap();
    let mut keys = Keys::load(&h.join(keys::FILE)).unwrap();
    let (agent, _) = keys.issue("sentinel-agent", None).unwrap();
    write_private(&h.join(keys::FILE), &keys.to_toml()).unwrap();
    (dir, agent)
}

/// Every secret the home holds, read from its own files: account keys and sign-in tokens.
fn home_secrets(h: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    for file in [nullrouter_engine::accounts::FILE, nullrouter_engine::tokens::FILE] {
        let text = fs::read_to_string(h.join(file)).unwrap_or_default();
        for line in text.lines() {
            let line = line.trim();
            for field in ["secret = \"", "access_token = \"", "refresh_token = \""] {
                if let Some(v) = line.strip_prefix(field).and_then(|r| r.strip_suffix('"')) {
                    out.push(v.to_owned());
                }
            }
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn no_route_carries_a_secret_or_a_prompt() {
    let mock = MockUpstream::start().await;
    mock.respond(|_| {
        Step::json(
            200,
            json!({"id": "up-1", "object": "chat.completion", "created": 1, "model": "m1",
                   "choices": [{"index": 0, "message": {"role": "assistant", "content": ANSWER}, "finish_reason": "stop"}],
                   "usage": {"prompt_tokens": 3, "completion_tokens": 3}}),
        )
    });
    let (dir, agent) = home(&mock);
    let mut sentinels = home_secrets(dir.path());
    assert!(sentinels.iter().any(|s| s == KEY) && sentinels.len() >= 4, "{sentinels:?}");
    sentinels.extend([agent.clone(), TOKEN.to_owned(), PROMPT.to_owned(), ANSWER.to_owned()]);
    let d = Dash::start(dir).await;

    // A served request whose prompt and answer are sentinels: the provider got the key, so the
    // sentinel is live.
    let http = Client::builder().redirect(redirect::Policy::none()).no_proxy().build().unwrap();
    let r = http
        .post(format!("http://{}/v1/chat/completions", d.client_addr))
        .bearer_auth(&agent)
        .body(json!({"model": "sentinel/m1", "messages": [{"role": "user", "content": PROMPT}]}).to_string())
        .send()
        .await
        .unwrap();
    let body = r.text().await.unwrap();
    assert!(body.contains(ANSWER), "{body}");
    assert!(mock.received().iter().any(|r| r.headers.values().any(|v| v.to_str().is_ok_and(|v| v.contains(KEY)))), "the key went upstream");

    let record = d.view(nullrouter_dashboard::page::ViewName::Records, json!({"limit": 1, "before": null})).await;
    let record = record[0]["id"].as_str().unwrap().to_owned();
    let mut paths: Vec<String> = vec!["/".into(), "/signin".into(), "/no-such-page".into()];
    for id in Id::ALL {
        paths.push(id.path().to_owned());
        paths.push(format!("{}?notices", id.path()));
    }
    paths.extend([
        "/providers/sentinel".into(),
        "/providers/xai".into(),
        "/providers?kind=llm&q=sent".into(),
        "/quota?provider=sentinel&account=main".into(),
        format!("/usage/records/{record}"),
        format!("/usage/records/{record}?notices"),
    ]);
    paths.extend(assets::STYLESHEETS.iter().map(|s| assets::href(s)));
    paths.extend(Index::of(&d.engine).hrefs().map(str::to_owned));

    let mut leaks = Vec::new();
    for path in &paths {
        for cookie in [true, false] {
            let mut req = http.get(format!("http://{}{path}", d.addr));
            if cookie {
                req = req.header(COOKIE, format!("nr_dashboard={TOKEN}"));
            }
            let r = req.send().await.unwrap();
            let headers: String = r.headers().iter().map(|(k, v)| format!("{k}: {}\n", v.to_str().unwrap_or("?"))).collect();
            let bytes = r.bytes().await.unwrap();
            let body = String::from_utf8_lossy(&bytes);
            for s in &sentinels {
                if headers.contains(s.as_str()) {
                    leaks.push(format!("{path} (cookie {cookie}): header carries {s:?}"));
                }
                if body.contains(s.as_str()) {
                    leaks.push(format!("{path} (cookie {cookie}): body carries {s:?}"));
                }
            }
        }
    }
    assert!(leaks.is_empty(), "{}", leaks.join("\n"));
}

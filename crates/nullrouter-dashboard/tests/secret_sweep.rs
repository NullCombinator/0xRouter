//! SC-004 (spec 014, T043): no secret typed into a write-only field comes back out. Marker secrets go
//! into every write-only field (the account secret, the proxy password, the sign-in paste code), each
//! also in a refused variant so the refill of a kept form is covered. Then every page, window, confirm
//! window and task panel is crawled as the style guide's check does, and every response (headers,
//! `Location` and body) is scanned. Last, every file in the home is read. The two secret stores may
//! hold their own marker and no other: `accounts.toml` the account marker, `proxies.toml` the proxy
//! marker. No marker is in any other file, including `dashboard-log.jsonl`.

mod common;

use std::path::Path;

use common::{Site, files_under, notice};
use nullrouter_dashboard::page::ViewName;
use nullrouter_dashboard::pages::Id;
use nullrouter_engine::connection::proxy;
use nullrouter_engine::tasks::TaskKind;
use nullrouter_engine::{accounts, testkit::homes};
use nullrouter_server::views::change_log;
use reqwest::StatusCode;
use reqwest::header::{COOKIE, HeaderMap, LOCATION};
use serde_json::json;

/// The account secret that is kept (the account is added).
const ACCOUNT: &str = "sk-MARKER-acct-7f3a";
/// The account secret in a refused add (an invalid name).
const ACCOUNT_REFUSED: &str = "sk-MARKER-acct-bad-2e81";
/// The proxy password that is kept (the proxy is added).
const PROXY: &str = "pw-MARKER-proxy-91c2";
/// The proxy password in a refused add (a bad URL).
const PROXY_REFUSED: &str = "pw-MARKER-proxy-bad-6a5d";
/// The sign-in paste code, pasted to a well-formed task id that is no task.
const PASTE: &str = "code-MARKER-paste-44d0";
/// The sign-in paste code in a refused paste (a malformed task id).
const PASTE_REFUSED: &str = "code-MARKER-paste-bad-0f9e";

/// The password in a pasted `https://u:p@host/v1` base URL (security review L2).
const URL_USERINFO: &str = "pw-MARKER-url-userinfo-5b1c";
/// The key in a pasted `https://host/v1?api_key=...` base URL.
const URL_QUERY: &str = "key-MARKER-url-query-c83e";

const MARKERS: [&str; 8] =
    [ACCOUNT, ACCOUNT_REFUSED, PROXY, PROXY_REFUSED, PASTE, PASTE_REFUSED, URL_USERINFO, URL_QUERY];
const FAKE_TASK: &str = "tk_abcdefghijklmnop";
const BACK_PROVIDER: &str = "/providers/anthropic";
const BACK_PROVIDERS: &str = "/providers";
const BACK_PROXY: &str = "/proxy-pools";

/// A response's headers as text: `name: value` lines, so a marker in any header is found.
fn headers_text(h: &HeaderMap) -> String {
    h.iter().map(|(k, v)| format!("{k}: {}\n", String::from_utf8_lossy(v.as_bytes()))).collect()
}

/// `GET path` with the cookie, as a browser. Its headers and body go into `seen`.
async fn get(site: &Site, seen: &mut Vec<String>, path: &str) -> (StatusCode, String) {
    let r = site.http.get(site.url(path)).header(COOKIE, &site.cookie).send().await.unwrap();
    let status = r.status();
    seen.push(headers_text(r.headers()));
    let body = r.text().await.unwrap();
    seen.push(body.clone());
    (status, body)
}

/// Posts `fields` to `route` as the page's form does, and follows the redirect to the page it shows.
/// The post's headers, its `Location` and its body go into `seen`, and so does the page. Returns
/// whether the change was refused.
async fn submit(site: &Site, seen: &mut Vec<String>, route: &str, back: &str, fields: &[(&str, &str)]) -> bool {
    let fields: Vec<(&str, String)> = fields.iter().map(|(k, v)| (*k, (*v).to_owned())).collect();
    let r = site.post(route, back, &fields).await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER, "{route}");
    let location = r.headers()[LOCATION].to_str().unwrap().to_owned();
    seen.push(headers_text(r.headers()));
    seen.push(location.clone());
    seen.push(r.text().await.unwrap());
    assert!(location.starts_with(&format!("{back}?done=")), "{route} went to {location}");
    let (status, page) = get(site, seen, &location).await;
    assert_eq!(status, StatusCode::OK, "{location}");
    notice(&page).is_some_and(|(refused, _)| refused)
}

#[tokio::test(flavor = "multi_thread")]
async fn no_secret_typed_into_a_write_only_field_comes_back_out() {
    let site = Site::start_on(homes::dashboard()).await;
    let mut seen: Vec<String> = Vec::new();

    // The account secret: kept by a good add, and refused with an invalid name.
    let refused = submit(
        &site,
        &mut seen,
        "accounts/add",
        BACK_PROVIDER,
        &[("provider", "anthropic"), ("name", "marked"), ("secret", ACCOUNT), ("order", "")],
    )
    .await;
    assert!(!refused, "the marked account was refused");
    let refused = submit(
        &site,
        &mut seen,
        "accounts/add",
        BACK_PROVIDER,
        &[("provider", "anthropic"), ("name", "Bad Name"), ("secret", ACCOUNT_REFUSED), ("order", "7")],
    )
    .await;
    assert!(refused, "an invalid account name is not refused");

    // The proxy password: kept by a good add, and refused with a bad URL.
    submit(
        &site,
        &mut seen,
        "proxy/add",
        BACK_PROXY,
        &[("name", "marked"), ("url", "http://10.0.0.3:3128"), ("username", "carol"), ("password", PROXY)],
    )
    .await;
    let refused = submit(
        &site,
        &mut seen,
        "proxy/add",
        BACK_PROXY,
        &[("name", "bad"), ("url", "not a url"), ("username", "dave"), ("password", PROXY_REFUSED)],
    )
    .await;
    assert!(refused, "a bad proxy URL is not refused");

    // A base URL with userinfo or a query: refused, and neither the refusal, the refilled form nor
    // the change log carries the credential.
    let with_userinfo = format!("https://u:{URL_USERINFO}@example.com/v1");
    let with_query = format!("https://example.com/v1?api_key={URL_QUERY}");
    for (id, url) in [("userinfo", &with_userinfo), ("query", &with_query)] {
        let refused = submit(
            &site,
            &mut seen,
            "providers/add-compatible",
            BACK_PROVIDERS,
            &[("id", id), ("name", id), ("style", "openai"), ("base_url", url), ("models", "m1")],
        )
        .await;
        assert!(refused, "a base URL with a credential is not refused");
    }

    // The sign-in paste code: a well-formed task id that is no task, and a malformed one.
    let refused =
        submit(&site, &mut seen, "accounts/signin-paste", BACK_PROVIDERS, &[("task", FAKE_TASK), ("code", PASTE)])
            .await;
    assert!(refused, "a paste to no task is not refused");
    let refused = submit(
        &site,
        &mut seen,
        "accounts/signin-paste",
        BACK_PROVIDERS,
        &[("task", "not-a-task"), ("code", PASTE_REFUSED)],
    )
    .await;
    assert!(refused, "a paste to a malformed task id is not refused");

    // The crawl: the style guide's list (every page, its notices, every window and confirm window,
    // the task panels), on the ids this home now holds. A running test task gives a panel.
    let keys = site.d.view(ViewName::Keys, json!({})).await;
    let key = keys[0]["id"].as_str().unwrap().to_owned();
    let Ok(task) = site.d.engine.tasks.start(TaskKind::Test, "u") else { panic!("the fixture starts a task") };
    let task_id = task.id().as_str().to_owned();

    let mut urls: Vec<String> = Id::ALL.iter().map(|id| id.path().to_owned()).collect();
    urls.extend(Id::ALL.iter().map(|id| format!("{}?notices", id.path())));
    urls.extend([
        "/providers/anthropic".to_owned(),
        "/endpoint?window=add".to_owned(),
        format!("/endpoint?window=revoke&key={key}"),
        "/endpoint?window=revoke".to_owned(),
        "/providers?window=add".to_owned(),
        "/providers?window=add-openai".to_owned(),
        "/providers?window=add-anthropic".to_owned(),
        "/providers?window=uninstall-plugin&id=bluesminds".to_owned(),
        "/settings?window=private-endpoints".to_owned(),
        "/endpoint?window=clear-adapter&harness=claude-code&version=v1".to_owned(),
        "/endpoint?window=remove-adapter&harness=claude-code&version=v1".to_owned(),
        "/providers?window=signin&id=anthropic".to_owned(),
        "/providers?window=remove-account&id=anthropic&name=marked".to_owned(),
        "/proxy-pools?window=remove&name=marked".to_owned(),
        "/proxy-pools?window=remove&name=eu".to_owned(),
        "/combo?window=test&target=all".to_owned(),
        "/combo?window=remove-unified&name=u".to_owned(),
        "/combo?window=remove-combo&name=c".to_owned(),
        format!("/combo?task={task_id}"),
        format!("/providers?task={task_id}"),
    ]);
    for url in &urls {
        let (status, _) = get(&site, &mut seen, url).await;
        assert_eq!(status, StatusCode::OK, "{url}");
    }
    // Without a cookie: the redirect to sign-in and its Location.
    let r = site.http.get(site.url("/providers")).send().await.unwrap();
    seen.push(headers_text(r.headers()));
    seen.push(r.text().await.unwrap());
    drop(task);

    // Every response the browser saw: no marker in a header, a Location or a body.
    let mut leaks = Vec::new();
    for marker in MARKERS {
        for (n, text) in seen.iter().enumerate() {
            if text.contains(marker) {
                leaks.push(format!("response {n} carries {marker}"));
            }
        }
    }
    assert!(leaks.is_empty(), "{}", leaks.join("\n"));

    // Every file in the home. The two secret stores may hold their own marker and no other; no
    // other file may hold any marker, the change log included.
    let holds =
        |path: &Path, marker: &str| std::fs::read(path).unwrap().windows(marker.len()).any(|w| w == marker.as_bytes());
    let home = site.home();
    assert!(holds(&home.join(accounts::FILE), ACCOUNT), "the account secret is not in accounts.toml");
    assert!(holds(&home.join(proxy::FILE), PROXY), "the proxy password is not in proxies.toml");
    let mut leaks = Vec::new();
    for file in files_under(home) {
        let path = home.join(&file);
        for marker in MARKERS {
            let legit = (file == accounts::FILE && marker == ACCOUNT) || (file == proxy::FILE && marker == PROXY);
            if !legit && holds(&path, marker) {
                leaks.push(format!("{file} holds {marker}"));
            }
        }
    }
    assert!(leaks.is_empty(), "{}", leaks.join("\n"));
    let log = std::fs::read_to_string(change_log::path(&site.d.home())).unwrap();
    assert!(log.contains("anthropic/marked"), "the change log has no account add: {log}");
    assert!(!MARKERS.iter().any(|m| log.contains(*m)), "the change log holds a marker: {log}");
}

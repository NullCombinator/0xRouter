//! US4 (spec 014, T038): the plugin, provider and adapter forms through the browser.
//!
//! The CLI side is the shared ops (`nullrouter_server::ops::{plugins, adapters}`) called with
//! `Ctx { engine: None }` on a twin home, as the US1 and US3 suites do. Each post's flash must read
//! what the CLI printed (its refusal, or its line; the only word that may differ is "applied" for
//! "saved; applies at next start"), and the homes must hold the same `config.toml` and plugin files.
//!
//! Not covered here: `catalogue install`. Its mock (a catalogue index and archive server) is local to
//! `crates/nullrouter-adapters/tests/catalogue.rs`, and this crate has no dependency on
//! `nullrouter-adapters` to reach it. Without the mock the op would try the network, so it is left out.
//!
//! The confirm windows: uninstall and adapter remove open from a window that names what they lose.
//! The window's own form is the confirmation. The handlers take no `confirmed` field.

mod common;

use std::path::Path;

use common::{Site, ctx_of, decode, files_under, notice, text_of, twin_of};
use nullrouter_engine::testkit::{MockUpstream, Step};
use nullrouter_server::ops::{Ctx, Done, OpError, adapters, plugins};
use reqwest::StatusCode;
use reqwest::header::LOCATION;
use serde_json::{Value, json};

const SECRET: &str = "sk-fixture-compat-CCCC0003";

/// A provider-level CLI op that takes an id.
type ById = fn(&Ctx, &str) -> Result<Done, OpError>;

fn reply() -> Step {
    Step::json(
        200,
        json!({"id": "up-1", "object": "chat.completion", "created": 1, "model": "llama-a",
               "choices": [{"index": 0, "message": {"role": "assistant", "content": "hi"}, "finish_reason": "stop"}],
               "usage": {"prompt_tokens": 3, "completion_tokens": 1}}),
    )
}

/// The CLI's line with the one word a server-less CLI differs on.
fn cli_line(d: &Done) -> String {
    d.text.replace("saved; applies at next start", "applied")
}

/// `text` with the home's directory replaced, so two homes' messages compare equal.
fn at_home(text: &str, home: &Path) -> String {
    text.replace(&home.display().to_string(), "<home>")
}

fn config(home: &Path) -> String {
    std::fs::read_to_string(home.join("config.toml")).unwrap_or_default()
}

/// The files under `plugins/` with their bytes.
fn plugin_files(home: &Path) -> Vec<(String, Vec<u8>)> {
    files_under(home)
        .into_iter()
        .filter(|f| f.starts_with("plugins/"))
        .map(|f| {
            let bytes = std::fs::read(home.join(&f)).unwrap();
            (f, bytes)
        })
        .collect()
}

/// The first form posting to `action`, through its `</form>`.
fn form_to<'a>(html: &'a str, action: &str) -> &'a str {
    let at = html.find(&format!("action=\"{action}\"")).unwrap_or_else(|| panic!("no form posts to {action}"));
    let start = html[..at].rfind("<form").expect("the action belongs to a form");
    let end = at + html[at..].find("</form>").expect("the form closes");
    &html[start..end]
}

/// Posts `fields` to `route` as the page's form does, and follows the 303: the location must start
/// with `back` and carry a flash. Returns whether the flash is a refusal, its text, and the page.
async fn flash(site: &Site, route: &str, back: &str, fields: &[(&str, String)]) -> (bool, String, String) {
    let r = site.post(route, back, fields).await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER, "{route}");
    let location = r.headers()[LOCATION].to_str().unwrap().to_owned();
    assert!(location.starts_with(back) && location.contains("done="), "{route} went to {location}");
    let (status, page) = site.get(&location).await;
    assert_eq!(status, StatusCode::OK, "{location}");
    let (is_error, shown) = notice(&page).unwrap_or_else(|| panic!("{route}: no flash on {location}"));
    (is_error, shown, page)
}

/// Posts through the browser and runs the CLI's op on the twin: the flash must be the CLI's refusal
/// or its line, with each home's directory named the same way.
async fn same_as_cli(
    site: &Site,
    twin: &Path,
    route: &str,
    back: &str,
    fields: &[(&str, String)],
    cli: impl FnOnce(&Ctx) -> Result<Done, OpError>,
) {
    let (is_error, shown, _) = flash(site, route, back, fields).await;
    let shown = at_home(&shown, site.home());
    match cli(&ctx_of(twin)) {
        Err(e) => {
            assert!(is_error, "{route}: the CLI refused, the page did not: {shown}");
            assert_eq!(shown, at_home(&e.text, twin), "{route} {fields:?}: the page and the CLI disagree");
        }
        Ok(d) => {
            assert!(!is_error, "{route}: the page refused, the CLI did not: {shown}");
            assert_eq!(shown, at_home(&cli_line(&d), twin), "{route} {fields:?}: the page and the CLI disagree");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn every_offline_plugin_and_provider_op_through_the_browser_matches_the_cli() {
    let site = Site::start().await;
    let twin = twin_of(site.home());
    let back = "/providers";

    // plugins.install: "baidu" is a community plugin that fits this core ("deepseek" is refused).
    same_as_cli(&site, twin.path(), "plugins/install", back, &[("id", "baidu".into())], |c| {
        plugins::install(c, "baidu")
    })
    .await;
    // Installed already: the same install is refused.
    same_as_cli(&site, twin.path(), "plugins/install", back, &[("id", "baidu".into())], |c| {
        plugins::install(c, "baidu")
    })
    .await;

    // providers.enable, disable, hide, show on a bundled provider, then one that does not exist.
    let switches: [(&str, &str, ById); 5] = [
        ("providers/disable", "anthropic", plugins::disable),
        ("providers/hide", "anthropic", plugins::hide),
        ("providers/enable", "anthropic", plugins::enable),
        ("providers/show", "anthropic", plugins::show),
        ("providers/hide", "nope", plugins::hide),
    ];
    for (route, id, cli) in switches {
        same_as_cli(&site, twin.path(), route, back, &[("id", id.into())], |c| cli(c, id)).await;
    }

    // providers.add_compatible: a public address, so no private-endpoint switch is needed.
    let models = vec!["llama-a".to_owned(), "llama-b".to_owned()];
    let compat = [
        ("id", "local-llm".to_owned()),
        ("name", "Local".to_owned()),
        ("style", "openai".to_owned()),
        ("base_url", "https://llm.example.com/v1".to_owned()),
        ("models", "llama-a\r\nllama-b\n\n".to_owned()),
    ];
    let add = |c: &Ctx| {
        plugins::add_compatible(c, "local-llm", Some("Local"), "openai", "https://llm.example.com/v1", &models)
    };
    same_as_cli(&site, twin.path(), "providers/add-compatible", back, &compat, add).await;
    // The id is taken now: refused the same way.
    same_as_cli(&site, twin.path(), "providers/add-compatible", back, &compat, add).await;

    // plugins.private_endpoints: on (the form's `on` switch), then off (the switch unposted).
    same_as_cli(&site, twin.path(), "plugins/private-endpoints", back, &[("on", "on".into())], |c| {
        plugins::private_endpoints(c, true)
    })
    .await;
    same_as_cli(&site, twin.path(), "plugins/private-endpoints", back, &[], |c| plugins::private_endpoints(c, false))
        .await;

    // catalogue.install is not compared here: see the module comment.

    assert_eq!(config(site.home()), config(twin.path()), "config.toml differs");
    assert_eq!(plugin_files(site.home()), plugin_files(twin.path()), "the plugin files differ");
    // The comparison must not pass on two empty homes: the steps left work behind.
    let files = plugin_files(site.home());
    assert!(files.iter().any(|(f, _)| f.contains("baidu")), "baidu is not installed: {files:?}");
    assert!(files.iter().any(|(f, _)| f.contains("local-llm")), "local-llm is not written: {files:?}");
    assert!(!config(site.home()).contains("allow_private_endpoints = true"), "private endpoints are still on");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_custom_provider_made_in_the_browser_serves_a_request_through_the_mock() {
    let mock = MockUpstream::start().await;
    mock.respond(|_| reply());
    let site = Site::start().await;
    let back = "/providers";

    // The mock listens on loopback, a private address: the switch must be on first.
    let (refused, shown, _) = flash(&site, "plugins/private-endpoints", back, &[("on", "on".into())]).await;
    assert!(!refused, "{shown}");

    let add = [
        ("id", "local-llm".to_owned()),
        ("name", "Local".to_owned()),
        ("style", "openai".to_owned()),
        ("base_url", mock.url("/v1")),
        ("models", "llama-a".to_owned()),
    ];
    let (refused, shown, _) = flash(&site, "providers/add-compatible", back, &add).await;
    assert!(!refused, "{shown}");

    let account = [
        ("provider", "local-llm".to_owned()),
        ("name", "main".to_owned()),
        ("secret", SECRET.to_owned()),
        ("order", String::new()),
    ];
    let (refused, shown, _) = flash(&site, "accounts/add", back, &account).await;
    assert!(!refused, "{shown}");

    // A client key, read from the once box the page shows it in.
    let key_fields = [
        ("name", "ci".to_owned()),
        ("break", String::new()),
        ("harness", String::new()),
        ("adapter", String::new()),
    ];
    // A new key shows in its once box, not as a flash (`Site::flow` accepts either).
    let page = site.flow("keys/issue", "/endpoint", &key_fields).await.page;
    let marker = "once__value\">";
    let at = page.find(marker).expect("the new key is shown");
    let rest = &page[at + marker.len()..];
    let key = decode(&rest[..rest.find("</pre>").unwrap()]);

    // The request goes to the dashboard's client listener, as an SDK would send it.
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let r = client
        .post(format!("http://{}/v1/chat/completions", site.d.client_addr))
        .bearer_auth(&key)
        .header("content-type", "application/json")
        .body(json!({"model": "local-llm/llama-a", "messages": [{"role": "user", "content": "hi"}]}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let body: Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    assert_eq!(body["choices"][0]["message"]["content"], "hi", "{body}");

    // The mock got the one call, carrying the account's key and not the client's.
    let got = mock.received();
    assert_eq!(got.len(), 1, "one call to the mock");
    assert_eq!(got[0].path_and_query, "/v1/chat/completions");
    assert!(got[0].json()["model"].as_str().unwrap_or_default().ends_with("llama-a"), "{}", got[0].json());
    let headers: Vec<String> = got[0].headers.values().filter_map(|v| v.to_str().ok()).map(str::to_owned).collect();
    assert!(headers.iter().any(|h| h.contains(SECRET)), "the account's key did not reach the mock");
    assert!(!headers.iter().any(|h| h.contains(&key)), "the client's key reached the mock");
}

#[tokio::test(flavor = "multi_thread")]
async fn uninstall_and_adapter_remove_go_through_their_confirm_windows() {
    let site = Site::start().await;
    let (refused, shown, _) = flash(&site, "plugins/install", "/providers", &[("id", "baidu".into())]).await;
    assert!(!refused, "{shown}");

    // The Providers page links to the uninstall window; it has no form that uninstalls directly.
    let (_, page) = site.get("/providers").await;
    assert!(page.contains("window=uninstall-plugin&amp;id=baidu"), "a link to the confirm window");
    assert!(!page.contains("/do/plugins/uninstall"), "uninstalling needs the window");

    // The window names what uninstalling loses, and its form posts the id with its window name.
    let (status, window) = site.get("/providers?window=uninstall-plugin&id=baidu").await;
    assert_eq!(status, StatusCode::OK);
    let loss = plugins::uninstall_question("baidu");
    assert!(!loss.is_empty(), "uninstall names a loss");
    assert!(text_of(&window).contains(&loss), "{loss}");
    let form = form_to(&window, "/do/plugins/uninstall");
    assert!(form.contains("name=\"_window\" value=\"uninstall-plugin\""), "{form}");
    assert!(form.contains("name=\"id\" value=\"baidu\""), "{form}");

    // Posting the window's form applies the uninstall.
    assert!(plugin_files(site.home()).iter().any(|(f, _)| f.contains("baidu")), "baidu is installed");
    let (refused, shown, _) = flash(
        &site,
        "plugins/uninstall",
        "/providers",
        &[("_window", "uninstall-plugin".into()), ("id", "baidu".into())],
    )
    .await;
    assert!(!refused, "{shown}");
    assert!(!plugin_files(site.home()).iter().any(|(f, _)| f.contains("baidu")), "baidu is still installed");

    // Adapter remove: the window names the loss and posts the version with its window name.
    let (status, window) = site.get("/endpoint?window=remove-adapter&harness=claude-code&version=v1").await;
    assert_eq!(status, StatusCode::OK);
    let loss = adapters::remove_question("claude-code", Some("v1"));
    assert!(!loss.is_empty(), "remove names a loss");
    assert!(text_of(&window).contains(&loss), "{loss}");
    let form = form_to(&window, "/do/adapters/remove");
    assert!(form.contains("name=\"_window\" value=\"remove-adapter\""), "{form}");
    assert!(form.contains("name=\"harness\" value=\"claude-code\"") && form.contains("name=\"version\" value=\"v1\""));

    // No adapter version is installed in this home, so the remove is refused and the CLI says why.
    let twin = twin_of(site.home());
    let cli = adapters::remove(&ctx_of(twin.path()), "claude-code", Some("v1"), false)
        .expect_err("nothing is installed, so the CLI refuses");
    let back = "/endpoint?harness=claude-code&version=v1";
    let fields = [
        ("_window", "remove-adapter".to_owned()),
        ("harness", "claude-code".to_owned()),
        ("version", "v1".to_owned()),
    ];
    let (refused, shown, _) = flash(&site, "adapters/remove", back, &fields).await;
    assert!(refused, "{shown}");
    assert_eq!(shown, at_home(&cli.text, twin.path()), "the page and the CLI disagree");
}

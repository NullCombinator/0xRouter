//! Endpoint & Key against `check --json` and `keys list`, and Settings against `behaviour show`,
//! `check` and `dashboard status` (T047, SC-001): every scalar of a fact the page shows is on the
//! page, instants through `<time datetime>`. Neither page carries a full key or the token.

use nullrouter_dashboard::page::ViewName;
use nullrouter_engine::files::DashboardToken;
use serde_json::{Value, json};

use crate::common::{Dash, TOKEN, assert_fields, assert_shows, text_of};

/// The key cards of the page, each as its own HTML.
fn key_cards(html: &str) -> Vec<String> {
    let card = |s: &str| s.split("</article>").next().unwrap_or_default().to_owned();
    html.split("<article class=\"key-card").skip(1).map(card).collect()
}

/// The digests `keys.toml` holds: what must never reach a page.
fn digests(dash: &Dash) -> Vec<String> {
    let text = std::fs::read_to_string(dash.dir.path().join("keys.toml")).unwrap();
    let quoted = |l: &str| l.strip_prefix("digest = \"")?.strip_suffix('"').map(str::to_owned);
    text.lines().filter_map(quoted).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_endpoint_is_what_check_says_and_a_served_page_never_calls_it_configured() {
    let d = Dash::dashboard().await;
    let check = d.view(ViewName::Check, json!({})).await;
    let html = d.ok("/endpoint").await;
    let text = text_of(&html);
    assert_eq!(check["endpoint_source"], "server", "the fixture serves");
    assert_shows(&html, &check["endpoint"], "endpoint");
    assert!(!text.contains("configured; no server running"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn each_key_card_shows_what_keys_list_shows() {
    let d = Dash::dashboard().await;
    let keys = d.view(ViewName::Keys, json!({})).await;
    let keys = keys.as_array().unwrap();
    let html = d.ok("/endpoint").await;
    let cards = key_cards(&html);
    assert!(!keys.is_empty(), "the fixture has keys");
    assert_eq!(cards.len(), keys.len(), "one card per key");
    for (card, k) in cards.iter().zip(keys) {
        let what = format!("key {}", k["id"]);
        assert_fields(card, k, &["id", "name", "key", "created"], &what);
        let text = text_of(card);
        match k["revoked"].as_str() {
            Some(_) => assert_shows(card, &k["revoked"], &what),
            None => assert!(!text.contains("revoked"), "{what} is not revoked: {text}"),
        }
        match k["harness"].as_str() {
            Some(tag) => {
                assert!(
                    card.contains(&format!("badge--primary\"><span class=\"badge__dot\"></span>{tag}<")),
                    "{what}: harness {tag}\n{card}"
                )
            }
            None => assert!(!card.contains("badge"), "{what} has no tag, so no badge: {card}"),
        }
        let behaviour = k["break"].as_str().unwrap_or("default");
        assert!(text.contains(&format!("break {behaviour}")), "{what}: break {behaviour}\n{text}");
        match k["last_used"].as_str() {
            Some(_) => assert_shows(card, &k["last_used"], &what),
            None => assert!(text.contains("last used never"), "{what} was never used: {text}"),
        }
        assert!(text.contains("Requests today"), "{what} has its slot");
    }
    assert!(keys.iter().any(|k| k["last_used"].is_string()), "a fixture key has records");
    assert!(keys.iter().any(|k| k["last_used"].is_null()), "a fixture key has none");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_empty_home_says_there_are_no_agent_keys_and_how_to_add_one() {
    let d = Dash::empty().await;
    let html = d.ok("/endpoint").await;
    let text = text_of(&html);
    assert!(text.contains("No agent keys."), "{text}");
    assert!(text.contains("Run nullrouter keys issue <name>"), "{text}");
    assert!(html.contains("<code class=\"code\">nullrouter keys issue &lt;name&gt;</code>"), "the command is code");
    assert!(key_cards(&html).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn neither_page_carries_a_full_key_or_the_token() {
    let d = Dash::dashboard().await;
    let digests = digests(&d);
    assert!(!digests.is_empty());
    for path in ["/endpoint", "/settings"] {
        let html = d.ok(path).await;
        assert!(!html.contains(TOKEN), "{path} shows the token");
        let token_digest = DashboardToken::digest_of(TOKEN);
        assert!(!html.contains(&token_digest), "{path} shows the token's digest");
        assert!(!html.contains("0r-"), "{path} shows a key");
        for digest in &digests {
            assert!(!html.contains(digest.as_str()), "{path} shows a key's digest");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_shows_behaviour_the_home_and_the_dashboard_status() {
    let d = Dash::dashboard().await;
    let html = d.ok("/settings").await;
    let text = text_of(&html);

    let behaviour = d.view(ViewName::Behaviour, json!({})).await;
    let settings = behaviour.as_object().unwrap();
    assert!(!settings.is_empty());
    for (name, v) in settings {
        let tag = if v["default"] == true { " (default)" } else { "" };
        let want = format!("{name} {}{tag}", v["value"].as_str().unwrap());
        assert!(text.contains(&want), "{want:?}\n{text}");
    }

    let check = d.view(ViewName::Check, json!({})).await;
    assert_shows(&html, &check["home"], "home");

    let status = d.view(ViewName::Dashboard, json!({})).await;
    let listen = status["listen"].as_str().unwrap();
    let want = match (status["server"].as_str().unwrap(), status["enabled"] == true, status["serving"] == true) {
        ("running", false, _) => "off (config.toml [dashboard] enabled = false)".to_owned(),
        ("running", true, true) => format!("on, listening on {listen}"),
        ("running", true, false) => format!("on, not listening: {}", status["error"].as_str().unwrap_or(listen)),
        (_, true, _) => format!("no server running; config.toml: on, {listen}"),
        (_, false, _) => "no server running; config.toml: off".to_owned(),
    };
    assert!(text.contains(&format!("Status {want}")), "{want:?}\n{text}");
    assert_shows(&html, &Value::String(listen.to_owned()), "listen");
    assert_eq!(status["token_issued"], "2026-10-06T08:00:00Z");
    assert_shows(&html, &status["token_issued"], "token issued");
    assert!(text.contains("Change it with nullrouter dashboard token"), "{text}");
}

// ---------------------------------------------------------------------------------------------
// The landscape against `latency --json` (spec 010 US3, T038)

/// The CLI's time words: under a second in ms, otherwise seconds with one decimal.
fn latency_ms(v: f64) -> String {
    if v < 1000.0 { format!("{} ms", v.round() as u64) } else { format!("{:.1} s", v / 1000.0) }
}

fn latency_pair(v: &Value) -> String {
    match (v["p50"].as_f64(), v["p95"].as_f64()) {
        (Some(a), Some(b)) => format!("{} / {}", latency_ms(a), latency_ms(b)),
        _ => "none".into(),
    }
}

/// The fixture home plus three requests a minute ago: two by `ak_fixture1` on `xai`, one by
/// `ak_fixture2` on `anthropic`, so the last 24 hours have traffic to draw.
fn home_with_traffic() -> tempfile::TempDir {
    let dir = nullrouter_engine::testkit::homes::dashboard();
    let now = jiff::Timestamp::now() - jiff::SignedDuration::from_secs(60);
    let at = now.to_string();
    let day = &at[..10];
    let mut text = String::new();
    for (n, (agent, provider, started, ttft)) in [
        ("ak_fixture1", "xai", 4.0, 300.0),
        ("ak_fixture1", "xai", 6.0, 1900.0),
        ("ak_fixture2", "anthropic", 5.0, 640.0),
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("rq_land{n}");
        for line in [
            json!({"v":1,"t":"open","id":id,"arrived":at,"agent":agent,"style":"anthropic-messages","op":"generate","type":"text","target":"t"}),
            json!({"v":1,"t":"attempt","id":id,"attempt":{"n":1,"provider":provider,"account":"main","model":"m","kind":"initial","started":started,"ended":2000.0,"outcome":{"state":"ok"},"dropped":[],"forced":[]}}),
            json!({"v":1,"t":"close","id":id,"outcome":"succeeded","served_by":{"provider":provider,"account":"main","model":"m"},"ttft_ms":ttft,"total_ms":2000.0,"usage":null,"break_handling":{"kind":"none"},"job":null}),
        ] {
            text += &(line.to_string() + "\n");
        }
    }
    let path = dir.path().join("records").join(format!("{day}.jsonl"));
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    std::fs::write(path, old + &text).unwrap();
    dir
}

#[tokio::test(flavor = "multi_thread")]
async fn the_landscape_prints_what_latency_prints() {
    let d = Dash::start(home_with_traffic()).await;
    let html = d.ok("/endpoint").await;
    let text = text_of(&html);
    let latency = d.view(ViewName::Latency, json!({})).await;
    assert!(text.contains("Agent traffic · last 24 h · as of "), "{text}");
    assert!(!text.contains("No requests in the last 24 hours"), "the home has traffic");

    let agents = latency["agents"].as_array().unwrap();
    let providers = latency["providers"].as_array().unwrap();
    assert!(agents.len() >= 2 && !providers.is_empty(), "{latency}");
    for a in agents {
        let what = format!("agent {}", a["id"]);
        assert!(
            text.contains(&latency_pair(&a["overhead"])),
            "{what}: overhead {}\n{text}",
            latency_pair(&a["overhead"])
        );
        assert!(text.contains(&latency_pair(&a["ttft"])), "{what}: ttft\n{text}");
        assert!(text.contains(&format!("{} req", a["requests"])), "{what}: requests\n{text}");
        assert!(text.contains(a["name"].as_str().or(a["id"].as_str()).unwrap()), "{what}: name");
    }
    for p in providers {
        let what = format!("provider {}", p["id"]);
        assert!(text.contains(p["id"].as_str().unwrap()), "{what}");
        assert!(text.contains(&latency_pair(&p["own_ttft"])), "{what}: own ttft\n{text}");
        assert!(text.contains(&format!("{} req", p["requests"])), "{what}: requests");
        assert!(
            text.contains(&format!("last response {}", p["last"]["result"].as_str().unwrap())),
            "{what}: last response\n{text}"
        );
        for per in p["agents"].as_array().unwrap() {
            let name = per["name"].as_str().or(per["id"].as_str()).unwrap();
            assert!(text.contains(&format!("{name} {}", per["requests"])), "{what}: {name} count\n{text}");
        }
    }
    assert!(!html.contains("<script") && !html.contains("<animate"), "no script or animation");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agents_colour_is_the_same_on_two_loads_and_after_a_key_is_appended() {
    let dir = home_with_traffic();
    let path = dir.path().join("keys.toml");
    let d = Dash::start(dir).await;
    let colours = |html: &str| -> Vec<String> {
        html.split("landscape__dot landscape__dot--")
            .skip(1)
            .map(|s| s.split('"').next().unwrap_or_default().to_owned())
            .collect()
    };
    let first = colours(&d.ok("/endpoint").await);
    assert!(first.len() >= 2, "{first:?}");
    assert_eq!(first, colours(&d.ok("/endpoint").await), "the same on a second load");

    // A key appended to the file takes the next colour and moves none of the others.
    let mut text = std::fs::read_to_string(&path).unwrap();
    text += "\n[[key]]\nid = \"ak_fixture9\"\nname = \"newest\"\ndigest = \"0000000000000000000000000000000000000000000000000000000000000009\"\nlast4 = \"N9N9\"\ncreated = \"2026-10-08T08:00:00Z\"\n";
    std::fs::write(&path, text).unwrap();
    let after = colours(&d.ok("/endpoint").await);
    assert_eq!(after[..first.len()], first[..], "{after:?} vs {first:?}");
    assert_eq!(after.len(), first.len() + 1);
}

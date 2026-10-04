//! The routing view over the operator socket (spec 006, US4, T051): per target and account the
//! pace, share, deficit, priority, cache lifetime in effect, each window's remaining amount, unit,
//! reset and reserve, the quota source with its last poll, the price now of pay-as-you-go rows,
//! and the journal's health. A priority set on disk applies at the next decision after a reload.

mod common;

use std::time::{Duration, SystemTime};

use common::server_custom;
use nullrouter_engine::testkit::{MockQuota, QuotaRoute, SimQuota, SimWindow};
use nullrouter_server::operator;
use serde_json::{Value, json};

const ALPHA: &str = r#"
[routing.cache]
mode = "automatic"
lifetime = "5m"
min_tokens = 0

[[routing.window]]
name = "5h"
length = "5h"
unit = "weighted_tokens"
capacity = 1000000
reserve = "10%"
"#;

const PAY: &str = r#"
[routing.cache]
mode = "automatic"
lifetime = "10m"
min_tokens = 0

[[routing.price]]
input = 3.0
output = 12.0
"#;

fn accounts(priority_a: f64) -> String {
    format!(
        "schema = 2\n\
         [[account]]\nprovider = \"alpha\"\nname = \"a\"\nsecret = \"sk-view-a\"\npriority = {priority_a:?}\n\
         [[account]]\nprovider = \"alpha\"\nname = \"b\"\nsecret = \"sk-view-b\"\n\
         [[account]]\nprovider = \"pay\"\nname = \"key\"\nsecret = \"sk-view-p\"\n"
    )
}

fn chat(mock: &nullrouter_engine::testkit::MockUpstream, id: &str, extra: &str) -> String {
    format!(
        "schema = 2\nid = \"{id}\"\ncategory = \"apikey\"\n[auth]\nkind = \"apikey\"\n[endpoints.text]\nurl = \"{}\"\nwire = \"openai-chat\"\n[[models]]\nid = \"m1\"\n{extra}",
        mock.url(&format!("/{id}/chat/completions"))
    )
}

fn row<'a>(view: &'a Value, account: &str) -> &'a Value {
    view["targets"][0]["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| format!("{}/{}", a["provider"].as_str().unwrap(), a["account"].as_str().unwrap()) == account)
        .unwrap_or_else(|| panic!("{account} missing from {view:#}"))
}

#[tokio::test]
async fn the_view_carries_every_number_the_operator_reads_and_a_priority_applies_after_a_reload() {
    let quota_mock = MockQuota::start().await;
    let quota = SimQuota::new();
    quota_mock.simulate(&quota);
    quota.account("sk-view-a", "alpha/a");
    quota.account("sk-view-b", "alpha/b");
    let reset = nullrouter_engine::clock::rfc3339(SystemTime::now() + Duration::from_secs(3600));
    // a has 80% left with 20% of its window to go; b has 40% left.
    quota.set("alpha/a", vec![SimWindow::new("5h", "tokens", 1_000_000.0, &reset).used(200_000.0)]);
    quota.set("alpha/b", vec![SimWindow::new("5h", "tokens", 1_000_000.0, &reset).used(600_000.0)]);
    let url = quota_mock.url(QuotaRoute::Sim);
    let s = server_custom(
        |m| {
            let alpha = format!("{}{ALPHA}\n{}", chat(m, "alpha", ""), SimQuota::quota_toml(&url, &[("5h", "tokens")]));
            vec![("alpha", alpha), ("pay", chat(m, "pay", PAY))]
        },
        &accounts(1.0),
        "[[unified_model]]\nname = \"u\"\nmembers = [{ provider = \"alpha\", model = \"m1\" }, { provider = \"pay\", model = \"m1\" }]\n",
    )
    .await;
    s.engine.poll_quota("alpha", "a").await.expect("polled");
    s.engine.poll_quota("alpha", "b").await.expect("polled");

    let view = operator::handle(&s.engine, &json!({"op": "routing.view"})).await;
    assert_eq!(view["ok"], true, "{view:#}");
    assert_eq!(view["targets"][0]["target"], "u");
    assert_eq!(view["amortization"]["length"], "5h");
    assert_eq!(view["journal"]["kept"], true);

    let a = row(&view, "alpha/a");
    assert_eq!(a["source"], "polled");
    assert_eq!(a["tier"], "subscription");
    assert_eq!(a["priority"], 1.0);
    assert_eq!(a["cache_lifetime"], "5m");
    assert!(a["polled_at"].is_string());
    assert!(a["pace"].as_f64().unwrap() > row(&view, "alpha/b")["pace"].as_f64().unwrap());
    assert!(a["share"].as_f64().unwrap() > row(&view, "alpha/b")["share"].as_f64().unwrap());
    assert_eq!(a["deficit"], 0);
    let w = &a["windows"][0];
    assert_eq!(w["name"], "5h");
    assert_eq!(w["unit"], "weighted_tokens");
    assert_eq!(w["capacity"], 1_000_000.0);
    assert_eq!(w["reserve"], 0.1);
    assert!((w["remaining_at_poll"].as_f64().unwrap() - 800_000.0).abs() < 1.0, "{w:#}");
    assert_eq!(w["remaining_now"], w["remaining_at_poll"]);
    assert_eq!(w["cost_since_poll"], 0.0, "nothing sent since the poll");
    assert!(w["resets_at"].is_string());

    // Traffic sent since the poll is counted: the view shows the poll's figure, the cost, and
    // what is left now (US7).
    let usage = nullrouter_engine::records::Usage {
        input: Some(1_000),
        output: Some(200),
        cache_read: None,
        cache_write: None,
        reasoning: None,
        input_semantics: nullrouter_registry::schema::InputSemantics::ExcludesCache,
        estimated: false,
    };
    s.engine.history.tally.attempt("alpha", "a", "m1", Some(&usage));
    let counted = operator::handle(&s.engine, &json!({"op": "routing.view"})).await;
    let w = &row(&counted, "alpha/a")["windows"][0];
    assert_eq!(w["cost_since_poll"], 1_200.0);
    assert_eq!(w["remaining_at_poll"], row(&view, "alpha/a")["windows"][0]["remaining_at_poll"]);
    assert!((w["remaining_now"].as_f64().unwrap() - (800_000.0 - 1_200.0)).abs() < 1.0, "{w:#}");
    s.engine.history.tally.take("alpha", "a");

    let p = row(&view, "pay/key");
    assert_eq!((p["source"].as_str(), p["tier"].as_str()), (Some("pay-as-you-go"), Some("payg")));
    assert_eq!(p["price_now"], 3.0);
    assert_eq!(p["cache_lifetime"], "10m");
    assert_eq!(p["share"], 1.0);

    // A target the operator names narrows the view; an unknown one has no accounts.
    let one = operator::handle(&s.engine, &json!({"op": "routing.view", "target": "u"})).await;
    assert_eq!(one["targets"].as_array().unwrap().len(), 1);
    let none = operator::handle(&s.engine, &json!({"op": "routing.view", "target": "nope"})).await;
    assert_eq!(none["targets"].as_array().unwrap().len(), 0);

    let health = operator::handle(&s.engine, &json!({"op": "routing.health"})).await;
    assert_eq!(health["ok"], true);
    assert_eq!(health["journal"]["kept"], true);
    assert_eq!(health["journal"]["unkept_requests"], 0);

    // Scenario 1: priority 3 on alpha/b, saved and reloaded, changes its share without a restart.
    let before = row(&view, "alpha/b")["share"].as_f64().unwrap();
    let text = accounts(1.0).replace("name = \"b\"\nsecret = \"sk-view-b\"\n", "name = \"b\"\nsecret = \"sk-view-b\"\npriority = 3.0\n");
    nullrouter_engine::files::write_private(&s.home().join(nullrouter_engine::accounts::FILE), &text).unwrap();
    let reloaded = operator::handle(&s.engine, &json!({"op": "reload"})).await;
    assert_eq!(reloaded["ok"], true, "{reloaded:#}");
    let after_view = operator::handle(&s.engine, &json!({"op": "routing.view"})).await;
    let b = row(&after_view, "alpha/b");
    assert_eq!(b["priority"], 3.0);
    assert!(b["share"].as_f64().unwrap() > before * 2.0, "{before} -> {}", b["share"]);
}

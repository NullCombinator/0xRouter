//! Spec 010 SC-002: the summaries over the hand-worked fixture home. Every figure in
//! `fixtures/summaries/expected.toml` was derived by hand, with the working beside it.

use std::path::{Path, PathBuf};

use nullrouter_engine::accounts::PriceOverride;
use nullrouter_engine::clock::parse_rfc3339;
use nullrouter_engine::journal::summary::{self, Totals, Window};
use nullrouter_engine::routing::price::PriceSpec;
use nullrouter_registry::schema::PriceDecl;
use toml::Value;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/summaries")
}

fn table(file: &str) -> toml::Table {
    std::fs::read_to_string(fixture().join(file)).unwrap().parse().unwrap()
}

fn rate(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(|x| x.as_float().or_else(|| x.as_integer().map(|i| i as f64)))
}

/// The price lookup the fixture describes: `None` for an account that is gone, an empty spec for
/// one with no price.
fn prices(provider: &str, account: Option<&str>) -> Option<PriceSpec> {
    let t = table("prices.toml");
    let gone = t["gone"].as_array().unwrap();
    if gone.iter().any(|g| g[0].as_str() == Some(provider) && Some(g[1].as_str().unwrap()) == account) {
        return None;
    }
    let acc = t["account"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["provider"].as_str() == Some(provider) && Some(a["account"].as_str().unwrap()) == account)?;
    let schedule: Vec<PriceDecl> = acc
        .get("price")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|p| p.clone().try_into().unwrap())
        .collect();
    let flat = acc.get("flat").map(|f| PriceOverride {
        input: rate(f, "input").unwrap(),
        output: rate(f, "output"),
        cache_read: rate(f, "cache_read"),
        cache_write: rate(f, "cache_write"),
    });
    Some(PriceSpec { schedule, flat })
}

fn window(t: &Value) -> Window {
    Window {
        from: t.get("from").map(|f| parse_rfc3339(f.as_str().unwrap()).unwrap()),
        to: parse_rfc3339(t["to"].as_str().unwrap()).unwrap(),
    }
}

fn counts(v: &Value) -> Vec<(String, u64)> {
    v.as_table().unwrap().iter().map(|(k, n)| (k.clone(), n.as_integer().unwrap() as u64)).collect()
}

fn check(name: &str, got: &Totals, want: &Value) {
    let n = |k: &str| want[k].as_integer().unwrap() as u64;
    assert_eq!(got.requests, n("requests"), "{name}: requests");
    assert_eq!(got.in_flight, n("in_flight"), "{name}: in flight");
    assert_eq!(got.not_reported, n("not_reported"), "{name}: not reported");
    assert_eq!(got.input, n("input"), "{name}: input");
    assert_eq!(got.cached, n("cached"), "{name}: cached");
    assert_eq!(got.output, n("output"), "{name}: output");
    let usd = rate(want, "cost_usd").unwrap();
    assert!((got.cost_usd - usd).abs() < 0.005, "{name}: cost {} vs {usd}", got.cost_usd);
    let u = &want["unpriced"];
    assert_eq!(
        (got.unpriced.no_price, got.unpriced.account_gone, got.unpriced.no_output_price),
        (n_of(u, "no_price"), n_of(u, "account_gone"), n_of(u, "no_output_price")),
        "{name}: unpriced"
    );
    let mut agents: Vec<_> = got.agents.iter().map(|(k, v)| (k.clone(), *v)).collect();
    agents.sort();
    let mut want_agents = counts(&want["agents"]);
    want_agents.sort();
    assert_eq!(agents, want_agents, "{name}: agents");
    let mut providers: Vec<_> = got.providers.iter().map(|(k, v)| (k.clone(), *v)).collect();
    providers.sort();
    let mut want_providers = counts(&want["providers"]);
    want_providers.sort();
    assert_eq!(providers, want_providers, "{name}: providers");
}

fn n_of(v: &Value, k: &str) -> u64 {
    v[k].as_integer().unwrap() as u64
}

#[test]
fn totals_match_the_hand_figures_for_every_window() {
    let expected = table("expected.toml");
    let windows = expected["totals"].as_array().unwrap();
    assert!(windows.len() >= 5);
    for want in windows {
        let name = want["name"].as_str().unwrap();
        let got = summary::totals(&fixture(), &window(want), &prices, true);
        check(name, &got, want);
    }
}

#[test]
fn the_cached_path_gives_the_same_totals_and_serves_finished_days() {
    let expected = table("expected.toml");
    for want in expected["totals"].as_array().unwrap() {
        let name = want["name"].as_str().unwrap();
        let cold = summary::totals_with(&fixture(), &window(want), &prices, Some(1), &[], true);
        check(name, &cold, want);
        // The second read is served from the cache for whole days and must not differ.
        let warm = summary::totals_with(&fixture(), &window(want), &prices, Some(1), &[], true);
        assert_eq!(cold, warm, "{name}: warm equals cold");
    }
}

#[test]
fn without_a_server_an_unfinished_request_was_cut_short() {
    let expected = table("expected.toml");
    let all = expected["totals"].as_array().unwrap().iter().find(|t| t["name"].as_str() == Some("all")).unwrap();
    let got = summary::totals(&fixture(), &window(all), &prices, false);
    assert_eq!((got.in_flight, got.not_reported), (0, 4), "fx15 is now not reported, not in flight");
}

fn pct_of(want: &Value) -> Option<(f64, f64, u64)> {
    let f = |k: &str| rate(want, k).unwrap();
    Some((f("p50"), f("p95"), want["n"].as_integer()? as u64))
}

fn got_pct(p: Option<summary::Pct>) -> Option<(f64, f64, u64)> {
    p.map(|p| (p.p50, p.p95, p.n))
}

fn last_of(want: &Value) -> (String, String, Option<u64>) {
    (
        want["result"].as_str().unwrap().to_owned(),
        want["at"].as_str().unwrap().to_owned(),
        want.get("status").map(|s| s.as_integer().unwrap() as u64),
    )
}

fn got_last(l: Option<&summary::Last>) -> (String, String, Option<u64>) {
    let l = l.expect("a last response");
    (l.result.to_owned(), l.at.clone(), l.status)
}

fn day_window() -> Window {
    let at = parse_rfc3339(table("expected.toml")["latency"]["at"].as_str().unwrap()).unwrap();
    Window { from: Some(at - std::time::Duration::from_secs(86_400)), to: at }
}

#[test]
fn latency_matches_the_hand_figures_per_agent_and_provider() {
    let expected = table("expected.toml");
    let want = &expected["latency"];
    let got = summary::latency(&fixture(), &day_window(), &[], true);

    let agents = want["agents"].as_table().unwrap();
    assert_eq!(got.agents.keys().collect::<Vec<_>>(), agents.keys().collect::<Vec<_>>(), "agents with a row");
    for (id, w) in agents {
        let g = &got.agents[id];
        assert_eq!(g.requests, w["requests"].as_integer().unwrap() as u64, "{id}: requests");
        assert_eq!(got_pct(g.overhead), pct_of(&w["overhead"]), "{id}: overhead");
        assert_eq!(got_pct(g.ttft), pct_of(&w["ttft"]), "{id}: ttft");
        assert_eq!(got_last(g.last.as_ref()), last_of(&w["last"]), "{id}: last");
    }

    let providers = want["providers"].as_table().unwrap();
    assert_eq!(got.providers.keys().collect::<Vec<_>>(), {
        let mut k: Vec<_> = providers.keys().collect();
        k.sort();
        k
    });
    for (id, w) in providers {
        let g = &got.providers[id];
        assert_eq!(g.requests, w["requests"].as_integer().unwrap() as u64, "{id}: requests");
        assert_eq!(got_pct(g.own_ttft), pct_of(&w["own_ttft"]), "{id}: own ttft");
        let mut per_agent: Vec<_> = g.agents.iter().map(|(k, v)| (k.clone(), *v)).collect();
        per_agent.sort();
        let mut want_agents = counts(&w["agents"]);
        want_agents.sort();
        assert_eq!(per_agent, want_agents, "{id}: per agent");
        assert_eq!(got_last(g.last.as_ref()), last_of(&w["last"]), "{id}: last");
    }
}

#[test]
fn a_request_refused_before_a_key_matched_is_in_no_row() {
    let got = summary::latency(&fixture(), &day_window(), &[], true);
    // fx12 has no agent and no attempts: the agents' requests add up to 11 of the window's 12.
    assert_eq!(got.agents.values().map(|a| a.requests).sum::<u64>(), 11);
}

#[test]
fn a_row_with_no_values_has_none_not_zero_and_old_records_are_left_out() {
    let w = day_window();
    let got = summary::latency(&fixture(), &w, &[], true);
    // fx14 and fx15 have no first token, so alice's and bob's ttft counts are one short of their requests.
    assert_eq!(got.agents["ak_alice"].ttft.unwrap().n, 4);
    assert_eq!(got.agents["ak_bob"].ttft.unwrap().n, 4);
    // A window with no records gives no rows at all.
    let quiet = Window {
        from: Some(parse_rfc3339("2026-09-01T00:00:00Z").unwrap()),
        to: parse_rfc3339("2026-09-02T00:00:00Z").unwrap(),
    };
    let none = summary::latency(&fixture(), &quiet, &[], true);
    assert!(none.agents.is_empty() && none.providers.is_empty());
    // Only fx01..fx03, older than 24 h, fall before the window: a window ending at 10-06 12:00 sees them.
    let older = Window { from: None, to: parse_rfc3339("2026-10-06T12:00:00Z").unwrap() };
    let got = summary::latency(&fixture(), &older, &[], true);
    assert_eq!(got.agents.values().map(|a| a.requests).sum::<u64>(), 3);
    assert!(got.agents["ak_alice"].last.is_some());
}

#[test]
fn without_a_server_an_unfinished_request_is_a_failed_last_response() {
    let w = Window {
        from: Some(parse_rfc3339("2026-10-07T11:00:00Z").unwrap()),
        to: parse_rfc3339("2026-10-07T12:00:00Z").unwrap(),
    };
    let with = summary::latency(&fixture(), &w, &[], true);
    let without = summary::latency(&fixture(), &w, &[], false);
    // fx15 (bob, unfinished, 11:55) is in flight with a server and cut short without one.
    assert!(with.agents["ak_bob"].last.is_none());
    assert_eq!(without.agents["ak_bob"].last.as_ref().map(|l| l.result), Some("failed"));
}

//! The quota fit's unit and integration checks (spec 012): rows, the model, classification,
//! separability, the meter in effect and its FR-011 invariant, stores and epochs.
//!
//! Covers SC-002, SC-003, SC-008 and SC-010 together with `sim_week` and `sim_suite`.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nullrouter_engine::quota::QuotaWindow;
use nullrouter_engine::quota::fit::TokenClass;
use nullrouter_engine::quota::fit::rows::{Class, Group, SetAside, rows_from};
use nullrouter_engine::quota::history::{Entry, VERSION};
use nullrouter_engine::quota::tally::ModelTally;
use nullrouter_registry::schema::{MeterDecl, QuotaUnit};

fn t(min: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_800_000_000 + min * 60)
}

fn meter(extra: &str) -> MeterDecl {
    let toml = format!("name = \"5-hour\"\nlength = \"5h\"\nunit = \"weighted_tokens\"\ncapacity = 1000000\n{extra}\n");
    toml::from_str(&toml).unwrap_or_else(|e| panic!("{e}\n{toml}"))
}

fn window(used: f64, resets_min: u64) -> QuotaWindow {
    QuotaWindow {
        name: "5-hour".into(),
        unit: QuotaUnit::Percent,
        used: Some(used),
        limit: Some(100.0),
        remaining: Some(100.0 - used),
        resets_at: Some(t(resets_min)),
    }
}

fn model(requests: u64, unreported: u64, input: u64, output: u64) -> ModelTally {
    ModelTally { requests, requests_usage_unreported: unreported, input, output, cache_read: 0, cache_write: 0 }
}

fn entry(min: u64, ok: bool, w: Option<QuotaWindow>, tally: &[(&str, ModelTally)]) -> Entry {
    Entry {
        v: VERSION,
        at: nullrouter_engine::quota::extract::rfc3339_millis(t(min)),
        ok,
        error: None,
        windows: w.into_iter().collect(),
        tally: tally.iter().map(|(m, t)| ((*m).to_owned(), *t)).collect::<BTreeMap<_, _>>(),
    }
}

#[test]
fn a_failed_poll_is_bridged_with_the_tallies_summed() {
    let h = vec![
        entry(0, true, Some(window(10.0, 300)), &[]),
        entry(10, true, Some(window(12.0, 300)), &[("m", model(1, 0, 1000, 100))]),
        entry(20, false, None, &[("m", model(2, 0, 2000, 200))]),
        entry(30, true, Some(window(17.0, 300)), &[("m", model(1, 0, 500, 50))]),
    ];
    let rows = rows_from("a", &h, &meter(""), t(0));
    assert_eq!(rows.len(), 2);
    assert_eq!((rows[0].y, rows[0].class), (2.0, Class::Evidence));
    // The bridged interval runs from minute 10 to minute 30 and sums both tallies.
    assert_eq!((rows[1].start, rows[1].end, rows[1].y), (t(10), t(30), 5.0));
    assert_eq!(rows[1].requests, 3);
    assert_eq!(rows[1].x[&(Group::Plain, TokenClass::Input)], 2500);
    assert_eq!(rows[1].x[&(Group::Plain, TokenClass::Output)], 250);
    assert!((rows[1].hours - 20.0 / 60.0).abs() < 1e-12);
}

#[test]
fn a_reset_is_set_aside() {
    let reset_by_time = vec![entry(0, true, Some(window(40.0, 300)), &[]), entry(10, true, Some(window(41.0, 600)), &[])];
    assert_eq!(rows_from("a", &reset_by_time, &meter(""), t(0))[0].class, Class::SetAside(SetAside::Reset));
    let reset_by_fall = vec![entry(0, true, Some(window(40.0, 300)), &[]), entry(10, true, Some(window(3.0, 300)), &[])];
    assert_eq!(rows_from("a", &reset_by_fall, &meter(""), t(0))[0].class, Class::SetAside(SetAside::Reset));
}

#[test]
fn unreported_usage_and_exhaustion_are_set_aside() {
    let unreported = vec![
        entry(0, true, Some(window(10.0, 300)), &[]),
        entry(10, true, Some(window(11.0, 300)), &[("m", model(3, 1, 100, 10))]),
    ];
    assert_eq!(rows_from("a", &unreported, &meter(""), t(0))[0].class, Class::SetAside(SetAside::UsageUnreported));
    for (a, b) in [(100.0, 100.0), (99.0, 100.0), (100.0, 100.0)] {
        let full = vec![entry(0, true, Some(window(a, 300)), &[]), entry(10, true, Some(window(b, 300)), &[])];
        assert_eq!(rows_from("a", &full, &meter(""), t(0))[0].class, Class::SetAside(SetAside::Exhausted), "{a}→{b}");
    }
}

#[test]
fn tokens_are_grouped_by_the_first_matching_multiplier_glob() {
    let m = meter("model_multiplier = { \"big-*\" = 2.0, \"*\" = 1.5 }");
    let h = vec![
        entry(0, true, Some(window(10.0, 300)), &[]),
        entry(10, true, Some(window(15.0, 300)), &[("big-1", model(1, 0, 100, 0)), ("small", model(1, 0, 40, 0))]),
    ];
    let r = &rows_from("a", &h, &m, t(0))[0];
    assert_eq!(r.x[&(Group::Glob("big-*".into()), TokenClass::Input)], 100);
    assert_eq!(r.x[&(Group::Glob("*".into()), TokenClass::Input)], 40);
    // With no globs declared, every model is in the plain group.
    let plain = &rows_from("a", &h, &meter(""), t(0))[0];
    assert_eq!(plain.x[&(Group::Plain, TokenClass::Input)], 140);
}

#[test]
fn an_interval_that_starts_before_the_epoch_is_left_out() {
    let h = vec![
        entry(0, true, Some(window(10.0, 300)), &[]),
        entry(10, true, Some(window(11.0, 300)), &[]),
        entry(20, true, Some(window(12.0, 300)), &[]),
    ];
    let rows = rows_from("a", &h, &meter(""), t(10));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].start, t(10));
}

#[test]
fn remaining_stands_in_for_used() {
    let mut a = window(0.0, 300);
    a.used = None;
    a.remaining = Some(90.0);
    let mut b = window(0.0, 300);
    b.used = None;
    b.remaining = Some(87.0);
    let h = vec![entry(0, true, Some(a), &[]), entry(10, true, Some(b), &[])];
    assert_eq!(rows_from("a", &h, &meter(""), t(0))[0].y, 3.0);
}

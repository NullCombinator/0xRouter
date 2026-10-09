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

// ---- the model (T006) ----

use nullrouter_engine::quota::fit::model::{self, Kind, P, Spec, Theta};
use nullrouter_engine::quota::fit::rows::Row;

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 11) as f64) / (1u64 << 53) as f64
    }
}

fn spec1() -> Spec {
    Spec { kind: Kind::Percent, accounts: vec!["a".into()], globs: vec![], utc_offset_secs: 0 }
}

const TRUE_K: f64 = 4.0e-6;
const TRUE_RHO: [f64; 3] = [15.0, 0.3, 3.75];
const TRUE_B: f64 = 0.02;

/// `n` contiguous 10-minute rows. The true level is continuous; readings are rounded to whole
/// steps when `round` is set, so each row's `y` is a difference of two rounded readings.
fn synthetic(n: usize, round: bool, seed: u64) -> Vec<Row> {
    let mut rng = Lcg(seed);
    let mut level = rng.next() * 100.0;
    let mut reading = if round { level.round() } else { level };
    let mut rows = Vec::with_capacity(n);
    for i in 0..n {
        let tok = |max: f64, rng: &mut Lcg| (rng.next() * max) as u64;
        let (input, output, cr, cw) = (tok(40_000.0, &mut rng), tok(8_000.0, &mut rng), tok(200_000.0, &mut rng), tok(20_000.0, &mut rng));
        let cost = input as f64 + TRUE_RHO[0] * output as f64 + TRUE_RHO[1] * cr as f64 + TRUE_RHO[2] * cw as f64;
        level += TRUE_K * cost + TRUE_B * (10.0 / 60.0);
        let next = if round { level.round() } else { level };
        let mut x = BTreeMap::new();
        for (c, v) in [(TokenClass::Input, input), (TokenClass::Output, output), (TokenClass::CacheRead, cr), (TokenClass::CacheWrite, cw)] {
            x.insert((Group::Plain, c), v);
        }
        rows.push(Row {
            account: "a".into(),
            window: "w".into(),
            start: t(i as u64 * 10),
            end: t((i as u64 + 1) * 10),
            y: next - reading,
            x,
            requests: 1,
            hours: 10.0 / 60.0,
            class: Class::Evidence,
        });
        reading = next;
    }
    rows
}

fn start_theta() -> Theta {
    let mut th = Theta::neutral(1, 0);
    th.k[0] = TRUE_K * 2.0;
    th.rho = [5.0, 0.1, 1.25];
    th
}

#[test]
fn gauss_newton_recovers_noiseless_parameters() {
    let spec = spec1();
    let rows = model::prepare(&spec, &synthetic(600, false, 7));
    let fit = model::fit(&spec, &rows, &start_theta()).expect("fit");
    let rel = |got: f64, want: f64| ((got - want) / want).abs();
    assert!(rel(fit.theta.k[0], TRUE_K) < 1e-6, "k {}", fit.theta.k[0]);
    for (c, want) in TRUE_RHO.iter().enumerate() {
        assert!(rel(fit.theta.rho[c], *want) < 1e-6, "rho[{c}] {}", fit.theta.rho[c]);
    }
    // The steady rate is spread over the parts of the day the rows cover.
    for q in 0..6 {
        let b = fit.theta.b[0][q];
        assert!((b - TRUE_B).abs() < 1e-6 || !fit.active.contains(&P::B(0, q)), "b[{q}] {b}");
    }
    assert!(fit.converged);
}

#[test]
fn the_95_percent_range_covers_the_truth_under_whole_step_rounding() {
    let spec = spec1();
    let truth: [(P, f64); 4] = [(P::K(0), TRUE_K), (P::Rho(0), TRUE_RHO[0]), (P::Rho(1), TRUE_RHO[1]), (P::Rho(2), TRUE_RHO[2])];
    let mut hits = [0u32; 4];
    const REPS: u32 = 500;
    for seed in 0..REPS {
        let rows = model::prepare(&spec, &synthetic(2_000, true, 0x012_0000 + u64::from(seed)));
        let fit = model::fit(&spec, &rows, &start_theta()).expect("fit");
        for (i, (p, want)) in truth.iter().enumerate() {
            let (_, lo, hi) = fit.range(*p).expect("range");
            if lo <= *want && *want <= hi {
                hits[i] += 1;
            }
        }
    }
    let cover: Vec<f64> = hits.iter().map(|h| f64::from(*h) / f64::from(REPS)).collect();
    for ((p, _), c) in truth.iter().zip(&cover) {
        println!("coverage {p:?}: {c:.3}");
    }
    assert!(cover.iter().all(|c| *c >= 0.93), "coverage per number: {cover:?}");
}

#[test]
fn parameters_no_row_informs_are_left_out() {
    // No cache writes at all: ρ_w has no column, and the fit still works.
    let spec = spec1();
    let mut rows = synthetic(300, false, 3);
    for r in &mut rows {
        r.x.insert((Group::Plain, TokenClass::CacheWrite), 0);
        r.y -= TRUE_K * TRUE_RHO[2] * 0.0;
    }
    let prepared = model::prepare(&spec, &rows);
    let fit = model::fit(&spec, &prepared, &start_theta());
    // The rows were generated with cache writes; zeroing x leaves a misfit but must not panic.
    assert!(fit.is_none_or(|f| !f.active.contains(&P::Rho(2))));
}

// ---- the meter in effect (T008, FR-011, FR-012) ----

use nullrouter_engine::quota::fit::{MeterNumber, NumberOverrides, Source, WindowFit, in_effect};

/// Every `[[routing.window]]` of every bundled plugin.
fn bundled_meters() -> Vec<MeterDecl> {
    let mut out = Vec::new();
    for (name, source) in nullrouter_registry::bundled_sources() {
        let v: toml::Value = toml::from_str(source).unwrap_or_else(|e| panic!("{name}: {e}"));
        let windows = v.get("routing").and_then(|r| r.get("window")).and_then(|w| w.as_array());
        for w in windows.into_iter().flatten() {
            out.push(w.clone().try_into().unwrap_or_else(|e| panic!("{name}: {e}")));
        }
    }
    out
}

fn weighted() -> MeterDecl {
    meter("token_weights = { input = 1.0, output = 5.0, cache_read = 0.1, cache_write = 1.25 }\nmodel_multiplier = { \"big-*\" = 2.0, \"*\" = 1.5 }")
}

#[test]
fn nothing_to_apply_leaves_the_declared_meter_unchanged() {
    let meters = bundled_meters();
    assert!(!meters.is_empty(), "no bundled plugin declares a meter");
    for m in meters {
        let (got, sources) = in_effect(&m, None, None, &WindowFit::default(), "acct");
        assert_eq!(got, m, "{}", m.name);
        assert!(sources.is_empty());
        // Empty override sets change nothing either.
        let empty = NumberOverrides::default();
        let (got, sources) = in_effect(&m, Some(&empty), Some(&empty), &WindowFit::default(), "acct");
        assert_eq!(got, m);
        assert!(sources.is_empty());
    }
}

#[test]
fn an_account_override_changes_only_its_field() {
    let m = weighted();
    let mut o = NumberOverrides::default();
    o.weights[1] = Some(15.0);
    let (got, sources) = in_effect(&m, None, Some(&o), &WindowFit::default(), "acct");
    let w = got.token_weights.expect("weights");
    assert_eq!((w.input, w.output, w.cache_read, w.cache_write), (1.0, 15.0, 0.1, 1.25));
    assert_eq!(got.capacity, m.capacity);
    assert_eq!(got.model_multiplier, m.model_multiplier);
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[&MeterNumber::Weight(TokenClass::Output)], Source::AccountOverride);
}

#[test]
fn account_beats_plugin_beats_fit_beats_declaration() {
    let m = weighted();
    let mut fit = WindowFit::default();
    fit.weights.insert(TokenClass::Output, 12.0);
    fit.multipliers.insert("big-*".into(), 3.0);
    fit.capacity.insert("acct".into(), 700_000.0);

    let (got, src) = in_effect(&m, None, None, &fit, "acct");
    assert_eq!(got.token_weights.unwrap().output, 12.0);
    assert_eq!(got.model_multiplier["big-*"], 3.0);
    assert_eq!(got.model_multiplier["*"], 1.5, "an unfitted glob stays declared");
    assert_eq!(got.capacity, Some(700_000.0));
    assert!(src.values().all(|s| *s == Source::Fit));

    let mut plugin = NumberOverrides::default();
    plugin.weights[1] = Some(9.0);
    plugin.multipliers.insert("big-*".into(), 4.0);
    plugin.capacity = Some(1.0); // a plugin level never sets capacity (clarify Q5)
    let (got, src) = in_effect(&m, Some(&plugin), None, &fit, "acct");
    assert_eq!(got.token_weights.unwrap().output, 9.0);
    assert_eq!(got.model_multiplier["big-*"], 4.0);
    assert_eq!(got.capacity, Some(700_000.0));
    assert_eq!(src[&MeterNumber::Weight(TokenClass::Output)], Source::PluginOverride);
    assert_eq!(src[&MeterNumber::Capacity], Source::Fit);

    let mut account = NumberOverrides::default();
    account.weights[1] = Some(15.0);
    account.capacity = Some(2_000_000.0);
    let (got, src) = in_effect(&m, Some(&plugin), Some(&account), &fit, "acct");
    assert_eq!(got.token_weights.unwrap().output, 15.0);
    assert_eq!(got.model_multiplier["big-*"], 4.0, "the account set no multiplier");
    assert_eq!(got.capacity, Some(2_000_000.0));
    assert_eq!(src[&MeterNumber::Weight(TokenClass::Output)], Source::AccountOverride);
    assert_eq!(src[&MeterNumber::Capacity], Source::AccountOverride);
    // The glob order the plugin declared is kept.
    assert_eq!(got.model_multiplier.keys().collect::<Vec<_>>(), m.model_multiplier.keys().collect::<Vec<_>>());
}

#[test]
fn fitted_ratios_follow_the_yardstick() {
    let m = weighted();
    let mut fit = WindowFit { relative_to_input: true, ..WindowFit::default() };
    fit.weights.insert(TokenClass::Output, 15.0);
    // Declared input weight 1: ratio 15 is weight 15.
    assert_eq!(in_effect(&m, None, None, &fit, "a").0.token_weights.unwrap().output, 15.0);
    // Overriding the yardstick rescales the fitted ratio with it.
    let mut o = NumberOverrides::default();
    o.weights[0] = Some(2.0);
    let (got, src) = in_effect(&m, None, Some(&o), &fit, "a");
    let w = got.token_weights.unwrap();
    assert_eq!((w.input, w.output), (2.0, 30.0));
    assert_eq!(src[&MeterNumber::Weight(TokenClass::Input)], Source::AccountOverride);
    // The input weight is never taken from the fit.
    fit.weights.insert(TokenClass::Input, 9.0);
    assert_eq!(in_effect(&m, None, None, &fit, "a").0.token_weights.unwrap().input, 1.0);
}

// ---- the learner (T027, T028) ----

use nullrouter_engine::quota::fit::learner::Learner;
use nullrouter_engine::quota::fit::store::{self, Loaded};
use nullrouter_engine::quota::fit::{Fits, Meters, NumberState};
use nullrouter_engine::quota::history;
use nullrouter_engine::state::Engine;
use nullrouter_registry::OperatorHome;

/// A home with provider `keyco` (one `5-hour` meter declared at 1,000,000 weighted tokens,
/// reported in percent) and two accounts, `main` and `spare`.
fn keyco_home() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), "allow_private_endpoints = true\n").unwrap();
    std::fs::create_dir(dir.path().join("plugins")).unwrap();
    let plugin = r#"schema = 2
id = "keyco"
category = "apikey"
[auth]
kind = "apikey"
[endpoints.text]
url = "http://127.0.0.1:9/keyco/chat/completions"
wire = "openai-chat"
[quota]
accounts = "key"
request = { url = "http://127.0.0.1:9/keyco/usage" }
[[quota.window]]
path = "usage.rolling"
name = "5-hour"
unit = "percent"
used = "percent"
[[routing.window]]
name = "5-hour"
length = "5h"
unit = "weighted_tokens"
capacity = 1000000
[[models]]
id = "m1"
"#;
    std::fs::write(dir.path().join("plugins/keyco.toml"), plugin).unwrap();
    let mut accounts = String::from("schema = 2\n");
    for (order, name) in ["main", "spare"].iter().enumerate() {
        accounts += &format!(
            "[[account]]\nprovider = \"keyco\"\nname = \"{name}\"\nsecret = \"sk-{name}\"\norder = {order}\n"
        );
    }
    nullrouter_engine::files::write_private(&dir.path().join(nullrouter_engine::accounts::FILE), &accounts).unwrap();
    nullrouter_engine::files::write_private(&nullrouter_engine::tokens::path(dir.path()), "schema = 1\n").unwrap();
    dir
}

/// `n + 1` good polls ten minutes apart. The true capacity is a third of the declared one
/// (333,333 input tokens per 100 percent), readings are rounded to whole percent points, and
/// only input tokens are sent.
fn third_capacity_history(n: usize) -> Vec<Entry> {
    let mut rng = Lcg(0x012_7027);
    let k = 100.0 / 333_333.0;
    let mut level = 5.3f64;
    let mut out = vec![entry(0, true, Some(window(level.round(), 3000)), &[])];
    for i in 1..=n {
        let input = 800 + (rng.next() * 3600.0) as u64;
        level += k * input as f64;
        out.push(entry(10 * i as u64, true, Some(window(level.round(), 3000)), &[("m1", model(1, 0, input, 0))]));
    }
    out
}

#[tokio::test]
async fn a_factor_three_capacity_becomes_fitted_and_reaches_the_meters() {
    let dir = keyco_home();
    let (engine, _) = Engine::open_parity(OperatorHome::new(dir.path())).unwrap();
    let st = engine.snapshot();
    let entries = third_capacity_history(100);

    let mut learner = Learner::default();
    let mut fits = Fits::default();
    // Two rows are fewer than the parameters: nothing is published (FR-011).
    assert!(!fits.observe(&mut learner, dir.path(), &st, "keyco", "main", &entries[..3], t(20)));
    assert!(fits.windows.is_empty());

    let mut first_change = None;
    for n in 4..=entries.len() {
        let now = t(10 * (n as u64 - 1));
        if fits.observe(&mut learner, dir.path(), &st, "keyco", "main", &entries[..n], now) && first_change.is_none() {
            first_change = Some(n);
            let states = learner.number_states("keyco", "5-hour");
            assert!(
                matches!(states["capacity@main"], NumberState::Fitted { .. }),
                "first publish must be the capacity becoming Fitted: {states:?}"
            );
            assert!(fits.window("keyco", "5-hour").capacity.contains_key("main"));
        }
    }
    assert!(first_change.is_some_and(|n| n < entries.len()), "capacity never became significant");
    let c = fits.window("keyco", "5-hour").capacity["main"];
    assert!((c / 333_333.0 - 1.0).abs() < 0.15, "fitted capacity {c}");
    assert!(fits.window("keyco", "5-hour").capacity.get("spare").is_none(), "spare has no evidence");
    let states = learner.number_states("keyco", "5-hour");
    assert!(matches!(states["capacity@main"], NumberState::Fitted { .. }), "{states:?}");

    // The meter in effect carries it; an account with nothing replaced has no entry (FR-011).
    let meters = Meters::default();
    meters.rebuild(&st, &fits);
    let m = meters.get("keyco", "main").expect("main's meter is replaced");
    assert_eq!(m.windows[0].capacity, Some(c));
    assert_eq!(m.sources["5-hour"][&nullrouter_engine::quota::fit::MeterNumber::Capacity], nullrouter_engine::quota::fit::Source::Fit);
    assert!(meters.get("keyco", "spare").is_none());

    // The state was saved, and a restart rebuilds the same fit by replaying the history.
    let Loaded::Ok(saved) = store::load(dir.path(), "keyco").unwrap() else { panic!("fit state not saved") };
    assert_eq!(saved.windows["5-hour"].numbers["capacity@main"].state, "fitted");
    for e in &entries {
        history::append(dir.path(), "keyco", "main", e).unwrap();
    }
    let mut again = Learner::default();
    let since = again.since(dir.path(), "keyco", "main");
    assert_eq!(since, Some(t(0)));
    let tail = history::read(dir.path(), "keyco", "main", since, None).unwrap();
    let mut replayed = Fits::default();
    assert!(replayed.observe(&mut again, dir.path(), &st, "keyco", "main", &tail, t(1000)));
    let c2 = replayed.window("keyco", "5-hour").capacity["main"];
    assert!((c2 / 333_333.0 - 1.0).abs() < 0.15, "replayed capacity {c2}");
    assert_eq!(again.number_states("keyco", "5-hour")["capacity@main"], states["capacity@main"]);
}

//! The right-plugin suite (spec 012, SC-002, SC-003): 100 fixed seeds of right-plugin weeks, with
//! and without outside use. Every test here is `#[ignore]`; CI runs them in release with
//! `--ignored`. A failing seed is investigated, never replaced (research R5): the seeds are a
//! fixed list derived from `SEED`, and a failure prints `seed=… number=… what=…` for each of them.
//!
//! The sim harness is the one of `sim_week.rs`; the few helpers that file keeps inside itself are
//! copied here in the smallest form this suite needs.
#![allow(dead_code)]

#[path = "sim_week/fit.rs"]
mod fit;
#[path = "sim_week/router.rs"]
mod router;
#[path = "sim_week/world.rs"]
mod world;

use std::time::Duration;

use nullrouter_engine::quota::fit::test::{ALPHA, simulated_null_rate};
use nullrouter_engine::quota::fit::NumberState;
use nullrouter_engine::routing::meter::cost_spent;
use nullrouter_engine::routing::{CandidateKey, PriceSpec};
use nullrouter_registry::schema::{MeterDecl, RoutingDecl};

use fit::FitRun;
use router::{AccountDef, Defs, POLL_MS, Placed, Sim};
use world::{DAY, HOUR, MIN, OutsideUse, Req, Reset, Rounding, TrueAccount, TrueWindow, WEEK_MS, World, plan};

const SEED: u64 = 0x006_0000_5EED;
const SEEDS: u64 = 100;
const PROVIDER: &str = "fitted";
const ACCOUNT: &str = "acct";
const WINDOW: &str = "5-hour";
const WEIGHTS: &str = "token_weights = { input = 1.0, output = 5.0, cache_read = 0.1, cache_write = 1.25 }";
/// `(output, cache_read, cache_write)` ratios to input, as the fit names them.
const RATIOS: [f64; 3] = [5.0, 0.1, 1.25];
const PEAK_SHARE: f64 = 0.55;

fn decl(toml: &str) -> RoutingDecl {
    toml::from_str(toml).unwrap_or_else(|e| panic!("{e}\n{toml}"))
}

fn five_hour(capacity: f64) -> MeterDecl {
    let toml = format!(
        "[[window]]\nname = \"{WINDOW}\"\nlength = \"5h\"\nunit = \"weighted_tokens\"\ncapacity = {capacity}\n{WEIGHTS}\nreserve = \"10%\"\n"
    );
    decl(&toml).window.remove(0)
}

/// The most the week's traffic costs a 5-hour stretch of one account that never refuses it.
fn peak_five_hours(reqs: &[Req]) -> f64 {
    let meter = five_hour(1.0e12);
    let mut world = World::new(vec![TrueAccount { windows: vec![TrueWindow::new(&meter, Reset::FirstUse, false, 0.0)] }]);
    let mut costs: Vec<(u64, f64)> = Vec::with_capacity(reqs.len());
    let mut swept = 0;
    for r in reqs {
        if r.at_ms / HOUR != swept {
            swept = r.at_ms / HOUR;
            world.sweep(r.at_ms);
        }
        let usage = world.serve(0, r, r.at_ms).expect("an account with no limit serves");
        costs.push((r.at_ms, cost_spent(&meter, &usage.spent())));
    }
    let (mut lo, mut sum, mut peak) = (0, 0.0, 0.0f64);
    for hi in 0..costs.len() {
        sum += costs[hi].1;
        while costs[hi].0 - costs[lo].0 >= 5 * HOUR {
            sum -= costs[lo].1;
            lo += 1;
        }
        peak = peak.max(sum);
    }
    peak
}

/// The router's definitions and the provider's truth for a plugin that declares what the provider
/// does: one subscription account over two pay-as-you-go keys that catch overflow.
fn week(capacity: f64) -> (Defs, World) {
    let payg = |key: &str, order: i64, toml: &str| {
        let (provider, account) = key.split_once('/').expect("provider/account");
        AccountDef {
            key: CandidateKey::new(provider, account, "m"),
            order,
            priority: 1.0,
            meters: Vec::new(),
            reported: false,
            price: PriceSpec { schedule: decl(toml).price, flat: None },
        }
    };
    let defs = Defs {
        accounts: vec![
            AccountDef {
                key: CandidateKey::new(PROVIDER, ACCOUNT, "m"),
                order: 0,
                priority: 1.0,
                meters: vec![five_hour(capacity)],
                reported: true,
                price: PriceSpec::default(),
            },
            payg("openrouter/key", 1, "[[price]]\ninput = 1.5\n"),
            payg("deepseek/key", 2, "[[price]]\ninput = 1.6\n"),
        ],
        target: "sonnet".into(),
        amortization: Duration::from_secs(5 * 3600),
    };
    let mut world = World::new(vec![
        TrueAccount { windows: vec![TrueWindow::new(&five_hour(capacity), Reset::FirstUse, true, 0.0)] },
        TrueAccount::default(),
        TrueAccount::default(),
    ]);
    world.rounding = Some(Rounding::HalfUp);
    (defs, world)
}

struct Week {
    placed: Vec<Placed>,
    fit: Option<FitRun>,
}

fn run(capacity: f64, reqs: &[Req], outside: &[OutsideUse], fitting: bool) -> Week {
    let (defs, mut world) = week(capacity);
    world.outside = outside.to_vec();
    let dir = tempfile::tempdir().unwrap();
    let mut sim = Sim::new(&defs, dir.path());
    if fitting {
        sim = sim.with_fit(FitRun::new(defs.accounts.len()));
    }
    for r in reqs {
        sim.handle(&mut world, r);
    }
    sim.journal.flush_blocking();
    assert!(sim.placed.iter().all(|p| p.served.is_some()), "every request was served");
    Week { placed: std::mem::take(&mut sim.placed), fit: sim.fit.take() }
}

fn drop_at(at_ms: u64, pct: f64, busy: bool) -> OutsideUse {
    OutsideUse::Drop { account: 0, window: WINDOW.into(), at_ms, pct, busy }
}

fn rate(pct_per_hour: f64, office: bool) -> OutsideUse {
    OutsideUse::Rate { account: 0, window: WINDOW.into(), pct_per_hour, office }
}

/// The poll instants (from day 2 on) whose ten minutes saw no request at all.
fn idle_polls(reqs: &[Req]) -> Vec<u64> {
    (DAY / POLL_MS + 1..WEEK_MS / POLL_MS - 1)
        .map(|k| k * POLL_MS)
        .filter(|p| {
            let from = reqs.partition_point(|r| r.at_ms <= p - POLL_MS - 1_000);
            reqs.get(from).is_none_or(|r| r.at_ms > p + 1_000)
        })
        .collect()
}

/// `n` of `all`, spread over the week.
fn spread(all: &[u64], n: usize) -> Vec<u64> {
    if all.len() <= n {
        return all.to_vec();
    }
    (0..n).map(|i| all[i * all.len() / n + all.len() / (2 * n)]).collect()
}

/// The outside use of seed index `i`: every 4th an office-hours rate, the others alternately a
/// flat rate (its size varying with `i`) or bursts, idle and during traffic.
fn outside_of(i: u64, reqs: &[Req]) -> (&'static str, Vec<OutsideUse>) {
    if i.is_multiple_of(4) {
        return ("office-hours rate", vec![rate(1.0 + (i % 3) as f64 * 0.5, true)]);
    }
    if i % 2 == 1 {
        return ("flat rate", vec![rate(0.5 + (i % 5) as f64 * 0.5, false)]);
    }
    let mut out: Vec<OutsideUse> =
        spread(&idle_polls(reqs), 6).iter().map(|p| drop_at(p - 5 * MIN, 6.0, false)).collect();
    for day in 2..5 {
        out.push(drop_at(day * DAY + (11 + (i % 5)) * HOUR + 25 * MIN, 8.0, true));
    }
    ("bursts", out)
}

/// The truth of each number the fit can name, by its key.
fn truth_of(capacity: f64) -> [(String, f64); 4] {
    [
        (format!("capacity@{ACCOUNT}"), capacity),
        ("weight.output".into(), RATIOS[0]),
        ("weight.cache_read".into(), RATIOS[1]),
        ("weight.cache_write".into(), RATIOS[2]),
    ]
}

#[derive(Default)]
struct Tally {
    failures: Vec<String>,
    misses: Vec<String>,
    tested: u64,
    rejected: u64,
    checks: u64,
    hits: u64,
    placements: u64,
    equal: u64,
}

/// Checks one fitted run against its fit-off baseline.
fn check(t: &mut Tally, seed: u64, what: &str, capacity: f64, fitted: &Week, baseline: &Week) {
    let fit = fitted.fit.as_ref().expect("a fit run");
    if !fit.fits.windows.is_empty() {
        t.failures.push(format!("seed={seed:#x} number=* what={what}: significant numbers {:?}", fit.fits.windows));
    }
    if fit.first_change.is_some() {
        t.failures.push(format!("seed={seed:#x} number=* what={what}: first change at {:?}", fit.first_change));
    }
    let states = fit.learner.number_states(PROVIDER, WINDOW);
    t.tested += states.len() as u64;
    for (key, s) in &states {
        if matches!(s, NumberState::Fitted { .. }) {
            t.rejected += 1;
            t.failures.push(format!("seed={seed:#x} number={key} what={what}: state {s:?}"));
        }
    }
    t.placements += 1;
    if fitted.placed == baseline.placed {
        t.equal += 1;
    } else {
        let first = fitted.placed.iter().zip(&baseline.placed).position(|(a, b)| a != b);
        t.failures.push(format!("seed={seed:#x} number=* what={what}: placements differ from the baseline (first at {first:?})"));
    }
    let ranges = fit.learner.ranges(PROVIDER, WINDOW);
    for (key, want) in truth_of(capacity) {
        let Some((_, lo, hi)) = ranges.get(&key).copied() else { continue };
        t.checks += 1;
        if lo <= want && want <= hi {
            t.hits += 1;
        } else {
            // A miss alone is allowed (SC-003 asks 93%); it is printed so that a failing run shows which.
            t.misses.push(format!("seed={seed:#x} number={key} what={what}: 95% range {lo}..{hi} misses the truth {want}"));
        }
    }
}

#[test]
#[ignore = "the right-plugin suite: 100 seeds, each clean and with outside use; CI runs it in release"]
fn a_right_plugin_gets_no_number_and_its_ranges_cover_the_truth_over_100_seeds() {
    let started = std::time::Instant::now();
    let mut t = Tally::default();
    for i in 0..SEEDS {
        let seed = SEED ^ i.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let reqs = plan(seed);
        let capacity = (peak_five_hours(&reqs) / PEAK_SHARE).round();
        let clean = run(capacity, &reqs, &[], true);
        let clean_base = run(capacity, &reqs, &[], false);
        check(&mut t, seed, "clean", capacity, &clean, &clean_base);

        // The seed's own outside use, and the next kind along, so that every seed meets two and
        // the suite makes at least 1,000 range checks.
        for j in [i, i + 1] {
            let (kind, outside) = outside_of(j, &reqs);
            let with = run(capacity, &reqs, &outside, true);
            let with_base = run(capacity, &reqs, &outside, false);
            check(&mut t, seed, kind, capacity, &with, &with_base);
        }
    }

    // The null rejection rate with the sim's noise: the score is a random walk with 0.3
    // information per ten-minute look, over a week of looks, for several numbers tested together.
    for m in [1usize, 4, 12] {
        let rate = simulated_null_rate(5_000, 1_008, 0.3, m, SEED + m as u64);
        println!("null rejection rate (m = {m}): {:.3}% (bound {:.1}%)", rate * 100.0, ALPHA * 100.0);
    }

    println!(
        "null rejection rate from the sim: {} of {} ({:.3}%, bound {:.1}%)",
        t.rejected,
        t.tested,
        if t.tested == 0 { 0.0 } else { 100.0 * t.rejected as f64 / t.tested as f64 },
        ALPHA * 100.0
    );
    let coverage = if t.checks == 0 { 0.0 } else { t.hits as f64 / t.checks as f64 };
    println!(
        "right-plugin suite: {} runs, placements equal {}/{}, range coverage {}/{} = {:.1}%, {:.1}s",
        3 * SEEDS,
        t.equal,
        t.placements,
        t.hits,
        t.checks,
        coverage * 100.0,
        started.elapsed().as_secs_f64()
    );
    if t.checks < 1_000 {
        t.failures.push(format!("seed=* number=* what=coverage: only {} range checks (at least 1000 needed)", t.checks));
    }
    if coverage < 0.93 {
        t.failures.extend(t.misses.iter().cloned());
        t.failures.push(format!("seed=* number=* what=coverage: {:.2}% of ranges hold the truth (SC-003 asks 93%)", coverage * 100.0));
    }
    assert!(t.failures.is_empty(), "{} failures (a failing seed is investigated, never replaced):\n{}", t.failures.len(), t.failures.join("\n"));
}

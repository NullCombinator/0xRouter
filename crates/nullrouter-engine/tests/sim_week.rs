//! A simulated week proves the routing rules (spec 006, US6, T081–T084, research R17).
//!
//! Twelve agents send about 60,000 requests over mock accounts whose windows charge exactly by
//! their meters. The decision core, settlement and the real journal run on an injected clock;
//! only HTTP is left out. The run checks SC-001 to SC-006, restarts from the journal files in the
//! middle (SC-005), and prints one table.
//!
//! `nice cargo test -p nullrouter-engine --release --test sim_week -j 2 -- --nocapture`

#[path = "sim_week/fit.rs"]
mod fit;
#[path = "sim_week/router.rs"]
mod router;
#[path = "sim_week/world.rs"]
mod world;

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::Duration;

use nullrouter_engine::journal::records;
use nullrouter_engine::journal::state;
use nullrouter_engine::clock;
use indexmap::IndexMap;
use nullrouter_engine::quota::extract::rfc3339_millis;
use nullrouter_engine::quota::fit::outside::{self, OutsideEntry, OutsideType};
use nullrouter_engine::quota::fit::store::{self, Loaded};
use nullrouter_engine::quota::fit::{MeterNumber, NumberOverrides, NumberState, Source, TokenClass, WindowFit, in_effect};
use nullrouter_engine::routing::meter::{Spent, cost_spent};
use nullrouter_engine::routing::view::{NumberView, WindowMeterView};
use nullrouter_engine::routing::{
    CandidateKey, CandidateRow, Decision, DecisionKind, MovedBecause, PlacementReason, PriceSpec, Tier, WhyNot,
};
use nullrouter_registry::schema::{MeterDecl, MeterUnit, RoutingDecl};
use serde_json::Value;

use fit::FitRun;
use router::{AccountDef, Defs, POLL_MS, Placed, Sim, copy_dir, usage_of};
use world::{DAY, HOUR, Injected, MIN, OutsideUse, Req, Reset, Rng, Rounding, TrueAccount, TrueWindow, WEEK_MS, World, at, plan};

const SEED: u64 = 0x006_0000_5EED;
/// Amortization windows with fewer cold requests than this are too thin for a 5% bound: one
/// request is already a visible share of them.
const THIN: u64 = 30;

const WEIGHTS: &str = "token_weights = { input = 1.0, output = 5.0, cache_read = 0.1, cache_write = 1.25 }";

fn decl(toml: &str) -> RoutingDecl {
    toml::from_str(toml).unwrap_or_else(|e| panic!("{e}\n{toml}"))
}

/// The six accounts of the target: two anthropic-shaped subscriptions (the first also has a
/// per-minute limit), a request-counted one, an estimated one, and two pay-as-you-go accounts.
fn defs() -> Defs {
    let sub = |key: &str, order: i64, toml: String, reported: bool| {
        let (provider, account) = key.split_once('/').expect("provider/account");
        AccountDef {
            key: CandidateKey::new(provider, account, "m"),
            order,
            priority: 1.0,
            meters: decl(&toml).window,
            reported,
            price: PriceSpec::default(),
        }
    };
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
    let tokens = |name: &str, length: &str, capacity: u64, extra: &str| {
        format!(
            "[[window]]\nname = \"{name}\"\nlength = \"{length}\"\nunit = \"weighted_tokens\"\ncapacity = {capacity}\n{WEIGHTS}\n{extra}\n"
        )
    };
    let max = tokens("5-hour", "5h", CAP.max_5h, "reserve = \"10%\"")
        + &tokens("weekly", "7d", CAP.max_week, "")
        + "[[window]]\nname = \"per-minute\"\nlength = \"1m\"\nunit = \"requests\"\ncapacity = 5\n";
    let pro = tokens("5-hour", "5h", CAP.pro_5h, "reserve = \"10%\"") + &tokens("weekly", "7d", CAP.pro_week, "");
    let daily =
        format!("[[window]]\nname = \"daily\"\nlength = \"1d\"\nunit = \"requests\"\ncapacity = {}\n", CAP.kimi_day);
    let glm = tokens("daily", "1d", CAP.glm_day, "reset = \"fixed\"\nanchor = \"00:00+00:00\"");
    Defs {
        accounts: vec![
            sub("anthropic/max", 0, max, true),
            sub("anthropic/pro", 1, pro, true),
            sub("kimi/daily", 2, daily, true),
            sub("glm/plan", 3, glm, false),
            payg(
                "openrouter/key",
                4,
                "[[price]]\nwhen = { from = \"14:00\", to = \"22:00\" }\ninput = 3.0\n[[price]]\ninput = 1.5\n",
            ),
            payg(
                "deepseek/key",
                5,
                "[[price]]\nwhen = { from = \"01:00\", to = \"09:00\" }\ninput = 0.8\n[[price]]\ninput = 1.6\n",
            ),
        ],
        target: "sonnet".into(),
        amortization: Duration::from_secs(5 * 3600),
    }
}

/// The windows' capacities, in each meter's unit.
struct Caps {
    max_5h: u64,
    max_week: u64,
    pro_5h: u64,
    pro_week: u64,
    kimi_day: u64,
    glm_day: u64,
}

const CAP: Caps = Caps {
    max_5h: 30_000_000,
    max_week: 400_000_000,
    pro_5h: 10_000_000,
    pro_week: 130_000_000,
    kimi_day: 2_500,
    glm_day: 80_000_000,
};

/// How each declared window resets at the provider, and how full it already was on Monday.
fn truth(defs: &Defs) -> World {
    let accounts = defs
        .accounts
        .iter()
        .map(|d| TrueAccount {
            windows: d
                .meters
                .iter()
                .map(|m| {
                    let (reset, prior) = match (d.key.account_key().as_str(), m.name.as_str()) {
                        ("anthropic/max", "weekly") => (Reset::Fixed { offset_ms: 3 * DAY + 12 * HOUR }, 0.30),
                        ("anthropic/pro", "weekly") => (Reset::Fixed { offset_ms: 5 * DAY + 6 * HOUR }, 0.25),
                        (_, "daily") => (Reset::Fixed { offset_ms: 0 }, 0.0),
                        _ => (Reset::FirstUse, 0.0),
                    };
                    TrueWindow::new(m, reset, d.reported && !m.is_admission(), prior)
                })
                .collect(),
        })
        .collect();
    World::new(accounts)
}

// ---------------------------------------------------------------------------------------------
// Reading the journal back

/// One request, as the records hold it.
struct Rec {
    at_ms: u64,
    d: Decision,
    /// `(account, reason, ok, plain tokens)` per attempt.
    attempts: Vec<(usize, PlacementReason, bool, u64)>,
    served: Option<usize>,
}

fn millis(v: &Value) -> u64 {
    let t = v.as_str().and_then(nullrouter_engine::clock::parse_rfc3339).expect("a time");
    u64::try_from(t.duration_since(at(0)).unwrap_or_default().as_millis()).unwrap_or(0)
}

/// Feeds every record of `home` to `f`, a day at a time.
fn read_all(home: &Path, defs: &Defs, mut f: impl FnMut(Rec)) -> usize {
    let mut count = 0;
    for (_, path) in records::segments(home) {
        for r in records::fold(&fs::read_to_string(path).expect("a segment")) {
            count += 1;
            let Ok(d) = serde_json::from_value::<Decision>(r["decision"].clone()) else { continue };
            let index = |a: &Value| {
                defs.accounts
                    .iter()
                    .position(|d| a["provider"] == d.key.provider.as_str() && a["account"] == d.key.account.as_str())
            };
            let attempts: Vec<_> = r["attempts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|a| {
                    let reason = serde_json::from_value(a["placement"]["reason"].clone()).ok()?;
                    Some((index(a)?, reason, a["outcome"]["state"] == "ok", usage_of(&a["usage"]).plain()))
                })
                .collect();
            let served = attempts.iter().find(|a| a.2).map(|a| a.0);
            f(Rec { at_ms: millis(&r["arrived"]), d, attempts, served });
        }
    }
    count
}

// ---------------------------------------------------------------------------------------------
// The checks

#[derive(Default)]
struct Window {
    count: u64,
    total: f64,
    largest: f64,
    target: Vec<f64>,
    received: Vec<f64>,
}

#[derive(Default)]
struct Analysis {
    requests: usize,
    unserved: usize,
    kinds: BTreeMap<&'static str, usize>,
    served_by: Vec<usize>,
    failed_attempts: usize,
    windows: BTreeMap<u64, Window>,
    week_target: Vec<f64>,
    week_received: Vec<f64>,
    // SC-002
    warm_hits: usize,
    warm_stays: usize,
    moves: BTreeMap<String, usize>,
    bad_warm: Vec<String>,
    // SC-003
    payg_served: usize,
    payg_overflow: usize,
    payg_warm: usize,
    payg_fallback: usize,
    bad_payg: Vec<String>,
    // SC-004: `(time, account)` of a cold placement that went elsewhere while the account owed most.
    passed_over: Vec<(u64, usize)>,
    // SC-006
    recomputed: usize,
    within_rounding: usize,
    bad_recompute: Vec<String>,
}

fn name(defs: &Defs, i: usize) -> String {
    defs.accounts[i].key.account_key()
}

/// The account the recorded rows choose for the first attempt: among the eligible rows of the
/// tier cold work went to, the largest deficit, then the higher share, then the operator's order.
fn recomputed(d: &Decision) -> usize {
    let tier =
        if d.candidates.iter().any(|c| c.eligible && c.tier == Tier::Subscription && c.weight.unwrap_or(0.0) > 0.0) {
            Tier::Subscription
        } else {
            Tier::Payg
        };
    let mut rows: Vec<(usize, &CandidateRow)> =
        d.candidates.iter().enumerate().filter(|(_, c)| c.eligible && c.tier == tier).collect();
    rows.sort_by(|(i, a), (j, b)| {
        b.deficit_before
            .unwrap_or(0)
            .cmp(&a.deficit_before.unwrap_or(0))
            .then(b.share.unwrap_or(0.0).total_cmp(&a.share.unwrap_or(0.0)))
            .then(i.cmp(j))
    });
    rows[0].0
}

impl Analysis {
    fn new(n: usize) -> Self {
        Self { served_by: vec![0; n], week_target: vec![0.0; n], week_received: vec![0.0; n], ..Self::default() }
    }

    fn feed(&mut self, defs: &Defs, r: &Rec) {
        let n = defs.accounts.len();
        let d = &r.d;
        let rows = &d.candidates;
        self.requests += 1;
        *self
            .kinds
            .entry(match d.kind {
                DecisionKind::Warm => "warm",
                DecisionKind::Cold => "cold",
                DecisionKind::Overflow => "overflow",
                DecisionKind::None => "none",
            })
            .or_default() += 1;
        match r.served {
            Some(i) => self.served_by[i] += 1,
            None => self.unserved += 1,
        }
        self.failed_attempts += r.attempts.iter().filter(|a| !a.2).count();
        let id = format!("@{}", r.at_ms);

        // SC-001: what each account was owed and what it got, in plain tokens, per amortization
        // window. Warm work is not placed by the ledger and is not counted.
        if let Some(&(acct, reason, _, plain)) = r.attempts.iter().find(|a| a.2) {
            let sub = |c: &CandidateRow| c.eligible && c.tier == Tier::Subscription && c.share.unwrap_or(0.0) > 0.0;
            let placed_on = rows.iter().position(|c| defs.index_of(&c.key()) == acct);
            if reason != PlacementReason::Warm && placed_on.is_some_and(|i| sub(&rows[i])) {
                let key =
                    u64::try_from(d.amortization_window.start.duration_since(at(0)).unwrap_or_default().as_millis())
                        .unwrap_or(0);
                let w = self.windows.entry(key).or_insert_with(|| Window {
                    target: vec![0.0; n],
                    received: vec![0.0; n],
                    ..Window::default()
                });
                w.count += 1;
                w.total += plain as f64;
                w.largest = w.largest.max(plain as f64);
                w.received[acct] += plain as f64;
                self.week_received[acct] += plain as f64;
                for c in rows.iter().filter(|c| sub(c)) {
                    let share = c.share.unwrap_or(0.0) * plain as f64;
                    w.target[defs.index_of(&c.key())] += share;
                    self.week_target[defs.index_of(&c.key())] += share;
                }
            }
        }

        // SC-002: a warm request stays unless its account can't serve it or it leaves pay-as-you-go.
        if let Some(h) = &d.warm {
            self.warm_hits += 1;
            let at_ = rows.iter().find(|c| c.provider == h.provider && c.account == h.account).expect("the warm row");
            let sub_can_serve =
                rows.iter().any(|c| c.eligible && c.tier == Tier::Subscription && c.weight.unwrap_or(0.0) > 0.0);
            if h.stayed {
                self.warm_stays += 1;
                let first = d.order.first().map(|i| &rows[*i]);
                if first.is_none_or(|f| f.provider != h.provider || f.account != h.account)
                    || d.kind != DecisionKind::Warm
                {
                    self.bad_warm.push(format!("{id}: stayed but was placed elsewhere"));
                }
                if at_.tier == Tier::Payg && sub_can_serve {
                    self.bad_warm.push(format!("{id}: stayed on pay-as-you-go while a subscription could serve"));
                }
                if !at_.eligible && at_.why_not != Some(WhyNot::PriorityZero) {
                    self.bad_warm.push(format!("{id}: stayed on an account that could not serve ({:?})", at_.why_not));
                }
            } else {
                let because = h.moved_because.expect("a move has a reason");
                *self.moves.entry(format!("{because:?}")).or_default() += 1;
                let justified = match because {
                    MovedBecause::RateLimited => matches!(at_.why_not, Some(WhyNot::Cooling | WhyNot::Admission)),
                    MovedBecause::ReserveFloor => at_.why_not == Some(WhyNot::ReserveFloor),
                    MovedBecause::LeftPayAsYouGo => at_.tier == Tier::Payg && sub_can_serve,
                    MovedBecause::WarmUnusable => false,
                };
                if !justified {
                    self.bad_warm.push(format!("{id}: moved for {because:?} but its row says {:?}", at_.why_not));
                }
            }
        }

        // SC-003: pay-as-you-go serves only when no subscription could.
        if let Some(s) = r.served.filter(|s| rows.iter().any(|c| defs.index_of(&c.key()) == *s && c.tier == Tier::Payg))
        {
            self.payg_served += 1;
            let first = d.order.first().map(|i| defs.index_of(&rows[*i].key()));
            let sub_can_serve =
                rows.iter().any(|c| c.eligible && c.tier == Tier::Subscription && c.weight.unwrap_or(0.0) > 0.0);
            match (first == Some(s), d.kind) {
                (true, DecisionKind::Warm) => self.payg_warm += 1,
                (true, _) => self.payg_overflow += 1,
                (false, _) => self.payg_fallback += 1,
            }
            if first == Some(s) && sub_can_serve {
                self.bad_payg.push(format!("{id}: {} served while a subscription could", name(defs, s)));
            }
        }

        // SC-004, first part: cold work that went elsewhere while an account that could serve
        // owed the most.
        if matches!(d.kind, DecisionKind::Cold | DecisionKind::Overflow)
            && let Some(first) = d.order.first().map(|i| &rows[*i])
        {
            let eligible: Vec<&CandidateRow> = rows
                .iter()
                .filter(|c| c.eligible && c.tier == Tier::Subscription && c.weight.unwrap_or(0.0) > 0.0)
                .collect();
            let chosen = if first.tier == Tier::Subscription { first.deficit_before.unwrap_or(0) } else { i64::MIN };
            for c in eligible {
                if c.deficit_before.unwrap_or(0) > chosen.saturating_add(1) {
                    self.passed_over.push((r.at_ms, defs.index_of(&c.key())));
                }
            }

            // SC-006: the chosen account is the one the recorded rows choose.
            self.recomputed += 1;
            let want = recomputed(d);
            let got = d.order[0];
            if want != got {
                let top = rows
                    .iter()
                    .filter(|c| c.eligible && c.tier == rows[got].tier)
                    .filter_map(|c| c.deficit_before)
                    .max()
                    .unwrap_or(0);
                if rows[got].deficit_before.unwrap_or(0) >= top - 1 {
                    // The ledger compares unrounded deficits; the record keeps whole tokens.
                    self.within_rounding += 1;
                } else {
                    self.bad_recompute.push(format!(
                        "{id}: rows choose {} but it went to {}",
                        name(defs, want),
                        name(defs, got)
                    ));
                }
            }
        }
    }

    /// Everything SC-001 to SC-006 asserts; each failure says which request or window.
    fn failures(&self, defs: &Defs, events: &[world::ResetEvent]) -> Vec<String> {
        let mut bad = Vec::new();
        let n = defs.accounts.len();
        // SC-001
        for (start, w) in self.windows.iter().filter(|(_, w)| w.count >= THIN) {
            for i in 0..n {
                // A request is placed whole, so no scheme can come closer than one request of a
                // window whose cold work is a few of them.
                let off = (w.received[i] - w.target[i]).abs();
                if off > (0.05 * w.total).max(w.largest) {
                    bad.push(format!(
                        "SC-001: {} in the window at +{}h ({} cold requests, {:.0} tokens, off by {:.0}, largest {:.0}) received {:.1}% of the cold work against a target of {:.1}%",
                        name(defs, i),
                        start / HOUR,
                        w.count,
                        w.total,
                        w.received[i] - w.target[i],
                        w.largest,
                        w.received[i] / w.total * 100.0,
                        w.target[i] / w.total * 100.0
                    ));
                }
            }
        }
        // SC-002, SC-003, SC-006
        bad.extend(self.bad_warm.iter().map(|s| format!("SC-002 {s}")));
        bad.extend(self.bad_payg.iter().map(|s| format!("SC-003 {s}")));
        bad.extend(self.bad_recompute.iter().map(|s| format!("SC-006 {s}")));
        // SC-004: a window that ended with quota to spare while cold work went elsewhere.
        for e in events {
            if e.left_frac > e.reserve + 0.05
                && self
                    .passed_over
                    .iter()
                    .any(|(t, i)| *i == account_of(defs, e) && *t >= e.started_ms && *t <= e.at_ms)
            {
                bad.push(format!(
                    "SC-004: {} {} ended at +{}h with {:.0}% left while cold work went to an account owed less",
                    name(defs, e.account),
                    e.window,
                    e.at_ms / HOUR,
                    e.left_frac * 100.0
                ));
            }
            // The floor holds: no window ends more than 5% of its capacity under it.
            if e.left_frac < e.reserve - 0.05 {
                bad.push(format!(
                    "SC-004: {} {} ended at {:.0}% left, under its floor",
                    name(defs, e.account),
                    e.window,
                    e.left_frac * 100.0
                ));
            }
        }
        bad
    }

    fn report(&self, defs: &Defs, events: &[world::ResetEvent]) -> String {
        let mut out = String::new();
        let total_target: f64 = self.week_target.iter().sum();
        let total_received: f64 = self.week_received.iter().sum();
        let w = |s: &mut String, line: String| {
            s.push_str(&line);
            s.push('\n');
        };
        w(
            &mut out,
            format!("requests {}  unserved {}  failed attempts {}", self.requests, self.unserved, self.failed_attempts),
        );
        w(
            &mut out,
            format!(
                "decisions {:?}; cold work passed over an account that owed more: {}",
                self.kinds,
                self.passed_over.len()
            ),
        );
        w(&mut out, format!("warm: {} hits, {} stayed, moved {:?}", self.warm_hits, self.warm_stays, self.moves));
        w(
            &mut out,
            format!(
                "pay-as-you-go served {} (overflow {}, warm stays {}, after a failed subscription {})",
                self.payg_served, self.payg_overflow, self.payg_warm, self.payg_fallback
            ),
        );
        let full: Vec<&Window> = self.windows.values().filter(|w| w.count >= THIN).collect();
        let worst = full
            .iter()
            .flat_map(|w| (0..defs.accounts.len()).map(move |i| (w.received[i] - w.target[i]).abs() / w.total.max(1.0)))
            .fold(0.0, f64::max);
        w(
            &mut out,
            format!(
                "amortization windows {} ({} with at least {THIN} cold requests); worst |received - target| {:.2}% of the window",
                self.windows.len(),
                full.len(),
                worst * 100.0
            ),
        );
        w(
            &mut out,
            format!(
                "recomputed {} cold and overflow decisions from their records; {} within the rounding of whole tokens",
                self.recomputed, self.within_rounding
            ),
        );
        w(&mut out, String::new());
        w(
            &mut out,
            format!(
                "{:<16} {:<11} {:>6} {:>9} {:>9} {:>8} {:>8} {:>8} {:>8}",
                "account", "window", "resets", "target %", "actual %", "left avg", "left min", "left max", "served"
            ),
        );
        for (i, d) in defs.accounts.iter().enumerate() {
            let share = |v: &[f64], total: f64| if total > 0.0 { v[i] / total * 100.0 } else { 0.0 };
            let (target, actual) = (share(&self.week_target, total_target), share(&self.week_received, total_received));
            let mut names: Vec<&str> = events.iter().filter(|e| e.account == i).map(|e| e.window.as_str()).collect();
            names.sort_unstable();
            names.dedup();
            if names.is_empty() {
                w(
                    &mut out,
                    format!(
                        "{:<16} {:<11} {:>6} {target:>9.1} {actual:>9.1} {:>8} {:>8} {:>8} {:>8}",
                        d.key.account_key(),
                        "-",
                        0,
                        "-",
                        "-",
                        "-",
                        self.served_by[i]
                    ),
                );
            }
            for window in names {
                let left: Vec<f64> = events
                    .iter()
                    .filter(|e| e.account == i && e.window == window)
                    .map(|e| e.left_frac * 100.0)
                    .collect();
                let avg = left.iter().sum::<f64>() / left.len() as f64;
                let (min, max) = left.iter().fold((f64::MAX, f64::MIN), |(a, b), x| (a.min(*x), b.max(*x)));
                w(
                    &mut out,
                    format!(
                        "{:<16} {:<11} {:>6} {target:>9.1} {actual:>9.1} {avg:>7.1}% {min:>7.1}% {max:>7.1}% {:>8}",
                        d.key.account_key(),
                        window,
                        left.len(),
                        self.served_by[i]
                    ),
                );
            }
        }
        out
    }
}

fn account_of(_defs: &Defs, e: &world::ResetEvent) -> usize {
    e.account
}

fn analyse(home: &Path, defs: &Defs) -> (Analysis, usize) {
    let mut a = Analysis::new(defs.accounts.len());
    let count = read_all(home, defs, |r| a.feed(defs, &r));
    (a, count)
}

// ---------------------------------------------------------------------------------------------
// The runs

fn units_of(defs: &Defs) -> usize {
    defs.accounts.iter().filter(|a| a.meters.iter().any(|m| m.unit == MeterUnit::Requests)).count()
}

#[test]
fn a_simulated_week_meets_every_target_and_survives_a_restart() {
    let started = std::time::Instant::now();
    let defs = defs();
    let reqs = plan(SEED);
    let sessions = reqs.iter().filter(|r| r.session).count();
    assert!(
        (50_000..70_000).contains(&reqs.len()),
        "about 60,000 requests, got {} ({sessions} in sessions)",
        reqs.len()
    );
    assert!(reqs.windows(2).all(|p| p[0].at_ms <= p[1].at_ms) && reqs.last().is_some_and(|r| r.at_ms < WEEK_MS));
    assert!(units_of(&defs) >= 2);

    let a_dir = tempfile::tempdir().unwrap();
    let b_dir = tempfile::tempdir().unwrap();
    let mut world = truth(&defs);
    let mut sim = Sim::new(&defs, a_dir.path());

    // A seeded point somewhere in the middle of the week.
    let cut = reqs.len() * (40 + (Rng::new(SEED).next() % 21) as usize) / 100;
    for r in &reqs[..cut] {
        sim.handle(&mut world, r);
    }
    let first_half = started.elapsed();
    // The crash: everything queued is written, nothing is synced, and all memory is gone.
    sim.crash_point();
    copy_dir(a_dir.path(), b_dir.path());
    let restart_ms = reqs[cut].at_ms;
    let mut control_world = world.clone();
    let mut control = sim.fork(b_dir.path());
    let mut restarted = sim.restart(restart_ms);
    assert!(restarted.deficits().len() <= control.deficits().len() + 6);
    for r in &reqs[cut..] {
        control.handle(&mut control_world, r);
        restarted.handle(&mut world, r);
    }
    let both = started.elapsed();
    control.journal.flush_blocking();
    restarted.journal.flush_blocking();

    // SC-005: the restarted run places exactly as the one that never stopped, and holds the same state.
    let same = control.placed[cut..] == restarted.placed[cut..];
    let first_difference = control.placed[cut..].iter().zip(&restarted.placed[cut..]).find(|(a, b)| a != b);
    assert!(same, "placements differ after the restart: {first_difference:?}");
    assert_eq!(control.warm.stored(), restarted.warm.stored(), "the warm store after the week");
    assert_eq!(control.deficits(), restarted.deficits(), "the deficits after the week");

    let (control_a, control_count) = analyse(b_dir.path(), &defs);
    let (restarted_a, restarted_count) = analyse(a_dir.path(), &defs);
    assert_eq!(control_count, reqs.len(), "every request has its record");
    assert_eq!(restarted_count, reqs.len(), "a restart loses no record");

    let events = &control_world.events;
    let report = control_a.report(&defs, events);
    println!("\n{report}");
    println!("post-restart placements identical: {}", if same { "yes" } else { "no" });
    println!(
        "restart at request {cut} of {} (+{:.1}h); first half {:.1}s, both halves {:.1}s, all {:.1}s",
        reqs.len(),
        restart_ms as f64 / HOUR as f64,
        first_half.as_secs_f64(),
        both.as_secs_f64(),
        started.elapsed().as_secs_f64()
    );
    assert_eq!(report, restarted_a.report(&defs, &world.events), "the restarted run's table is the same table");

    // The week is not trivial: every rule had something to decide.
    assert!(control_a.warm_stays > 1_000, "warm sessions stayed: {}", control_a.warm_stays);
    assert!(control_a.kinds.get("cold").copied().unwrap_or(0) > 1_000);
    assert!(control_a.moves.values().sum::<usize>() > 0, "some warm session moved for capacity");
    assert!(control_a.payg_overflow > 0, "some overflow reached pay-as-you-go");
    assert_eq!(control_a.unserved, 0, "every request was served");

    let bad = control_a.failures(&defs, events);
    assert!(
        bad.is_empty(),
        "{} failures, the first ten:\n{}",
        bad.len(),
        bad.iter().take(10).cloned().collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn a_power_loss_leaves_at_most_the_last_simulated_second_missing() {
    let defs = defs();
    // Monday only: the cut and the seconds before it.
    let reqs: Vec<Req> = plan(SEED).into_iter().filter(|r| r.at_ms < DAY).collect();
    let cut = reqs.len() * (45 + (Rng::new(SEED ^ 1).next() % 30) as usize) / 100;
    let cut_ms = reqs[cut].at_ms;
    let dir = tempfile::tempdir().unwrap();
    let mut world = truth(&defs);
    let mut sim = Sim::new(&defs, dir.path());
    for r in &reqs[..cut] {
        // From twenty minutes before the cut, every simulated second is synced before the next.
        if r.at_ms + 20 * MIN >= cut_ms && sim.sync_each_second.is_none() {
            sim.journal.flush_blocking();
            sim.sync_each_second = Some(r.at_ms / 1000);
        }
        sim.handle(&mut world, r);
    }
    let last_second = reqs[cut - 1].at_ms / 1000;

    // The power goes: whatever is past each file's last sync is gone.
    sim.crash_point();
    let synced = sim.journal.faults().synced.lock().unwrap().clone();
    for (path, len) in &synced {
        let f = fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_len((*len).min(f.metadata().unwrap().len())).unwrap();
    }
    for (_, path) in records::segments(dir.path()) {
        if !synced.contains_key(&path) {
            fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(0).unwrap();
        }
    }

    let mut present = std::collections::BTreeSet::new();
    read_all(dir.path(), &defs, |r| {
        if r.served.is_some() {
            present.insert(r.at_ms);
        }
    });
    let missing: Vec<&Req> = reqs[..cut].iter().filter(|r| !present.contains(&r.at_ms)).collect();
    for r in &missing {
        assert!(
            r.at_ms / 1000 >= last_second,
            "request {} at second {} is gone; the last second is {last_second}",
            r.n,
            r.at_ms / 1000
        );
    }
    println!(
        "power loss at +{:.2}h: {} of the {} requests before it are missing, all from the last simulated second",
        cut_ms as f64 / HOUR as f64,
        missing.len(),
        cut
    );
    // The routing state still loads, and holds the work of every earlier second.
    let loaded = state::load(dir.path());
    assert!(!loaded.warm.is_empty() && !loaded.ledgers.is_empty());
}

// ---------------------------------------------------------------------------------------------
// The quota fit over the week (spec 012, T017, T018; research R14)

const FIT_PROVIDER: &str = "fitted";
const FIT_ACCOUNT: &str = "acct";
const FIT_WINDOW: &str = "5-hour";
/// What the declaration says, and what a provider that charges the output and cache classes three
/// times as much, relative to input, really does.
const DECLARED_WEIGHTS: &str = WEIGHTS;
const THREE_TIMES_WEIGHTS: &str = "token_weights = { input = 1.0, output = 15.0, cache_read = 0.3, cache_write = 3.75 }";
/// `(output, cache_read, cache_write)` of each, as the fit names them: ratios to input (1.0).
const DECLARED_RATIOS: [f64; 3] = [5.0, 0.1, 1.25];
const THREE_TIMES_RATIOS: [f64; 3] = [15.0, 0.3, 3.75];
/// The busiest five hours of the week use this share of the true capacity, so that no window runs out.
const PEAK_SHARE: f64 = 0.55;

/// One plugin with one subscription account, over two pay-as-you-go keys that catch overflow.
struct Scenario {
    declared_weights: &'static str,
    true_weights: &'static str,
    true_ratios: [f64; 3],
    declared_capacity: f64,
    true_capacity: f64,
}

fn five_hour(capacity: f64, weights: &str) -> MeterDecl {
    let toml = format!(
        "[[window]]\nname = \"{FIT_WINDOW}\"\nlength = \"5h\"\nunit = \"weighted_tokens\"\ncapacity = {capacity}\n{weights}\nreserve = \"10%\"\n"
    );
    decl(&toml).window.remove(0)
}

/// The most the week's traffic costs a 5-hour stretch of one account that never refuses it.
fn peak_five_hours(reqs: &[Req], weights: &str) -> f64 {
    let meter = five_hour(1.0e12, weights);
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

impl Scenario {
    /// Output and cache weights 3x the declared ones relative to input, true capacity 0.6x declared.
    fn three_times_off(reqs: &[Req]) -> Self {
        let truth = (peak_five_hours(reqs, THREE_TIMES_WEIGHTS) / PEAK_SHARE).round();
        Self {
            declared_weights: DECLARED_WEIGHTS,
            true_weights: THREE_TIMES_WEIGHTS,
            true_ratios: THREE_TIMES_RATIOS,
            declared_capacity: (truth / 0.6).round(),
            true_capacity: truth,
        }
    }

    /// The plugin declares what the provider does.
    fn right(reqs: &[Req]) -> Self {
        let truth = (peak_five_hours(reqs, DECLARED_WEIGHTS) / PEAK_SHARE).round();
        Self {
            declared_weights: DECLARED_WEIGHTS,
            true_weights: DECLARED_WEIGHTS,
            true_ratios: DECLARED_RATIOS,
            declared_capacity: truth,
            true_capacity: truth,
        }
    }

    /// Right weights; the declared capacity is half the true one (true = 2x declared).
    fn half_capacity(reqs: &[Req]) -> Self {
        let truth = (peak_five_hours(reqs, DECLARED_WEIGHTS) / PEAK_SHARE).round();
        Self {
            declared_weights: DECLARED_WEIGHTS,
            true_weights: DECLARED_WEIGHTS,
            true_ratios: DECLARED_RATIOS,
            declared_capacity: (truth / 2.0).round(),
            true_capacity: truth,
        }
    }

    /// The router's definitions and the provider's truth. Polls show whole percents, rounded half up.
    fn week(&self) -> (Defs, World) {
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
                    key: CandidateKey::new(FIT_PROVIDER, FIT_ACCOUNT, "m"),
                    order: 0,
                    priority: 1.0,
                    meters: vec![five_hour(self.declared_capacity, self.declared_weights)],
                    reported: true,
                    price: PriceSpec::default(),
                },
                payg("openrouter/key", 1, "[[price]]\ninput = 1.5\n"),
                payg("deepseek/key", 2, "[[price]]\ninput = 1.6\n"),
            ],
            target: "sonnet".into(),
            amortization: Duration::from_secs(5 * 3600),
        };
        let truth = five_hour(self.true_capacity, self.true_weights);
        let mut world = World::new(vec![
            TrueAccount { windows: vec![TrueWindow::new(&truth, Reset::FirstUse, true, 0.0)] },
            TrueAccount::default(),
            TrueAccount::default(),
        ]);
        world.rounding = Some(Rounding::HalfUp);
        (defs, world)
    }
}

/// What a week left behind.
struct Week {
    placed: Vec<Placed>,
    fit: Option<FitRun>,
}

fn run_week(scenario: &Scenario, reqs: &[Req], fitting: bool) -> Week {
    let (defs, mut world) = scenario.week();
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

/// Capacity is `Fitted` within the week and within 10% of the truth; so is every other number the
/// fit made significant; and every significant number's 95% range holds its truth (SC-001, US1
/// scenarios 1 and 3). With `weights` the output weight must be significant as well.
fn assert_fitted(fit: &FitRun, scenario: &Scenario, weights: bool) {
    let states = fit.learner.number_states(FIT_PROVIDER, FIT_WINDOW);
    let ranges = fit.learner.ranges(FIT_PROVIDER, FIT_WINDOW);
    let published = fit.fits.window(FIT_PROVIDER, FIT_WINDOW);
    let capacity = format!("capacity@{FIT_ACCOUNT}");
    let [out, read, write] = scenario.true_ratios;
    let truth = [
        (capacity.as_str(), scenario.true_capacity),
        ("weight.output", out),
        ("weight.cache_read", read),
        ("weight.cache_write", write),
    ];
    let mut required = vec![capacity.as_str()];
    if weights {
        required.push("weight.output");
    }
    for key in required {
        match states.get(key) {
            Some(NumberState::Fitted { since }) => assert!(*since <= at(WEEK_MS), "{key} fitted after the week"),
            other => panic!("{key} is {other:?}, not fitted within the week"),
        }
    }
    let value = |key: &str| match key {
        "weight.output" => published.weights.get(&TokenClass::Output).copied(),
        "weight.cache_read" => published.weights.get(&TokenClass::CacheRead).copied(),
        "weight.cache_write" => published.weights.get(&TokenClass::CacheWrite).copied(),
        _ => published.capacity.get(FIT_ACCOUNT).copied(),
    };
    for (key, want) in truth {
        if !matches!(states.get(key), Some(NumberState::Fitted { .. })) {
            continue;
        }
        let got = value(key).unwrap_or_else(|| panic!("{key} is fitted but not published"));
        assert!((got / want - 1.0).abs() <= 0.10, "{key}: fitted {got}, true {want}");
        let (_, lo, hi) = ranges.get(key).copied().unwrap_or_else(|| panic!("{key} has no range"));
        assert!(lo <= want && want <= hi, "{key}: the 95% range {lo}..{hi} misses the truth {want}");
    }
    // FR-014: between polls, remaining quota is the last poll's fraction of the fitted capacity
    // less our own traffic since, costed with the meter in effect, and nothing for outside use.
    assert!(fit.audit_failures.is_empty(), "{} remaining-quota failures, first: {:?}", fit.audit_failures.len(), fit.audit_failures.first());
    assert!(fit.audited > 100, "the remaining-quota check ran only {} times", fit.audited);
}

#[test]
fn a_meter_three_times_off_is_fitted_within_the_week_and_placements_agree_until_then() {
    let started = std::time::Instant::now();
    let reqs = plan(SEED);
    let scenario = Scenario::three_times_off(&reqs);
    let fitted = run_week(&scenario, &reqs, true);
    let baseline = run_week(&scenario, &reqs, false);
    let fit = fitted.fit.as_ref().expect("a fit run");
    assert_fitted(fit, &scenario, true);

    // SC-005: until the fit first has a significant number, no placement differs from the baseline.
    let first = fit.first_change.expect("the fit gained a significant number");
    assert_eq!(fitted.placed[..first], baseline.placed[..first], "a placement before the first significance differs");
    assert!(first > 0 && first < reqs.len(), "first significance at request {first} of {}", reqs.len());
    println!(
        "three times off: first significant number after {first} of {} requests (+{:.1}h); {:.1}s",
        reqs.len(),
        reqs[first.min(reqs.len() - 1)].at_ms as f64 / HOUR as f64,
        started.elapsed().as_secs_f64()
    );
}

#[test]
fn a_right_plugin_places_exactly_as_with_the_fit_off() {
    let reqs = plan(SEED);
    let scenario = Scenario::right(&reqs);
    let fitted = run_week(&scenario, &reqs, true);
    let baseline = run_week(&scenario, &reqs, false);
    let fit = fitted.fit.as_ref().expect("a fit run");
    // US1 scenario 2: nothing is significant, so nothing changes.
    assert!(fit.fits.windows.is_empty(), "a right declaration got a significant number: {:?}", fit.fits.windows);
    assert_eq!(fit.first_change, None);
    assert_eq!(fitted.placed, baseline.placed, "placements with the fit on differ from the baseline");
}

#[test]
fn a_capacity_declared_at_half_the_truth_is_fitted_within_the_week() {
    let reqs = plan(SEED);
    let scenario = Scenario::half_capacity(&reqs);
    let fitted = run_week(&scenario, &reqs, true);
    // US1 scenario 3 as written: true capacity is twice the declared one. The weights are right.
    assert_fitted(fitted.fit.as_ref().expect("a fit run"), &scenario, false);
}

// ---------------------------------------------------------------------------------------------
// Three accounts of one plugin (T019; US1 scenarios 4 and 5; research R8)

const POOL_ACCOUNTS: [&str; 3] = ["a1", "a2", "a3"];
/// What account `a3` charges when its output weight is twice the other accounts' (15.0).
const DOUBLE_OUTPUT_WEIGHTS: &str = "token_weights = { input = 1.0, output = 30.0, cache_read = 0.3, cache_write = 3.75 }";
/// True capacities as shares of the busiest five hours' cost: different per account.
const POOL_SHARES: [f64; 3] = [0.30, 0.40, 0.50];

/// Three subscription accounts of one plugin that declares the weights wrong (the true ones are
/// 3x on output and cache) and each capacity 1/0.6 too high, over two overflow keys.
fn pooled_week(reqs: &[Req], odd_account_weights: &'static str) -> Week {
    pooled_run(reqs, odd_account_weights, Vec::new(), |_, _| {}).0
}

/// [`pooled_week`] with outside use injected, and `setup` run on the fit before the first request.
/// Also returns the drops applied and the definitions (for the checks after the week).
fn pooled_run(
    reqs: &[Req],
    odd_account_weights: &'static str,
    outside: Vec<OutsideUse>,
    setup: impl FnOnce(&Defs, &mut FitRun),
) -> (Week, Vec<Injected>, Defs) {
    let base = (peak_five_hours(reqs, THREE_TIMES_WEIGHTS) / PEAK_SHARE).round();
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
    let mut accounts: Vec<AccountDef> = POOL_ACCOUNTS
        .iter()
        .zip(POOL_SHARES)
        .enumerate()
        .map(|(i, (name, share))| AccountDef {
            key: CandidateKey::new(FIT_PROVIDER, *name, "m"),
            order: i as i64,
            priority: 1.0,
            meters: vec![five_hour((base * share / 0.6).round(), DECLARED_WEIGHTS)],
            reported: true,
            price: PriceSpec::default(),
        })
        .collect();
    accounts.push(payg("openrouter/key", 3, "[[price]]\ninput = 1.5\n"));
    accounts.push(payg("deepseek/key", 4, "[[price]]\ninput = 1.6\n"));
    let defs = Defs { accounts, target: "sonnet".into(), amortization: Duration::from_secs(5 * 3600) };
    let mut truth: Vec<TrueAccount> = POOL_ACCOUNTS
        .iter()
        .zip(POOL_SHARES)
        .enumerate()
        .map(|(i, (_, share))| {
            let weights = if i == 2 { odd_account_weights } else { THREE_TIMES_WEIGHTS };
            let meter = five_hour((base * share).round(), weights);
            TrueAccount { windows: vec![TrueWindow::new(&meter, Reset::FirstUse, true, 0.0)] }
        })
        .collect();
    truth.extend([TrueAccount::default(), TrueAccount::default()]);
    let mut world = World::new(truth);
    world.rounding = Some(Rounding::HalfUp);
    world.outside = outside;

    let dir = tempfile::tempdir().unwrap();
    let mut sim = Sim::new(&defs, dir.path()).with_fit(FitRun::new(defs.accounts.len()));
    setup(&defs, sim.fit.as_mut().expect("a fit run"));
    for r in reqs {
        sim.handle(&mut world, r);
    }
    sim.journal.flush_blocking();
    let week = Week { placed: std::mem::take(&mut sim.placed), fit: sim.fit.take() };
    drop(sim);
    (week, world.injected, defs)
}

fn assert_in_range(fit: &FitRun, key: &str, want: f64) {
    let got = fit.learner.ranges(FIT_PROVIDER, FIT_WINDOW);
    let (est, lo, hi) = got.get(key).copied().unwrap_or_else(|| panic!("{key} has no range"));
    assert!((est / want - 1.0).abs() <= 0.10, "{key}: fitted {est}, true {want}");
    assert!(lo <= want && want <= hi, "{key}: the 95% range {lo}..{hi} misses the truth {want}");
}

#[test]
fn accounts_with_equal_weights_pool_them_and_fit_each_capacity_alone() {
    let reqs = plan(SEED);
    let week = pooled_week(&reqs, THREE_TIMES_WEIGHTS);
    let fit = week.fit.as_ref().expect("a fit run");
    let states = fit.learner.number_states(FIT_PROVIDER, FIT_WINDOW);
    // US1 scenario 4: one estimate for the weights, one capacity per account.
    assert!(matches!(states.get("weight.output"), Some(NumberState::Fitted { .. })), "weight.output is {:?}", states.get("weight.output"));
    assert!(
        states.keys().all(|k| !k.starts_with("weight.") || !k.contains('@')),
        "a weight was fitted per account: {:?}",
        states.keys().collect::<Vec<_>>()
    );
    assert!(fit.learner.splits(FIT_PROVIDER, FIT_WINDOW).is_empty(), "equal accounts were split: {:?}", fit.learner.splits(FIT_PROVIDER, FIT_WINDOW));
    assert_in_range(fit, "weight.output", THREE_TIMES_RATIOS[0]);
    let base = (peak_five_hours(&reqs, THREE_TIMES_WEIGHTS) / PEAK_SHARE).round();
    let published = fit.fits.window(FIT_PROVIDER, FIT_WINDOW);
    for (name, share) in POOL_ACCOUNTS.iter().zip(POOL_SHARES) {
        let key = format!("capacity@{name}");
        assert!(matches!(states.get(&key), Some(NumberState::Fitted { .. })), "{key} is {:?}", states.get(&key));
        assert_in_range(fit, &key, (base * share).round());
        assert!(published.capacity.contains_key(*name), "{key} is not published");
    }
}

#[test]
fn an_account_with_twice_the_output_weight_is_split_off_and_leaves_the_pooled_fit() {
    let reqs = plan(SEED);
    let week = pooled_week(&reqs, DOUBLE_OUTPUT_WEIGHTS);
    let fit = week.fit.as_ref().expect("a fit run");
    // US1 scenario 5: the odd account is split off, naming the number; the others are not.
    let splits = fit.learner.splits(FIT_PROVIDER, FIT_WINDOW);
    let reason = splits.get("a3").unwrap_or_else(|| panic!("a3 was not split off: {splits:?}"));
    assert!(reason.contains("weight.output"), "the reason doesn't name weight.output: {reason}");
    assert_eq!(splits.len(), 1, "only the odd account is split: {splits:?}");
    // Without its rows the pooled output weight is the other accounts' 15, not a blend toward 30.
    assert_in_range(fit, "weight.output", THREE_TIMES_RATIOS[0]);
    assert!(fit.learner.pooled_note(FIT_PROVIDER, FIT_WINDOW).is_none(), "two accounts remain pooled");
}

// ---------------------------------------------------------------------------------------------
// Outside use (T020; US2 scenarios 2 and 5; SC-004; research R6, R10)

/// A week with outside use injected on the fitted account: the run and the drops that were applied.
fn run_outside(scenario: &Scenario, reqs: &[Req], outside: Vec<OutsideUse>) -> (Week, Vec<Injected>) {
    let (defs, mut world) = scenario.week();
    world.outside = outside;
    let dir = tempfile::tempdir().unwrap();
    let mut sim = Sim::new(&defs, dir.path()).with_fit(FitRun::new(defs.accounts.len()));
    for r in reqs {
        sim.handle(&mut world, r);
    }
    sim.journal.flush_blocking();
    assert!(sim.placed.iter().all(|p| p.served.is_some()), "every request was served");
    let injected: Vec<Injected> =
        world.injected.iter().filter(|i| i.account == 0 && i.window == FIT_WINDOW).cloned().collect();
    (Week { placed: std::mem::take(&mut sim.placed), fit: sim.fit.take() }, injected)
}

fn drop_at(at_ms: u64, pct: f64, busy: bool) -> OutsideUse {
    OutsideUse::Drop { account: 0, window: FIT_WINDOW.into(), at_ms, pct, busy }
}

fn rate(pct_per_hour: f64, office: bool) -> OutsideUse {
    OutsideUse::Rate { account: 0, window: FIT_WINDOW.into(), pct_per_hour, office }
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

/// Of the injected drops, how many are listed as `idle` or `busy` outside use overlapping the
/// instant they were applied, and the ones that are not.
fn listed(fit: &FitRun, injected: &[Injected]) -> (usize, Vec<Injected>) {
    let entries = outside::read(fit.home(), FIT_PROVIDER, FIT_ACCOUNT, None, None).expect("the outside-use file reads");
    let spans: Vec<_> = entries
        .iter()
        .filter(|e| matches!(e.ty, OutsideType::Idle | OutsideType::Busy))
        .filter_map(|e| {
            let start = e.start_time()?;
            let end = e.end.as_deref().and_then(clock::parse_rfc3339).unwrap_or(start);
            Some((start, end))
        })
        .collect();
    let (hit, missed): (Vec<_>, Vec<_>) =
        injected.iter().cloned().partition(|i| spans.iter().any(|(s, e)| *s <= at(i.at_ms) && at(i.at_ms) <= *e));
    (hit.len(), missed)
}

fn fitted_value(fit: &FitRun, key: &str) -> Option<f64> {
    let published = fit.fits.window(FIT_PROVIDER, FIT_WINDOW);
    match key {
        "weight.output" => published.weights.get(&TokenClass::Output).copied(),
        "weight.cache_read" => published.weights.get(&TokenClass::CacheRead).copied(),
        "weight.cache_write" => published.weights.get(&TokenClass::CacheWrite).copied(),
        _ => published.capacity.get(FIT_ACCOUNT).copied(),
    }
}

/// SC-004: capacity and the output weight are fitted in the run with outside use, and every fitted
/// value lies inside the 95% range of the clean run.
fn assert_inside_clean_ranges(clean: &FitRun, run: &FitRun, what: &str) {
    let states = run.learner.number_states(FIT_PROVIDER, FIT_WINDOW);
    let clean_ranges = clean.learner.ranges(FIT_PROVIDER, FIT_WINDOW);
    let capacity = format!("capacity@{FIT_ACCOUNT}");
    for key in [capacity.as_str(), "weight.output"] {
        assert!(matches!(states.get(key), Some(NumberState::Fitted { .. })), "{what}: {key} is {:?}, not fitted", states.get(key));
    }
    for key in [capacity.as_str(), "weight.output", "weight.cache_read", "weight.cache_write"] {
        if !matches!(states.get(key), Some(NumberState::Fitted { .. })) {
            continue;
        }
        let got = fitted_value(run, key).unwrap_or_else(|| panic!("{what}: {key} is fitted but not published"));
        let (_, lo, hi) = clean_ranges.get(key).copied().unwrap_or_else(|| panic!("{what}: the clean run has no range for {key}"));
        assert!(lo <= got && got <= hi, "{what}: {key} fitted {got} lies outside the clean run's 95% range {lo}..{hi}");
    }
}

#[test]
fn a_meter_three_times_off_is_fitted_inside_the_clean_range_with_outside_use() {
    let reqs = plan(SEED);
    let scenario = Scenario::three_times_off(&reqs);
    let clean = run_week(&scenario, &reqs, true);
    let clean_fit = clean.fit.as_ref().expect("a fit run");

    // Idle bursts, bursts during traffic (Wednesday to Friday, late morning and afternoon), and a steady rate.
    let idle = spread(&idle_polls(&reqs), 8);
    assert!(idle.len() >= 3, "only {} idle polls in the week to inject into", idle.len());
    let mut outside: Vec<OutsideUse> = idle.iter().map(|p| drop_at(p - 5 * MIN, 6.0, false)).collect();
    for day in 2..5 {
        for hour in [11, 15] {
            outside.push(drop_at(day * DAY + hour * HOUR + 25 * MIN, 12.0, true));
        }
    }
    outside.push(rate(1.5, false));
    let expected = idle.len() + 6;
    let (week, injected) = run_outside(&scenario, &reqs, outside);
    let fit = week.fit.as_ref().expect("a fit run");
    assert_eq!(injected.len(), expected, "every drop was applied");
    assert_inside_clean_ranges(clean_fit, fit, "bursts and a steady rate");

    let beyond: Vec<Injected> = injected.iter().filter(|i| i.pct.round() >= 2.0).cloned().collect();
    let (hit, missed) = listed(fit, &beyond);
    println!("outside use: {hit} of {} injected intervals listed ({} idle, {} busy)", beyond.len(), idle.len(), beyond.iter().filter(|i| i.busy).count());
    assert!(hit * 10 >= beyond.len() * 9, "{hit} of {} listed (SC-004 asks 90%); missed: {missed:?}", beyond.len());

    // The same, with a rate that follows the agents' daily rhythm.
    let (week, _) = run_outside(&scenario, &reqs, vec![rate(2.0, true)]);
    assert_inside_clean_ranges(clean_fit, week.fit.as_ref().expect("a fit run"), "an office-hours rate");
}

#[test]
fn a_right_plugin_gets_no_number_from_outside_use_and_a_steady_rate_is_reported() {
    let reqs = plan(SEED);
    let scenario = Scenario::right(&reqs);
    for (what, use_) in [("a steady rate", rate(1.5, false)), ("an office-hours rate", rate(2.0, true))] {
        let (week, _) = run_outside(&scenario, &reqs, vec![use_]);
        let fit = week.fit.as_ref().expect("a fit run");
        // US2 scenario 2: outside use alone makes no meter number significant.
        assert!(fit.fits.windows.is_empty(), "{what}: a right declaration got a significant number: {:?}", fit.fits.windows);
        assert_eq!(fit.first_change, None, "{what}");
        let entries = outside::read(fit.home(), FIT_PROVIDER, FIT_ACCOUNT, None, None).expect("the outside-use file reads");
        assert!(entries.iter().any(|e| e.ty == OutsideType::Steady), "{what}: no steady rate was reported ({} entries)", entries.len());
    }
}

#[test]
fn an_idle_change_of_one_step_is_not_listed() {
    let reqs = plan(SEED);
    let scenario = Scenario::right(&reqs);
    let idle = spread(&idle_polls(&reqs), 3);
    assert!(!idle.is_empty(), "no idle polls in the week to inject into");
    // One percent is one step of the polls' whole-percent readings: rounding noise (R6 rule 1).
    let outside: Vec<OutsideUse> = idle.iter().map(|p| drop_at(p - 5 * MIN, 1.0, false)).collect();
    let (week, injected) = run_outside(&scenario, &reqs, outside);
    let fit = week.fit.as_ref().expect("a fit run");
    assert_eq!(injected.len(), idle.len(), "every drop was applied");
    let (hit, _) = listed(fit, &injected);
    assert_eq!(hit, 0, "{hit} one-step changes were listed as outside use");
    assert!(fit.fits.windows.is_empty(), "a right declaration got a significant number: {:?}", fit.fits.windows);
}

// ---------------------------------------------------------------------------------------------
// Overrides (T041; US4 scenarios 1-3 and 5; FR-020)

/// An output weight that is neither the declared 5.0 nor the true 15.0.
const OVERRIDE_OUTPUT: f64 = 12.0;

fn weights_override(output: f64) -> NumberOverrides {
    NumberOverrides { weights: [None, Some(output), None, None], ..NumberOverrides::default() }
}

/// The output weight routing reads for account `i`'s window.
fn output_in_effect(fit: &FitRun, defs: &Defs, i: usize) -> f64 {
    fit.meters(i)
        .unwrap_or(&defs.accounts[i].meters)
        .iter()
        .find(|m| m.name == FIT_WINDOW)
        .and_then(|m| m.token_weights)
        .expect("a weighted window")
        .output
}

fn number<'a>(v: &'a WindowMeterView, name: &str) -> &'a NumberView {
    v.numbers.iter().find(|n| n.number == name).unwrap_or_else(|| panic!("the view has no {name}: {:?}", v.numbers))
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum SetWhen {
    /// Before the first poll: the override is in place while nothing is significant.
    Start,
    /// When the output weight has just become significant.
    AfterSignificance,
    /// At the start, and removed again after `REMOVED_AT` requests, before any poll could fit.
    RemovedEarly,
}

const REMOVED_AT: usize = 50;

/// A week of the three-times-off mock with an account override on `weight.output`; every request
/// after the override is set is checked to be placed with it.
fn override_week(reqs: &[Req], scenario: &Scenario, when: SetWhen) -> (FitRun, Defs) {
    let (defs, mut world) = scenario.week();
    let dir = tempfile::tempdir().unwrap();
    let mut sim = Sim::new(&defs, dir.path()).with_fit(FitRun::new(defs.accounts.len()));
    let mut active = false;
    if when != SetWhen::AfterSignificance {
        sim.fit.as_mut().expect("a fit run").set_account_override(&defs, 0, FIT_WINDOW, Some(weights_override(OVERRIDE_OUTPUT)));
        active = true;
    }
    let (mut before, mut after) = (0u64, 0u64);
    for (k, r) in reqs.iter().enumerate() {
        sim.handle(&mut world, r);
        let fit = sim.fit.as_mut().expect("a fit run");
        let published = fit.fits.window(FIT_PROVIDER, FIT_WINDOW);
        let significant = published.weights.contains_key(&TokenClass::Output);
        if when == SetWhen::AfterSignificance && !active && significant {
            // Until the override arrives, routing reads the fitted weight.
            let fitted = published.weights[&TokenClass::Output];
            let got = output_in_effect(fit, &defs, 0);
            assert!((got - fitted).abs() < 1e-9, "request {k}: the output weight in effect is {got}, the fit {fitted}");
            fit.set_account_override(&defs, 0, FIT_WINDOW, Some(weights_override(OVERRIDE_OUTPUT)));
            active = true;
        }
        if when == SetWhen::RemovedEarly && k == REMOVED_AT {
            assert!(fit.first_change.is_none(), "the fit was significant after only {REMOVED_AT} requests");
            fit.set_account_override(&defs, 0, FIT_WINDOW, None);
            assert!(fit.meters(0).is_none(), "with nothing fitted and no override, routing reads the declaration");
            assert!((output_in_effect(fit, &defs, 0) - 5.0).abs() < 1e-12, "the declared output weight is back");
            active = false;
        }
        if active {
            let got = output_in_effect(fit, &defs, 0);
            assert!((got - OVERRIDE_OUTPUT).abs() < 1e-12, "request {k}: routing reads output weight {got}, not the override");
            if significant {
                after += 1;
            } else {
                before += 1;
            }
        }
    }
    match when {
        SetWhen::Start => assert!(before > 0 && after > 0, "overridden requests: {before} before significance, {after} after"),
        SetWhen::AfterSignificance => assert!(after > 0, "no request after the override was set"),
        SetWhen::RemovedEarly => assert!(before >= REMOVED_AT as u64, "{before} overridden requests"),
    }
    sim.journal.flush_blocking();
    let fit = sim.fit.take().expect("a fit run");
    drop(sim);
    (fit, defs)
}

#[test]
fn an_account_override_is_used_before_and_after_significance_and_removing_it_returns_the_fit() {
    let reqs = plan(SEED);
    let scenario = Scenario::three_times_off(&reqs);
    for when in [SetWhen::Start, SetWhen::AfterSignificance] {
        let (mut fit, defs) = override_week(&reqs, &scenario, when);
        assert!(fit.fits.window(FIT_PROVIDER, FIT_WINDOW).weights.contains_key(&TokenClass::Output), "{when:?}: the output weight never became significant");
        let v = fit.view(&defs, 0, FIT_WINDOW);
        let n = number(&v, "weight.output");
        assert_eq!(n.source, Source::AccountOverride, "{when:?}");
        assert_eq!(n.account_override, Some(OVERRIDE_OUTPUT), "{when:?}");
        assert_eq!(n.in_use, Some(OVERRIDE_OUTPUT), "{when:?}");
        assert_eq!(n.declared, Some(5.0), "{when:?}");
        // The fit goes on beside the override and the view shows its range.
        let range = n.fit.as_ref().unwrap_or_else(|| panic!("{when:?}: the view shows no fit range"));
        let want = THREE_TIMES_RATIOS[0];
        assert!(range.low <= want && want <= range.high, "{when:?}: the range {}..{} misses {want}", range.low, range.high);

        // Removed: the fit is used (it is significant).
        fit.set_account_override(&defs, 0, FIT_WINDOW, None);
        let v = fit.view(&defs, 0, FIT_WINDOW);
        let n = number(&v, "weight.output");
        assert_eq!(n.source, Source::Fit, "{when:?}: after removal");
        assert_eq!(n.account_override, None, "{when:?}");
        let used = n.in_use.expect("a weight in use");
        assert!((used / want - 1.0).abs() <= 0.10, "{when:?}: after removal {used} is in use, true {want}");
    }
}

#[test]
fn an_override_removed_before_anything_is_significant_returns_the_declaration() {
    let reqs = plan(SEED);
    let scenario = Scenario::three_times_off(&reqs);
    // The checks are inside: the override is read on every request until it is removed, and the
    // declaration after.
    let (fit, defs) = override_week(&reqs, &scenario, SetWhen::RemovedEarly);
    // Later in the week the fit does gain a number, and routing then reads it, not the override.
    let v = fit.view(&defs, 0, FIT_WINDOW);
    assert_eq!(number(&v, "weight.output").account_override, None);
}

const WITH_MULTIPLIERS: &str = "token_weights = { input = 1.0, output = 5.0, cache_read = 0.1, cache_write = 1.25 }\nmodel_multiplier = { \"m\" = 1.0, \"other-*\" = 1.0 }";

#[test]
fn a_multiplier_override_applies_only_to_the_models_it_names() {
    // The week's accounts serve one model, so this checks the meter the fit hands routing directly.
    let meter = five_hour(1.0e9, WITH_MULTIPLIERS);
    let spent = |model: &str| Spent { model: model.into(), requests: 1, input: 1000, output: 100, cache_read: 0, cache_write: 0 };
    let base = cost_spent(&meter, &spent("m"));
    let mult = |glob: &str, v: f64| NumberOverrides { multipliers: IndexMap::from([(glob.to_owned(), v)]), ..NumberOverrides::default() };
    let none = WindowFit::default();
    let key = |g: &str| MeterNumber::Multiplier(g.to_owned());

    let (m, src) = in_effect(&meter, None, Some(&mult("other-*", 3.0)), &none, "acct");
    assert_eq!(cost_spent(&m, &spent("m")), base, "a model the override doesn't match is charged as declared");
    assert_eq!(cost_spent(&m, &spent("other-x")), base * 3.0);
    assert_eq!(src.get(&key("other-*")), Some(&Source::AccountOverride));
    assert!(!src.contains_key(&key("m")));

    let (m, src) = in_effect(&meter, Some(&mult("m", 2.0)), None, &none, "acct");
    assert_eq!(cost_spent(&m, &spent("m")), base * 2.0);
    assert_eq!(cost_spent(&m, &spent("other-x")), base);
    assert_eq!(src.get(&key("m")), Some(&Source::PluginOverride));

    let (m, src) = in_effect(&meter, Some(&mult("m", 2.0)), Some(&mult("m", 4.0)), &none, "acct");
    assert_eq!(cost_spent(&m, &spent("m")), base * 4.0, "the account's own override wins over the plugin's");
    assert_eq!(src.get(&key("m")), Some(&Source::AccountOverride));
}

#[test]
fn a_plugin_override_applies_to_every_account_but_one_with_its_own() {
    let reqs = plan(SEED);
    let (week, _, defs) = pooled_run(&reqs, THREE_TIMES_WEIGHTS, Vec::new(), |defs, fit| {
        fit.set_plugin_override(defs, FIT_WINDOW, Some(weights_override(OVERRIDE_OUTPUT)));
        fit.set_account_override(defs, 1, FIT_WINDOW, Some(weights_override(20.0)));
    });
    let mut fit = week.fit.expect("a fit run");
    assert!(fit.fits.window(FIT_PROVIDER, FIT_WINDOW).weights.contains_key(&TokenClass::Output), "the output weight never became significant");
    let want = THREE_TIMES_RATIOS[0];
    for (i, value, source) in [(0, 12.0, Source::PluginOverride), (1, 20.0, Source::AccountOverride), (2, 12.0, Source::PluginOverride)] {
        assert_eq!(output_in_effect(&fit, &defs, i), value, "account {i}");
        let v = fit.view(&defs, i, FIT_WINDOW);
        let n = number(&v, "weight.output");
        assert_eq!(n.source, source, "account {i}");
        assert_eq!(n.plugin_override, Some(12.0), "account {i}");
        assert_eq!(n.account_override, (i == 1).then_some(20.0), "account {i}");
        assert_eq!(n.in_use, Some(value), "account {i}");
    }
    // Without its own override the account follows the plugin's.
    fit.set_account_override(&defs, 1, FIT_WINDOW, None);
    assert_eq!(output_in_effect(&fit, &defs, 1), 12.0);
    assert_eq!(number(&fit.view(&defs, 1, FIT_WINDOW), "weight.output").source, Source::PluginOverride);
    // Without the plugin's, every account reads the fit.
    fit.set_plugin_override(&defs, FIT_WINDOW, None);
    for i in 0..3 {
        let v = fit.view(&defs, i, FIT_WINDOW);
        let n = number(&v, "weight.output");
        assert_eq!(n.source, Source::Fit, "account {i}");
        let used = n.in_use.expect("a weight in use");
        assert!((used / want - 1.0).abs() <= 0.10, "account {i}: {used} in use, true {want}");
    }
}

// ---------------------------------------------------------------------------------------------
// A provider that changes its rules (T046; US5 scenario 2; SC-006)

/// Thursday 06:00: the fourth day, before its working hours.
const HALVE_AT: u64 = 3 * DAY + 6 * HOUR;

#[test]
fn a_halved_capacity_is_a_break_found_within_a_day_and_relearned_from_the_rows_after_it() {
    let reqs = plan(SEED);
    let scenario = Scenario::three_times_off(&reqs);
    let (defs, mut world) = scenario.week();
    let new_capacity = (scenario.true_capacity / 2.0).round();
    world.set_true_capacity(HALVE_AT, 0, FIT_WINDOW, new_capacity);
    let dir = tempfile::tempdir().unwrap();
    let mut sim = Sim::new(&defs, dir.path()).with_fit(FitRun::new(defs.accounts.len()));
    // Exclusive-use, so that a busy row wrongly called outside use would raise an alert.
    sim.fit.as_mut().expect("a fit run").set_exclusive(&defs, 0, Some(at(0)));
    let cap_key = format!("capacity@{FIT_ACCOUNT}");

    let mut old: Option<f64> = None;
    let mut known_at: Option<u64> = None;
    let mut after = 0u64;
    for r in &reqs {
        sim.handle(&mut world, r);
        let fit = sim.fit.as_ref().expect("a fit run");
        match fit.learner.number_states(FIT_PROVIDER, FIT_WINDOW).get(&cap_key) {
            Some(NumberState::Fitted { .. }) if known_at.is_none() => {
                old = fit.fits.window(FIT_PROVIDER, FIT_WINDOW).capacity.get(FIT_ACCOUNT).copied().or(old);
            }
            Some(NumberState::Relearning { .. }) if known_at.is_none() => known_at = Some(r.at_ms),
            _ => {}
        }
        if let (Some(_), Some(was)) = (known_at, old) {
            // SC-006: from the request that saw the report on, the old fitted capacity is not used.
            let now_in_effect = fit.capacity_in_effect(&defs, 0).expect("a capacity in effect");
            assert!((now_in_effect / was - 1.0).abs() > 0.10, "request {} is placed with {now_in_effect}, the old fitted {was}", r.n);
            after += 1;
        }
    }
    sim.journal.flush_blocking();
    let fit = sim.fit.as_ref().expect("a fit run");
    let known = known_at.expect("the halving was never reported as a break");
    assert!(after > 0);
    assert!(known >= HALVE_AT && known <= HALVE_AT + DAY, "the break was reported at +{:.1}h, the halving was at +{:.1}h", known as f64 / HOUR as f64, HALVE_AT as f64 / HOUR as f64);

    let Loaded::Ok(stored) = store::load(fit.home(), FIT_PROVIDER).expect("the fit state reads") else { panic!("no fit state saved") };
    let breaks = &stored.windows.get(FIT_WINDOW).expect("the window is stored").breaks;
    let found: Vec<_> = breaks.iter().filter(|b| b.number == cap_key).collect();
    assert_eq!(found.len(), 1, "breaks recorded: {breaks:?}");
    let b = found[0];
    let (b_at, detected) = (clock::parse_rfc3339(&b.at).expect("a time"), clock::parse_rfc3339(&b.detected_at).expect("a time"));
    assert!(b_at >= at(HALVE_AT - 3 * HOUR) && b_at <= detected, "the break is placed at {}", b.at);
    assert!(detected <= at(HALVE_AT + DAY), "detected at {}", b.detected_at);
    assert!((b.replaced / scenario.true_capacity - 1.0).abs() <= 0.10, "the replaced value {} was not the old capacity {}", b.replaced, scenario.true_capacity);

    // Only rows after the break count: the new fit is the new capacity, not a blend with the old.
    let states = fit.learner.number_states(FIT_PROVIDER, FIT_WINDOW);
    match states.get(&cap_key) {
        Some(NumberState::Fitted { since }) => assert!(*since >= b_at, "fitted since {since:?}, before the break"),
        other => panic!("{cap_key} is {other:?} at the end of the week, not fitted again"),
    }
    let got = fitted_value(fit, &cap_key).expect("the new capacity is published");
    assert!((got / new_capacity - 1.0).abs() <= 0.10, "relearned {got}, true {new_capacity}");
    let (_, lo, hi) = fit.learner.ranges(FIT_PROVIDER, FIT_WINDOW).get(&cap_key).copied().expect("a range");
    assert!(lo <= new_capacity && new_capacity <= hi, "the 95% range {lo}..{hi} misses {new_capacity}");

    // The busy rows around the halving were a rule change, not outside use.
    let entries = outside::read(fit.home(), FIT_PROVIDER, FIT_ACCOUNT, None, None).expect("the outside-use file reads");
    let stray: Vec<_> = entries
        .iter()
        .filter(|e| matches!(e.ty, OutsideType::Idle | OutsideType::Busy))
        .filter(|e| e.start_time().is_some_and(|t| t >= at(HALVE_AT - HOUR) && t <= detected))
        .collect();
    assert!(stray.is_empty(), "rows around the halving were listed as outside use: {stray:?}");
    let alerts = outside::alerts(fit.home(), FIT_PROVIDER, FIT_ACCOUNT);
    assert!(alerts.is_empty(), "the halving raised alerts: {alerts:?}");
    println!("halved capacity: reported +{:.1}h after the halving; {after} requests placed after the report", (known - HALVE_AT) as f64 / HOUR as f64);
}

// ---------------------------------------------------------------------------------------------
// Restart and crash (T047; US5 scenario 5; SC-008)

/// Friday noon.
const RESTART_AT: u64 = 4 * DAY + 12 * HOUR;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Restart {
    Never,
    /// Between two polls.
    Clean,
    /// At the first poll after `RESTART_AT`: the entry is in the history, the process dies before
    /// learning from it.
    Crash,
}

/// What a run's fit left behind, in a form two runs can be compared in.
#[derive(Debug)]
struct Outcome {
    states: BTreeMap<String, String>,
    numbers: Vec<(String, f64)>,
    breaks: Vec<(String, String)>,
    /// `(type, start, end, amount or rate)`.
    entries: Vec<(String, String, String, f64)>,
    alerts: Vec<String>,
    exclusive: Vec<(usize, String)>,
}

fn describe(s: &NumberState) -> String {
    match s {
        NumberState::Learning { .. } => "learning".into(),
        NumberState::Fitted { since } => format!("fitted@{}", rfc3339_millis(*since)),
        NumberState::Relearning { since, .. } => format!("relearning@{}", rfc3339_millis(*since)),
        NumberState::Restarted { since, reason } => format!("restarted@{}:{reason}", rfc3339_millis(*since)),
        other => format!("{other:?}"),
    }
}

fn outcome_of(fit: &FitRun) -> Outcome {
    let states = fit.learner.number_states(FIT_PROVIDER, FIT_WINDOW).iter().map(|(k, s)| (k.clone(), describe(s))).collect();
    let published = fit.fits.window(FIT_PROVIDER, FIT_WINDOW);
    let mut numbers: Vec<(String, f64)> = published.capacity.iter().map(|(a, v)| (format!("capacity@{a}"), *v)).collect();
    numbers.extend(published.weights.iter().map(|(c, v)| (format!("weight.{}", c.as_str()), *v)));
    for (k, (est, lo, hi)) in fit.learner.ranges(FIT_PROVIDER, FIT_WINDOW) {
        numbers.extend([(format!("{k}.estimate"), est), (format!("{k}.low"), lo), (format!("{k}.high"), hi)]);
    }
    let Loaded::Ok(stored) = store::load(fit.home(), FIT_PROVIDER).expect("the fit state reads") else { panic!("no fit state saved") };
    let mut breaks: Vec<(String, String)> =
        stored.windows.values().flat_map(|w| w.breaks.iter().map(|b| (b.at.clone(), b.number.clone()))).collect();
    breaks.sort();
    let mut entries: Vec<(String, String, String, f64)> = outside::read(fit.home(), FIT_PROVIDER, FIT_ACCOUNT, None, None)
        .expect("the outside-use file reads")
        .into_iter()
        .map(|e| (format!("{:?}", e.ty), e.start.clone(), e.end.clone().unwrap_or_default(), e.amount.or(e.rate_per_hour).unwrap_or(0.0)))
        .collect();
    entries.sort_by(|a, b| (&a.0, &a.1, &a.2).cmp(&(&b.0, &b.1, &b.2)));
    let mut alerts: Vec<String> =
        outside::alerts(fit.home(), FIT_PROVIDER, FIT_ACCOUNT).into_iter().map(|a| a.text.unwrap_or_default()).collect();
    alerts.sort();
    let exclusive = fit.exclusive().iter().map(|(i, t)| (*i, rfc3339_millis(*t))).collect();
    Outcome { states, numbers, breaks, entries, alerts, exclusive }
}

fn same(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-4 * a.abs().max(b.abs()).max(1.0)
}

fn assert_same(what: &str, a: &Outcome, b: &Outcome) {
    assert_eq!(a.states, b.states, "{what}: number states");
    assert_eq!(a.breaks, b.breaks, "{what}: breaks");
    assert_eq!(a.exclusive, b.exclusive, "{what}: exclusive-use declarations");
    assert_eq!(a.alerts, b.alerts, "{what}: alerts");
    assert_eq!(a.numbers.len(), b.numbers.len(), "{what}: fitted numbers {:?} against {:?}", a.numbers, b.numbers);
    for ((ka, va), (kb, vb)) in a.numbers.iter().zip(&b.numbers) {
        assert!(ka == kb && same(*va, *vb), "{what}: {ka} is {va}, then {kb} is {vb}");
    }
    assert_eq!(a.entries.len(), b.entries.len(), "{what}: outside-use entries {:?} against {:?}", a.entries, b.entries);
    for (x, y) in a.entries.iter().zip(&b.entries) {
        assert!((&x.0, &x.1, &x.2) == (&y.0, &y.1, &y.2) && same(x.3, y.3), "{what}: entry {x:?} against {y:?}");
    }
}

/// Idle drops (never within an hour of a burst), and busy bursts.
fn drops_and_bursts(reqs: &[Req], idle_n: usize, bursts: &[(u64, u64)], burst_pct: f64) -> Vec<(u64, f64, bool)> {
    let burst_at: Vec<u64> = bursts.iter().map(|(d, h)| d * DAY + h * HOUR + 25 * MIN).collect();
    let idle: Vec<u64> =
        idle_polls(reqs).into_iter().filter(|p| *p >= 2 * DAY && burst_at.iter().all(|b| p.abs_diff(*b) > HOUR)).collect();
    let sizes = [2.0, 3.0, 6.0];
    let mut out: Vec<(u64, f64, bool)> = spread(&idle, idle_n).iter().enumerate().map(|(k, p)| (p - 5 * MIN, sizes[k % 3], false)).collect();
    out.extend(burst_at.iter().map(|b| (*b, burst_pct, true)));
    out
}

fn restart_week(reqs: &[Req], mode: Restart) -> Outcome {
    let scenario = Scenario::three_times_off(reqs);
    let (defs, mut world) = scenario.week();
    world.set_true_capacity(HALVE_AT, 0, FIT_WINDOW, (scenario.true_capacity / 2.0).round());
    world.outside = drops_and_bursts(reqs, 10, &[(1, 11), (2, 15), (4, 16)], 15.0)
        .into_iter()
        .map(|(at_ms, pct, busy)| drop_at(at_ms, pct, busy))
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let mut sim = Sim::new(&defs, dir.path()).with_fit(FitRun::new(defs.accounts.len()));
    sim.fit.as_mut().expect("a fit run").set_exclusive(&defs, 0, Some(at(0)));
    let mut done = false;
    for r in reqs {
        sim.handle(&mut world, r);
        if !done && r.at_ms >= RESTART_AT {
            done = true;
            let fit = sim.fit.as_mut().expect("a fit run");
            match mode {
                Restart::Never => {}
                Restart::Clean => fit.restart(&defs, at(r.at_ms / POLL_MS * POLL_MS)),
                Restart::Crash => fit.arm_crash(),
            }
        }
    }
    sim.journal.flush_blocking();
    let fit = sim.fit.as_ref().expect("a fit run");
    assert_eq!(fit.crashes, u32::from(mode == Restart::Crash), "{mode:?}");
    outcome_of(fit)
}

#[test]
fn a_clean_restart_and_a_crash_mid_week_replay_to_the_same_fits_entries_and_alerts() {
    let reqs = plan(SEED);
    let reference = restart_week(&reqs, Restart::Never);
    // The comparison must have something in it.
    assert!(reference.states.values().any(|s| s.starts_with("fitted@")), "{:?}", reference.states);
    assert!(!reference.breaks.is_empty(), "the halving left no break");
    assert!(reference.entries.iter().any(|e| e.0 == "Idle"), "no idle outside use was listed");
    assert!(!reference.alerts.is_empty(), "no alert was raised");
    assert_eq!(reference.exclusive.len(), 1);
    for mode in [Restart::Clean, Restart::Crash] {
        let run = restart_week(&reqs, mode);
        assert_same(&format!("{mode:?}"), &reference, &run);
    }
}

// ---------------------------------------------------------------------------------------------
// Usage alerts (T055; US6 scenarios 1, 3, 4, 8; SC-007)

/// A busy burst well beyond what a ten-minute stretch of traffic costs (about 2% at the peak).
const BURST_PCT: f64 = 15.0;

fn overlaps(e: &OutsideEntry, t: std::time::SystemTime) -> bool {
    let Some(start) = e.start_time() else { return false };
    let end = e.end.as_deref().and_then(clock::parse_rfc3339).unwrap_or(start);
    start <= t && t <= end
}

#[test]
fn alerts_follow_idle_drops_and_busy_bursts_on_an_exclusive_account_and_nothing_else() {
    let reqs = plan(SEED);
    let bursts = [(2, 10), (2, 16), (3, 11), (3, 15), (4, 10), (4, 16)];
    // Half of these idle polls get a real drop (2, 3 or 6 steps), the others one step of noise.
    let idle: Vec<u64> = idle_polls(&reqs)
        .into_iter()
        .filter(|p| *p >= 2 * DAY && bursts.iter().all(|(d, h)| p.abs_diff(d * DAY + h * HOUR + 25 * MIN) > HOUR))
        .collect();
    let picks = spread(&idle, 18);
    assert!(picks.len() >= 12, "only {} idle polls in the week to inject into", picks.len());
    let sizes = [2.0, 3.0, 6.0];
    let drops: Vec<(u64, f64)> = picks.iter().step_by(2).enumerate().map(|(k, p)| (*p, sizes[k % 3])).collect();
    let noise: Vec<u64> = picks.iter().skip(1).step_by(2).copied().collect();
    let mut outside_use = Vec::new();
    // Account 0 is exclusive-use; account 1 gets the same use and is not.
    for account in [0, 1] {
        let mk = |at_ms: u64, pct: f64, busy: bool| OutsideUse::Drop { account, window: FIT_WINDOW.into(), at_ms, pct, busy };
        outside_use.extend(drops.iter().map(|(p, pct)| mk(p - 5 * MIN, *pct, false)));
        outside_use.extend(noise.iter().map(|p| mk(p - 5 * MIN, 1.0, false)));
        outside_use.extend(bursts.iter().map(|(d, h)| mk(d * DAY + h * HOUR + 25 * MIN, BURST_PCT, true)));
    }
    let (week, injected, _defs) = pooled_run(&reqs, THREE_TIMES_WEIGHTS, outside_use, |defs, fit| fit.set_exclusive(defs, 0, Some(at(0))));
    let fit = week.fit.as_ref().expect("a fit run");
    let mine: Vec<&Injected> = injected.iter().filter(|i| i.account == 0).collect();
    assert_eq!(mine.len(), drops.len() + noise.len() + bursts.len(), "every drop was applied");

    // Scenario 1: nothing is alerted on an account that is not exclusive-use, though its use is listed.
    let (exclusive, other) = (POOL_ACCOUNTS[0], POOL_ACCOUNTS[1]);
    let listed_other = outside::read(fit.home(), FIT_PROVIDER, other, None, None).expect("the outside-use file reads");
    assert!(listed_other.iter().any(|e| e.ty == OutsideType::Idle), "the non-exclusive account's idle use was not listed");
    let alerts_other = outside::alerts(fit.home(), FIT_PROVIDER, other);
    assert!(alerts_other.is_empty(), "alerts on the non-exclusive account: {alerts_other:?}");

    let entries = outside::read(fit.home(), FIT_PROVIDER, exclusive, None, None).expect("the outside-use file reads");
    let alerts = outside::alerts(fit.home(), FIT_PROVIDER, exclusive);
    let entry_of = |id: &str| entries.iter().find(|e| e.id == id);

    // Scenarios 3 and 4: every alert (steady rates aside) stands on an injected change of 2 steps or more.
    for a in &alerts {
        let e = entry_of(&a.entry).unwrap_or_else(|| panic!("alert {a:?} names no listed entry"));
        if e.ty == OutsideType::Steady {
            continue;
        }
        assert!(
            mine.iter().any(|i| i.pct.round() >= 2.0 && overlaps(e, at(i.at_ms))),
            "an alert without injected use (1-step noise or ordinary traffic): {a:?} on {e:?}"
        );
    }
    // Every idle drop is alerted at the first poll that shows it.
    for (p, pct) in &drops {
        let raised = rfc3339_millis(at(*p));
        let hit = alerts.iter().any(|a| a.raised_at == raised && entry_of(&a.entry).is_some_and(|e| overlaps(e, at(p - 5 * MIN))));
        assert!(hit, "the idle drop of {pct}% shown at poll +{:.2}h raised no alert at that poll", *p as f64 / HOUR as f64);
    }
    // Scenario 8: each burst that passes the test is alerted (the bursts are far beyond it).
    for i in mine.iter().filter(|i| i.busy) {
        let hit = alerts.iter().any(|a| entry_of(&a.entry).is_some_and(|e| overlaps(e, at(i.at_ms))));
        assert!(hit, "the busy burst at +{:.2}h raised no alert", i.at_ms as f64 / HOUR as f64);
    }
    println!("alerts: {} on the exclusive account for {} idle drops and {} bursts, {} noise steps; none on the other", alerts.len(), drops.len(), bursts.len(), noise.len());
}

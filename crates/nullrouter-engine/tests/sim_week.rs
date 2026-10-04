//! A simulated week proves the routing rules (spec 006, US6, T081–T084, research R17).
//!
//! Twelve agents send about 60,000 requests over mock accounts whose windows charge exactly by
//! their meters. The decision core, settlement and the real journal run on an injected clock;
//! only HTTP is left out. The run checks SC-001 to SC-006, restarts from the journal files in the
//! middle (SC-005), and prints one table.
//!
//! `nice cargo test -p nullrouter-engine --release --test sim_week -j 2 -- --nocapture`

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
use nullrouter_engine::routing::ledger::window_start;
use nullrouter_engine::routing::{
    CandidateKey, CandidateRow, Decision, DecisionKind, MovedBecause, PlacementReason, PriceSpec, Tier, WhyNot,
};
use nullrouter_registry::schema::{MeterUnit, RoutingDecl};
use serde_json::Value;

use router::{AccountDef, Defs, Sim, copy_dir, journal_options, usage_of};
use world::{DAY, HOUR, MIN, Req, Reset, Rng, TrueAccount, TrueWindow, WEEK_MS, World, at, plan};

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
        format!("[[window]]\nname = \"{name}\"\nlength = \"{length}\"\nunit = \"weighted_tokens\"\ncapacity = {capacity}\n{WEIGHTS}\n{extra}\n")
    };
    let max = tokens("5-hour", "5h", CAP.max_5h, "reserve = \"10%\"")
        + &tokens("weekly", "7d", CAP.max_week, "")
        + "[[window]]\nname = \"per-minute\"\nlength = \"1m\"\nunit = \"requests\"\ncapacity = 5\n";
    let pro = tokens("5-hour", "5h", CAP.pro_5h, "reserve = \"10%\"") + &tokens("weekly", "7d", CAP.pro_week, "");
    let daily = format!("[[window]]\nname = \"daily\"\nlength = \"1d\"\nunit = \"requests\"\ncapacity = {}\n", CAP.kimi_day);
    let glm = tokens("daily", "1d", CAP.glm_day, "reset = \"fixed\"\nanchor = \"00:00+00:00\"");
    Defs {
        accounts: vec![
            sub("anthropic/max", 0, max, true),
            sub("anthropic/pro", 1, pro, true),
            sub("kimi/daily", 2, daily, true),
            sub("glm/plan", 3, glm, false),
            payg("openrouter/key", 4, "[[price]]\nwhen = { from = \"14:00\", to = \"22:00\" }\ninput = 3.0\n[[price]]\ninput = 1.5\n"),
            payg("deepseek/key", 5, "[[price]]\nwhen = { from = \"01:00\", to = \"09:00\" }\ninput = 0.8\n[[price]]\ninput = 1.6\n"),
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
                defs.accounts.iter().position(|d| a["provider"] == d.key.provider.as_str() && a["account"] == d.key.account.as_str())
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
    let tier = if d.candidates.iter().any(|c| c.eligible && c.tier == Tier::Subscription && c.weight.unwrap_or(0.0) > 0.0) {
        Tier::Subscription
    } else {
        Tier::Payg
    };
    let mut rows: Vec<(usize, &CandidateRow)> = d.candidates.iter().enumerate().filter(|(_, c)| c.eligible && c.tier == tier).collect();
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
        *self.kinds.entry(match d.kind {
            DecisionKind::Warm => "warm",
            DecisionKind::Cold => "cold",
            DecisionKind::Overflow => "overflow",
            DecisionKind::None => "none",
        }).or_default() += 1;
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
                let key = u64::try_from(d.amortization_window.start.duration_since(at(0)).unwrap_or_default().as_millis()).unwrap_or(0);
                let w = self.windows.entry(key).or_insert_with(|| Window { target: vec![0.0; n], received: vec![0.0; n], ..Window::default() });
                w.count += 1;
                w.total += plain as f64;
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
            let sub_can_serve = rows.iter().any(|c| c.eligible && c.tier == Tier::Subscription && c.weight.unwrap_or(0.0) > 0.0);
            if h.stayed {
                self.warm_stays += 1;
                let first = d.order.first().map(|i| &rows[*i]);
                if first.is_none_or(|f| f.provider != h.provider || f.account != h.account) || d.kind != DecisionKind::Warm {
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
        if let Some(s) = r.served.filter(|s| rows.iter().any(|c| defs.index_of(&c.key()) == *s && c.tier == Tier::Payg)) {
            self.payg_served += 1;
            let first = d.order.first().map(|i| defs.index_of(&rows[*i].key()));
            let sub_can_serve = rows.iter().any(|c| c.eligible && c.tier == Tier::Subscription && c.weight.unwrap_or(0.0) > 0.0);
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
            let eligible: Vec<&CandidateRow> = rows.iter().filter(|c| c.eligible && c.tier == Tier::Subscription && c.weight.unwrap_or(0.0) > 0.0).collect();
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
                let top = rows.iter().filter(|c| c.eligible && c.tier == rows[got].tier).filter_map(|c| c.deficit_before).max().unwrap_or(0);
                if rows[got].deficit_before.unwrap_or(0) >= top - 1 {
                    // The ledger compares unrounded deficits; the record keeps whole tokens.
                    self.within_rounding += 1;
                } else {
                    self.bad_recompute.push(format!("{id}: rows choose {} but it went to {}", name(defs, want), name(defs, got)));
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
                let off = (w.received[i] - w.target[i]).abs() / w.total;
                if off > 0.05 {
                    bad.push(format!(
                        "SC-001: {} in the window at +{}h received {:.1}% of the cold work against a target of {:.1}%",
                        name(defs, i),
                        start / HOUR,
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
            if e.left_frac > e.reserve + 0.05 && self.passed_over.iter().any(|(t, i)| *i == account_of(defs, e) && *t >= e.started_ms && *t <= e.at_ms) {
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
                bad.push(format!("SC-004: {} {} ended at {:.0}% left, under its floor", name(defs, e.account), e.window, e.left_frac * 100.0));
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
        w(&mut out, format!("requests {}  unserved {}  failed attempts {}", self.requests, self.unserved, self.failed_attempts));
        w(&mut out, format!("decisions {:?}; cold work passed over an account that owed more: {}", self.kinds, self.passed_over.len()));
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
            .flat_map(|w| (0..defs.accounts.len()).map(move |i| (w.received[i] - w.target[i]).abs() / w.total))
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
                    format!("{:<16} {:<11} {:>6} {target:>9.1} {actual:>9.1} {:>8} {:>8} {:>8} {:>8}", d.key.account_key(), "-", 0, "-", "-", "-", self.served_by[i]),
                );
            }
            for window in names {
                let left: Vec<f64> = events.iter().filter(|e| e.account == i && e.window == window).map(|e| e.left_frac * 100.0).collect();
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
    assert!((50_000..70_000).contains(&reqs.len()), "about 60,000 requests, got {} ({sessions} in sessions)", reqs.len());
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
    assert_eq!(restarted.deficits().len() <= control.deficits().len() + 6, true);
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
    assert!(bad.is_empty(), "{} failures, the first ten:\n{}", bad.len(), bad.iter().take(10).cloned().collect::<Vec<_>>().join("\n"));
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
        assert!(r.at_ms / 1000 >= last_second, "request {} at second {} is gone; the last second is {last_second}", r.n, r.at_ms / 1000);
    }
    println!("power loss at +{:.2}h: {} of the {} requests before it are missing, all from the last simulated second", cut_ms as f64 / HOUR as f64, missing.len(), cut);
    // The routing state still loads, and holds the work of every earlier second.
    let loaded = state::load(dir.path());
    assert!(!loaded.warm.is_empty() && !loaded.ledgers.is_empty());
    let _ = (window_start(at(0), defs.amortization), journal_options());
}

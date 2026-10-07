//! The provider side of the simulated week (research R17): a seeded traffic plan, and accounts
//! whose windows charge exactly what their meters say.
//!
//! Nothing here knows about the router. The router sees only what a poll reports and what its
//! own counted traffic says.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use nullrouter_engine::quota::extract::QuotaWindow;
use nullrouter_engine::routing::meter::{Spent, cost_spent};
use nullrouter_registry::schema::{MeterDecl, MeterUnit, QuotaUnit};

pub const MIN: u64 = 60_000;
pub const HOUR: u64 = 3_600_000;
pub const DAY: u64 = 24 * HOUR;
pub const WEEK_MS: u64 = 7 * DAY;
/// The prompt cache lifetime of every simulated provider.
pub const CACHE_MS: u64 = 5 * MIN;

/// Monday 2026-09-28 00:00 UTC: the week's first instant.
pub fn t0() -> SystemTime {
    nullrouter_engine::clock::parse_rfc3339("2026-09-28T00:00:00Z").expect("a time")
}

pub fn at(ms: u64) -> SystemTime {
    t0() + Duration::from_millis(ms)
}

/// SplitMix64: 20 lines, no dependency, the same stream on every machine.
#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// In `[0, 1)`.
    pub fn f(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// In `lo..=hi`.
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo + 1)
    }

    /// An exponential gap with this mean.
    pub fn exp(&mut self, mean: f64) -> f64 {
        -mean * (1.0 - self.f()).ln()
    }
}

/// A hash seed for a prompt prefix: the same seed is the same prefix.
pub fn mix(a: u64, b: u64) -> u64 {
    Rng::new(a ^ b.rotate_left(32).wrapping_mul(0x2545_F491_4F6C_DD1D)).next()
}

// ---------------------------------------------------------------------------------------------
// Traffic

/// One request of the week.
#[derive(Clone, Debug)]
pub struct Req {
    pub n: usize,
    pub at_ms: u64,
    pub agent: usize,
    /// Part of a long session (the agent expects a warm prefix), against a one-shot.
    pub session: bool,
    /// The whole prompt, in tokens.
    pub prompt: u64,
    pub output: u64,
    /// `(hash seed, prefix tokens)`, shortest first: tools and system, the previous turn, this one.
    pub chain: Vec<(u64, u64)>,
}

pub const WARM_AGENTS: usize = 8;
pub const COLD_AGENTS: usize = 4;
/// The largest prompt a session reaches before it starts over.
const CONTEXT: u64 = 150_000;

fn weekday(ms: u64) -> u64 {
    ms / DAY % 7
}

fn hour(ms: u64) -> u64 {
    ms / HOUR % 24
}

/// 1 in working hours, a little on weekend days, little at night.
fn intensity(ms: u64) -> f64 {
    let (d, h) = (weekday(ms), hour(ms));
    match (d >= 5, (7..21).contains(&h)) {
        (false, true) => 1.0,
        (true, true) => 0.2,
        (false, false) => 0.04,
        (true, false) => 0.02,
    }
}

/// The week's requests, in time order. About 60,000 of them.
pub fn plan(seed: u64) -> Vec<Req> {
    let mut all: Vec<Req> = Vec::new();
    for agent in 0..WARM_AGENTS {
        sessions(&mut Rng::new(mix(seed, agent as u64 + 1)), agent, &mut all);
    }
    for agent in WARM_AGENTS..WARM_AGENTS + COLD_AGENTS {
        one_shots(&mut Rng::new(mix(seed, agent as u64 + 1)), agent, &mut all);
    }
    all.sort_by_key(|r| (r.at_ms, r.agent));
    for (n, r) in all.iter_mut().enumerate() {
        r.n = n;
    }
    all
}

/// An agent working in long sessions: a few minutes' break, then a conversation of 8 to 45
/// turns, each turn 15 to 150 seconds after the last (now and then a long pause that lets the
/// cache expire).
fn sessions(rng: &mut Rng, agent: usize, out: &mut Vec<Req>) {
    let system = rng.range(4_000, 12_000);
    let sys_seed = mix(0x5157, agent as u64);
    let mut t = rng.range(0, 4 * HOUR);
    let mut session = 0u64;
    while t < WEEK_MS {
        // Whether this agent works now: the intensity is the chance, drawn once per session.
        if rng.f() > intensity(t).min(1.0) * 0.97 {
            t += rng.range(10 * MIN, 50 * MIN);
            continue;
        }
        session += 1;
        let turns = rng.range(8, 45);
        let mut prompt = system + rng.range(500, 4_000);
        let mut previous: Option<(u64, u64)> = None;
        for k in 0..turns {
            let seed = mix(mix(0x5E55, agent as u64), session * 1000 + k);
            let output = rng.range(200, 3_000);
            let mut chain = vec![(sys_seed, system)];
            chain.extend(previous);
            chain.push((seed, prompt));
            out.push(Req { n: 0, at_ms: t, agent, session: true, prompt, output, chain });
            previous = Some((seed, prompt));
            prompt += output + rng.range(500, 6_000);
            if prompt > CONTEXT {
                break;
            }
            t += if rng.f() < 0.03 { rng.range(6 * MIN, 15 * MIN) } else { rng.range(8_000, 90_000) };
        }
        t += rng.range(3 * MIN, 25 * MIN);
    }
}

/// An agent sending unrelated one-shot requests: bursts in working hours, a trickle at night.
fn one_shots(rng: &mut Rng, agent: usize, out: &mut Vec<Req>) {
    let mut t = rng.range(0, 2 * MIN);
    let mut count = 0u64;
    let mut burst_until = 0;
    while t < WEEK_MS {
        if rng.f() < 0.002 {
            burst_until = t + rng.range(20 * MIN, 60 * MIN);
        }
        let boost = if t < burst_until { 4.0 } else { 1.0 };
        let mean_s = 40.0 / (intensity(t) * boost).max(0.02);
        t += (rng.exp(mean_s) * 1000.0) as u64 + 1;
        if t >= WEEK_MS {
            break;
        }
        count += 1;
        // Mostly small, now and then large: 2k to 60k.
        let prompt = 2_000 + (rng.f().powi(3) * 58_000.0) as u64;
        let output = rng.range(100, 1_500);
        out.push(Req {
            n: 0,
            at_ms: t,
            agent,
            session: false,
            prompt,
            output,
            chain: vec![(mix(mix(0x0517, agent as u64), count), prompt)],
        });
    }
}

// ---------------------------------------------------------------------------------------------
// The provider's side of each account

/// How a window's reset is timed.
#[derive(Clone, Debug)]
pub enum Reset {
    /// Starts with the first request after the last one ended.
    FirstUse,
    /// `offset + n × length`, counted from the week's first instant.
    Fixed { offset_ms: u64 },
}

/// One window as the provider enforces it.
#[derive(Clone, Debug)]
pub struct TrueWindow {
    pub meter: MeterDecl,
    pub reset: Reset,
    pub len_ms: u64,
    pub start: Option<u64>,
    pub end: u64,
    pub used: f64,
    /// A poll shows it. An unreported window is only the provider's own limit.
    pub reported: bool,
    /// Shorter than an hour: counted over the trailing length, never reset.
    pub recent: VecDeque<u64>,
}

/// A window ended: how much of it was left, for the report and for SC-004.
#[derive(Clone, Debug)]
pub struct ResetEvent {
    pub account: usize,
    pub window: String,
    pub started_ms: u64,
    pub at_ms: u64,
    pub left_frac: f64,
    pub reserve: f64,
}

impl TrueWindow {
    pub fn new(meter: &MeterDecl, reset: Reset, reported: bool, prior: f64) -> Self {
        let len_ms = u64::try_from(meter.length.as_millis()).unwrap_or(u64::MAX);
        let mut w = Self {
            meter: meter.clone(),
            reset,
            len_ms,
            start: None,
            end: 0,
            used: 0.0,
            reported,
            recent: VecDeque::new(),
        };
        if let Reset::Fixed { .. } = w.reset {
            let (s, e) = w.fixed_around(0);
            w.start = Some(s);
            w.end = e;
            // The window was already running when the week began.
            w.used = w.meter.capacity.unwrap_or(0.0) * prior;
        }
        w
    }

    pub fn admission(&self) -> bool {
        self.meter.is_admission()
    }

    /// The fixed window around `t`: `offset + n × length`. The week's first window is cut at
    /// Monday 00:00, where the simulation starts.
    fn fixed_around(&self, t: u64) -> (u64, u64) {
        let Reset::Fixed { offset_ms } = self.reset else { return (t, t + self.len_ms) };
        let offset = offset_ms % self.len_ms;
        if t < offset {
            return (0, offset);
        }
        let start = offset + (t - offset) / self.len_ms * self.len_ms;
        (start, start + self.len_ms)
    }

    fn left(&self) -> f64 {
        let cap = self.meter.capacity.unwrap_or(0.0);
        if cap > 0.0 { ((cap - self.used) / cap).clamp(0.0, 1.0) } else { 0.0 }
    }

    /// Moves to `t`: a window whose end has passed is over, and a fixed one starts the next.
    pub fn roll(&mut self, t: u64, account: usize, events: &mut Vec<ResetEvent>) {
        if self.admission() || self.start.is_none() || t < self.end {
            return;
        }
        let reserve = self.meter.reserve_or_default().fraction();
        events.push(ResetEvent {
            account,
            window: self.meter.name.clone(),
            started_ms: self.start.unwrap_or(0),
            at_ms: self.end,
            left_frac: self.left(),
            reserve,
        });
        self.used = 0.0;
        match self.reset {
            Reset::FirstUse => self.start = None,
            Reset::Fixed { .. } => {
                let (s, e) = self.fixed_around(t);
                self.start = Some(s);
                self.end = e;
            }
        }
    }
}

/// One account at the provider.
#[derive(Clone, Debug, Default)]
pub struct TrueAccount {
    pub windows: Vec<TrueWindow>,
}

impl TrueAccount {
    pub fn roll(&mut self, t: u64, account: usize, events: &mut Vec<ResetEvent>) {
        for w in &mut self.windows {
            w.roll(t, account, events);
        }
    }

    /// Whether the account would serve `s` at `t`.
    pub fn admits(&self, t: u64, s: &Spent) -> bool {
        self.windows.iter().all(|w| {
            if w.admission() {
                let from = t.saturating_sub(w.len_ms);
                let n = w.recent.iter().filter(|r| **r > from).count() as f64;
                n + cost_spent(&w.meter, s) <= w.meter.capacity.unwrap_or(f64::MAX)
            } else {
                w.used + cost_spent(&w.meter, s) <= w.meter.capacity.unwrap_or(f64::MAX)
            }
        })
    }

    /// Charges every window exactly by its meter.
    pub fn charge(&mut self, t: u64, s: &Spent) {
        for w in &mut self.windows {
            if w.admission() {
                let from = t.saturating_sub(w.len_ms);
                while w.recent.front().is_some_and(|r| *r <= from) {
                    w.recent.pop_front();
                }
                w.recent.push_back(t);
                continue;
            }
            if w.start.is_none() {
                w.start = Some(t);
                w.end = t + w.len_ms;
                w.used = 0.0;
            }
            w.used += cost_spent(&w.meter, s);
        }
    }

    /// What a poll shows: every reported window, percent or request counts, with its next reset.
    pub fn report(&self, t: u64, rounding: Option<Rounding>) -> Vec<QuotaWindow> {
        self.windows
            .iter()
            .filter(|w| w.reported)
            .map(|w| {
                let cap = w.meter.capacity.unwrap_or(0.0);
                let resets_at = Some(at(if w.start.is_some() { w.end } else { t + w.len_ms }));
                match w.meter.unit {
                    MeterUnit::Requests => QuotaWindow {
                        name: w.meter.name.clone(),
                        unit: QuotaUnit::Requests,
                        used: Some(w.used),
                        limit: Some(cap),
                        remaining: None,
                        resets_at,
                    },
                    MeterUnit::WeightedTokens => QuotaWindow {
                        name: w.meter.name.clone(),
                        unit: QuotaUnit::Percent,
                        used: Some(round_to(rounding, w.used / cap * 100.0)),
                        limit: None,
                        remaining: None,
                        resets_at,
                    },
                }
            })
            .collect()
    }
}

/// The whole provider side: the accounts' windows and each account's prompt cache.
#[derive(Clone, Default)]
pub struct World {
    pub accounts: Vec<TrueAccount>,
    /// `(account, prefix seed)` → `(prefix tokens, last used)`.
    cache: HashMap<(usize, u64), (u64, u64)>,
    pub events: Vec<ResetEvent>,
    /// How a poll rounds a percent reading to whole steps; `None` reports the exact value (the
    /// default, so slice 006's checks see what they always saw).
    #[allow(dead_code)]
    pub rounding: Option<Rounding>,
    /// Use the router didn't cause (spec 012, research R14).
    #[allow(dead_code)]
    pub outside: Vec<OutsideUse>,
    /// Every drop the injector applied, for SC-004 and SC-007.
    #[allow(dead_code)]
    pub injected: Vec<Injected>,
    /// True capacity changes still to come: `(at_ms, account, window, capacity)`.
    capacity_at: Vec<(u64, usize, String, f64)>,
    /// The instant the injector and the capacity schedule have been applied up to.
    outside_t: u64,
}

/// What the provider reports for one served request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Usage {
    /// The work in plain tokens: every class, unweighted.
    pub fn plain(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }

    pub fn spent(&self) -> Spent {
        Spent {
            model: "m".into(),
            requests: 1,
            input: self.input,
            output: self.output,
            cache_read: self.cache_read,
            cache_write: self.cache_write,
        }
    }
}

impl World {
    pub fn new(accounts: Vec<TrueAccount>) -> Self {
        Self { accounts, ..Self::default() }
    }

    pub fn roll(&mut self, t: u64) {
        self.advance_outside(t);
        for (i, a) in self.accounts.iter_mut().enumerate() {
            a.roll(t, i, &mut self.events);
        }
    }

    /// The provider's cache hit for `req` on `account` at `t`, in tokens.
    pub fn cached(&self, account: usize, req: &Req, t: u64) -> u64 {
        req.chain
            .iter()
            .rev()
            .find_map(|(seed, tokens)| {
                let (prefix, last) = self.cache.get(&(account, *seed))?;
                (t.saturating_sub(*last) <= CACHE_MS).then_some((*prefix).min(*tokens))
            })
            .unwrap_or(0)
    }

    /// Serves `req` on `account` at `t`: its usage, or `None` for a 429.
    pub fn serve(&mut self, account: usize, req: &Req, t: u64) -> Option<Usage> {
        self.roll(t);
        let read = self.cached(account, req, t);
        let usage = Usage { input: req.prompt - read, output: req.output, cache_read: read, cache_write: 0 };
        if !self.accounts[account].admits(t, &usage.spent()) {
            return None;
        }
        self.accounts[account].charge(t, &usage.spent());
        // Every boundary of at least 1,024 tokens is now cached, and the hit one was refreshed.
        for (seed, tokens) in &req.chain {
            if *tokens >= 1024 {
                self.cache.insert((account, *seed), (*tokens, t));
            }
        }
        Some(usage)
    }

    pub fn sweep(&mut self, t: u64) {
        self.cache.retain(|_, (_, last)| t.saturating_sub(*last) <= CACHE_MS);
    }

    pub fn polls(&mut self, account: usize, t: u64) -> Vec<QuotaWindow> {
        self.roll(t);
        self.accounts[account].report(t, self.rounding)
    }
}

// ---------------------------------------------------------------------------------------------
// Rounded readings, outside use, and a provider that changes its rules (spec 012, R14)

/// How a provider rounds a percent reading to a whole step.
// Used by the slice 012 runs (T017–T020, T046); allowed until they land.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rounding {
    HalfUp,
    Floor,
}

fn round_to(rounding: Option<Rounding>, x: f64) -> f64 {
    match rounding {
        None => x,
        Some(Rounding::HalfUp) => (x + 0.5).floor(),
        Some(Rounding::Floor) => x.floor(),
    }
}

/// Use of an account the router didn't send. Amounts are percent of the window's true capacity.
// Used by the slice 012 runs (T017–T020, T046); allowed until they land.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub enum OutsideUse {
    /// One drop at `at_ms`. `busy` only labels it: whether it falls in a stretch of traffic.
    Drop { account: usize, window: String, at_ms: u64, pct: f64, busy: bool },
    /// A steady rate in percent per hour. `office` shapes it like the agents' traffic: the same
    /// daily and weekly intensity, so it is high exactly when the router is busy.
    Rate { account: usize, window: String, pct_per_hour: f64, office: bool },
}

/// One drop the injector applied.
// Used by the slice 012 runs (T017–T020, T046); allowed until they land.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq)]
pub struct Injected {
    pub account: usize,
    pub window: String,
    pub at_ms: u64,
    pub pct: f64,
    pub busy: bool,
}

// Used by the slice 012 runs (T017–T020, T046); allowed until they land.
#[allow(dead_code)]
impl World {
    /// The provider changes a window's capacity at `at_ms`. The percent already used stays put,
    /// as at a real provider: only what is charged from then on weighs differently.
    pub fn set_true_capacity(&mut self, at_ms: u64, account: usize, window: &str, value: f64) {
        self.capacity_at.push((at_ms, account, window.to_owned(), value));
        self.capacity_at.sort_by_key(|c| c.0);
    }

    fn window_mut(&mut self, account: usize, name: &str) -> Option<&mut TrueWindow> {
        self.accounts.get_mut(account)?.windows.iter_mut().find(|w| w.meter.name == name)
    }

    /// Applies the capacity schedule and the outside use falling in `(outside_t, t]`.
    fn advance_outside(&mut self, t: u64) {
        let from = self.outside_t;
        if t <= from && !(from == 0 && t == 0) {
            return;
        }
        while self.capacity_at.first().is_some_and(|c| c.0 <= t) {
            let (_, account, window, value) = self.capacity_at.remove(0);
            if let Some(w) = self.window_mut(account, &window) {
                let old = w.meter.capacity.unwrap_or(value);
                // The percent shown stays continuous: the used amount is rescaled.
                w.used *= value / old;
                w.meter.capacity = Some(value);
            }
        }
        for u in self.outside.clone() {
            match u {
                OutsideUse::Drop { account, window, at_ms, pct, busy } => {
                    if (from < at_ms || (from == 0 && at_ms == 0)) && at_ms <= t {
                        self.charge_outside(account, &window, at_ms, pct);
                        self.injected.push(Injected { account, window, at_ms, pct, busy });
                    }
                }
                OutsideUse::Rate { account, window, pct_per_hour, office } => {
                    // Ten-minute chunks, each weighted by the intensity at its start.
                    let mut at_ms = from;
                    let mut pct = 0.0;
                    while at_ms < t {
                        let step = (10 * MIN).min(t - at_ms);
                        let weight = if office { intensity(at_ms) } else { 1.0 };
                        pct += pct_per_hour * weight * step as f64 / HOUR as f64;
                        at_ms += step;
                    }
                    self.charge_outside(account, &window, t, pct);
                }
            }
        }
        self.outside_t = t;
    }

    fn charge_outside(&mut self, account: usize, window: &str, at_ms: u64, pct: f64) {
        let Some(w) = self.window_mut(account, window) else { return };
        if w.admission() {
            return;
        }
        if w.start.is_none() {
            w.start = Some(at_ms);
            w.end = at_ms + w.len_ms;
            w.used = 0.0;
        }
        w.used += w.meter.capacity.unwrap_or(0.0) * pct / 100.0;
    }
}

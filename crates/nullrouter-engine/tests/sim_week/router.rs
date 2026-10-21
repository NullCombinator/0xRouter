//! The router side of the simulated week: what the engine does around `routing::place`, minus
//! HTTP. It rolls the ledger, looks up the warm store, places, debits, calls the simulated
//! provider, settles, learns the fingerprints it left cached, and writes the real journal.
//!
//! The clock is an argument everywhere: the week's milliseconds since Monday 00:00 UTC.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use nullrouter_engine::accounts::RoutingOverrides;
use nullrouter_engine::clock;
use nullrouter_engine::journal::{Journal, Options, Target, records, state};
use nullrouter_engine::quota::extract::{QuotaWindow, rfc3339_millis};
use nullrouter_engine::routing::fingerprint::{Boundary, Chain, Hash};
use nullrouter_engine::routing::ledger::{Debit, Ledger, window_start};
use nullrouter_engine::routing::meter::{Bucket, MeterInput, Spent, Traffic, quota_for};
use nullrouter_engine::routing::pace::request_size;
use nullrouter_engine::routing::place::place;
use nullrouter_engine::routing::warm::WarmStore;
use nullrouter_engine::routing::{CacheSpec, Candidate, CandidateKey, Placement, PriceSpec, RoutingInput, Tier};
use nullrouter_registry::schema::{CacheMode, MeterDecl};
use serde_json::{Value, json};

use super::fit::FitRun;
use super::world::{CACHE_MS, DAY, MIN, Req, Usage, World, at};

/// How often every polled account is polled, in simulated time.
pub const POLL_MS: u64 = 10 * MIN;

#[derive(Clone)]
pub struct AccountDef {
    pub key: CandidateKey,
    pub order: i64,
    pub priority: f64,
    pub meters: Vec<MeterDecl>,
    pub reported: bool,
    pub price: PriceSpec,
}

pub struct Defs {
    pub accounts: Vec<AccountDef>,
    pub target: String,
    pub amortization: Duration,
}

impl Defs {
    pub fn index_of(&self, key: &CandidateKey) -> usize {
        self.accounts
            .iter()
            .position(|a| a.key.provider == key.provider && a.key.account == key.account)
            .expect("a known account")
    }
}

/// What one placement did, kept in memory to compare two runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placed {
    pub n: usize,
    pub order: Vec<usize>,
    pub served: Option<usize>,
}

/// An account's counted traffic: the hourly buckets estimated windows count from, what has gone
/// through since the last poll, and the last two minutes for admission limits.
#[derive(Clone, Default)]
pub struct Tally {
    pub buckets: Vec<Bucket>,
    pub since_poll: Spent,
    pub recent: Vec<Traffic>,
}

fn add(s: &mut Spent, u: &Usage) {
    s.model = "m".into();
    s.requests += 1;
    s.input += u.input;
    s.output += u.output;
    s.cache_read += u.cache_read;
    s.cache_write += u.cache_write;
}

impl Tally {
    fn add(&mut self, t: u64, u: &Usage) {
        let start = at(t / (DAY / 24) * (DAY / 24));
        match self.buckets.last_mut() {
            Some(b) if b.start == start => add(&mut b.spent[0], u),
            _ => {
                let mut s = Spent::default();
                add(&mut s, u);
                self.buckets.push(Bucket { start, spent: vec![s] });
            }
        }
        add(&mut self.since_poll, u);
        self.recent.push(Traffic {
            at: at(t),
            model: "m".into(),
            input: u.input,
            output: u.output,
            cache_read: u.cache_read,
            cache_write: u.cache_write,
        });
        let keep = at(t.saturating_sub(2 * MIN));
        let stale = self.recent.iter().take_while(|r| r.at < keep).count();
        self.recent.drain(..stale);
    }
}

fn hash_of(seed: u64) -> Hash {
    let b = seed.to_le_bytes();
    let mut h = [0u8; 16];
    h[..8].copy_from_slice(&b);
    h[8..].copy_from_slice(&b);
    Hash(h)
}

fn cache_spec() -> CacheSpec {
    CacheSpec { mode: CacheMode::Automatic, lifetime: Duration::from_millis(CACHE_MS), min_tokens: 1024 }
}

pub struct Sim<'a> {
    pub defs: &'a Defs,
    pub home: PathBuf,
    pub journal: Journal,
    pub warm: WarmStore,
    pub ledgers: BTreeMap<Tier, Ledger>,
    pub tallies: Vec<Tally>,
    pub polls: Vec<Option<(Vec<QuotaWindow>, SystemTime)>>,
    pub cooling: Vec<u64>,
    pub placed: Vec<Placed>,
    next_poll: u64,
    /// Synced every simulated second (the power-loss variant): the last second handled.
    pub sync_each_second: Option<u64>,
    /// The quota fit (spec 012): `None` places with the declared meters, as slice 006 did. A fork
    /// or restart does not carry it.
    pub fit: Option<FitRun>,
}

fn clone_warm(w: &WarmStore) -> WarmStore {
    let mut out = WarmStore::new();
    for s in w.stored() {
        out.restore(s);
    }
    out
}

pub fn journal_options() -> Options {
    // The simulation runs far faster than the clock: syncing is done by the run, not by the timer.
    Options { sync_every: Duration::from_secs(3600), retry_every: Duration::from_secs(5) }
}

impl<'a> Sim<'a> {
    pub fn new(defs: &'a Defs, home: &Path) -> Self {
        let n = defs.accounts.len();
        Self {
            defs,
            home: home.to_owned(),
            journal: Journal::start(home, journal_options()).expect("a journal"),
            warm: WarmStore::new(),
            ledgers: BTreeMap::new(),
            tallies: vec![Tally::default(); n],
            polls: vec![None; n],
            cooling: vec![0; n],
            placed: Vec::new(),
            next_poll: 0,
            sync_each_second: None,
            fit: None,
        }
    }

    /// Places with the meters the fit makes significant.
    pub fn with_fit(mut self, fit: FitRun) -> Self {
        self.fit = Some(fit);
        self
    }

    // -----------------------------------------------------------------------------------------
    // Crash and restart

    /// Every line queued so far is written (not synced): the files a killed process leaves.
    pub fn crash_point(&self) {
        self.journal.written().wait_blocking().expect("written");
    }

    /// The same in-memory state over a copy of the files, to run on without restarting.
    pub fn fork(&self, home: &Path) -> Sim<'a> {
        Sim {
            defs: self.defs,
            home: home.to_owned(),
            journal: Journal::start(home, journal_options()).expect("a journal"),
            warm: clone_warm(&self.warm),
            ledgers: self.ledgers.clone(),
            tallies: self.tallies.clone(),
            polls: self.polls.clone(),
            cooling: self.cooling.clone(),
            placed: self.placed.clone(),
            next_poll: self.next_poll,
            sync_each_second: self.sync_each_second,
            fit: None,
        }
    }

    /// Drops every bit of memory and reads the files back, as a start after a crash does: warm
    /// fingerprints and ledgers from `routing/`, counted traffic from the records, the last poll
    /// from where the poller keeps it.
    pub fn restart(self, now_ms: u64) -> Sim<'a> {
        let Sim { defs, home, placed, sync_each_second, .. } = self;
        let mut s = Sim::new(defs, &home);
        s.placed = placed;
        s.sync_each_second = sync_each_second;
        let now = at(now_ms);

        let loaded = state::load(&home);
        for w in loaded.warm {
            s.warm.restore(w);
        }
        s.warm.sweep(now, |_| Some(Duration::from_millis(CACHE_MS)));
        for l in loaded.ledgers {
            if l.window_start == window_start(now, defs.amortization) {
                s.ledgers.entry(l.tier).or_default().restore(l.window_start, l.deficits);
            }
        }

        let polls: Value = fs::read(home.join("sim-polls.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or(Value::Null);
        s.next_poll = polls["next"].as_u64().unwrap_or(0);
        let mut poll_at = vec![0u64; defs.accounts.len()];
        for (i, p) in s.polls.iter_mut().enumerate() {
            let e = &polls["accounts"][i.to_string()];
            if let (Some(ms), Ok(w)) =
                (e["at"].as_u64(), serde_json::from_value::<Vec<QuotaWindow>>(e["windows"].clone()))
            {
                *p = Some((w, at(ms)));
                poll_at[i] = ms;
            }
        }

        for (_, path) in records::segments(&home) {
            let text = fs::read_to_string(path).expect("a segment");
            for r in records::fold(&text) {
                let Some(arrived) = r["arrived"].as_str().and_then(clock::parse_rfc3339) else { continue };
                let t = u64::try_from(arrived.duration_since(at(0)).unwrap_or_default().as_millis()).unwrap_or(0);
                for a in r["attempts"].as_array().into_iter().flatten() {
                    let Some(i) = defs.accounts.iter().position(|d| {
                        a["provider"] == d.key.provider.as_str() && a["account"] == d.key.account.as_str()
                    }) else {
                        continue;
                    };
                    match a["outcome"]["state"].as_str() {
                        Some("ok") => {
                            let u = usage_of(&a["usage"]);
                            let tally = &mut s.tallies[i];
                            // `since_poll` is only what came after the poll: `add` counts it, so undo for older.
                            let before = tally.since_poll.clone();
                            tally.add(t, &u);
                            if t < poll_at[i] {
                                tally.since_poll = before;
                            }
                        }
                        Some("failed") => s.cooling[i] = s.cooling[i].max(t + MIN),
                        _ => {}
                    }
                }
            }
        }
        s.compact(now);
        s
    }

    // -----------------------------------------------------------------------------------------
    // The clock's housekeeping: polls, sweeps, compaction

    /// Everything due up to and including `t`: polls every ten minutes (which also sweep expired
    /// fingerprints), and a compaction of the routing files at each day's start.
    pub fn advance(&mut self, w: &mut World, t: u64) {
        while self.next_poll <= t {
            let p = self.next_poll;
            self.poll(w, p);
            self.next_poll += POLL_MS;
        }
    }

    fn poll(&mut self, w: &mut World, t: u64) {
        let now = at(t);
        for (i, d) in self.defs.accounts.iter().enumerate() {
            if !d.reported {
                continue;
            }
            self.polls[i] = Some((w.polls(i, t), now));
            if let Some(fit) = self.fit.as_mut() {
                let windows = self.polls[i].as_ref().map_or(&[][..], |p| p.0.as_slice());
                fit.observe_poll(self.defs, i, windows, &self.tallies[i].since_poll, now, self.placed.len());
            }
            self.tallies[i].since_poll = Spent::default();
        }
        w.roll(t);
        w.sweep(t);
        self.warm.sweep(now, |_| Some(Duration::from_millis(CACHE_MS)));
        if t.is_multiple_of(DAY) {
            self.compact(now);
        }
        let mut accounts = serde_json::Map::new();
        for (i, p) in self.polls.iter().enumerate() {
            if let Some((windows, at_)) = p {
                let ms = u64::try_from(at_.duration_since(at(0)).unwrap_or_default().as_millis()).unwrap_or(0);
                accounts.insert(i.to_string(), json!({"at": ms, "windows": windows}));
            }
        }
        let body = json!({"next": t + POLL_MS, "accounts": accounts});
        fs::write(self.home.join("sim-polls.json"), body.to_string()).expect("polls written");
    }

    /// Replaces both routing files with the live state, in order with the appends.
    fn compact(&self, now: SystemTime) {
        let warm: String = self.warm.stored().iter().map(|s| state::warm_line(s).to_string() + "\n").collect();
        let ledgers: String = self
            .ledgers
            .iter()
            .filter_map(|(tier, l)| {
                let win = l.window()?;
                (win == window_start(now, self.defs.amortization))
                    .then(|| state::ledger_line(&self.defs.target, *tier, win, l.deficits(), now).to_string() + "\n")
            })
            .collect();
        self.journal.replace(Target::Warm, warm);
        self.journal.replace(Target::Ledger, ledgers);
    }

    // -----------------------------------------------------------------------------------------
    // One request

    fn candidates(&self, t: u64) -> Vec<Candidate> {
        let now = at(t);
        let overrides = RoutingOverrides::default();
        self.defs
            .accounts
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let tally = &self.tallies[i];
                let polled = self.polls[i].as_ref().map(|(w, at)| (w.as_slice(), *at));
                let quota = quota_for(
                    &MeterInput {
                        declared: self.fit.as_ref().and_then(|f| f.meters(i)).unwrap_or(d.meters.as_slice()),
                        overrides: &overrides,
                        report_declared: d.reported,
                        polled,
                        last_poll_failed: false,
                        recent: &tally.recent,
                        since_poll: std::slice::from_ref(&tally.since_poll),
                        history: &tally.buckets,
                    },
                    now,
                );
                Candidate {
                    key: d.key.clone(),
                    order: d.order,
                    priority: d.priority,
                    out_of_service: None,
                    cooling: (self.cooling[i] > t).then(|| Duration::from_millis(self.cooling[i] - t)),
                    quota,
                    cache: cache_spec(),
                    price: d.price.clone(),
                    meter_sources: None,
                }
            })
            .collect()
    }

    pub fn handle(&mut self, w: &mut World, req: &Req) {
        let t = req.at_ms;
        let now = at(t);
        self.advance(w, t);
        if let Some(last) = self.sync_each_second.as_mut() {
            // Every simulated second that has passed is on disk before the next one's lines.
            if t / 1000 != *last {
                *last = t / 1000;
                self.journal.flush_blocking();
            }
        }
        let id = format!("rq_{:06}", req.n);
        let agent = format!("ak_{}", req.agent);
        let length = self.defs.amortization;

        let candidates = self.candidates(t);
        if let Some(fit) = self.fit.as_mut() {
            for (i, c) in candidates.iter().enumerate() {
                let polled = self.polls[i].as_ref().map(|p| p.0.as_slice());
                fit.audit(self.defs, i, &c.quota, polled, &self.tallies[i].since_poll);
            }
        }
        let chain = Chain {
            boundaries: req
                .chain
                .iter()
                .map(|(s, p)| Boundary { hash: hash_of(*s), prefix_tokens: *p, marker_ttl: None, covered: false })
                .collect(),
        };
        let lookup: Vec<(CandidateKey, Duration)> =
            candidates.iter().map(|c| (c.key.clone(), c.cache.lifetime)).collect();
        let warm = self.warm.lookup(&agent, &chain, &lookup, now);
        let known = warm.as_ref().and_then(|wm| {
            chain.boundaries.iter().find(|b| b.hash == wm.hash).map(|b| (wm.prefix_tokens, b.prefix_tokens))
        });
        let total = chain.boundaries.last().map_or(0, |b| b.prefix_tokens);
        let size = request_size(known, total);

        let mut deficits: BTreeMap<String, f64> = BTreeMap::new();
        for tier in [Tier::Subscription, Tier::Payg] {
            let ledger = self.ledgers.entry(tier).or_default();
            ledger.roll(now, length);
            for c in candidates.iter().filter(|c| c.quota.state.source.tier() == tier) {
                ledger.observe(&c.key.account_key(), c.priority);
            }
            deficits.extend(ledger.deficits().iter().map(|(k, v)| (k.clone(), *v)));
        }
        let input = RoutingInput {
            target: self.defs.target.clone(),
            candidates,
            amortization: length,
            size_tokens: size,
            warm,
            deficits,
        };
        let placement = place(&input, now);
        let (stayed_on, warm_account) = match (&placement.decision.warm, &input.warm) {
            (Some(h), Some(wm)) if h.stayed => (Some(wm.hash), Some(wm.key.clone())),
            _ => (None, None),
        };

        let day = Target::records_at(now);
        self.journal.append(
            day.clone(),
            "open",
            json!({"id": id, "arrived": rfc3339_millis(now), "agent": agent, "style": "openai-chat", "op": "generate",
                "type": "text", "target": self.defs.target}),
        );
        self.journal.append(
            day.clone(),
            "decision",
            json!({"id": id, "decision": serde_json::to_value(&placement.decision).expect("a decision")}),
        );

        let mut served = None;
        for (n, (key, reason, rank)) in placement.steps.iter().enumerate() {
            let i = self.defs.index_of(key);
            let debit = self.start(&placement, warm_account.as_ref(), key, size, now);
            let outcome = w.serve(i, req, t);
            let kind = if n == 0 { "initial" } else { "fallback" };
            let mut line = json!({"n": n + 1, "provider": key.provider, "account": key.account, "model": key.model, "kind": kind,
                "placement": {"reason": reason, "rank": rank}, "started": 0.0, "ended": 0.0, "dropped": [], "forced": []});
            match outcome {
                Some(u) => {
                    self.finish(debit, true, u.plain(), now);
                    self.tallies[i].add(t, &u);
                    self.learn(&agent, &chain, key, stayed_on, now);
                    line["outcome"] = json!({"state": "ok"});
                    line["usage"] = usage_json(&u);
                    self.journal.append(day.clone(), "attempt", json!({"id": id, "attempt": line}));
                    served = Some((i, u));
                    break;
                }
                None => {
                    self.finish(debit, false, 0, now);
                    self.cooling[i] = t + MIN;
                    line["outcome"] = json!({"state": "failed", "status": 429});
                    self.journal.append(day.clone(), "attempt", json!({"id": id, "attempt": line}));
                }
            }
        }
        let close = match &served {
            Some((i, u)) => {
                let k = &self.defs.accounts[*i].key;
                json!({"id": id, "outcome": "succeeded", "served_by": {"provider": k.provider, "account": k.account, "model": k.model},
                    "usage": usage_json(u), "break_handling": {"kind": "none"}})
            }
            None => {
                json!({"id": id, "outcome": "failed", "served_by": null, "usage": null, "break_handling": {"kind": "none"}})
            }
        };
        self.journal.append(day, "close", close);
        self.placed.push(Placed {
            n: req.n,
            order: placement.steps.iter().map(|(k, ..)| self.defs.index_of(k)).collect(),
            served: served.map(|(i, _)| i),
        });
    }

    /// An attempt starts: unless it is warm work, the account is debited the request's estimate.
    fn start(
        &mut self,
        p: &Placement,
        warm: Option<&CandidateKey>,
        at_: &CandidateKey,
        tokens: u64,
        now: SystemTime,
    ) -> Option<(Tier, Debit)> {
        if warm == Some(at_) {
            return None;
        }
        let key = at_.account_key();
        let (tier, shares) = if p.payg_shares.iter().any(|(k, _)| *k == key) {
            (Tier::Payg, &p.payg_shares)
        } else {
            (Tier::Subscription, &p.shares)
        };
        let ledger = self.ledgers.entry(tier).or_default();
        let debit = ledger.debit(now, self.defs.amortization, shares, &key, tokens)?;
        self.queue(tier, now);
        Some((tier, debit))
    }

    /// The attempt ended: a success settles the debit with the plain tokens, a failure takes it back.
    fn finish(&mut self, debit: Option<(Tier, Debit)>, ok: bool, tokens: u64, now: SystemTime) {
        let Some((tier, mut debit)) = debit else { return };
        let length = self.defs.amortization;
        let ledger = self.ledgers.entry(tier).or_default();
        if ok {
            ledger.settle(&mut debit, now, length, tokens);
        } else {
            ledger.reverse(&debit, now, length);
        }
        self.queue(tier, now);
    }

    fn queue(&self, tier: Tier, now: SystemTime) {
        let l = &self.ledgers[&tier];
        let Some(win) = l.window() else { return };
        let deficits: BTreeMap<&String, i64> = l.deficits().iter().map(|(k, v)| (k, v.round() as i64)).collect();
        self.journal.append(
            Target::Ledger,
            "ledger",
            json!({"target": self.defs.target, "tier": state::tier_name(tier), "window_start": rfc3339_millis(win),
                "deficits": deficits, "at": rfc3339_millis(now)}),
        );
    }

    /// A request succeeded on `at_`: remember what its provider now holds cached.
    fn learn(&mut self, agent: &str, chain: &Chain, at_: &CandidateKey, stayed_on: Option<Hash>, now: SystemTime) {
        let written = chain.writable(CacheMode::Automatic, 1024);
        if let Some(hit) = stayed_on {
            self.warm.touch(agent, at_, chain, hit, now);
        }
        self.warm.upsert(agent, at_, &written, now);
        for wr in &written {
            self.journal.append(
                Target::Warm,
                "warm",
                json!({"agent": agent, "hash": wr.hash.hex(), "provider": at_.provider, "account": at_.account, "model": at_.model,
                    "prefix_tokens": wr.prefix_tokens, "last_used": rfc3339_millis(now)}),
            );
        }
    }

    /// The deficits that stand, rounded as the journal writes them.
    pub fn deficits(&self) -> BTreeMap<(Tier, String), i64> {
        self.ledgers
            .iter()
            .flat_map(|(t, l)| l.deficits().iter().map(move |(k, v)| ((*t, k.clone()), v.round() as i64)))
            .collect()
    }
}

fn usage_json(u: &Usage) -> Value {
    json!({"input": u.input, "output": u.output, "cache_read": u.cache_read, "cache_write": u.cache_write})
}

pub fn usage_of(v: &Value) -> Usage {
    let n = |k: &str| v[k].as_u64().unwrap_or(0);
    Usage { input: n("input"), output: n("output"), cache_read: n("cache_read"), cache_write: n("cache_write") }
}

/// Copies a home directory, files as they are on disk now.
pub fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("a directory");
    for e in fs::read_dir(from).expect("a directory").flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            copy_dir(&src, &dst);
        } else {
            fs::copy(&src, &dst).expect("a copy");
        }
    }
}

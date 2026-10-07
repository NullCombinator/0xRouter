//! The engine's side of the routing decision (spec 006): builds a [`RoutingInput`] from the
//! operator state, calls the pure `place`, and remembers what a success left cached.
//!
//! This is where the clock is read and the journal is written; everything under `routing/` that
//! decides stays pure.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use nullrouter_registry::schema::{EffectiveCache, InputSemantics};
use serde_json::json;

use crate::attempt::TextRequest;
use crate::journal::Target;
use crate::plan::{self, RequestPlan, Step};
use crate::records::Usage;
use crate::routing::fingerprint::{Chain, Hash};
use crate::routing::ledger::Debit;
use crate::routing::meter::{self, AccountQuota, Bucket, MeterInput, Spent, Traffic};
use crate::routing::pace::request_size;
use crate::routing::{
    AmortizationWindow, CacheSpec, Candidate, CandidateKey, Decision, PlacementReason, PriceSpec, RoutingInput, Tier,
    place::place, view,
};
use crate::state::{Engine, EngineState};

/// One step of the order a request walks.
#[derive(Debug, Clone, Copy)]
pub struct Slot {
    /// Index into the plan's steps.
    pub step: usize,
    pub reason: PlacementReason,
    pub rank: usize,
}

/// A request's placement: what the attempt loop walks and what the record keeps.
#[derive(Debug)]
pub struct Routed {
    pub order: Vec<Slot>,
    pub decision: Decision,
    /// The request's prefix chain; `None` for a request with no prompt prefix to cache.
    pub chain: Option<Chain>,
    /// The boundary the warm account was matched on, when the request stayed on it.
    pub stayed_on: Option<Hash>,
    /// The account the request stayed on for its warm prefix: warm work never moves a deficit.
    pub warm_account: Option<CandidateKey>,
    /// How cold work in this request is counted: the target, its amortization length, the shares
    /// among its subscription accounts and the request's estimated tokens.
    pub tally: Tally,
    /// The debit of the attempt in flight, until it settles or is reversed.
    pub debit: Mutex<Option<(Tier, Debit)>>,
}

#[derive(Debug, Clone)]
pub struct Tally {
    pub target: String,
    pub length: Duration,
    pub shares: Vec<(String, f64)>,
    /// The same among the pay-as-you-go accounts, which keep a ledger of their own.
    pub payg_shares: Vec<(String, f64)>,
    pub tokens: u64,
}

pub(crate) fn cache_of(
    provider: &nullrouter_registry::schema::ProviderEntity,
    account: Option<&crate::accounts::Account>,
) -> CacheSpec {
    let EffectiveCache { mode, lifetime, min_tokens } = provider.routing().cache;
    let lifetime = account.and_then(|a| a.routing.cache_lifetime).unwrap_or(lifetime);
    CacheSpec { mode, lifetime, min_tokens }
}

/// One model's counted traffic as a window's meter charges it.
fn spent_of(model: &str, t: &crate::quota::tally::ModelTally) -> Spent {
    Spent {
        model: model.to_owned(),
        requests: t.requests,
        input: t.input,
        output: t.output,
        cache_read: t.cache_read,
        cache_write: t.cache_write,
    }
}

fn candidate_of(engine: &Engine, c: &plan::Candidate<'_>, order: i64, now: SystemTime) -> Candidate {
    let account = c.account;
    let name = account.map_or("", |a| a.name.as_str());
    let routing = c.provider.routing();
    let cooling = engine
        .cooldowns
        .cooling(&c.provider.id, name, &c.upstream_id)
        .map(|until| until.saturating_duration_since(tokio::time::Instant::now()));
    let quota = match account {
        None => AccountQuota::payg(),
        Some(a) => {
            let board = engine.quota.get(&a.provider, &a.name);
            let polled = board.latest.as_ref().map(|p| (p.windows.as_slice(), p.at));
            let failed_after = match (&board.last_failure, &board.latest) {
                (Some(f), Some(l)) => f.at > l.at,
                (Some(_), None) => true,
                _ => false,
            };
            let reported = crate::quota::poll::reported(c.provider, a).is_some();
            let tally = &engine.history.tally;
            let since_poll: Vec<Spent> =
                tally.since_good_poll(&a.provider, &a.name).iter().map(|(m, t)| spent_of(m, t)).collect();
            // The hourly counters are read only where a window counts from them: one nobody
            // reports, or one whose reset passed since its poll.
            let reset_passed =
                board.latest.as_ref().is_some_and(|p| p.windows.iter().any(|w| w.resets_at.is_some_and(|r| r <= now)));
            let reach =
                routing.windows.iter().map(|m| m.length).max().unwrap_or_default().min(crate::quota::tally::HORIZON);
            let history: Vec<Bucket> = if !reported || reset_passed {
                let from = now.checked_sub(reach).unwrap_or(SystemTime::UNIX_EPOCH);
                tally
                    .hours_since(&a.provider, &a.name, from)
                    .into_iter()
                    .map(|(h, t)| Bucket {
                        start: SystemTime::UNIX_EPOCH + Duration::from_secs(h * 3600),
                        spent: t.iter().map(|(m, t)| spent_of(m, t)).collect(),
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let recent: Vec<Traffic> = if routing.windows.iter().any(|m| m.is_admission()) {
                tally
                    .recent_since(
                        &a.provider,
                        &a.name,
                        now.checked_sub(Duration::from_secs(3600)).unwrap_or(SystemTime::UNIX_EPOCH),
                    )
                    .into_iter()
                    .map(|c| Traffic {
                        at: c.at,
                        model: c.model,
                        input: c.tally.input,
                        output: c.tally.output,
                        cache_read: c.tally.cache_read,
                        cache_write: c.tally.cache_write,
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let in_effect = engine.meters.get(&a.provider, &a.name);
            meter::quota_for(
                &MeterInput {
                    declared: in_effect.as_ref().map_or(routing.windows, |m| &m.windows[..]),
                    overrides: &a.routing,
                    report_declared: reported,
                    polled,
                    last_poll_failed: failed_after,
                    recent: &recent,
                    since_poll: &since_poll,
                    history: &history,
                },
                now,
            )
        }
    };
    Candidate {
        key: CandidateKey::new(&c.provider.id, name, &c.upstream_id),
        order,
        priority: account.map_or(1.0, |a| a.priority),
        out_of_service: None,
        cooling,
        quota,
        cache: cache_of(c.provider, account),
        price: PriceSpec { schedule: routing.prices.to_vec(), flat: account.and_then(|a| a.routing.price) },
    }
}

/// The plan's steps as routing candidates, with the plan step each came from.
fn candidates_of(
    engine: &Engine,
    st: &EngineState,
    plan: &RequestPlan<'_>,
    now: SystemTime,
) -> (Vec<Candidate>, Vec<usize>) {
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut step_of: Vec<usize> = Vec::new();
    for (i, step) in plan.steps.iter().enumerate() {
        let order = i64::try_from(i).unwrap_or(i64::MAX);
        match step {
            Step::Try(c) => candidates.push(candidate_of(engine, c, order, now)),
            Step::Skip(s) => candidates.push(Candidate {
                key: CandidateKey::new(&s.provider, s.account.as_deref().unwrap_or(""), &s.model),
                order,
                priority: s
                    .account
                    .as_deref()
                    .and_then(|a| st.accounts.get(&s.provider, a))
                    .map_or(1.0, |a| a.priority),
                out_of_service: Some(s.reason.clone()),
                cooling: None,
                quota: AccountQuota::payg(),
                cache: CacheSpec {
                    mode: nullrouter_registry::schema::CacheMode::Automatic,
                    lifetime: Duration::from_secs(300),
                    min_tokens: 1024,
                },
                price: PriceSpec::default(),
            }),
        }
        step_of.push(i);
    }
    (candidates, step_of)
}

/// The targets the routing view covers: every unified model, every target that has a ledger
/// (a direct one that has seen cold work), and `only` when it names one.
pub fn view_targets(engine: &Engine, st: &EngineState, only: Option<&str>) -> Vec<String> {
    if let Some(t) = only {
        return vec![t.to_owned()];
    }
    let mut targets: Vec<String> = st.registry.unified_models().map(|u| u.name.clone()).collect();
    for (target, _) in engine.router.lock().ledgers.keys() {
        if !targets.contains(target) {
            targets.push(target.clone());
        }
    }
    targets
}

/// The routing view of each target in `view_targets`, as a decision at `now` would see it. A
/// target that has no plan (no endpoint, no account) is left out.
pub fn view_all(engine: &Engine, st: &EngineState, only: Option<&str>, now: SystemTime) -> Vec<view::TargetView> {
    let routing = &st.settings().routing;
    let mut out = Vec::new();
    for target in view_targets(engine, st, only) {
        let Ok(plan) = plan::plan(
            &st.registry,
            &st.accounts,
            &st.tokens,
            &st.live_models,
            &target,
            nullrouter_registry::schema::ModelType::Text,
            "",
        ) else {
            continue;
        };
        let (candidates, _) = candidates_of(engine, st, &plan, now);
        let length = routing.amortization_for.get(&target).copied().unwrap_or(routing.amortization);
        let mut deficits: BTreeMap<String, f64> = BTreeMap::new();
        {
            let state = engine.router.lock();
            for tier in [Tier::Subscription, Tier::Payg] {
                if let Some(l) = state.ledgers.get(&(target.clone(), tier)) {
                    deficits.extend(l.deficits_at(now, length));
                }
            }
        }
        let input = RoutingInput {
            target: target.clone(),
            candidates,
            amortization: length,
            size_tokens: 0,
            warm: None,
            deficits,
        };
        let window = AmortizationWindow { start: crate::routing::ledger::window_start(now, length), length };
        out.push(view::view(&input, window, now));
    }
    out
}

/// Places `req` over the plan's candidates. The router lock is held for the warm lookup only.
pub fn decide(engine: &Engine, st: &EngineState, req: &TextRequest, plan: &RequestPlan<'_>, now: SystemTime) -> Routed {
    let (mut candidates, step_of) = candidates_of(engine, st, plan, now);

    // A text request that carries a prompt has a chain; media and counts are always cold.
    let chain = (req.media.is_none() && !req.count)
        .then(|| Chain::build(&req.ir, &engine.router.salt, req.client.cache_ttl_key.as_deref()));
    let routing = &st.settings().routing;
    let length = routing.amortization_for.get(&req.target).copied().unwrap_or(routing.amortization);

    // One lock covers the warm lookup, the placement and the debit, so a request placed at the
    // same moment sees this one's debit and doesn't land on the same account.
    let mut state = engine.router.lock();
    let mut warm = None;
    let mut known = None;
    if let Some(chain) = &chain {
        let mut lookup: Vec<(CandidateKey, Duration)> =
            candidates.iter().map(|c| (c.key.clone(), c.cache.lifetime)).collect();
        // A disabled account still holds its warm entries: a request that finds one there is
        // `warm_unusable`, not simply cold.
        let mut disabled = Vec::new();
        for a in st.accounts.iter().filter(|a| a.disabled) {
            if let Some(c) = candidates.iter().find(|c| c.key.provider == a.provider) {
                let key = CandidateKey::new(&a.provider, &a.name, &c.key.model);
                let lifetime = a.routing.cache_lifetime.unwrap_or(c.cache.lifetime);
                lookup.push((key.clone(), lifetime));
                disabled.push((key, c.cache, a.priority));
            }
        }
        warm = state.warm.lookup(&req.agent.key, chain, &lookup, now);
        if let Some(w) = &warm {
            // What the agent's longest known prefix was recorded at, against the estimate for it.
            known = chain.boundaries.iter().find(|b| b.hash == w.hash).map(|b| (w.prefix_tokens, b.prefix_tokens));
        }
        if let Some(w) = &warm
            && !candidates.iter().any(|c| c.key == w.key)
            && let Some((key, cache, priority)) = disabled.into_iter().find(|(k, ..)| *k == w.key)
        {
            candidates.push(Candidate {
                key,
                order: i64::MAX,
                priority,
                out_of_service: Some("the account is disabled".into()),
                cooling: None,
                quota: AccountQuota::payg(),
                cache,
                price: PriceSpec::default(),
            });
        }
    }

    let total = chain.as_ref().and_then(|c| c.boundaries.last()).map_or(0, |b| b.prefix_tokens);
    let size_tokens = request_size(known, total);
    // Account keys are unique, so both tiers' deficits read from one map.
    let mut deficits: BTreeMap<String, f64> = BTreeMap::new();
    for tier in [Tier::Subscription, Tier::Payg] {
        let ledger = state.ledgers.entry((req.target.clone(), tier)).or_default();
        ledger.roll(now, length);
        for c in candidates.iter().filter(|c| c.quota.state.source.tier() == tier) {
            ledger.observe(&c.key.account_key(), c.priority);
        }
        deficits.extend(ledger.deficits().iter().map(|(k, v)| (k.clone(), *v)));
    }
    drop(state);

    let input =
        RoutingInput { target: req.target.clone(), candidates, amortization: length, size_tokens, warm, deficits };
    let placement = place(&input, now);
    let (stayed_on, warm_account) = match (&placement.decision.warm, &input.warm) {
        (Some(h), Some(w)) if h.stayed => (Some(w.hash), Some(w.key.clone())),
        _ => (None, None),
    };
    let order = placement
        .steps
        .iter()
        .filter_map(|(key, reason, rank)| {
            let i = input.candidates.iter().position(|c| c.key == *key)?;
            Some(Slot { step: *step_of.get(i)?, reason: *reason, rank: *rank })
        })
        .collect();
    let tally = Tally {
        target: req.target.clone(),
        length,
        shares: placement.shares,
        payg_shares: placement.payg_shares,
        tokens: size_tokens,
    };
    Routed { order, decision: placement.decision, chain, stayed_on, warm_account, tally, debit: Mutex::new(None) }
}

/// An attempt starts on `at`: unless it is warm work, the account is debited the request's
/// estimated tokens for the work it is about to do (R8). Done under the router lock, with the
/// ledger line queued under it.
pub fn start(engine: &Engine, routed: &Routed, at: &CandidateKey, now: SystemTime) {
    if routed.warm_account.as_ref() == Some(at) {
        return;
    }
    let t = &routed.tally;
    let mut state = engine.router.lock();
    let key = at.account_key();
    let (tier, shares) = if t.payg_shares.iter().any(|(k, _)| *k == key) {
        (Tier::Payg, &t.payg_shares)
    } else {
        (Tier::Subscription, &t.shares)
    };
    let ledger = state.ledgers.entry((t.target.clone(), tier)).or_default();
    let debit = ledger.debit(now, t.length, shares, &key, t.tokens);
    if debit.is_some() {
        queue(engine, t, tier, &state.ledgers[&(t.target.clone(), tier)], now);
    }
    *routed.debit.lock().unwrap_or_else(|e| e.into_inner()) = debit.map(|d| (tier, d));
}

/// The work an attempt did in plain tokens: input, cache read, cache write and output, the same
/// for every account (R8).
pub fn plain_tokens(u: &Usage) -> u64 {
    let input = match u.input_semantics {
        InputSemantics::IncludesCache => u.input.unwrap_or(0),
        _ => u.input.unwrap_or(0) + u.cache_read.unwrap_or(0) + u.cache_write.unwrap_or(0),
    };
    input + u.output.unwrap_or(0)
}

/// The attempt ended: a success settles its debit with the tokens it used, a failure takes it
/// back. `tokens` are plain tokens: input, cache read, cache write and output, unweighted.
pub fn finish(engine: &Engine, routed: &Routed, ok: bool, tokens: u64, now: SystemTime) {
    let Some((tier, mut debit)) = routed.debit.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
    let t = &routed.tally;
    let mut state = engine.router.lock();
    let ledger = state.ledgers.entry((t.target.clone(), tier)).or_default();
    if ok {
        ledger.settle(&mut debit, now, t.length, tokens);
    } else {
        ledger.reverse(&debit, now, t.length);
    }
    queue(engine, t, tier, &state.ledgers[&(t.target.clone(), tier)], now);
}

/// The ledger line: the target, the window and every deficit.
fn queue(engine: &Engine, t: &Tally, tier: Tier, ledger: &crate::routing::ledger::Ledger, now: SystemTime) {
    let Some(window) = ledger.window() else { return };
    let deficits: BTreeMap<&String, i64> = ledger.deficits().iter().map(|(k, v)| (k, v.round() as i64)).collect();
    engine.journal.append(
        Target::Ledger,
        "ledger",
        json!({
            "target": t.target,
            "tier": if tier == Tier::Payg { "payg" } else { "subscription" },
            "window_start": crate::quota::extract::rfc3339_millis(window),
            "deficits": deficits,
            "at": crate::quota::extract::rfc3339_millis(now),
        }),
    );
}

/// A request succeeded on `at`: remember what its provider now holds cached, and queue the
/// journal lines. `usage` corrects the estimated prefix lengths.
pub fn learn(
    engine: &Engine,
    agent: &str,
    routed: &Routed,
    at: &CandidateKey,
    cache: CacheSpec,
    usage: Option<&Usage>,
    now: SystemTime,
) {
    let Some(chain) = &routed.chain else { return };
    let mut written = chain.writable(cache.mode, cache.min_tokens);
    let estimated = chain.boundaries.last().map_or(0, |b| b.prefix_tokens);
    // The whole prompt, cached or not.
    let actual = usage.map_or(0, |u| match u.input_semantics {
        InputSemantics::IncludesCache => u.input.unwrap_or(0),
        _ => u.input.unwrap_or(0) + u.cache_read.unwrap_or(0) + u.cache_write.unwrap_or(0),
    });
    if estimated > 0 && actual > 0 {
        let scale = actual as f64 / estimated as f64;
        for w in &mut written {
            w.prefix_tokens = (w.prefix_tokens as f64 * scale).round() as u64;
        }
    }
    let mut state = engine.router.lock();
    if let Some(hit) = routed.stayed_on {
        state.warm.touch(agent, at, chain, hit, now);
    }
    state.warm.upsert(agent, at, &written, now);
    for w in &written {
        let mut line = json!({
            "agent": agent,
            "hash": w.hash.hex(),
            "provider": at.provider,
            "account": at.account,
            "model": at.model,
            "prefix_tokens": w.prefix_tokens,
            "last_used": crate::quota::extract::rfc3339_millis(now),
        });
        if let Some(ttl) = w.ttl {
            line["ttl_s"] = json!(ttl.as_secs());
        }
        engine.journal.append(Target::Warm, "warm", line);
    }
}

// ---------------------------------------------------------------------------------------------
// Persistence of the routing state (research R12)

/// What a start put back.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Restored {
    pub fingerprints: usize,
    pub ledgers: usize,
}

/// The cache lifetime of the account a fingerprint lives on; `None` when it no longer exists.
fn lifetime_of(st: &EngineState, at: &CandidateKey) -> Option<Duration> {
    let account = st.accounts.get(&at.provider, &at.account)?;
    let provider = st.registry.provider(&at.provider).ok()?;
    Some(cache_of(provider, Some(account)).lifetime)
}

fn length_of(st: &EngineState, target: &str) -> Duration {
    let routing = &st.settings().routing;
    routing.amortization_for.get(target).copied().unwrap_or(routing.amortization)
}

/// Puts back what `routing/warm.jsonl` and `routing/ledger.jsonl` held: fingerprints of accounts
/// that still exist and are inside their lifetime, and a ledger only if its window is the one
/// `now` falls in (a past window's deficits were reset at its boundary). Then compacts both files.
pub fn restore(engine: &Engine, st: &EngineState, now: SystemTime) -> Restored {
    let loaded = crate::journal::state::load(engine.home().path());
    let mut restored = Restored::default();
    {
        let mut state = engine.router.lock();
        for s in loaded.warm {
            if lifetime_of(st, &s.at).is_some() {
                state.warm.restore(s);
            }
        }
        state.warm.sweep(now, |at| lifetime_of(st, at));
        restored.fingerprints = state.warm.len();
        for l in loaded.ledgers {
            let length = length_of(st, &l.target);
            if l.window_start == crate::routing::ledger::window_start(now, length) {
                // An account removed while the router was down owes and is owed nothing.
                let known = |key: &str| key.split_once('/').is_some_and(|(p, a)| st.accounts.get(p, a).is_some());
                let deficits = l.deficits.into_iter().filter(|(k, _)| known(k)).collect();
                state.ledgers.entry((l.target, l.tier)).or_default().restore(l.window_start, deficits);
                restored.ledgers += 1;
            }
        }
    }
    compact(engine, st, now);
    restored
}

/// Deletes fingerprints past their lifetime; returns how many.
pub fn sweep(engine: &Engine, st: &EngineState, now: SystemTime) -> usize {
    engine.router.lock().warm.sweep(now, |at| lifetime_of(st, at))
}

/// Replaces both routing files with the live state: expired fingerprints and past windows are
/// left out. The replacement is queued under the router lock, so it is in order with the lines
/// a request queues.
pub fn compact(engine: &Engine, st: &EngineState, now: SystemTime) {
    let mut state = engine.router.lock();
    state.warm.sweep(now, |at| lifetime_of(st, at));
    let warm: String =
        state.warm.stored().iter().map(|s| crate::journal::state::warm_line(s).to_string() + "\n").collect();
    let ledgers: String = state
        .ledgers
        .iter()
        .filter_map(|((target, tier), l)| {
            let window = l.window()?;
            (window == crate::routing::ledger::window_start(now, length_of(st, target))).then(|| {
                crate::journal::state::ledger_line(target, *tier, window, l.deficits(), now).to_string() + "\n"
            })
        })
        .collect();
    engine.journal.replace(Target::Warm, warm);
    engine.journal.replace(Target::Ledger, ledgers);
}

/// An account was removed: its fingerprints and its ledger entries go, in memory and on disk.
pub fn drop_account(engine: &Engine, st: &EngineState, provider: &str, account: &str, now: SystemTime) -> usize {
    let key = format!("{provider}/{account}");
    let gone = {
        let mut state = engine.router.lock();
        for l in state.ledgers.values_mut() {
            l.forget(&key);
        }
        state.warm.drop_account(provider, account)
    };
    compact(engine, st, now);
    gone
}

/// An agent's records were forgotten: its fingerprints go, in memory and on disk.
pub fn drop_agent(engine: &Engine, st: &EngineState, agent: &str, now: SystemTime) -> usize {
    let gone = engine.router.lock().warm.drop_agent(agent);
    compact(engine, st, now);
    gone
}

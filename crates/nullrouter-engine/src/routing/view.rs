//! The routing view (spec 006, FR-029): what the operator sees of one target's accounts.
//!
//! Pure like the rest of the decision: it asks `place` for the pace, share and deficit rows a
//! decision at `now` would use, and joins each account's quota windows and settings to them.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use nullrouter_registry::schema::MeterDecl;
use serde::Serialize;

use super::place::place;
use super::{AmortizationWindow, QuotaSource, RoutingInput, Tier, WhyNot};
use crate::quota::fit::learner::{self, SetAsideCounts, WindowSnapshot};
use crate::quota::fit::outside::{self, Alert, OutsideEntry, OutsideType};
use crate::quota::fit::{AccountMeters, MeterNumber, NumberOverrides, NumberState, Source, TokenClass, WindowOverrides};
use crate::state::{Engine, EngineState};

/// The request size a view prices at: the shares a request of this size would see.
const VIEW_SIZE_TOKENS: u64 = 1_000;

/// One quota window of one account.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WindowView {
    pub name: String,
    /// `weighted_tokens` or `requests`.
    pub unit: &'static str,
    /// What the last poll reported left, in `unit`.
    pub remaining_at_poll: Option<f64>,
    /// 0router's own counted traffic since that poll, in `unit`.
    pub cost_since_poll: f64,
    pub remaining_now: f64,
    pub capacity: f64,
    pub capacity_assumed: bool,
    /// The floor, as a fraction of capacity.
    pub reserve: f64,
    #[serde(with = "super::opt_time_serde")]
    pub resets_at: Option<SystemTime>,
}

/// One account of a target, with the numbers its next cold placement would use.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AccountView {
    pub provider: String,
    pub account: String,
    pub tier: Tier,
    pub source: QuotaSource,
    /// The latest poll failed after a good one: the estimate keeps running.
    pub stale: bool,
    /// The provider reports this account's quota but nothing has been polled yet: on pace.
    pub pending_first_poll: bool,
    #[serde(with = "super::opt_time_serde")]
    pub polled_at: Option<SystemTime>,
    pub priority: f64,
    /// `None` when the account is not an option right now.
    pub pace: Option<f64>,
    pub share: Option<f64>,
    /// Tokens owed to the account in this amortization window.
    pub deficit: i64,
    #[serde(with = "super::duration_serde")]
    pub cache_lifetime: Duration,
    pub price_now: Option<f64>,
    pub why_not: Option<WhyNot>,
    pub windows: Vec<WindowView>,
    /// Per quota window, the numbers of the meter routing uses and where each came from (spec 012).
    pub meter: Vec<WindowMeterView>,
    pub outside_use: OutsideSummary,
    /// Present only for an account the fit leaves alone.
    pub fit_note: Option<String>,
}

/// One target: a unified model or a direct `provider/model`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TargetView {
    pub target: String,
    pub amortization_window: AmortizationWindow,
    pub accounts: Vec<AccountView>,
}

/// A fitted number's range, in the number's natural unit.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FitRange {
    pub value: f64,
    pub low: f64,
    pub high: f64,
}

/// How far a number's fit has come.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProgressView {
    pub intervals: u64,
    pub half_width: f64,
}

/// One number of one window's meter for one account (contracts/operator-socket.md).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NumberView {
    /// `capacity`, `weight.<class>` or `multiplier.<glob>`.
    pub number: String,
    /// `None` for a capacity the plugin leaves to be assumed.
    pub declared: Option<f64>,
    pub account_override: Option<f64>,
    pub plugin_override: Option<f64>,
    pub fit: Option<FitRange>,
    pub in_use: Option<f64>,
    pub source: Source,
    /// `learning`, `fitted`, `relearning`, `restarted`, `not_separable`, `yardstick`, `not_reported`.
    pub state: &'static str,
    #[serde(with = "super::opt_time_serde")]
    pub since: Option<SystemTime>,
    pub progress: Option<ProgressView>,
    pub partner: Option<String>,
    pub reason: Option<String>,
}

/// An account split off a window's pooled numbers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SplitView {
    #[serde(with = "super::time_serde")]
    pub since: SystemTime,
    pub reason: String,
}

/// Rows set aside, per reason.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct SetAsideView {
    pub reset: u64,
    pub usage_unreported: u64,
    pub exhausted: u64,
}

/// One quota window of one account, as the fit sees it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WindowMeterView {
    pub window: String,
    /// `None` while the learner holds nothing for the window.
    #[serde(with = "super::opt_time_serde")]
    pub epoch: Option<SystemTime>,
    pub split: Option<SplitView>,
    pub numbers: Vec<NumberView>,
    pub set_aside: SetAsideView,
}

/// A part of the day with a steady outside rate that is still going.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SteadyView {
    pub part: String,
    pub rate_per_hour: f64,
    pub since: String,
}

/// An account's outside use at a glance.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct OutsideSummary {
    /// Idle and busy entries that started in the last 7 days.
    pub intervals_7d: u64,
    /// The newest entry of any type.
    pub last: Option<OutsideEntry>,
    pub steady: Vec<SteadyView>,
    /// Unacknowledged, newest first.
    pub alerts: Vec<Alert>,
}

/// What the view adds to one account.
#[derive(Debug, Clone, Default)]
struct AccountFit {
    meter: Vec<WindowMeterView>,
    outside_use: OutsideSummary,
    fit_note: Option<String>,
}

/// The fit's side of the view, built off the request path with owned data: the learner is locked
/// once, briefly, to copy its windows out.
#[derive(Debug, Default)]
pub struct FitInputs {
    accounts: BTreeMap<(String, String), AccountFit>,
}

impl FitInputs {
    /// Everything the view needs of the fit, for every account of `st`.
    pub fn build(engine: &Engine, st: &EngineState, now: SystemTime) -> Self {
        let snaps = engine.fit_learner.snapshot_all();
        let home = engine.home().path();
        let mut overrides: BTreeMap<String, BTreeMap<String, WindowOverrides>> = BTreeMap::new();
        let mut accounts = BTreeMap::new();
        for account in st.accounts.iter() {
            let Some(entity) = st.registry.providers().find(|p| p.id == account.provider) else { continue };
            let key = (account.provider.clone(), account.name.clone());
            if let Some(note) = learner::unfitted_note(entity, account) {
                accounts.insert(key, AccountFit { fit_note: Some(note.to_owned()), ..AccountFit::default() });
                continue;
            }
            let ovs = overrides
                .entry(account.provider.clone())
                .or_insert_with(|| crate::quota::fit::window_overrides(st, &account.provider));
            let cell = engine.meters.get(&account.provider, &account.name);
            let routing = entity.routing();
            let meter = routing
                .windows
                .iter()
                .map(|m| {
                    let snap = snaps.get(&(account.provider.clone(), m.name.clone()));
                    window_meter_view(m, snap, ovs.get(&m.name), &account.name, cell.as_deref())
                })
                .collect();
            let outside_use = outside_summary(home, &account.provider, &account.name, now);
            accounts.insert(key, AccountFit { meter, outside_use, fit_note: None });
        }
        Self { accounts }
    }
}

fn outside_summary(home: &std::path::Path, provider: &str, account: &str, now: SystemTime) -> OutsideSummary {
    let week = now.checked_sub(Duration::from_secs(7 * 24 * 3600)).unwrap_or(SystemTime::UNIX_EPOCH);
    // Newest first.
    let entries = outside::read(home, provider, account, None, None).unwrap_or_default();
    let intervals_7d = entries
        .iter()
        .filter(|e| matches!(e.ty, OutsideType::Idle | OutsideType::Busy))
        .filter(|e| e.start_time().is_some_and(|t| t >= week))
        .count() as u64;
    let mut seen: Vec<&str> = Vec::new();
    let mut steady = Vec::new();
    for e in entries.iter().filter(|e| e.ty == OutsideType::Steady) {
        let Some(part) = e.part.as_deref() else { continue };
        if seen.contains(&part) {
            continue;
        }
        seen.push(part);
        if e.end.is_none()
            && let Some(rate) = e.rate_per_hour
        {
            steady.push(SteadyView { part: part.to_owned(), rate_per_hour: rate, since: e.start.clone() });
        }
    }
    OutsideSummary {
        intervals_7d,
        last: entries.first().cloned(),
        steady,
        alerts: outside::alerts(home, provider, account),
    }
}

fn weight_index(c: TokenClass) -> usize {
    TokenClass::ALL.iter().position(|x| *x == c).unwrap_or(0)
}

fn weight_value(w: &nullrouter_registry::schema::TokenWeights, c: TokenClass) -> f64 {
    match c {
        TokenClass::Input => w.input,
        TokenClass::Output => w.output,
        TokenClass::CacheRead => w.cache_read,
        TokenClass::CacheWrite => w.cache_write,
    }
}

/// The value `number` has in `meter`.
fn value_in(meter: &MeterDecl, number: &MeterNumber) -> Option<f64> {
    match number {
        MeterNumber::Capacity => meter.capacity,
        MeterNumber::Weight(c) => Some(weight_value(&meter.token_weights.unwrap_or_default(), *c)),
        MeterNumber::Multiplier(g) => meter.model_multiplier.get(g).copied(),
    }
}

fn override_of(o: &NumberOverrides, number: &MeterNumber) -> Option<f64> {
    match number {
        MeterNumber::Capacity => o.capacity,
        MeterNumber::Weight(c) => o.weights[weight_index(*c)],
        MeterNumber::Multiplier(g) => o.multipliers.get(g).copied(),
    }
}

/// One window of one account's meter. `snap` is what the learner holds for the window, `cell`
/// the account's meters in effect (`None`: every number is declared).
pub fn window_meter_view(
    declared: &MeterDecl,
    snap: Option<&WindowSnapshot>,
    ov: Option<&WindowOverrides>,
    account: &str,
    cell: Option<&AccountMeters>,
) -> WindowMeterView {
    let Some(snap) = snap else {
        return WindowMeterView {
            window: declared.name.clone(),
            epoch: None,
            split: None,
            numbers: Vec::new(),
            set_aside: SetAsideView::default(),
        };
    };
    let in_effect = cell.and_then(|c| c.windows.iter().find(|m| m.name == declared.name)).unwrap_or(declared);
    let sources = cell.and_then(|c| c.sources.get(&declared.name));
    // The input weight in effect: the yardstick fitted weights are ratios to on a percent window.
    let input = in_effect.token_weights.unwrap_or_default().input;
    let mut keys: Vec<(String, MeterNumber)> = vec![(format!("capacity@{account}"), MeterNumber::Capacity)];
    keys.extend(TokenClass::ALL.map(|c| (MeterNumber::Weight(c).to_string(), MeterNumber::Weight(c))));
    keys.extend(declared.model_multiplier.keys().map(|g| (MeterNumber::Multiplier(g.clone()).to_string(), MeterNumber::Multiplier(g.clone()))));
    let numbers = keys
        .into_iter()
        .filter_map(|(key, number)| {
            let state = snap.numbers.get(&key)?;
            let scale = if snap.percent && matches!(number, MeterNumber::Weight(c) if c != TokenClass::Input) { input } else { 1.0 };
            let fit = snap.ranges.get(&key).map(|(value, low, high)| FitRange { value: value * scale, low: low * scale, high: high * scale });
            let (name, since, progress, partner, reason) = match state {
                NumberState::Learning { progress } => ("learning", None, Some(*progress), None, None),
                NumberState::Fitted { since } => ("fitted", Some(*since), None, None, None),
                NumberState::Relearning { since, progress } => ("relearning", Some(*since), Some(*progress), None, Some("break".to_owned())),
                NumberState::Restarted { since, reason } => ("restarted", Some(*since), None, None, Some(reason.clone())),
                NumberState::NotSeparable { partner } => ("not_separable", None, None, Some(partner.clone()), None),
                NumberState::Yardstick => ("yardstick", None, None, None, None),
                NumberState::NotReported | NumberState::NotFitted(_) => ("not_reported", None, None, None, None),
            };
            Some(NumberView {
                declared: value_in(declared, &number),
                account_override: ov.and_then(|o| o.accounts.get(account)).and_then(|o| override_of(o, &number)),
                plugin_override: match number {
                    MeterNumber::Capacity => None,
                    _ => ov.and_then(|o| override_of(&o.plugin, &number)),
                },
                fit,
                in_use: value_in(in_effect, &number),
                source: sources.and_then(|s| s.get(&number)).copied().unwrap_or(Source::Declared),
                state: name,
                since,
                progress: progress.map(|p| ProgressView { intervals: p.intervals, half_width: p.half_width }),
                partner,
                reason,
                number: number.to_string(),
            })
        })
        .collect();
    let counts: SetAsideCounts = snap.set_aside.get(account).copied().unwrap_or_default();
    WindowMeterView {
        window: declared.name.clone(),
        epoch: Some(snap.epoch),
        split: snap.splits.get(account).map(|(since, reason)| SplitView { since: *since, reason: reason.clone() }),
        numbers,
        set_aside: SetAsideView { reset: counts.reset, usage_unreported: counts.usage_unreported, exhausted: counts.exhausted },
    }
}

/// `input`'s accounts as the next cold decision at `now` would see them. `window` is the target's
/// current amortization window.
pub fn view(input: &RoutingInput, window: AmortizationWindow, fit: &FitInputs, now: SystemTime) -> TargetView {
    let mut sized = input.clone();
    sized.size_tokens = VIEW_SIZE_TOKENS;
    sized.warm = None;
    let placement = place(&sized, now);
    let mut accounts: Vec<AccountView> = Vec::new();
    for (c, row) in input.candidates.iter().zip(&placement.decision.candidates) {
        if accounts.iter().any(|a| a.provider == c.key.provider && a.account == c.key.account) {
            continue;
        }
        let remaining_at_poll = |w: &super::meter::WindowState| w.remaining_at_poll;
        let fitted = fit.accounts.get(&(c.key.provider.clone(), c.key.account.clone())).cloned().unwrap_or_default();
        accounts.push(AccountView {
            provider: c.key.provider.clone(),
            account: c.key.account.clone(),
            tier: row.tier,
            source: row.quota_source,
            stale: c.quota.state.stale,
            pending_first_poll: c.quota.state.pending_first_poll,
            polled_at: c.quota.polled_at,
            priority: c.priority,
            pace: row.pace.filter(|_| row.eligible),
            share: row.share,
            deficit: row.deficit_before.unwrap_or(0),
            cache_lifetime: c.cache.lifetime,
            price_now: row.price_now,
            why_not: row.why_not,
            windows: c
                .quota
                .windows
                .iter()
                .map(|w| WindowView {
                    name: w.name.clone(),
                    unit: w.unit.as_str(),
                    remaining_at_poll: remaining_at_poll(w),
                    cost_since_poll: w.cost_since_poll,
                    remaining_now: w.remaining_now,
                    capacity: w.capacity,
                    capacity_assumed: w.capacity_assumed,
                    reserve: w.reserve,
                    resets_at: w.resets_at,
                })
                .collect(),
            meter: fitted.meter,
            outside_use: fitted.outside_use,
            fit_note: fitted.fit_note,
        });
    }
    TargetView { target: input.target.clone(), amortization_window: window, accounts }
}

/// Lines the view prints on their own: stale polls, assumed capacities.
pub fn warnings(target: &TargetView) -> Vec<String> {
    let mut out = Vec::new();
    for a in &target.accounts {
        let who = format!("{}/{}", a.provider, a.account);
        if a.pending_first_poll {
            out.push(format!("{who}: no poll yet, assumed on pace"));
        }
        if a.stale {
            out.push(format!("{who}: the last poll failed, estimate running (stale)"));
        }
        for w in a.windows.iter().filter(|w| w.capacity_assumed) {
            out.push(format!("{who}: window {} capacity assumed", w.name));
        }
    }
    out
}

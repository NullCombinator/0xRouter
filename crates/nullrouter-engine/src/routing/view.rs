//! The routing view (spec 006, FR-029): what the operator sees of one target's accounts.
//!
//! Pure like the rest of the decision: it asks `place` for the pace, share and deficit rows a
//! decision at `now` would use, and joins each account's quota windows and settings to them.

use std::time::{Duration, SystemTime};

use serde::Serialize;

use super::place::place;
use super::{AmortizationWindow, QuotaSource, RoutingInput, Tier, WhyNot};

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
}

/// One target: a unified model or a direct `provider/model`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TargetView {
    pub target: String,
    pub amortization_window: AmortizationWindow,
    pub accounts: Vec<AccountView>,
}

/// `input`'s accounts as the next cold decision at `now` would see them. `window` is the target's
/// current amortization window.
pub fn view(input: &RoutingInput, window: AmortizationWindow, now: SystemTime) -> TargetView {
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

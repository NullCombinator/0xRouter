//! Pace, rate, weight and shares (research R7).
//!
//! A window's pace is the share of its quota left divided by the share of its time left: above 1
//! the window will expire with quota unspent, below 1 it is being spent too fast. An account's
//! weight is its sustainable rate times its capped pace times the operator's priority, and the
//! shares of a target's accounts follow the weights.

use std::time::{Duration, SystemTime};

use nullrouter_registry::schema::MeterUnit;

use super::meter::{AccountQuota, Role, WindowState};

/// How far a nearly expired, unused window can draw cold work.
pub const PACE_CAP: f64 = 10.0;
/// Time left in a window is never taken below this, so the rate stays finite before a reset.
pub const TIME_LEFT_FLOOR: Duration = Duration::from_secs(60);
/// A request is never sized below this many tokens.
pub const SIZE_FLOOR: u64 = 1;

/// What an account's pacing windows say.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pace {
    /// The smallest pace over its pacing windows; 1 when none apply.
    pub pace: f64,
    /// Tokens per second the account can sustain: the smallest over its pacing windows. `None`
    /// when no window paces it (a first poll still pending, or only balance windows).
    pub rate: Option<f64>,
}

fn time_left(w: &WindowState, now: SystemTime) -> Option<Duration> {
    let left = w.resets_at?.duration_since(now).unwrap_or_default();
    Some(left.max(TIME_LEFT_FLOOR))
}

/// Pace and rate of `quota` for a request of `size` estimated tokens.
pub fn pace_of(quota: &AccountQuota, size: u64, now: SystemTime) -> Pace {
    let size = size.max(SIZE_FLOOR) as f64;
    let mut pace: Option<f64> = None;
    let mut rate: Option<f64> = None;
    for w in quota.windows.iter().filter(|w| w.role == Role::Pacing) {
        let Some(left) = time_left(w, now) else { continue };
        let left_s = left.as_secs_f64();
        let pi = match w.length {
            Some(len) if !len.is_zero() => w.fraction_left() / (left_s / len.as_secs_f64()).min(1.0),
            _ => 1.0,
        };
        let mut r = w.remaining_now / left_s;
        // A request-counted window is compared in tokens by the size of the request in hand.
        if w.unit == MeterUnit::Requests {
            r *= size;
        }
        pace = Some(pace.map_or(pi, |p| p.min(pi)));
        rate = Some(rate.map_or(r, |m| m.min(r)));
    }
    Pace { pace: pace.unwrap_or(1.0), rate }
}

/// `rate × min(pace, 10) × priority`; 0 for an account that can't take cold work.
pub fn weight(pace: Pace, rate: f64, priority: f64) -> f64 {
    (rate * pace.pace.min(PACE_CAP) * priority).max(0.0)
}

/// Shares of `weights` (`None` for an account that can't take cold work). They sum to 1 over the
/// accounts that can. An account with no rate of its own takes the mean of the others', so a
/// window-less account isn't starved or favoured by a unit it doesn't have.
pub fn shares(entries: &[Option<(Pace, f64)>]) -> Vec<(f64, f64)> {
    let known: Vec<f64> = entries.iter().flatten().filter_map(|(p, _)| p.rate).collect();
    let fallback = if known.is_empty() { 1.0 } else { known.iter().sum::<f64>() / known.len() as f64 };
    let weights: Vec<f64> = entries
        .iter()
        .map(|e| e.map_or(0.0, |(p, priority)| weight(p, p.rate.unwrap_or(fallback), priority)))
        .collect();
    let total: f64 = weights.iter().sum();
    let eligible = entries.iter().flatten().count();
    weights
        .iter()
        .zip(entries)
        .map(|(w, e)| {
            let share = match (e, total > 0.0) {
                (None, _) => 0.0,
                (Some(_), true) => w / total,
                // Every eligible account weighs 0 (a window at exactly 0 with no floor): equal shares.
                (Some(_), false) => 1.0 / eligible as f64,
            };
            (*w, share)
        })
        .collect()
}

/// The request's size for the rate comparison and the ledger (R7): what the agent's longest
/// known prefix was recorded at, plus the estimate over what comes after it. `known` is the
/// recorded tokens at that boundary and the estimate for the same boundary; `total` is the
/// estimate for the whole request. A cold request has no `known`.
pub fn request_size(known: Option<(u64, u64)>, total: u64) -> u64 {
    match known {
        Some((recorded, estimated_at_boundary)) => recorded + total.saturating_sub(estimated_at_boundary),
        None => total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::QuotaSource;
    use crate::routing::meter::QuotaState;

    const H: u64 = 3600;

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000)
    }

    fn win(unit: MeterUnit, length_s: u64, capacity: f64, left_frac: f64, time_left_s: u64) -> WindowState {
        WindowState {
            name: "w".into(),
            unit,
            length: Some(Duration::from_secs(length_s)),
            capacity,
            capacity_assumed: false,
            remaining_at_poll: None,
            polled_at: None,
            remaining_now: capacity * left_frac,
            resets_at: Some(now() + Duration::from_secs(time_left_s)),
            reserve: 0.05,
            role: Role::Pacing,
            refuses: false,
            cost_since_poll: 0.0,
            rolled_over: false,
        }
    }

    fn quota(windows: Vec<WindowState>) -> AccountQuota {
        AccountQuota {
            state: QuotaState { source: QuotaSource::Polled, pending_first_poll: false, stale: false },
            polled_at: None,
            windows,
        }
    }

    #[test]
    fn pace_is_share_of_quota_left_over_share_of_time_left() {
        // 80% left with 20% of the time left: spending slowly, pace 4.
        let q = quota(vec![win(MeterUnit::WeightedTokens, 5 * H, 1000.0, 0.8, H)]);
        let p = pace_of(&q, 100, now());
        assert!((p.pace - 4.0).abs() < 1e-9);
        // 800 tokens over 3600 s.
        assert!((p.rate.unwrap() - 800.0 / 3600.0).abs() < 1e-9);
        // 50% and 50%: on pace.
        let q = quota(vec![win(MeterUnit::WeightedTokens, 5 * H, 1000.0, 0.5, 5 * H / 2)]);
        assert!((pace_of(&q, 100, now()).pace - 1.0).abs() < 1e-9);
    }

    #[test]
    fn time_left_is_floored_so_the_rate_stays_finite() {
        let q = quota(vec![win(MeterUnit::WeightedTokens, 5 * H, 1000.0, 0.5, 0)]);
        let p = pace_of(&q, 1, now());
        assert!((p.rate.unwrap() - 500.0 / 60.0).abs() < 1e-9);
        assert!(p.pace.is_finite());
    }

    #[test]
    fn an_account_takes_the_minimum_pace_and_rate_over_its_pacing_windows() {
        let q = quota(vec![
            win(MeterUnit::WeightedTokens, 5 * H, 1000.0, 0.8, H), // π 4, r 800/3600
            win(MeterUnit::WeightedTokens, 168 * H, 70_000.0, 0.3, 84 * H), // π 0.6, r 21000/302400
        ]);
        let p = pace_of(&q, 100, now());
        assert!((p.pace - 0.6).abs() < 1e-9);
        assert!((p.rate.unwrap() - 21_000.0 / (84.0 * 3600.0)).abs() < 1e-9);
    }

    #[test]
    fn admission_and_balance_windows_do_not_pace() {
        let mut a = win(MeterUnit::Requests, 60, 10.0, 0.1, 30);
        a.role = Role::Admission;
        let mut b = win(MeterUnit::WeightedTokens, 0, 10.0, 0.1, 30);
        b.role = Role::Balance;
        let p = pace_of(&quota(vec![a, b]), 100, now());
        assert_eq!(p, Pace { pace: 1.0, rate: None });
    }

    #[test]
    fn a_request_counted_window_is_compared_by_the_size_of_the_request() {
        // Requests: 100 left over an hour; tokens: 200 per second. A large request favours the
        // request-counted account, a small one the token-counted account (scenario 4).
        let requests = quota(vec![win(MeterUnit::Requests, 5 * H, 500.0, 0.2, H)]);
        let tokens = quota(vec![win(MeterUnit::WeightedTokens, 5 * H, 720_000.0, 1.0, H)]);
        let r = |q: &AccountQuota, size| pace_of(q, size, now()).rate.unwrap();
        assert!(r(&requests, 100_000) > r(&tokens, 100_000));
        assert!(r(&requests, 10) < r(&tokens, 10));
        // The size is floored at one token.
        assert_eq!(r(&requests, 0), r(&requests, 1));
    }

    #[test]
    fn weight_is_rate_times_capped_pace_times_priority() {
        let p = Pace { pace: 25.0, rate: Some(2.0) };
        assert_eq!(weight(p, 2.0, 3.0), 2.0 * PACE_CAP * 3.0);
        assert_eq!(weight(Pace { pace: 0.5, rate: Some(2.0) }, 2.0, 2.0), 2.0);
        assert_eq!(weight(p, 2.0, 0.0), 0.0);
    }

    #[test]
    fn shares_sum_to_one_over_eligible_accounts() {
        let a = Pace { pace: 4.0, rate: Some(1.0) };
        let b = Pace { pace: 1.0, rate: Some(1.0) };
        let s = shares(&[Some((a, 1.0)), None, Some((b, 1.0))]);
        assert_eq!(s[1], (0.0, 0.0));
        assert!((s[0].1 + s[2].1 - 1.0).abs() < 1e-12);
        assert!((s[0].1 - 0.8).abs() < 1e-12);
    }

    #[test]
    fn an_account_without_a_rate_takes_the_mean_and_zero_weights_split_equally() {
        let a = Pace { pace: 1.0, rate: Some(3.0) };
        let none = Pace { pace: 1.0, rate: None };
        let s = shares(&[Some((a, 1.0)), Some((none, 1.0))]);
        assert!((s[0].1 - 0.5).abs() < 1e-12 && (s[1].1 - 0.5).abs() < 1e-12);
        let dead = Pace { pace: 0.0, rate: Some(1.0) };
        let s = shares(&[Some((dead, 1.0)), Some((dead, 1.0))]);
        assert_eq!((s[0].1, s[1].1), (0.5, 0.5));
        assert!(shares(&[None, None]).iter().all(|x| x.1 == 0.0));
    }

    #[test]
    fn request_size_adds_the_unrecorded_remainder() {
        assert_eq!(request_size(None, 900), 900);
        // 1,000 recorded at a boundary the estimate put at 800; the whole request estimates 1,100.
        assert_eq!(request_size(Some((1000, 800)), 1100), 1300);
        assert_eq!(request_size(Some((1000, 800)), 700), 1000);
    }
}

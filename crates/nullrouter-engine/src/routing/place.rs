//! The placement decision: warm step, subscription tier, pay-as-you-go overflow, last resort
//! (research R4, R9).
//!
//! `place` reads a [`RoutingInput`] and `now`, and returns the attempt order with a reason for
//! each step and the table of rows behind it (the [`Decision`] a request record keeps).

use std::time::SystemTime;

use super::ledger::window_start;
use super::pace::{self, Pace};
use super::price;
use super::{
    AmortizationWindow, CandidateKey, CandidateRow, Decision, DecisionKind, MovedBecause, Placement, PlacementReason,
    RoutingInput, Tier, WarmHit, WhyNot,
};

/// What one candidate can do right now.
struct Standing {
    why_not: Option<WhyNot>,
    /// Held back only by a reserve floor: a last resort.
    floor_held: bool,
}

fn standing(c: &super::Candidate) -> Standing {
    let why_not = if c.out_of_service.is_some() {
        Some(WhyNot::OutOfService)
    } else if c.cooling.is_some() {
        Some(WhyNot::Cooling)
    } else if c.quota.refused() {
        Some(WhyNot::Admission)
    } else if c.quota.at_floor() {
        Some(WhyNot::ReserveFloor)
    } else if c.priority <= 0.0 {
        Some(WhyNot::PriorityZero)
    } else {
        None
    };
    Standing { why_not, floor_held: why_not == Some(WhyNot::ReserveFloor) && c.priority > 0.0 }
}

pub fn place(input: &RoutingInput, now: SystemTime) -> Placement {
    let stands: Vec<Standing> = input.candidates.iter().map(standing).collect();
    let tier = |i: usize| input.candidates[i].quota.state.source.tier();
    let eligible = |i: usize| stands[i].why_not.is_none();
    // Pace, weight and share of the target's eligible subscription accounts (R7).
    let paces: Vec<Pace> = input.candidates.iter().map(|c| pace::pace_of(&c.quota, input.size_tokens, now)).collect();
    let subs_all: Vec<usize> = (0..stands.len()).filter(|&i| eligible(i) && tier(i) == Tier::Subscription).collect();
    let entries: Vec<Option<(Pace, f64)>> =
        subs_all.iter().map(|&i| Some((paces[i], input.candidates[i].priority))).collect();
    let mut weight_of = vec![0.0; stands.len()];
    let mut share_of = vec![0.0; stands.len()];
    for (&i, (w, s)) in subs_all.iter().zip(pace::shares(&entries)) {
        weight_of[i] = w;
        share_of[i] = s;
    }
    // A subscription can serve when one is eligible and has weight to take cold work.
    let any_subscription = subs_all.iter().any(|&i| weight_of[i] > 0.0);

    // The warm step (R4): the warm candidate stays unless one of four things moves it.
    let mut warm_hit = None;
    let mut moved_from = None;
    let mut first_reason = PlacementReason::ColdByDeficit;
    if let Some(w) = &input.warm
        && let Some(i) = input.candidates.iter().position(|c| c.key == w.key)
    {
        let c = &input.candidates[i];
        // Priority never moves warm work, and a priority 0 account keeps what is warm on it.
        let because = if c.out_of_service.is_some() {
            Some(MovedBecause::WarmUnusable)
        } else if c.cooling.is_some() || c.quota.refused() {
            Some(MovedBecause::RateLimited)
        } else if c.quota.at_floor() {
            Some(MovedBecause::ReserveFloor)
        } else if tier(i) == Tier::Payg && any_subscription {
            Some(MovedBecause::LeftPayAsYouGo)
        } else {
            None
        };
        warm_hit = Some((i, because, w));
        if let Some(b) = because {
            moved_from = Some(i);
            first_reason = match b {
                MovedBecause::WarmUnusable => PlacementReason::ColdByDeficit,
                MovedBecause::RateLimited | MovedBecause::ReserveFloor => PlacementReason::MovedForCapacity,
                MovedBecause::LeftPayAsYouGo => PlacementReason::LeftPayAsYouGo,
            };
        }
    }
    let stays = warm_hit.as_ref().is_some_and(|(_, b, _)| b.is_none());

    let mut order: Vec<(usize, PlacementReason)> = Vec::new();
    if stays {
        order.push((warm_hit.as_ref().map_or(0, |(i, ..)| *i), PlacementReason::Warm));
    }

    // Cold work: subscriptions by deficit, then pay-as-you-go overflow by its own deficit, then
    // accounts held back only by a reserve floor.
    let by_order = |i: &usize, j: &usize| {
        let (a, b) = (&input.candidates[*i], &input.candidates[*j]);
        (a.order, &a.key.account, &a.key.provider).cmp(&(b.order, &b.key.account, &b.key.provider))
    };

    // Pay-as-you-go accounts share overflow by priority ÷ the price in effect now (R10).
    let prices: Vec<f64> = input.candidates.iter().map(|c| price::rank_price(&c.price, now)).collect();
    let paygs_all: Vec<usize> = (0..stands.len()).filter(|&i| eligible(i) && tier(i) == Tier::Payg).collect();
    let payg_weights: Vec<f64> = paygs_all.iter().map(|&i| input.candidates[i].priority / prices[i]).collect();
    let payg_total: f64 = payg_weights.iter().sum();
    for (&i, w) in paygs_all.iter().zip(&payg_weights) {
        weight_of[i] = *w;
        share_of[i] = if payg_total > 0.0 { w / payg_total } else { 1.0 / paygs_all.len() as f64 };
    }
    let deficit_of = |i: usize| input.deficits.get(&input.candidates[i].key.account_key()).copied().unwrap_or(0.0);
    // The largest deficit first; ties go to the higher share, then the operator's order, then the name (FR-016).
    let by_deficit = |i: &usize, j: &usize| {
        let (di, dj) = (deficit_of(*i), deficit_of(*j));
        if (di - dj).abs() > 1e-6 {
            return dj.total_cmp(&di);
        }
        if (share_of[*i] - share_of[*j]).abs() > 1e-12 {
            return share_of[*j].total_cmp(&share_of[*i]);
        }
        by_order(i, j)
    };

    let skip = |i: usize| Some(i) == moved_from || order.iter().any(|(j, _)| *j == i);
    let mut subs: Vec<usize> =
        (0..stands.len()).filter(|&i| eligible(i) && tier(i) == Tier::Subscription && !skip(i)).collect();
    let mut paygs: Vec<usize> =
        (0..stands.len()).filter(|&i| eligible(i) && tier(i) == Tier::Payg && !skip(i)).collect();
    let mut floored: Vec<usize> = (0..stands.len()).filter(|&i| stands[i].floor_held && !skip(i)).collect();
    // What a moved request left behind is still a way to serve it if everything else fails.
    let mut left: Vec<usize> = moved_from
        .filter(|&i| {
            matches!(stands[i].why_not, None | Some(WhyNot::ReserveFloor)) && input.candidates[i].priority > 0.0
        })
        .into_iter()
        .collect();
    subs.sort_by(by_deficit);
    paygs.sort_by(by_deficit);
    floored.sort_by(by_order);
    left.sort_by(by_order);

    let mut cold_first = true;
    for i in subs {
        let reason = if order.is_empty() && cold_first { first_reason } else { PlacementReason::Fallback };
        cold_first = false;
        order.push((i, reason));
    }
    for i in paygs {
        let reason = if order.is_empty() { first_reason_for_overflow(first_reason) } else { PlacementReason::Fallback };
        order.push((i, reason));
    }
    for i in floored.into_iter().chain(left) {
        let reason = if order.is_empty() { PlacementReason::LastResort } else { PlacementReason::Fallback };
        order.push((i, reason));
    }
    // A cooling account is last: the walk records its skip and its rest time (slice 003).
    let mut cooling: Vec<usize> = (0..stands.len())
        .filter(|&i| {
            stands[i].why_not == Some(WhyNot::Cooling)
                && input.candidates[i].priority > 0.0
                && !order.iter().any(|(j, _)| *j == i)
        })
        .collect();
    cooling.sort_by(by_order);
    order.extend(cooling.into_iter().map(|i| (i, PlacementReason::Fallback)));

    let rows: Vec<CandidateRow> = input
        .candidates
        .iter()
        .zip(&stands)
        .enumerate()
        .map(|(i, (c, s))| CandidateRow {
            provider: c.key.provider.clone(),
            account: c.key.account.clone(),
            model: c.key.model.clone(),
            tier: c.quota.state.source.tier(),
            eligible: s.why_not.is_none(),
            why_not: s.why_not,
            quota_source: c.quota.state.source,
            pace: (c.quota.state.source.tier() == Tier::Subscription).then_some(paces[i].pace),
            rate: if c.quota.state.source.tier() == Tier::Subscription { paces[i].rate } else { None },
            priority: c.priority,
            weight: s.why_not.is_none().then_some(weight_of[i]),
            share: s.why_not.is_none().then_some(share_of[i]),
            deficit_before: Some(deficit_of(i).round() as i64),
            price_now: (tier(i) == Tier::Payg).then(|| price::price_now(&c.price, now)).flatten(),
            meter_sources: c.meter_sources.clone(),
        })
        .collect();

    let kind = match order.first() {
        None => DecisionKind::None,
        Some((_, PlacementReason::Warm)) => DecisionKind::Warm,
        Some((i, _)) if tier(*i) == Tier::Payg && any_payg_only(&stands, &tier) => DecisionKind::Overflow,
        Some(_) => DecisionKind::Cold,
    };
    let warm = warm_hit.map(|(i, because, w)| WarmHit {
        provider: input.candidates[i].key.provider.clone(),
        account: input.candidates[i].key.account.clone(),
        model: input.candidates[i].key.model.clone(),
        prefix_tokens: w.prefix_tokens,
        idle_s: now.duration_since(w.last_used).map_or(0.0, |d| d.as_secs_f64()),
        stayed: because.is_none(),
        moved_because: because,
    });
    let decision = Decision {
        kind,
        at: now,
        amortization_window: AmortizationWindow {
            start: window_start(now, input.amortization),
            length: input.amortization,
        },
        size_tokens: input.size_tokens,
        warm,
        candidates: rows,
        order: order.iter().map(|(i, _)| *i).collect(),
    };
    let steps: Vec<(CandidateKey, PlacementReason, usize)> =
        order.iter().enumerate().map(|(rank, (i, reason))| (input.candidates[*i].key.clone(), *reason, rank)).collect();
    let shares = subs_all
        .iter()
        .filter(|&&i| share_of[i] > 0.0)
        .map(|&i| (input.candidates[i].key.account_key(), share_of[i]))
        .collect();
    let payg_shares = paygs_all
        .iter()
        .filter(|&&i| share_of[i] > 0.0)
        .map(|&i| (input.candidates[i].key.account_key(), share_of[i]))
        .collect();
    Placement { decision, steps, shares, payg_shares }
}

/// A first step on a pay-as-you-go account is overflow, unless the request left one for it.
fn first_reason_for_overflow(first: PlacementReason) -> PlacementReason {
    match first {
        PlacementReason::MovedForCapacity | PlacementReason::LeftPayAsYouGo => first,
        _ => PlacementReason::Overflow,
    }
}

/// No subscription is eligible: pay-as-you-go serves as overflow.
fn any_payg_only(stands: &[Standing], tier: &impl Fn(usize) -> Tier) -> bool {
    !(0..stands.len()).any(|i| tier(i) == Tier::Subscription && stands[i].why_not.is_none())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use nullrouter_registry::schema::CacheMode;

    use super::*;
    use crate::routing::meter::{AccountQuota, QuotaState, Role, WindowState};
    use crate::routing::{CacheSpec, Candidate, PriceSpec, QuotaSource, WarmEntry};
    use nullrouter_registry::schema::MeterUnit;

    fn t(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 + secs)
    }

    fn window(left: f64) -> WindowState {
        WindowState {
            name: "5h".into(),
            unit: MeterUnit::WeightedTokens,
            length: Some(Duration::from_secs(5 * 3600)),
            capacity: 100.0,
            capacity_assumed: false,
            remaining_at_poll: Some(left),
            polled_at: None,
            remaining_now: left,
            resets_at: Some(t(0) + Duration::from_secs(5 * 3600 / 2)),
            reserve: 0.05,
            role: Role::Pacing,
            refuses: false,
            cost_since_poll: 0.0,
            rolled_over: false,
        }
    }

    fn sub(name: &str, order: i64, left: f64) -> Candidate {
        Candidate {
            key: CandidateKey::new("p", name, "m"),
            order,
            priority: 1.0,
            out_of_service: None,
            cooling: None,
            quota: AccountQuota {
                state: QuotaState { source: QuotaSource::Polled, pending_first_poll: false, stale: false },
                polled_at: None,
                windows: vec![window(left)],
            },
            cache: CacheSpec { mode: CacheMode::Automatic, lifetime: Duration::from_secs(300), min_tokens: 1024 },
            price: PriceSpec::default(),
            meter_sources: None,
        }
    }

    fn payg(name: &str, order: i64) -> Candidate {
        Candidate { quota: AccountQuota::payg(), ..sub(name, order, 50.0) }
    }

    fn input(candidates: Vec<Candidate>, warm_on: Option<&str>) -> RoutingInput {
        RoutingInput {
            target: "u".into(),
            candidates,
            amortization: Duration::from_secs(5 * 3600),
            size_tokens: 100,
            warm: warm_on.map(|n| WarmEntry {
                key: CandidateKey::new("p", n, "m"),
                hash: crate::routing::fingerprint::Hash([1; 16]),
                prefix_tokens: 500,
                last_used: t(0),
            }),
            deficits: Default::default(),
        }
    }

    fn names(p: &Placement) -> Vec<(String, PlacementReason)> {
        p.steps.iter().map(|(k, r, _)| (k.account.clone(), *r)).collect()
    }

    #[test]
    fn a_warm_candidate_stays_whatever_the_order_says() {
        let p = place(&input(vec![sub("a", 0, 90.0), sub("b", 1, 10.0)], Some("b")), t(10));
        assert_eq!(names(&p)[0], ("b".into(), PlacementReason::Warm));
        assert_eq!(p.decision.kind, DecisionKind::Warm);
        let w = p.decision.warm.unwrap();
        assert!(w.stayed && w.moved_because.is_none());
        assert_eq!(w.idle_s, 10.0);
    }

    #[test]
    fn cold_work_goes_to_the_largest_deficit_then_share_then_order_then_name() {
        let mut i = input(vec![sub("a", 0, 50.0), sub("b", 1, 50.0), sub("c", 2, 50.0)], None);
        i.deficits.insert("p/c".into(), 500.0);
        i.deficits.insert("p/b".into(), 100.0);
        assert_eq!(names(&place(&i, t(0)))[0], ("c".into(), PlacementReason::ColdByDeficit));
        // Equal deficits and shares: the operator's order, then the name.
        let i = input(vec![sub("z", 1, 50.0), sub("y", 1, 50.0), sub("x", 0, 50.0)], None);
        let order: Vec<_> = names(&place(&i, t(0))).into_iter().map(|n| n.0).collect();
        assert_eq!(order, ["x", "y", "z"]);
        // Equal deficits, the larger share first: 80% left against 50% left, same time left.
        let i = input(vec![sub("a", 0, 50.0), sub("b", 1, 80.0)], None);
        let p = place(&i, t(0));
        assert_eq!(names(&p)[0].0, "b");
        assert!(p.decision.candidates[1].share > p.decision.candidates[0].share);
    }

    #[test]
    fn rows_carry_pace_rate_weight_share_and_deficit_and_shares_sum_to_one() {
        let mut i = input(vec![sub("a", 0, 80.0), sub("b", 1, 50.0), payg("k", 2)], None);
        i.deficits.insert("p/a".into(), 1234.4);
        let p = place(&i, t(0));
        let rows = &p.decision.candidates;
        assert!(rows[0].pace.is_some() && rows[0].rate.is_some() && rows[0].weight.is_some());
        assert_eq!(rows[0].deficit_before, Some(1234));
        assert!(rows[2].pace.is_none() && rows[2].price_now.is_none());
        let total: f64 = rows.iter().filter(|r| r.tier == Tier::Subscription).filter_map(|r| r.share).sum();
        assert!((total - 1.0).abs() < 1e-9);
        assert!((p.shares.iter().map(|s| s.1).sum::<f64>() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn priority_zero_is_left_out_of_every_cold_step() {
        let mut a = sub("a", 0, 4.0); // at its floor: would be a last resort
        a.priority = 0.0;
        let mut b = sub("b", 1, 50.0);
        b.cooling = Some(Duration::from_secs(5)); // would be a cooling fallback
        let mut c = sub("c", 2, 50.0);
        c.priority = 0.0;
        c.cooling = Some(Duration::from_secs(5));
        let p = place(&input(vec![a, b, c], None), t(0));
        assert_eq!(names(&p), [("b".into(), PlacementReason::Fallback)]);
    }

    #[test]
    fn an_account_refused_by_an_admission_window_takes_no_share_and_builds_no_deficit() {
        let mut a = sub("a", 0, 90.0);
        a.quota.windows[0].refuses = true;
        let mut i = input(vec![a, sub("b", 1, 50.0)], None);
        i.deficits.insert("p/b".into(), -40.0);
        let p = place(&i, t(0));
        assert_eq!(p.decision.candidates[0].why_not, Some(WhyNot::Admission));
        assert!(p.decision.candidates[0].share.is_none());
        // The ledger credits only the accounts that can take work: a is not among them.
        assert_eq!(p.shares, [("p/b".to_string(), 1.0)]);
        assert_eq!(names(&p)[0].0, "b");
    }

    #[test]
    fn cold_work_follows_the_operator_order_for_now() {
        let p = place(&input(vec![sub("b", 1, 50.0), sub("a", 0, 50.0)], None), t(0));
        assert_eq!(names(&p), [("a".into(), PlacementReason::ColdByDeficit), ("b".into(), PlacementReason::Fallback)]);
        assert_eq!(p.decision.kind, DecisionKind::Cold);
        assert_eq!(p.steps.iter().map(|s| s.2).collect::<Vec<_>>(), [0, 1]);
    }

    #[test]
    fn a_cooling_warm_account_moves_for_capacity() {
        let mut a = sub("a", 0, 50.0);
        a.cooling = Some(Duration::from_secs(30));
        let p = place(&input(vec![a, sub("b", 1, 50.0)], Some("a")), t(1));
        assert_eq!(names(&p)[0], ("b".into(), PlacementReason::MovedForCapacity));
        assert_eq!(p.decision.warm.as_ref().unwrap().moved_because, Some(MovedBecause::RateLimited));
        // The cooling account stays last, for the walk to record its rest.
        assert_eq!(names(&p).last().unwrap().0, "a");
    }

    #[test]
    fn a_window_at_its_floor_moves_warm_work_and_is_a_last_resort() {
        let p = place(&input(vec![sub("a", 0, 4.0), sub("b", 1, 50.0)], Some("a")), t(1));
        assert_eq!(
            names(&p),
            [("b".into(), PlacementReason::MovedForCapacity), ("a".into(), PlacementReason::Fallback)]
        );
        assert_eq!(p.decision.warm.unwrap().moved_because, Some(MovedBecause::ReserveFloor));
        // With nothing else, the floor-held account serves as the last resort.
        let p = place(&input(vec![sub("a", 0, 4.0)], None), t(1));
        assert_eq!(names(&p), [("a".into(), PlacementReason::LastResort)]);
    }

    #[test]
    fn warm_on_pay_as_you_go_leaves_once_a_subscription_can_serve() {
        let p = place(&input(vec![payg("k", 0), sub("a", 1, 50.0)], Some("k")), t(1));
        assert_eq!(names(&p)[0], ("a".into(), PlacementReason::LeftPayAsYouGo));
        assert_eq!(p.decision.warm.as_ref().unwrap().moved_because, Some(MovedBecause::LeftPayAsYouGo));
        // No subscription can serve: it stays.
        let mut a = sub("a", 1, 50.0);
        a.cooling = Some(Duration::from_secs(5));
        let p = place(&input(vec![payg("k", 0), a], Some("k")), t(1));
        assert_eq!(names(&p)[0], ("k".into(), PlacementReason::Warm));
    }

    #[test]
    fn an_unusable_warm_account_is_ignored() {
        let mut a = sub("a", 0, 50.0);
        a.out_of_service = Some("needs sign-in".into());
        let p = place(&input(vec![a, sub("b", 1, 50.0)], Some("a")), t(1));
        assert_eq!(names(&p), [("b".into(), PlacementReason::ColdByDeficit)]);
        assert_eq!(p.decision.warm.unwrap().moved_because, Some(MovedBecause::WarmUnusable));
    }

    #[test]
    fn priority_zero_keeps_its_warm_work_but_gets_no_cold_work() {
        let mut a = sub("a", 0, 50.0);
        a.priority = 0.0;
        let p = place(&input(vec![a.clone(), sub("b", 1, 50.0)], Some("a")), t(1));
        assert_eq!(names(&p)[0], ("a".into(), PlacementReason::Warm));
        let p = place(&input(vec![a, sub("b", 1, 50.0)], None), t(1));
        assert_eq!(names(&p), [("b".into(), PlacementReason::ColdByDeficit)]);
        assert_eq!(p.decision.candidates[0].why_not, Some(WhyNot::PriorityZero));
    }

    #[test]
    fn pay_as_you_go_serves_cold_work_only_as_overflow() {
        let p = place(&input(vec![payg("k", 0), sub("a", 1, 50.0)], None), t(1));
        assert_eq!(names(&p)[0].0, "a");
        assert_eq!(names(&p)[1], ("k".into(), PlacementReason::Fallback));
        let mut a = sub("a", 1, 50.0);
        a.cooling = Some(Duration::from_secs(5));
        let p = place(&input(vec![payg("k", 0), a], None), t(1));
        assert_eq!(names(&p)[0], ("k".into(), PlacementReason::Overflow));
        assert_eq!(p.decision.kind, DecisionKind::Overflow);
    }

    #[test]
    fn an_empty_order_is_a_decision_of_none() {
        let mut a = sub("a", 0, 50.0);
        a.priority = 0.0;
        let p = place(&input(vec![a], None), t(1));
        assert!(p.steps.is_empty());
        assert_eq!(p.decision.kind, DecisionKind::None);
        assert_eq!(p.decision.amortization_window.length, Duration::from_secs(5 * 3600));
    }
    fn priced(mut c: Candidate, input: f64, priority: f64) -> Candidate {
        c.priority = priority;
        c.price = PriceSpec {
            schedule: vec![nullrouter_registry::schema::PriceDecl {
                when: None,
                input,
                output: None,
                cache_read: None,
                cache_write: None,
            }],
            flat: None,
        };
        c
    }

    #[test]
    fn overflow_is_shared_by_priority_over_price_and_only_after_the_subscriptions() {
        // Priority 1 at $1 against priority 1 at $4: the cheaper one first, with 80% of the share.
        let i =
            input(vec![priced(payg("dear", 0), 4.0, 1.0), priced(payg("cheap", 1), 1.0, 1.0), sub("a", 2, 50.0)], None);
        let p = place(&i, t(0));
        let order: Vec<_> = names(&p).into_iter().map(|n| n.0).collect();
        assert_eq!(order, ["a", "cheap", "dear"]);
        let row = |n: &str| p.decision.candidates.iter().find(|c| c.account == n).unwrap().clone();
        assert!((row("cheap").share.unwrap() - 0.8).abs() < 1e-9);
        assert_eq!(row("cheap").price_now, Some(1.0));
        assert_eq!(p.payg_shares.len(), 2);
        // A subscription that can serve keeps the decision cold.
        assert_eq!(p.decision.kind, DecisionKind::Cold);
        // A higher priority offsets a higher price.
        let i = input(vec![priced(payg("dear", 0), 4.0, 8.0), priced(payg("cheap", 1), 1.0, 1.0)], None);
        assert_eq!(names(&place(&i, t(0)))[0], ("dear".into(), PlacementReason::Overflow));
    }

    #[test]
    fn overflow_goes_to_the_larger_deficit_and_the_largest_priority_over_price_first() {
        let mut i = input(vec![priced(payg("a", 0), 1.0, 1.0), priced(payg("b", 1), 1.0, 1.0)], None);
        i.deficits.insert("p/b".into(), 50.0);
        let p = place(&i, t(0));
        assert_eq!(names(&p)[0], ("b".into(), PlacementReason::Overflow));
        assert_eq!(p.decision.kind, DecisionKind::Overflow);
        // Equal deficits and shares: the operator's order.
        let i = input(vec![priced(payg("a", 0), 1.0, 1.0), priced(payg("b", 1), 1.0, 1.0)], None);
        assert_eq!(names(&place(&i, t(0)))[0].0, "a");
    }

    #[test]
    fn crossing_into_the_off_peak_period_changes_the_next_decision() {
        use nullrouter_registry::schema::{PriceDecl, PriceWhen};
        let off_peak = |input: f64| PriceDecl {
            when: Some(PriceWhen { from: Some("16:30".into()), to: Some("23:59".into()), ..Default::default() }),
            input,
            output: None,
            cache_read: None,
            cache_write: None,
        };
        let day = |input: f64| PriceDecl { when: None, input, output: None, cache_read: None, cache_write: None };
        // `a` is cheaper by day, `b` has an evening discount; the epoch plus 1,000,000 s is 13:46 UTC.
        let a = priced(payg("a", 0), 2.0, 1.0);
        let mut b = payg("b", 1);
        b.price = PriceSpec { schedule: vec![off_peak(1.0), day(4.0)], flat: None };
        let i = input(vec![a, b], None);
        assert_eq!(names(&place(&i, t(0)))[0].0, "a");
        let evening = t(0) + Duration::from_secs(3 * 3600);
        let p = place(&i, evening);
        assert_eq!(names(&p)[0].0, "b");
        assert_eq!(p.decision.candidates[1].price_now, Some(1.0));
    }

    #[test]
    fn priority_zero_overflow_accounts_receive_nothing_and_warm_pay_as_you_go_leaves_for_a_subscription() {
        let i = input(vec![priced(payg("k", 0), 1.0, 0.0)], None);
        let p = place(&i, t(0));
        assert!(p.steps.is_empty());
        assert_eq!(p.decision.candidates[0].why_not, Some(WhyNot::PriorityZero));
        // A subscription whose weight is 0 (priority 0) doesn't count as able to serve.
        let mut zero = sub("a", 1, 50.0);
        zero.priority = 0.0;
        let p = place(&input(vec![priced(payg("k", 0), 1.0, 1.0), zero], Some("k")), t(1));
        assert_eq!(names(&p)[0], ("k".into(), PlacementReason::Warm));
    }
}

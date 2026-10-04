//! Deficit ledgers per (target, tier) over epoch-aligned amortization windows (research R8).

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The start of the amortization window `now` falls in: windows are multiples of `length` since
/// the Unix epoch (UTC), so a restart or a second process finds the same boundaries.
pub fn window_start(now: SystemTime, length: Duration) -> SystemTime {
    let len = length.as_secs().max(1);
    let secs = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    UNIX_EPOCH + Duration::from_secs(secs - secs % len)
}


/// No deficit grows past this many tokens either way: larger than any single request, so a long
/// block can't build a burst larger than that.
pub const CLAMP: f64 = 2_000_000.0;

/// What one placement did to a ledger, kept so its settlement or reversal applies the same shares
/// to the same window.
#[derive(Debug, Clone, PartialEq)]
pub struct Debit {
    pub window_start: SystemTime,
    /// `(account, share)` of every account that was eligible, in plain tokens' fractions.
    pub shares: Vec<(String, f64)>,
    pub placed: String,
    /// Tokens debited so far (the estimate, then the actual after a settlement).
    pub tokens: u64,
}

/// Deficits of one target's accounts in one tier, in the current amortization window. A deficit
/// is the work an account is owed against its share, in plain tokens.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ledger {
    window: Option<SystemTime>,
    deficits: BTreeMap<String, f64>,
    /// The priority each account had when last seen: a change starts it again at 0.
    priorities: BTreeMap<String, f64>,
    /// The amortization length the window was aligned with.
    length: Option<Duration>,
}

impl Ledger {
    /// Moves into the window `now` falls in, resetting every deficit when it is a later one. A
    /// clock that jumps back never returns to an earlier window.
    pub fn roll(&mut self, now: SystemTime, length: Duration) {
        let current = window_start(now, length);
        // A changed length keeps what is owed and starts its new window at the next boundary of
        // the new alignment.
        if self.length.replace(length).is_some_and(|old| old != length) && self.window.is_some() {
            self.window = Some(current);
            return;
        }
        match self.window {
            Some(w) if current <= w => {}
            _ => {
                if self.window.is_some() {
                    self.deficits.clear();
                }
                self.window = Some(current);
            }
        }
    }

    /// Notes an account's priority; one that changed since it was last seen starts at 0.
    pub fn observe(&mut self, account: &str, priority: f64) {
        if self.priorities.insert(account.to_string(), priority).is_some_and(|p| p != priority) {
            self.deficits.remove(account);
        }
    }

    /// An account that was added or enabled again starts at 0 with no history.
    pub fn forget(&mut self, account: &str) {
        self.deficits.remove(account);
        self.priorities.remove(account);
    }

    /// The deficits that stand at `now`: none when the window has since moved on.
    pub fn deficits_at(&self, now: SystemTime, length: Duration) -> BTreeMap<String, f64> {
        // A changed length keeps what is owed until the new alignment's next boundary (see `roll`).
        let changed = self.length.is_some_and(|l| l != length);
        match self.window {
            Some(w) if changed || window_start(now, length) <= w => self.deficits.clone(),
            _ => BTreeMap::new(),
        }
    }

    pub fn deficit(&self, account: &str) -> f64 {
        self.deficits.get(account).copied().unwrap_or(0.0)
    }

    pub fn window(&self) -> Option<SystemTime> {
        self.window
    }

    /// The deficits, for the journal and the routing view.
    pub fn deficits(&self) -> &BTreeMap<String, f64> {
        &self.deficits
    }

    /// Puts back what the journal held. Deficits of another window are not restored.
    pub fn restore(&mut self, window: SystemTime, deficits: BTreeMap<String, f64>) {
        self.window = Some(window);
        self.deficits = deficits;
    }

    fn apply(&mut self, shares: &[(String, f64)], placed: &str, tokens: f64) {
        for (a, share) in shares {
            // Share-0 accounts don't accrue.
            if *share > 0.0 {
                self.add(a, share * tokens);
            }
        }
        self.add(placed, -tokens);
    }

    fn add(&mut self, account: &str, by: f64) {
        let d = self.deficits.entry(account.to_string()).or_insert(0.0);
        *d = (*d + by).clamp(-CLAMP, CLAMP);
    }

    /// A cold request of `tokens` was placed on `placed`: every eligible account is owed its
    /// share of the work, and `placed` has done it. `None` when `placed` isn't among the shares
    /// (a last resort: nothing is owed or paid).
    pub fn debit(
        &mut self,
        now: SystemTime,
        length: Duration,
        shares: &[(String, f64)],
        placed: &str,
        tokens: u64,
    ) -> Option<Debit> {
        self.roll(now, length);
        if !shares.iter().any(|(a, s)| a == placed && *s > 0.0) {
            return None;
        }
        self.apply(shares, placed, tokens as f64);
        Some(Debit {
            window_start: self.window?,
            shares: shares.to_vec(),
            placed: placed.to_string(),
            tokens,
        })
    }

    /// The attempt ended with `actual` tokens: the same shares are applied to the difference from
    /// the estimate. Dropped when the window has rolled since the debit.
    pub fn settle(&mut self, debit: &mut Debit, now: SystemTime, length: Duration, actual: u64) {
        self.roll(now, length);
        if self.window != Some(debit.window_start) {
            return;
        }
        let delta = actual as f64 - debit.tokens as f64;
        self.apply(&debit.shares, &debit.placed, delta);
        debit.tokens = actual;
    }

    /// The attempt failed: its debit is taken back. Dropped when the window has rolled.
    pub fn reverse(&mut self, debit: &Debit, now: SystemTime, length: Duration) {
        self.roll(now, length);
        if self.window != Some(debit.window_start) {
            return;
        }
        self.apply(&debit.shares, &debit.placed, -(debit.tokens as f64));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H5: Duration = Duration::from_secs(5 * 3600);

    fn at(s: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(s)
    }

    fn shares(v: &[(&str, f64)]) -> Vec<(String, f64)> {
        v.iter().map(|(a, s)| (a.to_string(), *s)).collect()
    }

    fn sum(l: &Ledger) -> f64 {
        l.deficits().values().sum()
    }

    #[test]
    fn deficits_sum_to_zero_and_the_placed_account_pays() {
        let mut l = Ledger::default();
        let s = shares(&[("a", 0.75), ("b", 0.25)]);
        l.debit(at(10), H5, &s, "a", 1000).unwrap();
        // a: +750 -1000, b: +250.
        assert_eq!((l.deficit("a"), l.deficit("b")), (-250.0, 250.0));
        l.debit(at(20), H5, &s, "b", 400).unwrap();
        assert!(sum(&l).abs() < 1e-9);
    }

    #[test]
    fn the_clamp_holds_and_share_zero_accounts_do_not_accrue() {
        let mut l = Ledger::default();
        let s = shares(&[("a", 1.0), ("idle", 0.0)]);
        l.debit(at(1), H5, &s, "a", 9_000_000).unwrap();
        assert_eq!(l.deficit("a"), -CLAMP);
        assert_eq!(l.deficit("idle"), 0.0);
        let s = shares(&[("a", 0.0), ("b", 1.0)]);
        for _ in 0..3 {
            l.debit(at(2), H5, &s, "b", 9_000_000).unwrap();
        }
        assert_eq!(l.deficit("b"), -CLAMP);
    }

    #[test]
    fn a_placement_outside_the_shares_changes_nothing() {
        let mut l = Ledger::default();
        assert!(l.debit(at(1), H5, &shares(&[("a", 1.0)]), "z", 100).is_none());
        assert!(l.deficits().is_empty());
    }

    #[test]
    fn settling_and_reversing_match_a_single_exact_debit() {
        let s = shares(&[("a", 0.6), ("b", 0.4)]);
        let mut exact = Ledger::default();
        exact.debit(at(5), H5, &s, "a", 1500).unwrap();

        let mut l = Ledger::default();
        let mut d = l.debit(at(5), H5, &s, "a", 1000).unwrap();
        l.settle(&mut d, at(9), H5, 1500);
        for a in ["a", "b"] {
            assert!((l.deficit(a) - exact.deficit(a)).abs() < 1e-9);
        }
        // A failure takes the whole debit back; the next candidate is debited instead.
        l.reverse(&d, at(10), H5);
        assert!(l.deficits().values().all(|d| d.abs() < 1e-9));
        l.debit(at(10), H5, &s, "b", 1500).unwrap();
        assert!(sum(&l).abs() < 1e-9);
    }

    #[test]
    fn a_settlement_after_the_window_rolled_is_dropped() {
        let s = shares(&[("a", 0.5), ("b", 0.5)]);
        let mut l = Ledger::default();
        let mut d = l.debit(at(100), H5, &s, "a", 1000).unwrap();
        l.settle(&mut d, at(18_000 + 5), H5, 5000);
        assert!(l.deficits().is_empty(), "the new window starts at 0 and the late settlement left no trace");
        l.reverse(&d, at(18_000 + 6), H5);
        assert!(l.deficits().is_empty());
        assert_eq!(l.window(), Some(at(18_000)));
    }

    #[test]
    fn a_boundary_resets_once_and_the_clock_never_goes_back() {
        let s = shares(&[("a", 0.5), ("b", 0.5)]);
        let mut l = Ledger::default();
        l.debit(at(100), H5, &s, "a", 1000).unwrap();
        // A forward jump over several windows resets once.
        l.roll(at(100_000), H5);
        assert!(l.deficits().is_empty());
        l.debit(at(100_001), H5, &s, "b", 10).unwrap();
        let before = l.clone();
        // Back in time: still the later window, nothing reset.
        l.roll(at(100), H5);
        assert_eq!(l, before);
    }

    #[test]
    fn an_account_added_or_reprioritised_mid_window_starts_at_zero() {
        let s = shares(&[("a", 0.5), ("b", 0.5)]);
        let mut l = Ledger::default();
        l.observe("a", 1.0);
        l.observe("b", 1.0);
        l.debit(at(5), H5, &s, "a", 1000).unwrap();
        assert_eq!(l.deficit("b"), 500.0);
        l.observe("b", 1.0);
        assert_eq!(l.deficit("b"), 500.0);
        l.observe("b", 2.0);
        assert_eq!(l.deficit("b"), 0.0);
        // A newly added account has no deficit at all.
        assert_eq!(l.deficit("new"), 0.0);
    }

    #[test]
    fn windows_align_to_the_epoch() {
        let h5 = Duration::from_secs(5 * 3600);
        let at = |s| UNIX_EPOCH + Duration::from_secs(s);
        assert_eq!(window_start(at(0), h5), at(0));
        assert_eq!(window_start(at(18_000 - 1), h5), at(0));
        assert_eq!(window_start(at(18_000), h5), at(18_000));
        assert_eq!(window_start(at(40_000), h5), at(36_000));
    }

    #[test]
    fn a_changed_length_keeps_the_deficits_until_the_next_boundary() {
        let mut l = Ledger::default();
        l.debit(at(10), H5, &shares(&[("a", 0.5), ("b", 0.5)]), "a", 1000).unwrap();
        // The target's window becomes 1 h at 02:00: what is owed stays...
        let h1 = Duration::from_secs(3600);
        l.roll(at(2 * 3600), h1);
        assert_eq!(l.deficit("b"), 500.0);
        // ...and clears when the next 1 h boundary passes.
        l.roll(at(3 * 3600 + 1), h1);
        assert!(l.deficits().is_empty());
    }

    #[test]
    fn a_forgotten_account_starts_again_at_zero() {
        let mut l = Ledger::default();
        l.observe("a", 1.0);
        l.debit(at(1), H5, &shares(&[("a", 0.5), ("b", 0.5)]), "a", 1000).unwrap();
        l.forget("b");
        assert_eq!(l.deficit("b"), 0.0);
        assert_eq!(l.deficits_at(at(2), H5).len(), 1);
        assert!(l.deficits_at(at(6 * 3600), H5).is_empty());
    }
}

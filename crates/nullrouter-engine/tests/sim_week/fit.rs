//! The quota fit inside the simulated week (spec 012, research R14).
//!
//! Every poll the simulation makes is also a history entry: the percent the provider showed and
//! the traffic since the last entry. The learner reads those entries as the engine would, and the
//! meters it makes significant replace the declared ones in the candidates the router places with
//! (as `route.rs` passes `MeterInput.declared`). No engine snapshot is needed: the learner is
//! driven through `Learner::observe_declared`, and the meters in effect come from `in_effect`.

use std::time::SystemTime;

use nullrouter_engine::quota::extract::{QuotaWindow, rfc3339_millis};
use nullrouter_engine::quota::fit::learner::Learner;
use nullrouter_engine::quota::fit::{Fits, in_effect};
use nullrouter_engine::quota::history::{Entry, VERSION};
use nullrouter_engine::quota::tally::{AccountTally, ModelTally};
use nullrouter_engine::routing::AccountQuota;
use nullrouter_engine::routing::meter::{Spent, cost_spent};
use nullrouter_registry::schema::MeterDecl;

use super::router::Defs;

/// The learner and everything a run reads back from it.
pub struct FitRun {
    home: tempfile::TempDir,
    pub learner: Learner,
    pub fits: Fits,
    /// Per account, its history entries so far, oldest first.
    entries: Vec<Vec<Entry>>,
    /// Per account, its meters in effect; `None` while every number is declared.
    effective: Vec<Option<Vec<MeterDecl>>>,
    /// How many placements had been made when the fit first gained a significant number.
    pub first_change: Option<usize>,
    /// Remaining-quota checks made, and the ones that failed (FR-014).
    pub audited: u64,
    pub audit_failures: Vec<String>,
}

impl FitRun {
    pub fn new(accounts: usize) -> Self {
        Self {
            home: tempfile::tempdir().expect("a fit home"),
            learner: Learner::default(),
            fits: Fits::default(),
            entries: vec![Vec::new(); accounts],
            effective: vec![None; accounts],
            first_change: None,
            audited: 0,
            audit_failures: Vec::new(),
        }
    }

    /// The meters routing reads for account `i`: those in effect, or `None` for the declared ones.
    pub fn meters(&self, i: usize) -> Option<&[MeterDecl]> {
        self.effective[i].as_deref()
    }

    /// Account `i` was polled at `now`: `windows` is what the provider showed, `since_poll` the
    /// traffic since the previous poll. `placed` is how many requests were placed so far.
    pub fn observe_poll(
        &mut self,
        defs: &Defs,
        i: usize,
        windows: &[QuotaWindow],
        since_poll: &Spent,
        now: SystemTime,
        placed: usize,
    ) {
        let d = &defs.accounts[i];
        let mut tally = AccountTally::new();
        if since_poll.requests > 0 {
            tally.insert(
                "m".to_owned(),
                ModelTally {
                    requests: since_poll.requests,
                    requests_usage_unreported: 0,
                    input: since_poll.input,
                    output: since_poll.output,
                    cache_read: since_poll.cache_read,
                    cache_write: since_poll.cache_write,
                },
            );
        }
        self.entries[i].push(Entry {
            v: VERSION,
            at: rfc3339_millis(now),
            ok: true,
            error: None,
            windows: windows.to_vec(),
            tally,
        });
        let (provider, account) = (d.key.provider.as_str(), d.key.account.as_str());
        let since = self.learner.since(self.home.path(), provider, account);
        let list = &self.entries[i];
        let from = since.map_or(0, |s| list.partition_point(|e| e.time().is_some_and(|t| t < s)));
        let polled: Vec<String> = defs
            .accounts
            .iter()
            .filter(|a| a.reported && a.key.provider == d.key.provider)
            .map(|a| a.key.account.clone())
            .collect();
        let changed = self.learner.observe_declared(
            &mut self.fits,
            self.home.path(),
            provider,
            &d.meters,
            &polled,
            &[(account, &list[from..])],
            now,
        );
        if changed {
            if !self.fits.windows.is_empty() {
                self.first_change.get_or_insert(placed);
            }
            self.rebuild(defs);
        }
    }

    /// Recomputes every polled account's meters in effect from the fits.
    fn rebuild(&mut self, defs: &Defs) {
        for (i, d) in defs.accounts.iter().enumerate().filter(|(_, d)| d.reported) {
            let mut replaced = false;
            let windows: Vec<MeterDecl> = d
                .meters
                .iter()
                .map(|m| {
                    let fit = self.fits.window(&d.key.provider, &m.name);
                    let (meter, sources) = in_effect(m, None, None, &fit, &d.key.account);
                    replaced |= !sources.is_empty();
                    meter
                })
                .collect();
            self.effective[i] = replaced.then_some(windows);
        }
    }

    /// FR-014: once an account's capacity is fitted, each window's `remaining_now` between polls
    /// is the last poll's fraction times the fitted capacity, less what 0router sent since,
    /// costed with the meter in effect. Nothing for outside use is subtracted.
    pub fn audit(
        &mut self,
        defs: &Defs,
        i: usize,
        quota: &AccountQuota,
        polled: Option<&[QuotaWindow]>,
        since_poll: &Spent,
    ) {
        let Some(reported) = polled else { return };
        let d = &defs.accounts[i];
        let Some(meters) = self.effective[i].as_deref() else { return };
        for w in &quota.windows {
            let fitted = self.fits.window(&d.key.provider, &w.name).capacity.get(&d.key.account).copied();
            let (Some(cap), Some(r), Some(m)) =
                (fitted, reported.iter().find(|r| r.name == w.name), meters.iter().find(|m| m.name == w.name))
            else {
                continue;
            };
            if w.rolled_over {
                continue;
            }
            let fraction = ((100.0 - r.used.unwrap_or(0.0)) / 100.0).clamp(0.0, 1.0);
            let want = (fraction * cap - cost_spent(m, since_poll)).max(0.0);
            self.audited += 1;
            let tol = 1e-6 * cap;
            if (w.remaining_now - want).abs() > tol || (w.capacity - cap).abs() > tol {
                self.audit_failures.push(format!(
                    "{}/{} {}: remaining_now {} capacity {}, expected {want} of {cap}",
                    d.key.provider, d.key.account, w.name, w.remaining_now, w.capacity
                ));
            }
        }
    }
}

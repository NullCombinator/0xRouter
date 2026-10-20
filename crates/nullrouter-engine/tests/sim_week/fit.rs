//! The quota fit inside the simulated week (spec 012, research R14).
//!
//! Every poll the simulation makes is also a history entry: the percent the provider showed and
//! the traffic since the last entry. The learner reads those entries as the engine would, and the
//! meters it makes significant replace the declared ones in the candidates the router places with
//! (as `route.rs` passes `MeterInput.declared`). No engine snapshot is needed: the learner is
//! driven through `Learner::observe_declared`, and the meters in effect come from `in_effect`.

use std::collections::BTreeMap;
use std::time::SystemTime;

use nullrouter_engine::quota::extract::{QuotaWindow, rfc3339_millis};
use nullrouter_engine::quota::fit::learner::Learner;
use nullrouter_engine::quota::fit::{AccountMeters, Fits, NumberOverrides, WindowOverrides, in_effect};
use nullrouter_engine::quota::history::{Entry, VERSION};
use nullrouter_engine::quota::tally::{AccountTally, ModelTally};
use nullrouter_engine::routing::AccountQuota;
use nullrouter_engine::routing::view::{WindowMeterView, window_meter_view};
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
    /// The same with the sources of each replaced number, as the view reads them.
    cells: Vec<Option<AccountMeters>>,
    /// Operator overrides (spec 012, US4): the plugin's, by window, and each account's, by
    /// `(account index, window)`. The learner is not told: it tests against the declaration.
    plugin: BTreeMap<String, NumberOverrides>,
    account: BTreeMap<(usize, String), NumberOverrides>,
    /// Accounts declared exclusive-use, and since when (account index).
    exclusive: BTreeMap<usize, SystemTime>,
    /// The next poll's entry is appended, then the process "dies" before learning from it and
    /// restarts by replay.
    crash_next: bool,
    pub crashes: u32,
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
            cells: vec![None; accounts],
            plugin: BTreeMap::new(),
            account: BTreeMap::new(),
            exclusive: BTreeMap::new(),
            crash_next: false,
            crashes: 0,
            first_change: None,
            audited: 0,
            audit_failures: Vec::new(),
        }
    }

    /// The home directory the run keeps its state in.
    pub fn home(&self) -> &std::path::Path {
        self.home.path()
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
        if self.crash_next {
            self.crash_next = false;
            self.crashes += 1;
            self.restart(defs, now);
            return;
        }
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

    /// Recomputes every polled account's meters in effect from the overrides and the fits.
    fn rebuild(&mut self, defs: &Defs) {
        for (i, d) in defs.accounts.iter().enumerate().filter(|(_, d)| d.reported) {
            let mut replaced = false;
            let mut sources = BTreeMap::new();
            let windows: Vec<MeterDecl> = d
                .meters
                .iter()
                .map(|m| {
                    let fit = self.fits.window(&d.key.provider, &m.name);
                    let (meter, src) =
                        in_effect(m, self.plugin.get(&m.name), self.account.get(&(i, m.name.clone())), &fit, &d.key.account);
                    replaced |= !src.is_empty();
                    if !src.is_empty() {
                        sources.insert(m.name.clone(), src);
                    }
                    meter
                })
                .collect();
            self.cells[i] = replaced.then(|| AccountMeters { windows: windows.clone().into(), sources });
            self.effective[i] = replaced.then_some(windows);
        }
    }

    /// Sets (or with `None` removes) the plugin-level override of `window`.
    pub fn set_plugin_override(&mut self, defs: &Defs, window: &str, o: Option<NumberOverrides>) {
        match o {
            Some(o) => self.plugin.insert(window.to_owned(), o),
            None => self.plugin.remove(window),
        };
        self.rebuild(defs);
    }

    /// Sets (or with `None` removes) account `i`'s override of `window`.
    pub fn set_account_override(&mut self, defs: &Defs, i: usize, window: &str, o: Option<NumberOverrides>) {
        let key = (i, window.to_owned());
        match o {
            Some(o) => self.account.insert(key, o),
            None => self.account.remove(&key),
        };
        self.rebuild(defs);
    }

    /// What the routing view shows for account `i`'s `window`.
    pub fn view(&self, defs: &Defs, i: usize, window: &str) -> WindowMeterView {
        let d = &defs.accounts[i];
        let declared = d.meters.iter().find(|m| m.name == window).expect("a declared window");
        let snaps = self.learner.snapshot_all();
        let snap = snaps.get(&(d.key.provider.clone(), window.to_owned()));
        let ov = WindowOverrides {
            plugin: self.plugin.get(window).cloned().unwrap_or_default(),
            accounts: self
                .account
                .iter()
                .filter(|((_, w), _)| w == window)
                .map(|((a, _), o)| (defs.accounts[*a].key.account.clone(), o.clone()))
                .collect(),
        };
        window_meter_view(declared, snap, Some(&ov), &d.key.account, self.cells[i].as_ref())
    }

    /// The capacity of account `i`'s first window routing reads: the fit's, or the declared one.
    pub fn capacity_in_effect(&self, defs: &Defs, i: usize) -> Option<f64> {
        self.meters(i).unwrap_or(&defs.accounts[i].meters).first().and_then(|m| m.capacity)
    }

    /// Declares account `i` exclusive-use since `since`, or not (`None`).
    pub fn set_exclusive(&mut self, defs: &Defs, i: usize, since: Option<SystemTime>) {
        let d = &defs.accounts[i];
        self.learner.set_exclusive(&d.key.provider, &d.key.account, since);
        match since {
            Some(t) => self.exclusive.insert(i, t),
            None => self.exclusive.remove(&i),
        };
    }

    /// The exclusive-use declarations the run holds.
    pub fn exclusive(&self) -> &BTreeMap<usize, SystemTime> {
        &self.exclusive
    }

    /// At the next poll, the entry reaches the history but the process dies before learning from
    /// it; it restarts by replay at once.
    pub fn arm_crash(&mut self) {
        self.crash_next = true;
    }

    /// A start: a new learner, the history replayed from where the stored fit says (the state
    /// files in the home stay), every plugin's accounts together as at `Shared::replay`.
    pub fn restart(&mut self, defs: &Defs, now: SystemTime) {
        self.learner = Learner::default();
        self.fits = Fits::default();
        for (i, since) in &self.exclusive {
            let d = &defs.accounts[*i];
            self.learner.set_exclusive(&d.key.provider, &d.key.account, Some(*since));
        }
        let mut providers: Vec<&str> = defs.accounts.iter().filter(|a| a.reported).map(|a| a.key.provider.as_str()).collect();
        providers.sort_unstable();
        providers.dedup();
        for provider in providers {
            let members: Vec<usize> =
                defs.accounts.iter().enumerate().filter(|(_, a)| a.reported && a.key.provider == provider).map(|(i, _)| i).collect();
            let polled: Vec<String> = members.iter().map(|i| defs.accounts[*i].key.account.clone()).collect();
            let froms: Vec<usize> = members
                .iter()
                .map(|&i| {
                    let since = self.learner.since(self.home.path(), provider, &defs.accounts[i].key.account);
                    since.map_or(0, |s| self.entries[i].partition_point(|e| e.time().is_some_and(|t| t < s)))
                })
                .collect();
            let tails: Vec<(&str, &[Entry])> = members
                .iter()
                .zip(&froms)
                .map(|(&i, &f)| (defs.accounts[i].key.account.as_str(), &self.entries[i][f..]))
                .collect();
            self.learner.observe_declared(&mut self.fits, self.home.path(), provider, &defs.accounts[members[0]].meters, &polled, &tails, now);
        }
        self.rebuild(defs);
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

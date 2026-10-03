//! The running per-account, per-model tally of tokens 0router sent (FR-023, FR-024).
//!
//! Every attempt sent through an account adds to that account's tally under the attempt's
//! upstream model, with the usage the provider reported for that attempt alone: a retry or
//! fallback counts on the account it ran on (research R15). An attempt whose provider
//! reported no usage counts in `requests_usage_unreported` and adds no tokens. The tally is
//! taken and reset when a poll entry is written ([`super::history`]), so every entry carries
//! the traffic since the previous one.
//!
//! The names are the quota meter's (FR-026): `input` is input *without* cache reads and
//! writes, whatever the provider's input semantics, so the four token categories never
//! overlap.
//!
//! Each account has its own short mutex; the map of accounts is read-locked on the hot path
//! and write-locked only when an account first tallies.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use nullrouter_registry::schema::InputSemantics;
use serde::{Deserialize, Serialize};

use crate::records::Usage;

/// One upstream model's traffic through one account (data-model § Traffic tally).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelTally {
    /// Attempts sent through the account.
    #[serde(default)]
    pub requests: u64,
    /// Attempts whose provider reported no usage.
    #[serde(default)]
    pub requests_usage_unreported: u64,
    /// Input tokens, cache reads and writes excluded.
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(default)]
    pub cache_read: u64,
    #[serde(default)]
    pub cache_write: u64,
}

impl ModelTally {
    /// Adds one attempt with `usage` (`None`: not reported).
    pub fn add(&mut self, usage: Option<&Usage>) {
        self.requests += 1;
        match usage.filter(|u| reported(u)) {
            None => self.requests_usage_unreported += 1,
            Some(u) => {
                let (cr, cw) = (u.cache_read.unwrap_or(0), u.cache_write.unwrap_or(0));
                let input = u.input.unwrap_or(0);
                self.input += match u.input_semantics {
                    InputSemantics::IncludesCache => input.saturating_sub(cr + cw),
                    InputSemantics::ExcludesCache => input,
                };
                self.output += u.output.unwrap_or(0);
                self.cache_read += cr;
                self.cache_write += cw;
            }
        }
    }

    /// Adds `other`'s counts (a reloaded checkpoint, a tally put back after a failed write).
    pub fn merge(&mut self, other: &ModelTally) {
        self.requests += other.requests;
        self.requests_usage_unreported += other.requests_usage_unreported;
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }
}

/// Whether the provider reported any token count.
fn reported(u: &Usage) -> bool {
    u.input.is_some() || u.output.is_some() || u.cache_read.is_some() || u.cache_write.is_some()
}

/// One account's tally, by upstream model id.
pub type AccountTally = BTreeMap<String, ModelTally>;

/// Adds `from` into `into`.
pub fn merge(into: &mut AccountTally, from: &AccountTally) {
    for (model, t) in from {
        into.entry(model.clone()).or_default().merge(t);
    }
}

#[derive(Debug, Default)]
struct Cell {
    models: AccountTally,
    /// Changed since the last checkpoint.
    dirty: bool,
}

type Key = (String, String);

/// Every account's running tally, in memory (checkpointed by [`super::history`]).
#[derive(Debug, Default)]
pub struct Tally {
    cells: RwLock<HashMap<Key, Arc<Mutex<Cell>>>>,
}

fn lock(m: &Mutex<Cell>) -> MutexGuard<'_, Cell> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Tally {
    fn cell(&self, provider: &str, account: &str) -> Arc<Mutex<Cell>> {
        {
            let cells = self.cells.read().unwrap_or_else(|e| e.into_inner());
            // A borrowed-tuple lookup would need a custom key type; accounts are few and the
            // read path only allocates two short strings.
            if let Some(c) = cells.get(&(provider.to_owned(), account.to_owned())) {
                return c.clone();
            }
        }
        let mut cells = self.cells.write().unwrap_or_else(|e| e.into_inner());
        cells.entry((provider.to_owned(), account.to_owned())).or_default().clone()
    }

    fn existing(&self, provider: &str, account: &str) -> Option<Arc<Mutex<Cell>>> {
        let cells = self.cells.read().unwrap_or_else(|e| e.into_inner());
        cells.get(&(provider.to_owned(), account.to_owned())).cloned()
    }

    /// Tallies one attempt sent through `provider/account` to upstream `model`, with the
    /// usage its provider reported (`None`: not reported).
    pub fn attempt(&self, provider: &str, account: &str, model: &str, usage: Option<&Usage>) {
        let cell = self.cell(provider, account);
        let mut c = lock(&cell);
        match c.models.get_mut(model) {
            Some(t) => t.add(usage),
            None => {
                let mut t = ModelTally::default();
                t.add(usage);
                c.models.insert(model.to_owned(), t);
            }
        }
        c.dirty = true;
    }

    /// Tallies one attempt that consumes no tokens (a count-tokens call): a request, with
    /// usage known to be zero.
    pub fn attempt_without_tokens(&self, provider: &str, account: &str, model: &str) {
        let zero = Usage {
            input: Some(0),
            output: None,
            cache_read: None,
            cache_write: None,
            reasoning: None,
            input_semantics: InputSemantics::ExcludesCache,
            estimated: false,
        };
        self.attempt(provider, account, model, Some(&zero));
    }

    /// The account's tally so far, without resetting it.
    pub fn get(&self, provider: &str, account: &str) -> AccountTally {
        self.existing(provider, account).map(|c| lock(&c).models.clone()).unwrap_or_default()
    }

    /// Takes the account's tally and resets it (a poll entry is being written).
    pub fn take(&self, provider: &str, account: &str) -> AccountTally {
        self.existing(provider, account).map(|c| std::mem::take(&mut lock(&c).models)).unwrap_or_default()
    }

    /// Adds `tally` back to the account (a reloaded checkpoint, a failed write).
    pub fn restore(&self, provider: &str, account: &str, tally: &AccountTally) {
        if tally.is_empty() {
            return;
        }
        let cell = self.cell(provider, account);
        let mut c = lock(&cell);
        merge(&mut c.models, tally);
        c.dirty = true;
    }

    /// The accounts changed since their last checkpoint, with their flag cleared. The caller
    /// checkpoints each; one that fails is marked again with [`mark_dirty`](Self::mark_dirty).
    pub fn take_dirty(&self) -> Vec<Key> {
        let cells = self.cells.read().unwrap_or_else(|e| e.into_inner());
        cells.iter().filter(|(_, c)| std::mem::replace(&mut lock(c).dirty, false)).map(|(k, _)| k.clone()).collect()
    }

    pub fn mark_dirty(&self, provider: &str, account: &str) {
        lock(&self.cell(provider, account)).dirty = true;
    }

    /// Drops the account's running tally (its history was forgotten).
    pub fn forget(&self, provider: &str, account: &str) {
        let mut cells = self.cells.write().unwrap_or_else(|e| e.into_inner());
        cells.remove(&(provider.to_owned(), account.to_owned()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: Option<u64>, output: Option<u64>, cr: Option<u64>, semantics: InputSemantics) -> Usage {
        Usage {
            input,
            output,
            cache_read: cr,
            cache_write: None,
            reasoning: None,
            input_semantics: semantics,
            estimated: false,
        }
    }

    #[test]
    fn input_excludes_cache_whatever_the_semantics() {
        let t = Tally::default();
        t.attempt("p", "a", "m", Some(&usage(Some(150), Some(20), Some(100), InputSemantics::IncludesCache)));
        t.attempt("p", "a", "m", Some(&usage(Some(50), Some(5), Some(100), InputSemantics::ExcludesCache)));
        let m = t.get("p", "a")["m"];
        assert_eq!((m.requests, m.input, m.output, m.cache_read), (2, 100, 25, 200));
    }

    #[test]
    fn unreported_adds_no_tokens_and_take_resets() {
        let t = Tally::default();
        t.attempt("p", "a", "m", None);
        t.attempt("p", "a", "m", Some(&usage(None, None, None, InputSemantics::ExcludesCache)));
        t.attempt_without_tokens("p", "a", "count");
        let got = t.take("p", "a");
        assert_eq!(got["m"], ModelTally { requests: 2, requests_usage_unreported: 2, ..ModelTally::default() });
        assert_eq!(got["count"], ModelTally { requests: 1, ..ModelTally::default() });
        assert!(t.get("p", "a").is_empty());
        assert_eq!(t.take_dirty(), [("p".to_owned(), "a".to_owned())]);
        assert!(t.take_dirty().is_empty());
    }
}

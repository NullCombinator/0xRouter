//! Per (provider, account, model) cooldowns with backoff levels (research R6, data-model
//! § Cooldown). The level is per account, as in 9router's `backoffLevel`; the rest period
//! is per model, as in its `modelLock_<model>`. Times are `tokio::time::Instant`, so tests
//! run on paused time. Held in memory only.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use tokio::time::Instant;

use crate::classify::Verdict;

type Model = (String, String, String);
type Account = (String, String);

#[derive(Default)]
struct Inner {
    until: HashMap<Model, Instant>,
    level: HashMap<Account, u8>,
}

#[derive(Default)]
pub struct Cooldowns {
    inner: Mutex<Inner>,
}

fn key(provider: &str, account: &str, model: &str) -> Model {
    (provider.to_owned(), account.to_owned(), model.to_owned())
}

impl Cooldowns {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// When the account's rest for `model` ends, if it is resting now.
    pub fn cooling(&self, provider: &str, account: &str, model: &str) -> Option<Instant> {
        let now = Instant::now();
        self.lock().until.get(&key(provider, account, model)).copied().filter(|u| *u > now)
    }

    /// Records a failure; returns the rest period it set (zero when the verdict sets none).
    pub fn fail(&self, provider: &str, account: &str, model: &str, verdict: &Verdict) -> Duration {
        let mut g = self.lock();
        let acct = (provider.to_owned(), account.to_owned());
        let level = g.level.get(&acct).copied().unwrap_or(0);
        let (ms, next) = verdict.cooldown_ms(level);
        if ms == 0 {
            return Duration::ZERO;
        }
        g.level.insert(acct, next);
        let d = Duration::from_millis(ms);
        g.until.insert(key(provider, account, model), Instant::now() + d);
        d
    }

    /// A success clears the model's rest and, when the account has no other active rest,
    /// its backoff level (9router `clearAccountError`).
    pub fn succeed(&self, provider: &str, account: &str, model: &str) {
        let now = Instant::now();
        let mut g = self.lock();
        g.until.remove(&key(provider, account, model));
        g.until.retain(|_, u| *u > now);
        if !g.until.keys().any(|(p, a, _)| p == provider && a == account) {
            g.level.remove(&(provider.to_owned(), account.to_owned()));
        }
    }

    pub fn level(&self, provider: &str, account: &str) -> u8 {
        self.lock().level.get(&(provider.to_owned(), account.to_owned())).copied().unwrap_or(0)
    }

    /// The earliest end among the active rests of `keys` (provider, account, model).
    pub fn earliest_end<'a>(&self, keys: impl IntoIterator<Item = (&'a str, &'a str, &'a str)>) -> Option<Instant> {
        let now = Instant::now();
        let g = self.lock();
        keys.into_iter().filter_map(|(p, a, m)| g.until.get(&key(p, a, m)).copied()).filter(|u| *u > now).min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classify;

    #[tokio::test(start_paused = true)]
    async fn rests_end_and_success_resets_the_level() {
        let c = Cooldowns::default();
        let rl = classify::upstream(429, "slow down");
        assert_eq!(c.fail("p", "main", "m1", &rl), Duration::from_secs(2));
        assert_eq!(c.fail("p", "main", "m1", &rl), Duration::from_secs(4));
        assert_eq!(c.level("p", "main"), 2);
        assert!(c.cooling("p", "main", "m1").is_some());
        assert!(c.cooling("p", "main", "m2").is_none(), "per model");
        assert_eq!(c.fail("p", "main", "m2", &classify::upstream(500, "boom")), Duration::from_secs(30));
        tokio::time::advance(Duration::from_secs(5)).await;
        assert!(c.cooling("p", "main", "m1").is_none());
        c.succeed("p", "main", "m1");
        assert_eq!(c.level("p", "main"), 2, "m2 still rests");
        tokio::time::advance(Duration::from_secs(30)).await;
        c.succeed("p", "main", "m1");
        assert_eq!(c.level("p", "main"), 0);
        assert_eq!(c.fail("p", "main", "m1", &classify::upstream(400, "bad")), Duration::ZERO, "no fallback, no rest");
    }

    #[tokio::test(start_paused = true)]
    async fn earliest_end_is_the_soonest_active_rest() {
        let c = Cooldowns::default();
        c.fail("p", "a", "m", &classify::upstream(401, ""));
        c.fail("p", "b", "m", &classify::upstream(500, ""));
        let now = Instant::now();
        assert_eq!(c.earliest_end([("p", "a", "m"), ("p", "b", "m")]), Some(now + Duration::from_secs(30)));
        assert_eq!(c.earliest_end([("p", "c", "m")]), None);
    }
}

//! One in-flight refresh per account, shared by every waiter (FR-012).
//!
//! Ports `ref/9router/open-sse/services/tokenRefresh/dedup.js`, keyed per account
//! (`(provider, name)`) instead of per old refresh token, and without its 10 s result
//! cache: a waiter that arrives after a refresh finished sees the fresh token in the cell
//! and doesn't ask again.
//!
//! The work runs as its own task, so a waiter whose client goes away never cancels the
//! refresh the others wait for. The entry is cleared when the work completes.

use std::collections::HashMap;
use std::future::Future;
use std::hash::Hash;
use std::sync::{Arc, Mutex};

use futures_util::FutureExt;
use futures_util::future::{BoxFuture, Shared};

/// A shared, cloneable handle on one in-flight piece of work.
pub type InFlight<V> = Shared<BoxFuture<'static, V>>;

/// In-flight work by key.
pub struct Dedup<K, V: Clone> {
    inflight: Arc<Mutex<HashMap<K, InFlight<V>>>>,
}

impl<K, V: Clone> Default for Dedup<K, V> {
    fn default() -> Self {
        Self { inflight: Arc::default() }
    }
}

impl<K, V: Clone> std::fmt::Debug for Dedup<K, V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let n = self.inflight.lock().map_or(0, |m| m.len());
        f.debug_struct("Dedup").field("in_flight", &n).finish()
    }
}

impl<K, V> Dedup<K, V>
where
    K: Eq + Hash + Clone + Send + 'static,
    V: Clone + Send + Sync + 'static,
{
    /// The in-flight work for `key`, or `work` started now (on the runtime) when there is
    /// none. `aborted` is every waiter's result if the work's task panics.
    pub fn run<F>(&self, key: K, work: F, aborted: V) -> InFlight<V>
    where
        F: Future<Output = V> + Send + 'static,
    {
        let mut map = self.inflight.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(f) = map.get(&key) {
            return f.clone();
        }
        let (inflight, k) = (self.inflight.clone(), key.clone());
        let task = tokio::spawn(async move {
            let v = work.await;
            inflight.lock().unwrap_or_else(|e| e.into_inner()).remove(&k);
            v
        });
        let shared = async move { task.await.unwrap_or(aborted) }.boxed().shared();
        map.insert(key, shared.clone());
        shared
    }

    /// Whether work for `key` is in flight.
    pub fn busy(&self, key: &K) -> bool {
        self.inflight.lock().unwrap_or_else(|e| e.into_inner()).contains_key(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[tokio::test]
    async fn concurrent_callers_share_one_run_and_the_entry_clears() {
        let d: Dedup<&str, u32> = Dedup::default();
        let runs = Arc::new(AtomicUsize::new(0));
        let waiters: Vec<_> = (0..50)
            .map(|_| {
                let runs = runs.clone();
                d.run(
                    "a",
                    async move {
                        runs.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        7
                    },
                    0,
                )
            })
            .collect();
        assert!(d.busy(&"a"));
        for w in waiters {
            assert_eq!(w.await, 7);
        }
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert!(!d.busy(&"a"), "cleared on completion");
        assert_eq!(d.run("a", async { 8 }, 0).await, 8, "a later call runs again");
    }

    async fn boom() -> u32 {
        panic!("boom")
    }

    #[tokio::test]
    async fn a_dropped_waiter_does_not_cancel_the_work() {
        let d: Dedup<&str, u32> = Dedup::default();
        let first = d.run(
            "a",
            async {
                tokio::time::sleep(Duration::from_millis(30)).await;
                1
            },
            0,
        );
        let second = d.run("a", async { 2 }, 0);
        drop(first);
        assert_eq!(second.await, 1);
        let panicked = d.run("b", boom(), 9);
        assert_eq!(panicked.await, 9);
    }
}

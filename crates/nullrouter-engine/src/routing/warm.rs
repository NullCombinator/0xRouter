//! The warm store: which account holds which of an agent's prompt prefixes cached (research R3).
//!
//! Entries are keyed `(agent, hash, provider, account, model)` and hold hashes only. An entry is
//! warm while `now - last_used <= lifetime`; past that it is deleted (Clarifications Q3). The
//! lifetime is the entry's own marker `ttl` when it has one, else the account's.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use super::fingerprint::{Chain, Hash, Written};
use super::{CandidateKey, WarmEntry};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    agent: String,
    hash: Hash,
    at: CandidateKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Value {
    prefix_tokens: u64,
    last_used: SystemTime,
    ttl: Option<Duration>,
}

/// One stored fingerprint, as the journal and recovery see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    pub agent: String,
    pub hash: Hash,
    pub at: CandidateKey,
    pub prefix_tokens: u64,
    pub last_used: SystemTime,
    pub ttl: Option<Duration>,
}

#[derive(Debug, Default)]
pub struct WarmStore {
    entries: HashMap<Key, Value>,
}

impl WarmStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The agent's longest warm prefix among `candidates` (each with its cache lifetime). Two
    /// accounts holding the same boundary: the newest `last_used`. Expired entries are skipped,
    /// not deleted; `sweep` deletes them.
    pub fn lookup(
        &self,
        agent: &str,
        chain: &Chain,
        candidates: &[(CandidateKey, Duration)],
        now: SystemTime,
    ) -> Option<WarmEntry> {
        for boundary in chain.boundaries.iter().rev() {
            let best = candidates
                .iter()
                .filter_map(|(at, lifetime)| {
                    let key = Key { agent: agent.to_owned(), hash: boundary.hash, at: at.clone() };
                    let v = self.entries.get(&key)?;
                    warm(v, *lifetime, now).then(|| WarmEntry {
                        key: at.clone(),
                        hash: boundary.hash,
                        prefix_tokens: v.prefix_tokens,
                        last_used: v.last_used,
                    })
                })
                .max_by_key(|e| e.last_used);
            if best.is_some() {
                return best;
            }
        }
        None
    }

    /// Remember what a successful request left cached on `at`. An entry already there keeps its
    /// longer `prefix_tokens` figure (the provider's reported usage, not the estimate).
    pub fn upsert(&mut self, agent: &str, at: &CandidateKey, written: &[Written], now: SystemTime) {
        for w in written {
            let key = Key { agent: agent.to_owned(), hash: w.hash, at: at.clone() };
            self.entries.insert(key, Value { prefix_tokens: w.prefix_tokens, last_used: now, ttl: w.ttl });
        }
    }

    /// A request used the cache on `at` up to `hit`: the provider refreshed every prefix up to
    /// it, so every entry of the chain up to it on that account is used now.
    pub fn touch(&mut self, agent: &str, at: &CandidateKey, chain: &Chain, hit: Hash, now: SystemTime) {
        for b in &chain.boundaries {
            let key = Key { agent: agent.to_owned(), hash: b.hash, at: at.clone() };
            if let Some(v) = self.entries.get_mut(&key) {
                v.last_used = now;
            }
            if b.hash == hit {
                break;
            }
        }
    }

    /// Delete what is past its lifetime. `lifetime` is the account's, for entries without a
    /// marker `ttl`; an account it doesn't know keeps its entries until `drop_account`.
    pub fn sweep(&mut self, now: SystemTime, lifetime: impl Fn(&CandidateKey) -> Option<Duration>) -> usize {
        let before = self.entries.len();
        self.entries.retain(|k, v| match v.ttl.or_else(|| lifetime(&k.at)) {
            Some(l) => warm(v, l, now),
            None => true,
        });
        before - self.entries.len()
    }

    pub fn drop_account(&mut self, provider: &str, account: &str) -> usize {
        let before = self.entries.len();
        self.entries.retain(|k, _| !(k.at.provider == provider && k.at.account == account));
        before - self.entries.len()
    }

    pub fn drop_agent(&mut self, agent: &str) -> usize {
        let before = self.entries.len();
        self.entries.retain(|k, _| k.agent != agent);
        before - self.entries.len()
    }

    /// Put an entry back as recovered from the journal.
    pub fn restore(&mut self, s: Stored) {
        let key = Key { agent: s.agent, hash: s.hash, at: s.at };
        let v = Value { prefix_tokens: s.prefix_tokens, last_used: s.last_used, ttl: s.ttl };
        match self.entries.get(&key) {
            Some(old) if old.last_used >= v.last_used => {}
            _ => {
                self.entries.insert(key, v);
            }
        }
    }

    pub fn stored(&self) -> Vec<Stored> {
        let mut out: Vec<Stored> = self
            .entries
            .iter()
            .map(|(k, v)| Stored {
                agent: k.agent.clone(),
                hash: k.hash,
                at: k.at.clone(),
                prefix_tokens: v.prefix_tokens,
                last_used: v.last_used,
                ttl: v.ttl,
            })
            .collect();
        out.sort_by(|a, b| (&a.agent, a.hash, &a.at).cmp(&(&b.agent, b.hash, &b.at)));
        out
    }
}

/// An entry's own marker `ttl` beats the account's lifetime.
fn warm(v: &Value, account_lifetime: Duration, now: SystemTime) -> bool {
    let lifetime = v.ttl.unwrap_or(account_lifetime);
    match now.duration_since(v.last_used) {
        Ok(idle) => idle <= lifetime,
        // `last_used` in the future (a clock step back): it was just used.
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use nullrouter_registry::schema::CacheMode;
    use nullrouter_wire::ir::{Message, Part, Request, Role};

    use super::*;
    use crate::routing::Salt;

    const MIN: Duration = Duration::from_secs(60);

    fn t(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 + secs)
    }

    fn conversation(system: &str, turns: &[&str]) -> Request {
        Request {
            model: "m".into(),
            system: vec![Part::text(system)],
            messages: turns
                .iter()
                .enumerate()
                .map(|(i, text)| Message {
                    role: if i % 2 == 0 { Role::User } else { Role::Assistant },
                    parts: vec![Part::text(*text)],
                })
                .collect(),
            ..Request::default()
        }
    }

    fn chain(system: &str, turns: &[&str]) -> Chain {
        Chain::build(&conversation(system, turns), &Salt::from_bytes([7; 32]), Some("ttl"))
    }

    fn key(account: &str) -> CandidateKey {
        CandidateKey::new("p", account, "m")
    }

    fn all(c: &Chain) -> Vec<Written> {
        c.writable(CacheMode::Automatic, 0)
    }

    fn cands(names: &[&str], lifetime: Duration) -> Vec<(CandidateKey, Duration)> {
        names.iter().map(|n| (key(n), lifetime)).collect()
    }

    #[test]
    fn the_longest_prefix_wins_over_the_system_prompt_alone() {
        let first = chain("sys", &["a"]);
        let later = chain("sys", &["a", "b", "c"]);
        let mut store = WarmStore::new();
        store.upsert("agent", &key("a"), &all(&first)[..1], t(0));
        store.upsert("agent", &key("b"), &all(&first), t(1));

        let hit = store.lookup("agent", &later, &cands(&["a", "b"], 5 * MIN), t(10)).unwrap();
        assert_eq!(hit.key, key("b"));
        assert_eq!(hit.hash, first.boundaries[1].hash);
    }

    #[test]
    fn two_accounts_on_one_boundary_go_to_the_newest() {
        let c = chain("sys", &["a"]);
        let mut store = WarmStore::new();
        store.upsert("agent", &key("a"), &all(&c), t(5));
        store.upsert("agent", &key("b"), &all(&c), t(9));
        store.upsert("agent", &key("c"), &all(&c), t(7));
        let hit = store.lookup("agent", &c, &cands(&["a", "b", "c"], 5 * MIN), t(10)).unwrap();
        assert_eq!(hit.key, key("b"));
    }

    #[test]
    fn agents_never_see_each_other() {
        let c = chain("sys", &["a"]);
        let mut store = WarmStore::new();
        store.upsert("one", &key("a"), &all(&c), t(0));
        assert!(store.lookup("two", &c, &cands(&["a"], 5 * MIN), t(1)).is_none());
        assert!(store.lookup("one", &c, &cands(&["a"], 5 * MIN), t(1)).is_some());
    }

    #[test]
    fn warm_up_to_the_lifetime_then_gone() {
        let c = chain("sys", &["a"]);
        let mut store = WarmStore::new();
        store.upsert("agent", &key("a"), &all(&c), t(0));
        let at_edge = t(300);
        let past = t(301);
        assert!(store.lookup("agent", &c, &cands(&["a"], 5 * MIN), at_edge).is_some());
        assert!(store.lookup("agent", &c, &cands(&["a"], 5 * MIN), past).is_none());

        assert_eq!(store.sweep(at_edge, |_| Some(5 * MIN)), 0);
        assert_eq!(store.len(), 2);
        assert_eq!(store.sweep(past, |_| Some(5 * MIN)), 2);
        assert!(store.is_empty());
    }

    #[test]
    fn the_account_override_beats_the_plugin_and_a_marker_ttl_beats_both() {
        let c = chain("sys", &["a"]);
        let mut store = WarmStore::new();
        store.upsert("agent", &key("short"), &all(&c), t(0));
        store.upsert("agent", &key("long"), &all(&c), t(0));
        // The candidate list carries each account's effective lifetime: the override already applied.
        let cs = vec![(key("short"), MIN), (key("long"), 60 * MIN)];
        let hit = store.lookup("agent", &c, &cs, t(600)).unwrap();
        assert_eq!(hit.key, key("long"));

        let marked = vec![Written { hash: c.boundaries[0].hash, prefix_tokens: 1, ttl: Some(60 * MIN) }];
        store.upsert("agent", &key("short"), &marked, t(0));
        let hit = store.lookup("agent", &c, &cands(&["short"], MIN), t(600)).unwrap();
        assert_eq!(hit.key, key("short"));
        // The marker's hour also decides the sweep.
        let lifetime = |k: &CandidateKey| Some(if k.account == "long" { 60 * MIN } else { MIN });
        assert_eq!(store.sweep(t(600), lifetime), 1);
        assert_eq!(store.len(), 2 + 1);
    }

    #[test]
    fn using_a_prefix_refreshes_it() {
        let c = chain("sys", &["a", "b"]);
        let mut store = WarmStore::new();
        store.upsert("agent", &key("a"), &all(&c), t(0));
        store.touch("agent", &key("a"), &c, c.boundaries[1].hash, t(200));
        // Boundaries 0 and 1 were refreshed at 200, boundary 2 was not.
        let at_400 = store.lookup("agent", &c, &cands(&["a"], 5 * MIN), t(400)).unwrap();
        assert_eq!(at_400.hash, c.boundaries[1].hash);
        assert_eq!(at_400.last_used, t(200));
    }

    #[test]
    fn dropping_an_account_or_an_agent_removes_only_theirs() {
        let c = chain("sys", &["a"]);
        let mut store = WarmStore::new();
        store.upsert("one", &key("a"), &all(&c), t(0));
        store.upsert("one", &key("b"), &all(&c), t(0));
        store.upsert("two", &key("a"), &all(&c), t(0));
        assert_eq!(store.drop_account("p", "a"), 4);
        assert_eq!(store.len(), 2);
        assert_eq!(store.drop_agent("one"), 2);
        assert!(store.is_empty());
    }

    #[test]
    fn stored_entries_restore_to_the_same_store() {
        let c = chain("sys", &["a", "b"]);
        let mut store = WarmStore::new();
        store.upsert("agent", &key("a"), &all(&c), t(3));
        let mut again = WarmStore::new();
        for s in store.stored() {
            again.restore(s);
        }
        assert_eq!(again.stored(), store.stored());
    }
}

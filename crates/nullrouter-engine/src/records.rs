//! Request records (data-model § RequestRecord, research R21): one per client request,
//! visible while in progress, kept in a bounded ring.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use nullrouter_registry::schema::{InputSemantics, ModelType, RouteOp};
use serde::Serialize;
use serde_json::Value;

use crate::keys::AgentId;
use crate::routing::{Decision, PlacementReason};

pub use nullrouter_wire::codec::Dropped;

pub const CAPACITY: usize = 10_000;

/// `rq_` + a ULID, monotonic within the process so ids sort in arrival order.
pub fn new_id() -> String {
    static GEN: Mutex<ulid::Generator> = Mutex::new(ulid::Generator::new());
    let id = GEN.lock().unwrap_or_else(|e| e.into_inner()).generate().unwrap_or_else(|_| ulid::Ulid::new());
    format!("rq_{id}")
}

/// Token counts as the provider reported them. `None` = not reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Usage {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_write: Option<u64>,
    pub reasoning: Option<u64>,
    pub input_semantics: InputSemantics,
    /// Count-tokens requests answered by an estimate.
    pub estimated: bool,
}

impl Usage {
    /// IR usage (input excludes cache) in `semantics`, as the provider reported it.
    pub fn reported(u: &nullrouter_wire::ir::Usage, semantics: InputSemantics) -> Self {
        let cached = u.cache_read.unwrap_or(0) + u.cache_write.unwrap_or(0);
        let input = u.input.map(|i| if semantics == InputSemantics::IncludesCache { i + cached } else { i });
        Self {
            input,
            output: u.output,
            cache_read: u.cache_read,
            cache_write: u.cache_write,
            reasoning: u.reasoning,
            input_semantics: semantics,
            estimated: false,
        }
    }

    /// Adds `other` (a later segment) to `self`, converting to `self`'s semantics.
    pub fn add(&mut self, other: &Usage) {
        let uncached = |u: &Usage| {
            u.input.map(|i| match u.input_semantics {
                InputSemantics::IncludesCache => {
                    i.saturating_sub(u.cache_read.unwrap_or(0) + u.cache_write.unwrap_or(0))
                }
                InputSemantics::ExcludesCache => i,
            })
        };
        let sum = |a: Option<u64>, b: Option<u64>| match (a, b) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
        };
        let input = sum(uncached(self), uncached(other));
        self.cache_read = sum(self.cache_read, other.cache_read);
        self.cache_write = sum(self.cache_write, other.cache_write);
        self.output = sum(self.output, other.output);
        self.reasoning = sum(self.reasoning, other.reasoning);
        self.estimated |= other.estimated;
        let cached = self.cache_read.unwrap_or(0) + self.cache_write.unwrap_or(0);
        self.input = input.map(|i| if self.input_semantics == InputSemantics::IncludesCache { i + cached } else { i });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptKind {
    Initial,
    SameAccountRetry,
    NextAccount,
    NextMember,
    Continuation,
    Restart,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    Transient,
    RateLimited,
    Auth,
    NotFound,
    RequestError,
    Network,
    Timeout,
    Stall,
    InBand,
    CannotCarry,
    NoAccount,
    /// A sign-in account with no usable tokens (spec 005, research R10).
    NeedsSignIn,
    /// A sign-in account the provider refused (FR-004b).
    Refused,
    /// A sign-in account whose expired token is being refreshed.
    TokenRefreshing,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AttemptOutcome {
    Ok,
    Failed {
        status: Option<u16>,
        class: ErrorClass,
        reason: String,
    },
    Skipped {
        reason: String,
        /// Why the plan couldn't try it, when it's a class (out-of-service accounts).
        #[serde(skip_serializing_if = "Option::is_none")]
        class: Option<ErrorClass>,
    },
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Attempt {
    pub n: u32,
    pub provider: String,
    pub account: Option<String>,
    pub model: String,
    pub kind: AttemptKind,
    /// Milliseconds from request arrival.
    pub started: f64,
    pub ended: Option<f64>,
    pub outcome: Option<AttemptOutcome>,
    pub usage: Option<Usage>,
    /// Client body keys this cross-style attempt couldn't carry (research R27). Paths
    /// only, never values. Empty on same-style attempts.
    pub dropped: Vec<Dropped>,
    /// Provider-forced body parameters this attempt carried (research R8), as sent.
    pub forced: Vec<(String, Value)>,
    /// Why the placement chose this account (slice 006). `None` for skips and for requests that
    /// no placement shaped (a continuation, a job poll).
    pub placement: Option<AttemptPlacement>,
}

/// Why an attempt went where it did, and where it stood in the placement's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AttemptPlacement {
    pub reason: PlacementReason,
    /// Position in the decision's attempt order, from 0.
    pub rank: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    InProgress,
    Succeeded,
    Failed,
    Refused,
    Cancelled,
    /// A crash cut the request short; set when the journal is recovered at start (slice 006).
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BreakHandling {
    None,
    Continued,
    Restarted,
    ErrorEvent { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServedBy {
    pub provider: String,
    pub account: Option<String>,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JobRef {
    pub nullrouter_job_id: String,
    pub upstream_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RequestRecord {
    pub id: String,
    /// RFC 3339.
    pub arrived: String,
    /// `None` for a request refused before a key matched.
    pub agent: Option<AgentId>,
    pub style: String,
    pub op: Option<RouteOp>,
    pub model_type: Option<ModelType>,
    pub target: Option<String>,
    pub unified_model: Option<String>,
    pub served_by: Option<ServedBy>,
    pub attempts: Vec<Attempt>,
    pub outcome: Outcome,
    pub break_handling: BreakHandling,
    pub ttft_ms: Option<f64>,
    pub total_ms: Option<f64>,
    pub usage: Option<Usage>,
    pub job: Option<JobRef>,
    /// The placement that chose the attempt order (slice 006).
    pub decision: Option<Decision>,
}

impl RequestRecord {
    pub fn new(id: String, arrived: String, style: impl Into<String>) -> Self {
        Self {
            id,
            arrived,
            agent: None,
            style: style.into(),
            op: None,
            model_type: None,
            target: None,
            unified_model: None,
            served_by: None,
            attempts: Vec::new(),
            outcome: Outcome::InProgress,
            break_handling: BreakHandling::None,
            ttft_ms: None,
            total_ms: None,
            usage: None,
            job: None,
            decision: None,
        }
    }

    fn providers(&self) -> BTreeSet<&str> {
        self.attempts
            .iter()
            .map(|a| a.provider.as_str())
            .chain(self.served_by.as_ref().map(|s| s.provider.as_str()))
            .collect()
    }
}

#[derive(Default)]
struct Ring {
    /// Sequence number of `records[0]`.
    first: u64,
    records: VecDeque<RequestRecord>,
    by_id: HashMap<String, u64>,
    by_provider: HashMap<String, BTreeSet<u64>>,
    by_unified: HashMap<String, BTreeSet<u64>>,
}

impl Ring {
    fn get(&self, seq: u64) -> &RequestRecord {
        &self.records[(seq - self.first) as usize]
    }

    fn index(&mut self, seq: u64) {
        let r = &self.records[(seq - self.first) as usize];
        for p in r.providers() {
            self.by_provider.entry(p.to_owned()).or_default().insert(seq);
        }
        if let Some(u) = &r.unified_model {
            self.by_unified.entry(u.clone()).or_default().insert(seq);
        }
    }

    fn unindex(map: &mut HashMap<String, BTreeSet<u64>>, key: &str, seq: u64) {
        if let Some(set) = map.get_mut(key) {
            set.remove(&seq);
            if set.is_empty() {
                map.remove(key);
            }
        }
    }

    fn evict(&mut self) {
        let Some(r) = self.records.pop_front() else { return };
        let seq = self.first;
        self.first += 1;
        self.by_id.remove(&r.id);
        for p in r.providers() {
            Self::unindex(&mut self.by_provider, p, seq);
        }
        if let Some(u) = &r.unified_model {
            Self::unindex(&mut self.by_unified, u, seq);
        }
    }
}

/// The last [`CAPACITY`] records, oldest evicted, indexed by id, provider and unified
/// model.
pub struct RecordStore {
    capacity: usize,
    ring: Mutex<Ring>,
    /// Where each change to a record is written as journal lines (spec 006). Absent in tests
    /// that keep records in memory only.
    journal: Option<Arc<crate::journal::Journal>>,
}

impl Default for RecordStore {
    fn default() -> Self {
        Self::with_capacity(CAPACITY)
    }
}

/// A records query; every filter is optional.
#[derive(Debug, Clone, Default)]
pub struct Query {
    pub provider: Option<String>,
    pub unified_model: Option<String>,
    pub limit: Option<usize>,
}

impl RecordStore {
    pub fn with_capacity(capacity: usize) -> Self {
        Self { capacity: capacity.max(1), ring: Mutex::new(Ring::default()), journal: None }
    }

    /// A store that writes every change to `journal` (the live ring stays the in-flight view).
    pub fn journaled(journal: Arc<crate::journal::Journal>) -> Self {
        Self { journal: Some(journal), ..Self::default() }
    }

    /// Queues the lines a record's change needs, under the ring lock so a request's lines keep
    /// their order.
    fn write(&self, before: &RequestRecord, after: &RequestRecord, force_open: bool) {
        let Some(journal) = &self.journal else { return };
        let target = crate::journal::Target::Records { day: crate::journal::records::day_of(&after.arrived).to_owned() };
        for (t, fields) in crate::journal::records::lines_for(before, after, force_open) {
            journal.append(target.clone(), t, fields);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Ring> {
        self.ring.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn insert(&self, record: RequestRecord) {
        let mut ring = self.lock();
        if self.journal.is_some() {
            self.write(&RequestRecord::new(record.id.clone(), record.arrived.clone(), record.style.clone()), &record, true);
        }
        if ring.records.len() == self.capacity {
            ring.evict();
        }
        let seq = ring.first + ring.records.len() as u64;
        ring.by_id.insert(record.id.clone(), seq);
        ring.records.push_back(record);
        ring.index(seq);
    }

    /// Changes a record in place; `false` when it was evicted.
    pub fn update(&self, id: &str, f: impl FnOnce(&mut RequestRecord)) -> bool {
        let mut ring = self.lock();
        let Some(&seq) = ring.by_id.get(id) else { return false };
        let at = (seq - ring.first) as usize;
        let before = ring.records[at].clone();
        f(&mut ring.records[at]);
        self.write(&before, &ring.records[at], false);
        for p in before.providers() {
            Ring::unindex(&mut ring.by_provider, p, seq);
        }
        if let Some(u) = &before.unified_model {
            Ring::unindex(&mut ring.by_unified, u, seq);
        }
        ring.index(seq);
        true
    }

    /// Drops the records whose JSON `drop` selects from the live ring (the journal is rewritten
    /// separately). Returns how many.
    pub fn forget(&self, drop: impl Fn(&Value) -> bool) -> usize {
        let mut ring = self.lock();
        let all: Vec<RequestRecord> = ring.records.drain(..).collect();
        let first = ring.first + all.len() as u64;
        *ring = Ring { first, ..Ring::default() };
        let before = all.len();
        let mut kept = 0;
        for r in all {
            if serde_json::to_value(&r).is_ok_and(|v| drop(&v)) {
                continue;
            }
            let seq = ring.first + ring.records.len() as u64;
            ring.by_id.insert(r.id.clone(), seq);
            ring.records.push_back(r);
            ring.index(seq);
            kept += 1;
        }
        before - kept
    }

    pub fn get(&self, id: &str) -> Option<RequestRecord> {
        let ring = self.lock();
        ring.by_id.get(id).map(|&seq| ring.get(seq).clone())
    }

    pub fn len(&self) -> usize {
        self.lock().records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Records matching `q`, newest first.
    pub fn query(&self, q: &Query) -> Vec<RequestRecord> {
        let ring = self.lock();
        let limit = q.limit.unwrap_or(usize::MAX);
        let set = |map: &HashMap<String, BTreeSet<u64>>, key: &Option<String>| {
            key.as_ref().map(|k| map.get(k).cloned().unwrap_or_default())
        };
        let seqs: Vec<u64> = match (set(&ring.by_provider, &q.provider), set(&ring.by_unified, &q.unified_model)) {
            (Some(a), Some(b)) => a.iter().rev().filter(|s| b.contains(s)).take(limit).copied().collect(),
            (Some(s), None) | (None, Some(s)) => s.iter().rev().take(limit).copied().collect(),
            (None, None) => (ring.first..ring.first + ring.records.len() as u64).rev().take(limit).collect(),
        };
        seqs.into_iter().map(|s| ring.get(s).clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(i: usize, provider: &str, unified: Option<&str>) -> RequestRecord {
        let mut r = RequestRecord::new(format!("rq_{i}"), "2026-09-27T15:00:00Z".into(), "openai-chat");
        r.unified_model = unified.map(str::to_owned);
        r.attempts.push(Attempt {
            n: 1,
            provider: provider.into(),
            account: Some("main".into()),
            model: "m".into(),
            kind: AttemptKind::Initial,
            started: 0.0,
            ended: None,
            outcome: None,
            usage: None,
            dropped: Vec::new(),
            forced: Vec::new(),
            placement: None,
        });
        r
    }

    #[test]
    fn ids_are_time_sortable() {
        let (a, b) = (new_id(), new_id());
        assert_eq!(a.len(), 3 + 26);
        assert!(a.starts_with("rq_") && a <= b);
    }

    #[test]
    fn ring_evicts_oldest_and_keeps_indexes_right() {
        let store = RecordStore::with_capacity(3);
        store.insert(rec(0, "a", Some("u")));
        store.insert(rec(1, "b", None));
        store.insert(rec(2, "a", None));
        store.insert(rec(3, "a", Some("u")));
        assert_eq!(store.len(), 3);
        assert!(store.get("rq_0").is_none());
        let ids = |q: Query| store.query(&q).into_iter().map(|r| r.id).collect::<Vec<_>>();
        assert_eq!(ids(Query::default()), ["rq_3", "rq_2", "rq_1"]);
        assert_eq!(ids(Query { provider: Some("a".into()), ..Query::default() }), ["rq_3", "rq_2"]);
        assert_eq!(ids(Query { unified_model: Some("u".into()), ..Query::default() }), ["rq_3"]);
        assert_eq!(ids(Query { limit: Some(1), ..Query::default() }), ["rq_3"]);
        assert!(ids(Query { provider: Some("zz".into()), ..Query::default() }).is_empty());
    }

    #[test]
    fn updates_reindex_and_move_state() {
        let store = RecordStore::default();
        store.insert(rec(0, "a", None));
        assert!(store.update("rq_0", |r| {
            r.attempts[0].outcome = Some(AttemptOutcome::Failed {
                status: Some(503),
                class: ErrorClass::Transient,
                reason: "overloaded".into(),
            });
            r.attempts.push(Attempt {
                provider: "b".into(),
                n: 2,
                kind: AttemptKind::NextMember,
                ..r.attempts[0].clone()
            });
            r.served_by = Some(ServedBy { provider: "b".into(), account: None, model: "m".into() });
            r.outcome = Outcome::Succeeded;
        }));
        let by_b = store.query(&Query { provider: Some("b".into()), ..Query::default() });
        assert_eq!(by_b[0].outcome, Outcome::Succeeded);
        assert!(!store.update("rq_missing", |_| {}));
        let json = serde_json::to_value(&by_b[0]).unwrap();
        assert_eq!(
            json["attempts"][0]["outcome"],
            serde_json::json!({ "state": "failed", "status": 503, "class": "transient", "reason": "overloaded" })
        );
    }

    #[test]
    fn dropped_fields_are_recorded_as_paths_only() {
        let mut r = rec(0, "a", None);
        r.attempts[0].dropped =
            vec![Dropped { path: "messages[0].x_opt".into(), reason: "no place in openai-chat".into() }];
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(
            json["attempts"][0]["dropped"],
            serde_json::json!([{ "path": "messages[0].x_opt", "reason": "no place in openai-chat" }])
        );
        assert_eq!(serde_json::to_value(rec(1, "a", None)).unwrap()["attempts"][0]["dropped"], serde_json::json!([]));
    }

    #[test]
    fn usage_sums_across_semantics() {
        let ir =
            nullrouter_wire::ir::Usage { input: Some(70), output: Some(5), cache_read: Some(30), ..Default::default() };
        let mut total = Usage::reported(&ir, InputSemantics::IncludesCache);
        assert_eq!(total.input, Some(100));
        let second = Usage::reported(&ir, InputSemantics::ExcludesCache);
        assert_eq!(second.input, Some(70));
        total.add(&second);
        assert_eq!((total.input, total.cache_read, total.output), (Some(200), Some(60), Some(10)));
    }

    #[test]
    fn sign_in_classes_and_forced_parameters_serialise() {
        let mut r = rec(1, "xai", None);
        r.attempts[0].forced.push(("reasoning.effort".into(), Value::from("high")));
        r.attempts[0].outcome = Some(AttemptOutcome::Skipped {
            reason: "token expired, refresh retrying".into(),
            class: Some(ErrorClass::TokenRefreshing),
        });
        let j = serde_json::to_value(&r).unwrap();
        assert_eq!(j["attempts"][0]["forced"], serde_json::json!([["reasoning.effort", "high"]]));
        assert_eq!(j["attempts"][0]["outcome"]["class"], "token_refreshing");
        for (c, name) in [(ErrorClass::NeedsSignIn, "needs_sign_in"), (ErrorClass::Refused, "refused")] {
            assert_eq!(serde_json::to_value(c).unwrap(), name);
        }
        let plain = AttemptOutcome::Skipped { reason: "x".into(), class: None };
        assert!(serde_json::to_value(plain).unwrap().get("class").is_none());
    }
}

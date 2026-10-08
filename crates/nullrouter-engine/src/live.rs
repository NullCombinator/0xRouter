//! The table of requests in flight and its snapshot (spec 013, research R9).
//!
//! One entry per request, inserted at arrival and removed when the request ends. An entry holds
//! no prompt, answer, header or secret (FR-019): the agent key's id, the target the client named,
//! and the running attempt's coordinates and clock. `snapshot` clones the entries under the lock
//! and reads the clocks after it is released, so a slow reader of the snapshot can't hold up a
//! request (FR-020).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use serde::Serialize;
use serde_json::{Value, json};

use crate::phases::{AttemptPhases, Phase, PhaseValue};
use crate::records::Attempt;
use crate::timing::AttemptClock;

#[derive(Default)]
pub struct Live {
    entries: Mutex<HashMap<String, Entry>>,
}

#[derive(Clone)]
struct Entry {
    arrived: Instant,
    agent: String,
    target: String,
    attempt: Option<Running>,
    finished: Vec<AttemptPhases>,
}

#[derive(Clone)]
struct Running {
    n: u32,
    provider: String,
    account: Option<String>,
    model: String,
    /// ms from arrival.
    started: f64,
    clock: Arc<AttemptClock>,
}

/// The attempt a request is in right now.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Current {
    pub n: u32,
    pub provider: String,
    pub account: Option<String>,
    pub model: String,
    pub phase: &'static str,
    pub in_phase_ms: f64,
    pub proxy: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LiveSnapshot {
    pub id: String,
    /// The agent key's id; the operator socket swaps in the key's name.
    pub agent: String,
    pub target: String,
    pub since_arrival_ms: f64,
    /// `None` before the first attempt and between attempts.
    pub attempt: Option<Current>,
    pub finished: Vec<Value>,
}

impl Live {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Entry>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn insert(&self, id: &str, arrived: Instant, agent: &str, target: &str) {
        let e = Entry { arrived, agent: agent.to_owned(), target: target.to_owned(), attempt: None, finished: Vec::new() };
        self.lock().insert(id.to_owned(), e);
    }

    /// Sets the attempt the request is now in.
    pub fn attempt(&self, id: &str, a: &Attempt, clock: Arc<AttemptClock>) {
        if let Some(e) = self.lock().get_mut(id) {
            e.attempt = Some(Running {
                n: a.n,
                provider: a.provider.clone(),
                account: a.account.clone(),
                model: a.model.clone(),
                started: a.started,
                clock,
            });
        }
    }

    /// The running attempt ended: it moves to `finished` with its phases.
    pub fn finish_attempt(&self, id: &str, phases: AttemptPhases) {
        if let Some(e) = self.lock().get_mut(id) {
            e.attempt = None;
            e.finished.push(phases);
        }
    }

    pub fn remove(&self, id: &str) {
        self.lock().remove(id);
    }

    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    /// Every request in flight, newest first.
    pub fn snapshot(&self) -> Vec<LiveSnapshot> {
        let mut all: Vec<(String, Entry)> = self.lock().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        all.sort_by(|a, b| b.1.arrived.cmp(&a.1.arrived).then_with(|| b.0.cmp(&a.0)));
        all.into_iter().map(|(id, e)| view(id, &e)).collect()
    }

    /// One request's current phase, for `records.list`'s `slowest` (FR-013).
    pub fn current(&self, id: &str) -> Option<(Phase, f64)> {
        let e = self.lock().get(id).cloned()?;
        let since = e.arrived.elapsed().as_secs_f64() * 1e3;
        Some(match &e.attempt {
            Some(r) => where_is(r, since),
            None => (Phase::RouterOverhead, since),
        })
    }
}

fn view(id: String, e: &Entry) -> LiveSnapshot {
    let since = e.arrived.elapsed().as_secs_f64() * 1e3;
    let attempt = e.attempt.as_ref().map(|r| {
        let (phase, in_phase) = where_is(r, since);
        Current {
            n: r.n,
            provider: r.provider.clone(),
            account: r.account.clone(),
            model: r.model.clone(),
            phase: phase.name(),
            in_phase_ms: in_phase,
            proxy: None,
        }
    });
    LiveSnapshot {
        id,
        agent: e.agent.clone(),
        target: e.target.clone(),
        since_arrival_ms: since,
        attempt,
        finished: e.finished.iter().map(finished).collect(),
    }
}

/// The phase a running attempt is in and how long it has been there. Before the first attempt
/// (no `Running`) the caller shows router overhead.
fn where_is(r: &Running, now: f64) -> (Phase, f64) {
    let c = &r.clock;
    let (phase, from) = if let Some(at) = c.upstream_done() {
        (Phase::Delivery, at)
    } else if let Some(at) = c.first_output() {
        (Phase::Generation, at)
    } else if let Some(at) = c.headers() {
        (Phase::FirstToken, at)
    } else if c.connecting().is_some() && c.connected().is_none() {
        (Phase::Connect, r.started)
    } else {
        (Phase::Headers, c.connected().unwrap_or(r.started))
    };
    (phase, (now - from).max(0.0))
}

/// An ended attempt as the snapshot shows it: the phases that have a time, and where it ended.
fn finished(p: &AttemptPhases) -> Value {
    let mut phases = serde_json::Map::new();
    for ph in Phase::ALL {
        if let PhaseValue::Ms(ms) = p.value(ph) {
            let name = if p.merged_wait && ph == Phase::Headers { Phase::WaitingForProvider.name() } else { ph.name() };
            phases.insert(name.into(), json!(ms));
        }
    }
    json!({"n": p.n, "phases": phases, "ended_in": serde_json::to_value(p).ok().map(|v| v["ended_in"].clone())})
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn attempt(n: u32, account: Option<&str>) -> Attempt {
        Attempt {
            n,
            provider: "p".into(),
            account: account.map(Into::into),
            model: "up".into(),
            kind: crate::records::AttemptKind::Initial,
            started: 0.0,
            ended: None,
            outcome: None,
            usage: None,
            dropped: Vec::new(),
            forced: Vec::new(),
            placement: None,
            timing: None,
        }
    }

    fn clock(arrived: Instant) -> Arc<AttemptClock> {
        Arc::new(AttemptClock::new(arrived, Duration::from_secs(30)))
    }

    #[test]
    fn a_request_before_its_first_attempt_has_none_and_is_listed_newest_first() {
        let live = Live::default();
        let t = Instant::now();
        live.insert("rq_1", t, "key_a", "sonnet");
        live.insert("rq_2", t + Duration::from_millis(5), "key_b", "opus");
        let s = live.snapshot();
        assert_eq!(s.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["rq_2", "rq_1"]);
        assert!(s[0].attempt.is_none() && s[0].finished.is_empty());
        assert_eq!(live.current("rq_1").map(|c| c.0), Some(Phase::RouterOverhead));
        live.remove("rq_1");
        live.remove("rq_2");
        assert!(live.is_empty() && live.snapshot().is_empty());
    }

    #[test]
    fn the_phase_follows_the_clock_marks() {
        let live = Live::default();
        let t = Instant::now();
        live.insert("rq_1", t, "k", "m");
        let c = clock(t);
        live.attempt("rq_1", &attempt(1, Some("a")), c.clone());
        let phase = |live: &Live| live.snapshot()[0].attempt.as_ref().unwrap().phase;
        assert_eq!(phase(&live), "headers", "a pooled connection has no connect");
        c.mark_connecting();
        assert_eq!(phase(&live), "connect");
        c.mark_connected();
        assert_eq!(phase(&live), "headers");
        c.mark_headers();
        assert_eq!(phase(&live), "first_token");
        c.mark_first_output();
        assert_eq!(phase(&live), "generation");
        c.mark_upstream_done();
        assert_eq!(phase(&live), "delivery");
    }

    #[test]
    fn a_finished_attempt_stays_with_the_request_and_the_json_holds_no_content() {
        let live = Live::default();
        let t = Instant::now();
        live.insert("rq_1", t, "key_a", "sonnet");
        live.attempt("rq_1", &attempt(1, None), clock(t));
        let done = AttemptPhases {
            n: 1,
            phases: [PhaseValue::Ms(4.0), PhaseValue::NotApplicable, PhaseValue::NotApplicable, PhaseValue::Ms(6.0)]
                .into_iter()
                .chain([PhaseValue::NotApplicable; 3])
                .collect::<Vec<_>>()
                .try_into()
                .unwrap(),
            merged_wait: false,
            ended_in: Some(Phase::Headers),
            slowest: None,
        };
        live.finish_attempt("rq_1", done);
        let s = &live.snapshot()[0];
        assert!(s.attempt.is_none());
        assert_eq!(s.finished[0]["phases"]["router_overhead"], 4.0);
        assert_eq!(s.finished[0]["ended_in"], "headers");
        let keys: Vec<_> = serde_json::to_value(s).unwrap().as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["agent", "attempt", "finished", "id", "since_arrival_ms", "target"]);
    }
}

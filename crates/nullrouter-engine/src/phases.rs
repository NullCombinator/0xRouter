//! The one phase definition (spec 013, research R1, R14, R17).
//!
//! `phases::of` turns a request record into per-attempt phases. The records view, the list
//! column, the live view and the summaries all call it. It is pure: phases are differences of
//! adjacent marks, so they add up to the request's total by construction (FR-009).
//!
//! A *whole answer* (no stream) is recorded with `first_output` and `upstream_done` taken from
//! one reading, so the two marks are equal; generation is then "not applicable".

use serde::Serialize;
use serde::ser::{SerializeMap, Serializer};

use crate::records::{Attempt, AttemptKind, AttemptTiming, Connection, RequestRecord};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    RouterOverhead,
    RetryWait,
    Connect,
    Headers,
    FirstToken,
    Generation,
    Delivery,
    /// Headers and first token together, when the provider flushed them together (R13).
    /// Only ever the name of the `Headers` slot; never an index.
    WaitingForProvider,
}

impl Phase {
    pub const ALL: [Phase; 7] = [
        Phase::RouterOverhead,
        Phase::RetryWait,
        Phase::Connect,
        Phase::Headers,
        Phase::FirstToken,
        Phase::Generation,
        Phase::Delivery,
    ];

    fn slot(self) -> usize {
        match self {
            Phase::WaitingForProvider => Phase::Headers.slot(),
            p => Phase::ALL.iter().position(|q| *q == p).unwrap_or(0),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Phase::RouterOverhead => "router_overhead",
            Phase::RetryWait => "retry_wait",
            Phase::Connect => "connect",
            Phase::Headers => "headers",
            Phase::FirstToken => "first_token",
            Phase::Generation => "generation",
            Phase::Delivery => "delivery",
            Phase::WaitingForProvider => "waiting_for_provider",
        }
    }
}

/// Whose time a phase is (the list column, SC-010).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Router,
    Retry,
    Network,
    Provider,
    Client,
}

pub fn side(p: Phase) -> Side {
    match p {
        Phase::RouterOverhead => Side::Router,
        Phase::RetryWait => Side::Retry,
        Phase::Connect => Side::Network,
        Phase::Headers | Phase::FirstToken | Phase::WaitingForProvider | Phase::Generation => Side::Provider,
        Phase::Delivery => Side::Client,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PhaseValue {
    Ms(f64),
    NotApplicable,
    NotRecorded,
    /// The attempt is in this phase now, for this long so far.
    InProgress(f64),
}

impl PhaseValue {
    pub fn ms(self) -> Option<f64> {
        match self {
            PhaseValue::Ms(v) | PhaseValue::InProgress(v) => Some(v),
            _ => None,
        }
    }
}

impl Serialize for PhaseValue {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            PhaseValue::Ms(v) => s.serialize_f64(*v),
            PhaseValue::NotApplicable => s.serialize_str("not_applicable"),
            PhaseValue::NotRecorded => s.serialize_str("not_recorded"),
            PhaseValue::InProgress(v) => {
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry("in_progress", v)?;
                m.end()
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AttemptPhases {
    pub n: u32,
    /// In `Phase::ALL` order. When `merged_wait`, the `Headers` slot holds the combined
    /// waiting-for-provider value and `FirstToken` is not applicable.
    pub phases: [PhaseValue; 7],
    pub merged_wait: bool,
    /// The phase a failed or cancelled attempt ended in.
    pub ended_in: Option<Phase>,
    /// The largest phase of this attempt.
    pub slowest: Option<(Phase, f64)>,
}

impl AttemptPhases {
    fn all(n: u32, v: PhaseValue) -> Self {
        Self { n, phases: [v; 7], merged_wait: false, ended_in: None, slowest: None }
    }

    pub fn value(&self, p: Phase) -> PhaseValue {
        self.phases[p.slot()]
    }

    /// The sum of every `Ms` (in-progress time is not counted).
    pub fn sum(&self) -> f64 {
        self.phases.iter().filter_map(|v| if let PhaseValue::Ms(m) = v { Some(*m) } else { None }).sum()
    }
}

impl Serialize for AttemptPhases {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(None)?;
        for p in Phase::ALL {
            match (p, self.merged_wait) {
                (Phase::Headers, true) => m.serialize_entry("waiting_for_provider", &self.value(p))?,
                (Phase::FirstToken, true) => {}
                _ => m.serialize_entry(p.name(), &self.value(p))?,
            }
        }
        m.serialize_entry("ended_in", &self.ended_in.map(|p| self.shown(p)))?;
        m.end()
    }
}

impl AttemptPhases {
    /// The name a view shows for `p`, with the merge applied.
    fn shown(&self, p: Phase) -> &'static str {
        match (p, self.merged_wait) {
            (Phase::Headers | Phase::FirstToken, true) => Phase::WaitingForProvider.name(),
            _ => p.name(),
        }
    }
}

/// Everything about the request the phases of one attempt need.
#[derive(Clone, Copy)]
struct Ctx {
    /// The previous non-skipped attempt's `ended`, or 0.
    prev_end: f64,
    /// The request's TTFT, when this attempt produced the request's first output.
    first_output: Option<f64>,
    /// The request's total, for its last attempt.
    final_end: Option<f64>,
    /// Now, for an attempt still running.
    now: f64,
}

/// The phases of every attempt of `rec`. An attempt still running shows its current phase as
/// in progress, up to the latest time the record knows of.
pub fn of(rec: &RequestRecord) -> Vec<AttemptPhases> {
    let latest = rec
        .attempts
        .iter()
        .flat_map(|a| [Some(a.started), a.ended])
        .chain([rec.total_ms, rec.ttft_ms])
        .flatten()
        .fold(0.0, f64::max);
    of_at(rec, latest)
}

/// Like `of`, with an explicit `now` (ms from arrival) for attempts still running.
pub fn of_at(rec: &RequestRecord, now: f64) -> Vec<AttemptPhases> {
    let last = rec.attempts.iter().rposition(|a| a.kind != AttemptKind::Skipped);
    let producer = rec
        .attempts
        .iter()
        .position(|a| a.timing.as_ref().is_some_and(|t| t.first_output.is_some()));
    let mut prev_end = 0.0;
    let mut out = Vec::with_capacity(rec.attempts.len());
    for (i, a) in rec.attempts.iter().enumerate() {
        let ctx = Ctx {
            prev_end,
            first_output: if producer == Some(i) { rec.ttft_ms } else { None },
            final_end: if last == Some(i) { rec.total_ms } else { None },
            now,
        };
        out.push(attempt(a, ctx));
        if a.kind != AttemptKind::Skipped {
            prev_end = a.ended.unwrap_or(prev_end);
        }
    }
    out
}

/// The largest phase across `attempts`; the first wins a tie.
pub fn slowest(attempts: &[AttemptPhases]) -> Option<(Phase, f64)> {
    attempts.iter().filter_map(|a| a.slowest).fold(None, |best, c| match best {
        Some(b) if b.1 >= c.1 => Some(b),
        _ => Some(c),
    })
}

fn attempt(a: &Attempt, ctx: Ctx) -> AttemptPhases {
    if a.kind == AttemptKind::Skipped {
        return AttemptPhases::all(a.n, PhaseValue::NotApplicable);
    }
    let Some(t) = &a.timing else {
        return AttemptPhases::all(a.n, PhaseValue::NotRecorded);
    };
    let mut p = compute(a, t, ctx);
    p.slowest = p
        .phases
        .iter()
        .enumerate()
        .filter_map(|(i, v)| v.ms().map(|m| (Phase::ALL[i], m)))
        .fold(None, |best: Option<(Phase, f64)>, c| match best {
            Some(b) if b.1 >= c.1 => Some(b),
            _ => Some(c),
        })
        .map(|(ph, m)| (if p.merged_wait && ph == Phase::Headers { Phase::WaitingForProvider } else { ph }, m));
    p
}

fn compute(a: &Attempt, t: &AttemptTiming, ctx: Ctx) -> AttemptPhases {
    use PhaseValue::{InProgress, Ms, NotApplicable};
    let mut out = AttemptPhases::all(a.n, NotApplicable);
    let refresh = t.refresh_ms.unwrap_or(0.0);
    let retry = t.retry_wait_ms.unwrap_or(0.0);

    out.phases[Phase::slot(Phase::RouterOverhead)] = Ms((a.started - ctx.prev_end - retry - refresh).max(0.0));
    if let Some(r) = t.retry_wait_ms {
        out.phases[Phase::slot(Phase::RetryWait)] = Ms(r);
    }

    // The attempt's end for these phases, and the request's end for its last attempt.
    let ended = a.ended;
    let top = ended.map(|e| e.max(a.started));
    let clamp = |v: f64, floor: f64| {
        let v = v.max(floor);
        top.map_or(v, |e| v.min(e.max(floor)))
    };

    // A phase whose end mark is missing is where the attempt is, or ended.
    let mut cursor = a.started;
    let mut stopped = false;
    let stop = |out: &mut AttemptPhases, p: Phase, cursor: f64, extra: f64| {
        match top {
            Some(e) => {
                out.phases[Phase::slot(p)] = Ms((e - cursor).max(0.0) + extra);
                out.ended_in = Some(p);
            }
            None => out.phases[Phase::slot(p)] = InProgress((ctx.now - cursor).max(0.0) + extra),
        }
    };

    // Connect.
    match (t.connection, t.connected) {
        (Connection::New, Some(c)) => {
            let c = clamp(c, cursor);
            out.phases[Phase::slot(Phase::Connect)] = Ms(c - cursor + refresh);
            cursor = c;
        }
        (Connection::Reused, _) => {
            if t.refresh_ms.is_some() {
                out.phases[Phase::slot(Phase::Connect)] = Ms(refresh);
            }
        }
        _ => {
            stop(&mut out, Phase::Connect, cursor, refresh);
            stopped = true;
        }
    }

    // Headers.
    if !stopped {
        match t.headers {
            Some(h) => {
                let h = clamp(h, cursor);
                out.phases[Phase::slot(Phase::Headers)] = Ms(h - cursor);
                cursor = h;
            }
            None => {
                stop(&mut out, Phase::Headers, cursor, 0.0);
                stopped = true;
            }
        }
    }

    // First token: ends at the request's TTFT for the attempt that produced it (R1).
    if !stopped {
        match t.first_output {
            Some(f) => {
                let f = clamp(ctx.first_output.unwrap_or(f).max(f), cursor);
                out.phases[Phase::slot(Phase::FirstToken)] = Ms(f - cursor);
                cursor = f;
            }
            None => {
                stop(&mut out, Phase::FirstToken, cursor, 0.0);
                stopped = true;
            }
        }
    }

    // Generation and delivery. Time the client blocked is delivery, never generation.
    let end_all = ctx.final_end.or(ended).unwrap_or(ctx.now).max(cursor);
    let whole = matches!((t.first_output, t.upstream_done), (Some(f), Some(d)) if f == d);
    if !stopped {
        match t.upstream_done {
            Some(d) => {
                let d = clamp(d, cursor);
                let b = t.blocked_ms.min(d - cursor).max(0.0);
                if !whole {
                    out.phases[Phase::slot(Phase::Generation)] = Ms(d - cursor - b);
                }
                out.phases[Phase::slot(Phase::Delivery)] = Ms(b + (end_all.max(d) - d));
            }
            None => {
                let b = t.blocked_ms.min(top.map_or(0.0, |e| (e - cursor).max(0.0))).max(0.0);
                stop(&mut out, Phase::Generation, cursor, 0.0);
                if let (PhaseValue::Ms(g), true) = (out.phases[Phase::slot(Phase::Generation)], b > 0.0) {
                    out.phases[Phase::slot(Phase::Generation)] = Ms((g - b).max(0.0));
                    out.phases[Phase::slot(Phase::Delivery)] = Ms(b);
                }
            }
        }
    } else if let (Some(e), Some(f)) = (top, ctx.final_end) {
        // The error answer going out after the last attempt failed.
        if f > e {
            out.phases[Phase::slot(Phase::Delivery)] = Ms(f - e);
        }
    }

    // Headers and first token together (R13).
    if t.merged_wait
        && let (Ms(h), Ms(f)) = (out.phases[Phase::slot(Phase::Headers)], out.phases[Phase::slot(Phase::FirstToken)])
    {
        out.phases[Phase::slot(Phase::Headers)] = Ms(h + f);
        out.phases[Phase::slot(Phase::FirstToken)] = NotApplicable;
        out.merged_wait = true;
    }
    out
}

/// Adds `phases` to each attempt of a record in its JSON form, and `slowest` to the record
/// (contracts/record.md, operator-socket.md). The journal and the live ring both hand records
/// over as JSON, so views decorate the JSON instead of keeping a typed copy. `now` is ms from
/// arrival for a request still running; `None` uses the latest time the record knows.
pub fn decorate(record: &mut serde_json::Value, now: Option<f64>) {
    use serde_json::{Value, json};
    let Some(items) = record.get("attempts").and_then(Value::as_array) else { return };
    let mut rec = RequestRecord::new(String::new(), String::new(), String::new());
    rec.ttft_ms = record["ttft_ms"].as_f64();
    rec.total_ms = record["total_ms"].as_f64();
    let mut recorded = false;
    for a in items {
        let timing = a.get("timing").filter(|t| !t.is_null()).and_then(|t| serde_json::from_value(t.clone()).ok());
        recorded |= timing.is_some();
        rec.attempts.push(Attempt {
            n: a["n"].as_u64().unwrap_or(0) as u32,
            provider: String::new(),
            account: None,
            model: String::new(),
            kind: serde_json::from_value(a["kind"].clone()).unwrap_or(AttemptKind::Initial),
            started: a["started"].as_f64().unwrap_or(0.0),
            ended: a["ended"].as_f64(),
            outcome: None,
            usage: None,
            dropped: Vec::new(),
            forced: Vec::new(),
            placement: None,
            adapter: None,
            member: None,
            timing,
        });
    }
    let running = record["outcome"] == "in_progress";
    let phases = match (running, now) {
        (true, Some(now)) => of_at(&rec, now),
        _ => of(&rec),
    };
    let best = slowest(&phases);
    let in_progress = best.is_some_and(|(p, ms)| {
        phases.iter().any(|a| a.slowest == Some((p, ms)) && matches!(a.value(p), PhaseValue::InProgress(_)))
    });
    for (a, p) in record["attempts"].as_array_mut().into_iter().flatten().zip(&phases) {
        a["phases"] = if recorded || a["kind"] == "skipped" { json!(p) } else { json!("not_recorded") };
    }
    record["slowest"] = match best {
        Some((p, ms)) => json!({"phase": p.name(), "ms": ms, "side": side(p), "in_progress": in_progress}),
        None => Value::Null,
    };
}

#[cfg(test)]
mod tests {
    use super::PhaseValue::{Ms, NotApplicable, NotRecorded};
    use super::*;
    use crate::records::{AttemptOutcome, ErrorClass, Source, SourceBy, SourceLevel, TimeoutHit, TimeoutKind};

    fn att(n: u32, kind: AttemptKind, started: f64, ended: Option<f64>, t: Option<AttemptTiming>) -> Attempt {
        Attempt {
            n,
            provider: "p".into(),
            account: Some("a".into()),
            model: "m".into(),
            kind,
            started,
            ended,
            outcome: ended.map(|_| AttemptOutcome::Ok),
            usage: None,
            dropped: Vec::new(),
            forced: Vec::new(),
            placement: None,
            adapter: None,
            member: None,
            timing: t,
        }
    }

    fn rec(attempts: Vec<Attempt>, ttft: Option<f64>, total: Option<f64>) -> RequestRecord {
        let mut r = RequestRecord::new("rq_1".into(), "2026-10-07T00:00:00Z".into(), "openai-chat");
        r.attempts = attempts;
        r.ttft_ms = ttft;
        r.total_ms = total;
        r
    }

    /// A streamed success on a new connection: started 5, connected 25, headers 125, first
    /// output 225, done 1225, total 1230.
    fn streamed() -> (RequestRecord, AttemptTiming) {
        let t = AttemptTiming {
            connected: Some(25.0),
            connection: Connection::New,
            headers: Some(125.0),
            first_output: Some(225.0),
            upstream_done: Some(1225.0),
            ..AttemptTiming::default()
        };
        let a = att(1, AttemptKind::Initial, 5.0, Some(1228.0), Some(t.clone()));
        (rec(vec![a], Some(226.0), Some(1230.0)), t)
    }

    fn ms(p: &AttemptPhases, ph: Phase) -> f64 {
        match p.value(ph) {
            Ms(v) => v,
            other => panic!("{ph:?} is {other:?}"),
        }
    }

    fn near(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-6, "{a} != {b}");
    }

    #[test]
    fn a_streamed_success_on_a_new_connection_has_seven_phases_that_add_up() {
        let (r, _) = streamed();
        let p = &of(&r)[0];
        near(ms(p, Phase::RouterOverhead), 5.0);
        assert_eq!(p.value(Phase::RetryWait), NotApplicable);
        near(ms(p, Phase::Connect), 20.0);
        near(ms(p, Phase::Headers), 100.0);
        near(ms(p, Phase::FirstToken), 101.0);
        near(ms(p, Phase::Generation), 999.0);
        near(ms(p, Phase::Delivery), 5.0);
        near(p.sum(), 1230.0);
        assert_eq!(p.ended_in, None);
        assert_eq!(p.slowest, Some((Phase::Generation, 999.0)));
    }

    #[test]
    fn the_phases_up_to_the_first_output_add_up_to_ttft() {
        let (r, _) = streamed();
        let p = &of(&r)[0];
        let up_to: f64 = [Phase::RouterOverhead, Phase::Connect, Phase::Headers, Phase::FirstToken]
            .iter()
            .map(|ph| ms(p, *ph))
            .sum();
        near(up_to, r.ttft_ms.unwrap());
    }

    #[test]
    fn a_reused_connection_has_no_connect_phase() {
        let (mut r, mut t) = streamed();
        t.connected = None;
        t.connection = Connection::Reused;
        r.attempts[0].timing = Some(t);
        let p = &of(&r)[0];
        assert_eq!(p.value(Phase::Connect), NotApplicable);
        near(ms(p, Phase::Headers), 120.0);
        near(p.sum(), 1230.0);
    }

    #[test]
    fn a_failed_then_a_successful_attempt() {
        // Attempt 1 times out waiting for headers at 1000; attempt 2 retries after a 200 ms wait.
        let t1 = AttemptTiming {
            connected: Some(30.0),
            connection: Connection::New,
            timeout: Some(TimeoutHit {
                which: TimeoutKind::Headers,
                ms: 970,
                source: Source { by: SourceBy::BuiltIn, level: SourceLevel::Default },
            }),
            ..AttemptTiming::default()
        };
        let mut a1 = att(1, AttemptKind::Initial, 10.0, Some(1000.0), Some(t1));
        a1.outcome = Some(AttemptOutcome::Failed {
            status: None,
            class: ErrorClass::Timeout,
            reason: "headers".into(),
        });
        let t2 = AttemptTiming {
            retry_wait_ms: Some(200.0),
            connection: Connection::Reused,
            headers: Some(1320.0),
            first_output: Some(1400.0),
            upstream_done: Some(1500.0),
            ..AttemptTiming::default()
        };
        let a2 = att(2, AttemptKind::SameAccountRetry, 1210.0, Some(1500.0), Some(t2));
        let r = rec(vec![a1, a2], Some(1401.0), Some(1500.0));
        let p = of(&r);
        assert_eq!(p[0].ended_in, Some(Phase::Headers));
        near(ms(&p[0], Phase::Headers), 970.0);
        for ph in [Phase::FirstToken, Phase::Generation, Phase::Delivery] {
            assert_eq!(p[0].value(ph), NotApplicable, "{ph:?}");
        }
        // 1210 − 1000 − 200 retry wait
        near(ms(&p[1], Phase::RouterOverhead), 10.0);
        near(ms(&p[1], Phase::RetryWait), 200.0);
        near(p[0].sum() + p[1].sum(), 1500.0);
    }

    #[test]
    fn a_skipped_attempt_gives_its_time_to_the_next_attempts_router_overhead() {
        let skipped = att(1, AttemptKind::Skipped, 4.0, Some(4.0), Some(AttemptTiming::default()));
        let (mut r, _) = streamed();
        r.attempts[0].n = 2;
        r.attempts[0].kind = AttemptKind::NextMember;
        r.attempts.insert(0, skipped);
        let p = of(&r);
        assert!(p[0].phases.iter().all(|v| *v == NotApplicable));
        near(ms(&p[1], Phase::RouterOverhead), 5.0);
        near(p[1].sum(), 1230.0);
    }

    #[test]
    fn merged_wait_is_one_waiting_for_provider_value() {
        let (mut r, mut t) = streamed();
        t.merged_wait = true;
        r.attempts[0].timing = Some(t);
        let p = &of(&r)[0];
        assert!(p.merged_wait);
        near(ms(p, Phase::Headers), 201.0);
        assert_eq!(p.value(Phase::FirstToken), NotApplicable);
        assert_eq!(p.slowest.map(|s| s.0), Some(Phase::Generation));
        let json = serde_json::to_value(p).unwrap();
        assert_eq!(json["waiting_for_provider"], 201.0);
        assert!(json.get("headers").is_none() && json.get("first_token").is_none());
    }

    #[test]
    fn a_whole_answer_has_no_generation() {
        let (mut r, mut t) = streamed();
        t.first_output = Some(1225.0);
        t.upstream_done = Some(1225.0);
        r.attempts[0].timing = Some(t);
        r.ttft_ms = Some(1226.0);
        let p = &of(&r)[0];
        assert_eq!(p.value(Phase::Generation), NotApplicable);
        near(p.sum(), 1230.0);
    }

    #[test]
    fn time_the_client_blocked_is_delivery_never_generation() {
        let (mut r, mut t) = streamed();
        t.blocked_ms = 400.0;
        r.attempts[0].timing = Some(t);
        let p = &of(&r)[0];
        near(ms(p, Phase::Generation), 599.0);
        near(ms(p, Phase::Delivery), 405.0);
        near(p.sum(), 1230.0);
    }

    #[test]
    fn a_token_refresh_is_inside_connect_and_outside_router_overhead() {
        let (mut r, mut t) = streamed();
        t.refresh_ms = Some(3.0);
        r.attempts[0].timing = Some(t);
        let p = &of(&r)[0];
        near(ms(p, Phase::RouterOverhead), 2.0);
        near(ms(p, Phase::Connect), 23.0);
        near(p.sum(), 1230.0);
    }

    #[test]
    fn a_cancelled_stream_ends_in_generation() {
        let (mut r, mut t) = streamed();
        t.upstream_done = None;
        r.attempts[0].timing = Some(t);
        r.attempts[0].ended = Some(700.0);
        r.attempts[0].outcome = Some(AttemptOutcome::Cancelled);
        r.total_ms = Some(700.0);
        let p = &of(&r)[0];
        assert_eq!(p.ended_in, Some(Phase::Generation));
        near(ms(p, Phase::Generation), 474.0);
        assert_eq!(p.value(Phase::Delivery), NotApplicable);
        near(p.sum(), 700.0);
    }

    #[test]
    fn a_record_without_timing_is_not_recorded() {
        let a = att(1, AttemptKind::Initial, 5.0, Some(50.0), None);
        let p = &of(&rec(vec![a], None, Some(50.0)))[0];
        assert!(p.phases.iter().all(|v| *v == NotRecorded));
        assert_eq!(p.slowest, None);
    }

    #[test]
    fn a_running_attempt_shows_its_current_phase_in_progress() {
        let t = AttemptTiming {
            connected: Some(25.0),
            connection: Connection::New,
            headers: Some(125.0),
            ..AttemptTiming::default()
        };
        let a = att(1, AttemptKind::Initial, 5.0, None, Some(t));
        let p = &of_at(&rec(vec![a], None, None), 1000.0)[0];
        assert_eq!(p.value(Phase::FirstToken), PhaseValue::InProgress(875.0));
        assert_eq!(p.value(Phase::Generation), NotApplicable);
        assert_eq!(p.ended_in, None);
        assert_eq!(p.slowest, Some((Phase::FirstToken, 875.0)));
    }

    #[test]
    fn slowest_picks_the_largest_phase_across_attempts_and_sides_follow_the_table() {
        let (r, _) = streamed();
        let mut p = of(&r);
        let mut second = p[0].clone();
        second.slowest = Some((Phase::Connect, 5000.0));
        p.push(second);
        assert_eq!(slowest(&p), Some((Phase::Connect, 5000.0)));
        assert_eq!(slowest(&[]), None);
        assert_eq!(side(Phase::RouterOverhead), Side::Router);
        assert_eq!(side(Phase::RetryWait), Side::Retry);
        assert_eq!(side(Phase::Connect), Side::Network);
        for ph in [Phase::Headers, Phase::FirstToken, Phase::WaitingForProvider, Phase::Generation] {
            assert_eq!(side(ph), Side::Provider);
        }
        assert_eq!(side(Phase::Delivery), Side::Client);
    }
}

//! The attempt loop (research R7, R8, R10, R5).
//!
//! One task per request walks the plan: each candidate gets its same-account retry budget,
//! then a cooldown, then the next account or member takes over. The first result goes back
//! through a oneshot: a whole answer, a stream, or a failure. Until a provider answers a
//! stream with 200, nothing has reached the client, so a request that fails everywhere
//! still gets a proper status and `retry-after`. Once a stream has started, header events
//! are held until the first content event (the preamble hold), a break before content is
//! an ordinary transient failure, and keepalives fill the gaps between attempts.
//!
//! The body is the client's own, edited at named paths, when the endpoint speaks the
//! client's style, and encoded from the IR otherwise (research R27). When the client
//! streams in the wire's own style, the provider's frames go out unchanged (R5, FR-041),
//! and each is still read for usage, errors and time to first token. Every await also
//! watches the request's `CancellationToken`, so a client that goes away stops the
//! upstream work, including a backoff sleep.

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use tokio::time;
use tokio_util::sync::CancellationToken;
use zerorouter_registry::schema::{Framing, InputSemantics, ModelType, RouteOp};
use zerorouter_wire::codec::request::{self, Edits};
use zerorouter_wire::codec::response::{self, ForClient};
use zerorouter_wire::codec::{Dropped, Style};
use zerorouter_wire::error_body::{self, Tried};
use zerorouter_wire::ir::{self, ErrorEvent, Event};
use zerorouter_wire::primitives::session;
use zerorouter_wire::stream::{Frame, Framer, StreamReader, StreamWriter, usage_unasked};

use crate::accounts;
use crate::classify::{self, Verdict};
use crate::keys::AgentId;
use crate::plan::{self, Candidate, Step, Warm};
use crate::records::{Attempt, AttemptKind, AttemptOutcome, BreakHandling, ErrorClass, Outcome, ServedBy, Usage};
use crate::state::{Engine, EngineState};
use crate::upstream::{self, RequestParts};

/// Events buffered between the upstream reader and the client relay.
pub const CHANNEL: usize = 64;

/// One text generation request, decoded.
pub struct TextRequest {
    /// The record the server inserted for this request.
    pub id: String,
    pub arrived: Instant,
    pub client: Arc<Style>,
    /// The client's body as received (parsed).
    pub body: Value,
    pub ir: ir::Request,
    pub headers: HeaderMap,
    pub agent: AgentId,
    pub target: String,
    /// Whether the client asked for a stream.
    pub stream: bool,
    pub cancel: CancellationToken,
}

/// What the provider answered.
pub enum Answer {
    /// A non-stream answer: the provider's bytes, and what the client gets from them.
    Whole { status: u16, content_type: Option<String>, raw: Bytes, answer: Box<ForClient> },
    /// A streamed answer. `forced`: the endpoint streams but the client didn't ask to;
    /// collect it with [`collect`].
    Events { rx: mpsc::Receiver<Piece>, forced: bool },
}

/// One piece of a streamed answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    /// An IR event, for the client's stream writer.
    Event(Event),
    /// A provider frame for a client streaming in the wire's own style: relayed with its
    /// event name and data unchanged, and no writer involved.
    Frame(Frame),
}

/// How the stream task hands the provider's stream on.
#[derive(Debug, Clone, Copy)]
struct Relay {
    /// Send frames unchanged (a native pair) rather than IR events.
    frames: bool,
    /// Leave out frames that carry only the usage 0router's own switch asked for (R13).
    drop_usage: bool,
}

/// A request that got no answer. `message` is redacted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub status: u16,
    pub message: String,
    /// Seconds until the earliest cooldown among the tried accounts ends (all failed).
    pub retry_after: Option<u64>,
    /// Every target tried, for the `zerorouter` details (research R11).
    pub tried: Vec<Tried>,
}

impl Failure {
    fn new(status: u16, message: impl Into<String>) -> Self {
        Self { status, message: message.into(), retry_after: None, tried: Vec::new() }
    }
}

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The value 0router's session derivations take: the agent key, and the client session.
fn session_input(agent: &AgentId) -> String {
    match &agent.session {
        Some(s) => format!("{}:{s}", agent.key),
        None => agent.key.clone(),
    }
}

/// The upstream body for `c` and the client keys it couldn't carry.
fn body_for(req: &TextRequest, c: &Candidate<'_>, wire: &Style, upstream_stream: bool) -> Result<(Value, Vec<Dropped>), Failure> {
    let usage_switch = Edits { include_usage: upstream_stream, ..Edits::default() };
    let carry = |e: zerorouter_wire::codec::CodecError| Failure::new(400, format!("0router: {} can't take this request: {e}", c.provider.id));
    if c.same_style(&req.client.id) {
        let edits = Edits { model: Some(&c.upstream_id), stream: c.endpoint.force_stream.then_some(true), include_usage: upstream_stream };
        return Ok((request::forward(&req.body, wire, &edits).map_err(carry)?, Vec::new()));
    }
    let mut ir = req.ir.clone();
    ir.model.clone_from(&c.upstream_id);
    ir.stream = upstream_stream;
    let enc = request::encode(&ir, wire, &req.client.id).map_err(carry)?;
    Ok((request::forward(&enc.body, wire, &usage_switch).map_err(carry)?, enc.dropped))
}

/// The provider's error message: the usual JSON places, else the start of the text.
fn error_message(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let v: Option<Value> = serde_json::from_str(&text).ok();
    let found = v.as_ref().and_then(|v| {
        [v.pointer("/error/message"), v.pointer("/message"), v.pointer("/error"), v.pointer("/detail/message"), v.pointer("/detail")]
            .into_iter()
            .flatten()
            .find_map(|m| m.as_str().map(str::to_owned))
    });
    found.unwrap_or_else(|| text.chars().take(500).collect())
}

/// How long the stream may stay silent between attempts before a keepalive.
pub const KEEPALIVE_EVERY: Duration = Duration::from_secs(10);

/// Why one try failed.
struct Fail {
    status: Option<u16>,
    verdict: Verdict,
    /// For the record and the attempt line.
    reason: String,
    /// The provider's own message (redacted), for a non-fallback error.
    message: String,
    indicated: Option<Duration>,
    /// The client had received content when it failed.
    after_output: bool,
    /// The client already got the provider's error event.
    told: bool,
}

impl Fail {
    fn transport(class: ErrorClass, reason: String, after_output: bool) -> Self {
        Fail { status: None, verdict: classify::transport(class), message: reason.clone(), reason, indicated: None, after_output, told: false }
    }
}

enum Ended {
    Ok(Option<Usage>),
    Failed(Fail),
    Cancelled,
}

/// One request's walk through its plan.
struct Run {
    engine: Arc<Engine>,
    req: TextRequest,
    first: Option<oneshot::Sender<Result<Answer, Failure>>>,
    /// The client stream, once a provider started one.
    tx: Option<mpsc::Sender<Piece>>,
    n: u32,
}

fn cooldown_key<'c>(c: &'c Candidate<'_>) -> (&'c str, &'c str, &'c str) {
    (&c.provider.id, c.account.map_or("", |a| a.name.as_str()), &c.upstream_id)
}

fn class_name(class: ErrorClass) -> Option<String> {
    serde_json::to_value(class).ok().and_then(|v| v.as_str().map(str::to_owned))
}

impl Engine {
    /// Runs one text generation request against `st`.
    pub async fn text(self: &Arc<Self>, st: Arc<EngineState>, req: TextRequest) -> Result<Answer, Failure> {
        let (first, answer) = oneshot::channel();
        let run = Run { engine: self.clone(), req, first: Some(first), tx: None, n: 0 };
        tokio::spawn(run.run(st));
        answer.await.unwrap_or_else(|_| Err(Failure::new(500, "0router: the request ended without an answer")))
    }
}

impl Run {
    fn id(&self) -> &str {
        &self.req.id
    }

    fn now(&self) -> f64 {
        ms(self.req.arrived)
    }

    async fn run(mut self, st: Arc<EngineState>) {
        let Err(f) = self.walk(&st).await else { return };
        if let Some(first) = self.first.take() {
            let _ = first.send(Err(f));
        } else if let Some(tx) = self.tx.take() {
            let ev = Event::Error(ErrorEvent { status: Some(f.status), kind: None, message: f.message, raw: None });
            let _ = tx.send(Piece::Event(ev)).await;
        }
    }

    async fn walk(&mut self, st: &EngineState) -> Result<(), Failure> {
        let req = &self.req;
        self.engine.records.update(&req.id, |r| {
            r.op = Some(RouteOp::Generate);
            r.model_type = Some(ModelType::Text);
            r.target = Some(req.target.clone());
        });
        let warm = self.engine.warm.get(&req.agent, &req.target);
        let (target, client_style) = (req.target.clone(), req.client.id.clone());
        let plan = match plan::plan(&st.registry, &st.accounts, &target, ModelType::Text, &client_style, warm.as_ref()) {
            Ok(p) => p,
            Err(e) => {
                self.end_request(Outcome::Failed, None);
                return Err(Failure::new(e.status(), format!("0router: {e}")));
            }
        };
        let unified = plan.unified.clone();
        self.engine.records.update(&self.req.id, |r| r.unified_model = unified);
        let mut tried = Vec::new();
        let mut rested: Vec<(&str, &str, &str)> = Vec::new();
        let mut prev: Option<&str> = None;
        for step in &plan.steps {
            let c = match step {
                Step::Skip(s) => {
                    self.skip(&s.provider, s.account.clone(), &s.model, &s.reason, &mut tried);
                    continue;
                }
                Step::Try(c) => c,
            };
            let key = cooldown_key(c);
            if let Some(until) = self.engine.cooldowns.cooling(key.0, key.1, key.2) {
                rested.push(key);
                let secs = until.saturating_duration_since(time::Instant::now()).as_secs_f64().ceil();
                self.skip(&c.provider.id, c.account.map(|a| a.name.clone()), &c.upstream_id, &format!("cooling down for {secs} s"), &mut tried);
                continue;
            }
            let kind = match prev {
                None => AttemptKind::Initial,
                Some(p) if p == c.provider.id => AttemptKind::NextAccount,
                Some(_) => AttemptKind::NextMember,
            };
            prev = Some(&c.provider.id);
            if self.candidate(st, c, kind, &mut tried).await? {
                return Ok(());
            }
            rested.push(key);
        }
        self.end_request(Outcome::Failed, None);
        let summary = format!("0router: no provider could serve {}", self.req.target);
        let retry_after = self.engine.cooldowns.earliest_end(rested).map(|u| u.saturating_duration_since(time::Instant::now()).as_secs_f64().ceil().max(1.0) as u64);
        let message = error_body::message(&summary, self.id(), &tried);
        Err(Failure { status: 503, message, retry_after, tried })
    }

    /// Tries `c` with its same-account retries. `Ok(true)`: answered.
    async fn candidate(&mut self, st: &EngineState, c: &Candidate<'_>, kind: AttemptKind, tried: &mut Vec<Tried>) -> Result<bool, Failure> {
        let account = c.account.map(|a| a.name.clone());
        let skip = |run: &mut Self, reason: String, tried: &mut Vec<Tried>| {
            run.skip(&c.provider.id, account.clone(), &c.upstream_id, &reason, tried);
            Ok(false)
        };
        let Some(wire) = c.endpoint.wire.as_deref().and_then(|w| st.style(w)).cloned() else {
            return skip(self, format!("0router: provider {} names a wire that isn't loaded", c.provider.id), tried);
        };
        let upstream_stream = self.req.stream || c.endpoint.force_stream;
        let (body, dropped) = match body_for(&self.req, c, &wire, upstream_stream) {
            Ok(b) => b,
            Err(f) => return skip(self, f.message, tried),
        };
        let mut kind = kind;
        let mut retries = 0;
        let mut budget = None;
        loop {
            let out = match self.outgoing(st, c, &body) {
                Ok(o) => o,
                Err(reason) => return skip(self, reason, tried),
            };
            self.start_attempt(c, kind, dropped.clone());
            let f = match self.once(st, c, &wire, out).await {
                Ended::Ok(usage) => {
                    self.succeed(c, usage);
                    return Ok(true);
                }
                Ended::Cancelled => {
                    self.end_attempt(AttemptOutcome::Cancelled, None);
                    self.end_request(Outcome::Cancelled, None);
                    return Err(Failure::new(499, "0router: the client went away"));
                }
                Ended::Failed(f) => f,
            };
            let (p, a, m) = cooldown_key(c);
            self.end_attempt(AttemptOutcome::Failed { status: f.status, class: f.verdict.class, reason: f.reason.clone() }, None);
            let line = |reason: String, retries| Tried {
                provider: c.provider.id.clone(),
                account: account.clone(),
                model: c.upstream_id.clone(),
                status: f.status,
                class: class_name(f.verdict.class),
                reason,
                retries,
            };
            if f.after_output {
                // Restart and continuation come with US4; until then the client is told.
                self.engine.cooldowns.fail(p, a, m, &f.verdict);
                tried.push(line(f.reason.clone(), retries));
                let why = f.reason.clone();
                self.engine.records.update(self.id(), |r| r.break_handling = BreakHandling::ErrorEvent { reason: why });
                self.end_request(Outcome::Failed, None);
                if f.told {
                    self.tx = None;
                }
                let summary = format!("0router: the answer from {} broke off", c.provider.id);
                return Err(Failure { status: 502, message: error_body::message(&summary, self.id(), tried), retry_after: None, tried: tried.clone() });
            }
            if !f.verdict.fallback {
                tried.push(line(f.reason.clone(), retries));
                self.end_request(Outcome::Failed, None);
                if f.told {
                    self.tx = None;
                }
                let message = error_body::message(&f.message, self.id(), tried);
                return Err(Failure { status: f.status.unwrap_or(502), message, retry_after: None, tried: tried.clone() });
            }
            let b = *budget.get_or_insert_with(|| classify::budget(f.status, &f.verdict, f.indicated, &c.endpoint.retry));
            if retries < b.retries {
                retries += 1;
                kind = AttemptKind::SameAccountRetry;
                if !self.pause(b.delay).await {
                    self.end_request(Outcome::Cancelled, None);
                    return Err(Failure::new(499, "0router: the client went away"));
                }
                continue;
            }
            let rest = self.engine.cooldowns.fail(p, a, m, &f.verdict);
            let reason = match rest.as_secs_f64().ceil() {
                0.0 => f.reason.clone(),
                secs => format!("{}, cooling down {secs} s", f.reason),
            };
            tried.push(line(reason, retries));
            if !self.keepalive().await {
                self.end_request(Outcome::Cancelled, None);
                return Err(Failure::new(499, "0router: the client went away"));
            }
            return Ok(false);
        }
    }

    /// The request for `c`, secret and session header included; `Err` is a skip reason.
    fn outgoing(&self, st: &EngineState, c: &Candidate<'_>, body: &Value) -> Result<upstream::Outgoing, String> {
        let secret = c.account.map(|a| accounts::release(a, c.provider)).transpose().map_err(|w| format!("0router: {w}"))?;
        let parts = RequestParts {
            provider: c.provider,
            endpoint: c.endpoint,
            floor: st.registry.floor(),
            redactor: &st.redactor,
            secret,
            client_style: &self.req.client.id,
            client_headers: &self.req.headers,
            model: &c.upstream_id,
            voice: None,
            content_type: Some("application/json"),
            body: Bytes::from(body.to_string()),
        };
        let mut out = upstream::build_request(parts).map_err(|e| format!("0router: {e}"))?;
        if let Some(s) = &c.provider.session
            && let Ok(name) = HeaderName::from_bytes(s.header.as_bytes())
        {
            let client = self.req.headers.get(&name).and_then(|v| v.to_str().ok());
            if let Ok(v) = HeaderValue::from_str(&session::derive(s.derive, &session_input(&self.req.agent), client)) {
                out.headers.insert(name, v);
            }
        }
        Ok(out)
    }

    /// One upstream request and its answer.
    async fn once(&mut self, st: &EngineState, c: &Candidate<'_>, wire: &Arc<Style>, out: upstream::Outgoing) -> Ended {
        let timeout = out.header_timeout;
        let send = time::timeout(timeout, out.into_request(&st.http).send());
        let resp = match self.wait(send).await {
            None => return Ended::Cancelled,
            Some(Err(_)) => {
                let reason = format!("no response headers within {} ms", timeout.as_millis());
                return Ended::Failed(Fail::transport(ErrorClass::Timeout, reason, false));
            }
            Some(Ok(Err(e))) => {
                let reason = format!("network error: {}", st.redactor.redact(&e.to_string()));
                return Ended::Failed(Fail::transport(ErrorClass::Network, reason, false));
            }
            Some(Ok(Ok(r))) => r,
        };
        let status = resp.status().as_u16();
        let content_type = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(str::to_owned);
        let is_stream = content_type.as_deref().is_some_and(|c| c.starts_with("text/event-stream") || c.contains("ndjson"));
        let stall = upstream::stall_timeout(c.endpoint);
        let ok = (200..300).contains(&status);

        if !ok || !is_stream {
            let indicated = classify::indicated_wait(resp.headers(), SystemTime::now());
            let raw = match self.read_all(resp, stall, st).await {
                Ok(b) => b,
                Err(e) => return e,
            };
            if !ok {
                let message = st.redactor.redact(&error_message(&raw)).into_owned();
                let verdict = classify::upstream(status, &String::from_utf8_lossy(&raw));
                return Ended::Failed(Fail { status: Some(status), verdict, reason: message.clone(), message, indicated, after_output: false, told: false });
            }
            let in_band = |reason: String| Ended::Failed(Fail::transport(ErrorClass::InBand, reason, false));
            let value: Value = match serde_json::from_slice(&raw) {
                Ok(v) => v,
                Err(e) => return in_band(format!("answered non-JSON: {e}")),
            };
            let semantics = wire.text().map_or(InputSemantics::ExcludesCache, |t| t.usage.semantics);
            if self.req.stream || self.tx.is_some() {
                // A client already streaming (or asking to) gets the whole answer as events.
                let r = match response::decode(wire, &value) {
                    Ok(r) => r,
                    Err(e) => return in_band(format!("0router: {e}")),
                };
                let usage = r.usage.map(|u| Usage::reported(&u, semantics));
                self.commit();
                self.ttft();
                for ev in r.events() {
                    if !self.send(Piece::Event(ev)).await {
                        return Ended::Cancelled;
                    }
                }
                self.tx = None;
                return Ended::Ok(usage);
            }
            let answer = match response::for_client(&self.req.client, wire, &value, unix_now()) {
                Ok(a) => a,
                Err(e) => return in_band(format!("0router: {e}")),
            };
            let read = match &answer {
                ForClient::AsReceived { read } => read.as_ref(),
                ForClient::Rebuilt { read, .. } => Some(read),
            };
            let usage = read.and_then(|r| r.usage).map(|u| Usage::reported(&u, semantics));
            self.ttft();
            if let Some(first) = self.first.take() {
                let _ = first.send(Ok(Answer::Whole { status, content_type, raw, answer: Box::new(answer) }));
            }
            return Ended::Ok(usage);
        }

        let frames = self.req.stream && c.same_style(&self.req.client.id) && wire.text().is_ok_and(|t| t.framing != Framing::JsonArray);
        let relay = Relay { frames, drop_usage: frames && usage_unasked(&self.req.client, &self.req.body) };
        self.commit();
        let end = self.pump(resp, wire, relay, stall, st).await;
        if matches!(end, Ended::Ok(_)) {
            self.tx = None;
        }
        end
    }

    /// A whole body, with the stall watchdog on every chunk.
    async fn read_all(&self, mut resp: reqwest::Response, stall: Duration, st: &EngineState) -> Result<Bytes, Ended> {
        let mut buf = bytes::BytesMut::new();
        loop {
            match self.wait(time::timeout(stall, resp.chunk())).await {
                None => return Err(Ended::Cancelled),
                Some(Err(_)) => {
                    let reason = format!("no byte for {} ms", stall.as_millis());
                    return Err(Ended::Failed(Fail::transport(ErrorClass::Stall, reason, false)));
                }
                Some(Ok(Err(e))) => {
                    let reason = format!("network error: {}", st.redactor.redact(&e.to_string()));
                    return Err(Ended::Failed(Fail::transport(ErrorClass::Network, reason, false)));
                }
                Some(Ok(Ok(Some(b)))) => buf.extend_from_slice(&b),
                Some(Ok(Ok(None))) => return Ok(buf.freeze()),
            }
        }
    }

    /// Reads a streamed answer until it ends, breaks or the client goes. Header events are
    /// held until the first content event, so a failure before content can be replaced.
    async fn pump(&mut self, mut resp: reqwest::Response, wire: &Arc<Style>, relay: Relay, stall: Duration, st: &EngineState) -> Ended {
        let Ok(t) = wire.text() else { return Ended::Failed(Fail::transport(ErrorClass::InBand, "0router: the wire has no text section".into(), false)) };
        let mut framer = Framer::new(t.framing);
        let Ok(mut reader) = StreamReader::new(wire) else {
            return Ended::Failed(Fail::transport(ErrorClass::InBand, "0router: the wire's stream can't be read".into(), false));
        };
        let mut usage = ir::Usage::default();
        let mut failed: Option<ErrorEvent> = None;
        let mut held: Vec<Piece> = Vec::new();
        let mut output = false;
        loop {
            let chunk = tokio::select! {
                _ = self.req.cancel.cancelled() => return Ended::Cancelled,
                c = time::timeout(stall, resp.chunk()) => c,
            };
            let (frames, eof) = match chunk {
                Err(_) => {
                    let reason = format!("no byte for {} ms", stall.as_millis());
                    return Ended::Failed(Fail::transport(ErrorClass::Stall, reason, output));
                }
                Ok(Err(e)) => {
                    let reason = format!("stream read failed: {}", st.redactor.redact(&e.to_string()));
                    return Ended::Failed(Fail::transport(ErrorClass::Network, reason, output));
                }
                Ok(Ok(Some(b))) => (framer.feed(&b), false),
                Ok(Ok(None)) => (framer.finish(), true),
            };
            // Each piece with the events it carries.
            let mut items: Vec<(Option<Piece>, Vec<Event>)> = Vec::new();
            for f in frames {
                // A frame the wire's templates can't read carries no events; relayed
                // unchanged, it still reaches a native client.
                let events = reader.read(&f).unwrap_or_default();
                if relay.frames {
                    let only_usage = !events.is_empty() && events.iter().all(|e| matches!(e, Event::Usage(_)));
                    let piece = (!(relay.drop_usage && only_usage)).then_some(Piece::Frame(f));
                    items.push((piece, events));
                } else {
                    items.extend(events.into_iter().map(|e| (Some(Piece::Event(e.clone())), vec![e])));
                }
            }
            if eof {
                let tail = reader.finish();
                if relay.frames {
                    items.push((None, tail));
                } else {
                    items.extend(tail.into_iter().map(|e| (Some(Piece::Event(e.clone())), vec![e])));
                }
            }
            for (piece, events) in items {
                let mut content = false;
                for ev in &events {
                    match ev {
                        Event::Usage(u) => usage.merge(*u),
                        Event::Error(e) => failed = Some(e.clone()),
                        e => content |= e.is_output(),
                    }
                }
                if !output && let Some(e) = failed.take() {
                    let status = e.status;
                    let raw = e.raw.as_ref().map_or_else(|| e.message.clone(), Value::to_string);
                    let verdict = classify::upstream(status.unwrap_or(502), &raw);
                    let message = st.redactor.redact(&e.message).into_owned();
                    return Ended::Failed(Fail { status, verdict, reason: message.clone(), message, indicated: None, after_output: false, told: false });
                }
                let Some(piece) = piece else { continue };
                if !output && !content {
                    held.push(piece);
                    continue;
                }
                if !output {
                    output = true;
                    self.ttft();
                    for p in std::mem::take(&mut held) {
                        if !self.send(p).await {
                            return Ended::Cancelled;
                        }
                    }
                }
                if !self.send(piece).await {
                    return Ended::Cancelled;
                }
            }
            if eof || reader.saw_done() {
                if let Some(e) = failed.take() {
                    let reason = st.redactor.redact(&e.message).into_owned();
                    let mut f = Fail::transport(ErrorClass::InBand, reason, true);
                    f.status = e.status;
                    f.told = true;
                    return Ended::Failed(f);
                }
                for p in std::mem::take(&mut held) {
                    if !self.send(p).await {
                        return Ended::Cancelled;
                    }
                }
                return Ended::Ok((!usage.is_empty()).then(|| Usage::reported(&usage, t.usage.semantics)));
            }
        }
    }

    /// Hands the client a stream, once.
    fn commit(&mut self) {
        if self.tx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel(CHANNEL);
        self.tx = Some(tx);
        if let Some(first) = self.first.take() {
            let _ = first.send(Ok(Answer::Events { rx, forced: !self.req.stream }));
        }
    }

    /// Sends one piece to the client stream; `false` when the client has gone.
    async fn send(&self, piece: Piece) -> bool {
        let Some(tx) = &self.tx else { return true };
        tokio::select! {
            _ = self.req.cancel.cancelled() => false,
            r = tx.send(piece) => r.is_ok(),
        }
    }

    /// A keepalive on a started stream; `false` when the client has gone.
    async fn keepalive(&self) -> bool {
        self.send(Piece::Event(Event::Keepalive)).await && !self.req.cancel.is_cancelled()
    }

    /// Awaits `fut` unless the client goes, sending keepalives on a started stream.
    async fn wait<F: Future>(&self, fut: F) -> Option<F::Output> {
        tokio::pin!(fut);
        let mut tick = time::interval_at(time::Instant::now() + KEEPALIVE_EVERY, KEEPALIVE_EVERY);
        loop {
            tokio::select! {
                _ = self.req.cancel.cancelled() => return None,
                r = &mut fut => return Some(r),
                _ = tick.tick(), if self.tx.is_some() => if !self.keepalive().await { return None },
            }
        }
    }

    /// A backoff sleep; `false` when the client went away during it.
    async fn pause(&self, d: Duration) -> bool {
        self.keepalive().await && self.wait(time::sleep(d)).await.is_some()
    }

    fn ttft(&self) {
        let at = self.now();
        self.engine.records.update(self.id(), |r| {
            r.ttft_ms.get_or_insert(at);
        });
    }

    fn start_attempt(&mut self, c: &Candidate<'_>, kind: AttemptKind, dropped: Vec<Dropped>) {
        self.n += 1;
        let a = Attempt {
            n: self.n,
            provider: c.provider.id.clone(),
            account: c.account.map(|a| a.name.clone()),
            model: c.upstream_id.clone(),
            kind,
            started: self.now(),
            ended: None,
            outcome: None,
            usage: None,
            dropped,
        };
        self.engine.records.update(self.id(), |r| r.attempts.push(a));
    }

    fn end_attempt(&self, outcome: AttemptOutcome, usage: Option<Usage>) {
        let at = self.now();
        self.engine.records.update(self.id(), |r| {
            if let Some(a) = r.attempts.last_mut() {
                a.ended = Some(at);
                a.outcome = Some(outcome);
                a.usage = usage;
            }
        });
    }

    fn end_request(&self, outcome: Outcome, usage: Option<Usage>) {
        let at = self.now();
        self.engine.records.update(self.id(), |r| {
            r.outcome = outcome;
            r.usage = usage;
            r.total_ms = Some(at);
        });
    }

    /// A plan entry that can't be tried: a `skipped` attempt and an attempt line.
    fn skip(&mut self, provider: &str, account: Option<String>, model: &str, reason: &str, tried: &mut Vec<Tried>) {
        self.n += 1;
        let at = self.now();
        let a = Attempt {
            n: self.n,
            provider: provider.to_owned(),
            account: account.clone(),
            model: model.to_owned(),
            kind: AttemptKind::Skipped,
            started: at,
            ended: Some(at),
            outcome: Some(AttemptOutcome::Skipped { reason: reason.to_owned() }),
            usage: None,
            dropped: Vec::new(),
        };
        self.engine.records.update(self.id(), |r| r.attempts.push(a));
        tried.push(Tried { provider: provider.to_owned(), account, model: model.to_owned(), status: None, class: None, reason: reason.to_owned(), retries: 0 });
    }

    /// The answer completed: the record, the cooldown and the warm account.
    fn succeed(&self, c: &Candidate<'_>, usage: Option<Usage>) {
        self.end_attempt(AttemptOutcome::Ok, usage);
        let account = c.account.map(|a| a.name.clone());
        let served = ServedBy { provider: c.provider.id.clone(), account: account.clone(), model: c.upstream_id.clone() };
        self.engine.records.update(self.id(), |r| r.served_by = Some(served));
        self.end_request(Outcome::Succeeded, usage);
        let (p, a, m) = cooldown_key(c);
        self.engine.cooldowns.succeed(p, a, m);
        self.engine.warm.set(&self.req.agent, &self.req.target, Warm { provider: c.provider.id.clone(), account });
    }
}

/// Collects a streamed answer into the non-stream response IR (an endpoint that forces
/// streaming, for a client that didn't ask for it). An error event ends it.
pub async fn collect(client: &Style, body: &Value, mut rx: mpsc::Receiver<Piece>) -> Result<ir::Response, ErrorEvent> {
    let mut w = StreamWriter::new(client, body, "", "", 0).map_err(|e| ErrorEvent {
        status: Some(500),
        kind: None,
        message: e.to_string(),
        raw: None,
    })?;
    // A forced stream never relays frames: the client didn't ask to stream.
    while let Some(piece) = rx.recv().await {
        match piece {
            Piece::Event(Event::Error(e)) => return Err(e),
            Piece::Event(ev) => {
                w.write(&ev);
            }
            Piece::Frame(_) => {}
        }
    }
    Ok(w.response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_error_messages_come_from_the_usual_places() {
        assert_eq!(error_message(br#"{"error":{"message":"bad key","type":"auth"}}"#), "bad key");
        assert_eq!(error_message(br#"{"message":"slow down"}"#), "slow down");
        assert_eq!(error_message(br#"{"error":"nope"}"#), "nope");
        assert_eq!(error_message(br#"{"detail":{"message":"x"}}"#), "x");
        assert_eq!(error_message(b"plain text"), "plain text");
    }

    #[test]
    fn the_session_input_is_the_key_then_the_session() {
        assert_eq!(session_input(&AgentId::new("ak_1", None)), "ak_1");
        assert_eq!(session_input(&AgentId::new("ak_1", Some("sess-9"))), "ak_1:sess-9");
    }
}

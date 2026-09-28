//! The attempt loop. Happy path (T055): one candidate, one attempt.
//!
//! The body is the client's own, edited at named paths, when the endpoint speaks the
//! client's style, and encoded from the IR otherwise (research R27). A streamed answer is
//! read by a task that frames the provider's bytes, turns them into IR events and sends
//! them on a bounded channel; the task finishes the record. When the client streams in the
//! wire's own style, the task sends the provider's frames unchanged instead (R5, FR-041)
//! and still reads each one for usage, errors and time to first token. Every await also watches the
//! request's `CancellationToken`, so a client that goes away stops the upstream work.

use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use zerorouter_registry::schema::{Framing, InputSemantics, ModelType, RouteOp};
use zerorouter_wire::codec::request::{self, Edits};
use zerorouter_wire::codec::response::{self, ForClient};
use zerorouter_wire::codec::{Dropped, Style};
use zerorouter_wire::ir::{self, ErrorEvent, Event};
use zerorouter_wire::primitives::session;
use zerorouter_wire::stream::{Frame, Framer, StreamReader, StreamWriter, usage_unasked};

use crate::accounts;
use crate::classify;
use crate::keys::AgentId;
use crate::plan::{self, Candidate};
use crate::records::{Attempt, AttemptKind, AttemptOutcome, ErrorClass, Outcome, ServedBy, Usage};
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
}

impl Failure {
    fn new(status: u16, message: impl Into<String>) -> Self {
        Self { status, message: message.into() }
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

impl Engine {
    fn finish(&self, id: &str, arrived: Instant, outcome: AttemptOutcome, usage: Option<Usage>) {
        let ok = outcome == AttemptOutcome::Ok;
        let cancelled = outcome == AttemptOutcome::Cancelled;
        self.records.update(id, |r| {
            if let Some(a) = r.attempts.last_mut() {
                a.ended = Some(ms(arrived));
                a.outcome = Some(outcome);
                a.usage = usage;
            }
            r.usage = usage;
            r.total_ms = Some(ms(arrived));
            r.outcome = if ok {
                Outcome::Succeeded
            } else if cancelled {
                Outcome::Cancelled
            } else {
                Outcome::Failed
            };
        });
    }

    fn fail(&self, id: &str, arrived: Instant, class: ErrorClass, f: Failure) -> Failure {
        let outcome = AttemptOutcome::Failed { status: Some(f.status), class, reason: f.message.clone() };
        self.finish(id, arrived, outcome, None);
        f
    }

    /// Runs one text generation request against `st`.
    pub async fn text(self: &Arc<Self>, st: Arc<EngineState>, req: TextRequest) -> Result<Answer, Failure> {
        let (id, arrived) = (req.id.clone(), req.arrived);
        self.records.update(&id, |r| {
            r.op = Some(RouteOp::Generate);
            r.model_type = Some(ModelType::Text);
            r.target = Some(req.target.clone());
        });
        let plan = match plan::plan(&st.registry, &st.accounts, &req.target, ModelType::Text, &req.client.id) {
            Ok(p) => p,
            Err(e) => {
                let status = e.status();
                self.records.update(&id, |r| {
                    r.outcome = Outcome::Failed;
                    r.total_ms = Some(ms(arrived));
                });
                return Err(Failure::new(status, format!("0router: {e}")));
            }
        };
        let c = &plan.candidates[0];
        let account = c.account.map(|a| a.name.clone());
        self.records.update(&id, |r| {
            r.unified_model.clone_from(&plan.unified);
            r.attempts.push(Attempt {
                n: 1,
                provider: c.provider.id.clone(),
                account: account.clone(),
                model: c.upstream_id.clone(),
                kind: AttemptKind::Initial,
                started: ms(arrived),
                ended: None,
                outcome: None,
                usage: None,
                dropped: Vec::new(),
            });
            r.served_by = Some(ServedBy { provider: c.provider.id.clone(), account, model: c.upstream_id.clone() });
        });

        let Some(wire) = c.endpoint.wire.as_deref().and_then(|w| st.style(w)).cloned() else {
            let f = Failure::new(500, format!("0router: provider {} names a wire that isn't loaded", c.provider.id));
            return Err(self.fail(&id, arrived, ErrorClass::CannotCarry, f));
        };
        let upstream_stream = req.stream || c.endpoint.force_stream;
        let (body, dropped) = match body_for(&req, c, &wire, upstream_stream) {
            Ok(b) => b,
            Err(f) => return Err(self.fail(&id, arrived, ErrorClass::CannotCarry, f)),
        };
        if !dropped.is_empty() {
            self.records.update(&id, |r| {
                if let Some(a) = r.attempts.last_mut() {
                    a.dropped = dropped;
                }
            });
        }
        let secret = match c.account.map(|a| accounts::release(a, c.provider)).transpose() {
            Ok(s) => s,
            Err(w) => return Err(self.fail(&id, arrived, ErrorClass::NoAccount, Failure::new(503, format!("0router: {w}")))),
        };
        let parts = RequestParts {
            provider: c.provider,
            endpoint: c.endpoint,
            floor: st.registry.floor(),
            redactor: &st.redactor,
            secret,
            client_style: &req.client.id,
            client_headers: &req.headers,
            model: &c.upstream_id,
            voice: None,
            content_type: Some("application/json"),
            body: Bytes::from(body.to_string()),
        };
        let mut out = match upstream::build_request(parts) {
            Ok(o) => o,
            Err(e) => return Err(self.fail(&id, arrived, ErrorClass::RequestError, Failure::new(500, format!("0router: {e}")))),
        };
        if let Some(s) = &c.provider.session
            && let Ok(name) = HeaderName::from_bytes(s.header.as_bytes())
        {
            let client = req.headers.get(&name).and_then(|v| v.to_str().ok());
            if let Ok(v) = HeaderValue::from_str(&session::derive(s.derive, &session_input(&req.agent), client)) {
                out.headers.insert(name, v);
            }
        }

        let timeout = out.header_timeout;
        let send = tokio::time::timeout(timeout, out.into_request(&st.http).send());
        let resp = tokio::select! {
            _ = req.cancel.cancelled() => {
                self.finish(&id, arrived, AttemptOutcome::Cancelled, None);
                return Err(Failure::new(499, "0router: the client went away"));
            }
            r = send => r,
        };
        let resp = match resp {
            Err(_) => {
                let f = Failure::new(504, format!("0router: {} sent no response headers within {} ms", c.provider.id, timeout.as_millis()));
                return Err(self.fail(&id, arrived, ErrorClass::Timeout, f));
            }
            Ok(Err(e)) => {
                let f = Failure::new(502, format!("0router: {}: {}", c.provider.id, st.redactor.redact(&e.to_string())));
                return Err(self.fail(&id, arrived, ErrorClass::Network, f));
            }
            Ok(Ok(r)) => r,
        };
        let status = resp.status().as_u16();
        let content_type = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(str::to_owned);
        let is_stream = content_type.as_deref().is_some_and(|c| c.starts_with("text/event-stream") || c.contains("ndjson"));

        if !(200..300).contains(&status) || !is_stream {
            let raw = tokio::select! {
                _ = req.cancel.cancelled() => {
                    self.finish(&id, arrived, AttemptOutcome::Cancelled, None);
                    return Err(Failure::new(499, "0router: the client went away"));
                }
                b = resp.bytes() => b,
            };
            let raw = match raw {
                Ok(b) => b,
                Err(e) => {
                    let f = Failure::new(502, format!("0router: {}: {}", c.provider.id, st.redactor.redact(&e.to_string())));
                    return Err(self.fail(&id, arrived, ErrorClass::Network, f));
                }
            };
            if !(200..300).contains(&status) {
                let message = st.redactor.redact(&error_message(&raw)).into_owned();
                return Err(self.fail(&id, arrived, classify::upstream(status, &String::from_utf8_lossy(&raw)).class, Failure::new(status, message)));
            }
            let value: Value = match serde_json::from_slice(&raw) {
                Ok(v) => v,
                Err(e) => return Err(self.fail(&id, arrived, ErrorClass::InBand, Failure::new(502, format!("0router: {} answered non-JSON: {e}", c.provider.id)))),
            };
            let answer = match response::for_client(&req.client, &wire, &value, unix_now()) {
                Ok(a) => a,
                Err(e) => return Err(self.fail(&id, arrived, ErrorClass::InBand, Failure::new(502, format!("0router: {e}")))),
            };
            let read = match &answer {
                ForClient::AsReceived { read } => read.as_ref(),
                ForClient::Rebuilt { read, .. } => Some(read),
            };
            let semantics = wire.text().map_or(InputSemantics::ExcludesCache, |t| t.usage.semantics);
            let usage = read.and_then(|r| r.usage).map(|u| Usage::reported(&u, semantics));
            self.records.update(&id, |r| r.ttft_ms = Some(ms(arrived)));
            self.finish(&id, arrived, AttemptOutcome::Ok, usage);
            return Ok(Answer::Whole { status, content_type, raw, answer: Box::new(answer) });
        }

        let frames = req.stream && c.same_style(&req.client.id) && wire.text().is_ok_and(|t| t.framing != Framing::JsonArray);
        let relay = Relay { frames, drop_usage: frames && usage_unasked(&req.client, &req.body) };
        let (tx, rx) = mpsc::channel(CHANNEL);
        let engine = self.clone();
        let cancel = req.cancel.clone();
        tokio::spawn(async move { engine.pump(resp, wire, relay, tx, cancel, id, arrived).await });
        Ok(Answer::Events { rx, forced: !req.stream })
    }

    /// Reads a streamed answer until it ends, the client goes away or the request is
    /// cancelled, then finishes the record.
    #[allow(clippy::too_many_arguments)]
    async fn pump(
        self: Arc<Self>,
        mut resp: reqwest::Response,
        wire: Arc<Style>,
        relay: Relay,
        tx: mpsc::Sender<Piece>,
        cancel: CancellationToken,
        id: String,
        arrived: Instant,
    ) {
        let Ok(t) = wire.text() else { return };
        let mut framer = Framer::new(t.framing);
        let Ok(mut reader) = StreamReader::new(&wire) else { return };
        let mut usage = ir::Usage::default();
        let mut failed: Option<ErrorEvent> = None;
        let mut first_output = true;
        let outcome = 'outer: loop {
            let chunk = tokio::select! {
                _ = cancel.cancelled() => break AttemptOutcome::Cancelled,
                c = resp.chunk() => c,
            };
            let (frames, eof) = match chunk {
                Ok(Some(b)) => (framer.feed(&b), false),
                Ok(None) => (framer.finish(), true),
                Err(e) => {
                    let reason = format!("stream read failed: {e}");
                    break AttemptOutcome::Failed { status: None, class: ErrorClass::Network, reason };
                }
            };
            let mut pieces = Vec::new();
            for f in frames {
                // A frame the wire's templates can't read carries no events; relayed
                // unchanged, it still reaches a native client.
                let events = reader.read(&f).unwrap_or_default();
                let only_usage = !events.is_empty() && events.iter().all(|e| matches!(e, Event::Usage(_)));
                if relay.frames {
                    for ev in &events {
                        self.note(ev, &mut usage, &mut failed, &mut first_output, &id, arrived);
                    }
                    if !(relay.drop_usage && only_usage) {
                        pieces.push(Piece::Frame(f));
                    }
                } else {
                    pieces.extend(events.into_iter().map(Piece::Event));
                }
            }
            if eof {
                let tail = reader.finish();
                if relay.frames {
                    for ev in &tail {
                        self.note(ev, &mut usage, &mut failed, &mut first_output, &id, arrived);
                    }
                } else {
                    pieces.extend(tail.into_iter().map(Piece::Event));
                }
            }
            for piece in pieces {
                if let Piece::Event(ev) = &piece {
                    self.note(ev, &mut usage, &mut failed, &mut first_output, &id, arrived);
                }
                tokio::select! {
                    _ = cancel.cancelled() => break 'outer AttemptOutcome::Cancelled,
                    r = tx.send(piece) => if r.is_err() { break 'outer AttemptOutcome::Cancelled },
                }
            }
            if eof || reader.saw_done() {
                break match failed.take() {
                    Some(e) => AttemptOutcome::Failed { status: e.status, class: ErrorClass::InBand, reason: e.message },
                    None => AttemptOutcome::Ok,
                };
            }
        };
        let usage = (!usage.is_empty()).then(|| Usage::reported(&usage, t.usage.semantics));
        self.finish(&id, arrived, outcome, usage);
    }

    /// What one streamed event tells the record: usage, an in-band error, the first output.
    fn note(&self, ev: &Event, usage: &mut ir::Usage, failed: &mut Option<ErrorEvent>, first_output: &mut bool, id: &str, arrived: Instant) {
        match ev {
            Event::Usage(u) => usage.merge(*u),
            Event::Error(e) => *failed = Some(e.clone()),
            e if *first_output && e.is_output() => {
                *first_output = false;
                self.records.update(id, |r| r.ttft_ms = Some(ms(arrived)));
            }
            _ => {}
        }
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

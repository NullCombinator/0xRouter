//! Text generation: the matched route's model, stream flag and session → the engine → the
//! answer in the client's style (T056).
//!
//! A streamed answer is written by a task that owns the client style and turns each IR
//! event into the client's bytes as it arrives; a provider frame for a client in the wire's
//! own style goes out unchanged. The body the client reads cancels the
//! request when dropped, so a client that goes away stops the upstream stream.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axum::body::Bytes;
use axum::http::HeaderMap;
use axum::response::Response;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use zerorouter_engine::attempt::{self, Answer, Piece, TextRequest};
use zerorouter_engine::keys::AgentId;
use zerorouter_engine::state::{Engine, EngineState};
use zerorouter_registry::schema::Framing;
use zerorouter_registry::template::FieldPath;
use zerorouter_wire::codec::Style;
use zerorouter_wire::codec::request;
use zerorouter_wire::codec::response::{self, ForClient};
use zerorouter_wire::stream::StreamWriter;
use zerorouter_wire::template::select_one;

use crate::relay;
use crate::router::Matched;

fn at<'v>(path: &str, body: &'v Value) -> Option<&'v Value> {
    select_one(&FieldPath::parse(path).ok()?, body)
}

/// The model the route names: a body field or a path capture.
pub fn model(m: &Matched<'_>, body: &Value) -> Option<String> {
    let loc = m.entry.route.model.as_ref()?;
    match (&loc.body, &loc.path) {
        (Some(p), _) => at(p, body)?.as_str().map(str::to_owned),
        (None, Some(name)) => m.capture(name).map(str::to_owned),
        (None, None) => None,
    }
}

/// Whether the client asked for a stream: a boolean body field or a path suffix.
pub fn stream(m: &Matched<'_>, path: &str, body: &Value) -> bool {
    let Some(loc) = m.entry.route.stream.as_ref() else { return false };
    match (&loc.body, &loc.path_suffix) {
        (Some(p), _) => at(p, body).and_then(Value::as_bool).unwrap_or(false),
        (None, Some(suffix)) => path.ends_with(suffix.as_str()),
        (None, None) => false,
    }
}

fn content_type(framing: Framing) -> &'static str {
    match framing {
        Framing::Ndjson => "application/x-ndjson",
        Framing::JsonArray => "application/json",
        _ => "text/event-stream",
    }
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// What the handler already knows about the request.
pub struct Incoming<'a> {
    pub id: String,
    pub arrived: Instant,
    pub path: &'a str,
    pub headers: HeaderMap,
    pub body: Value,
    pub agent: AgentId,
}

/// Runs one text generation request and answers in the client's style.
pub async fn generate(engine: &Arc<Engine>, st: Arc<EngineState>, m: &Matched<'_>, inc: Incoming<'_>) -> Response {
    let client = m.entry.style.codec.clone();
    let id = inc.id.clone();
    let fail = |status: u16, message: &str| crate::serve::style_error(m, status, message, &id);
    let Some(target) = model(m, &inc.body) else {
        engine.records.update(&id, |r| r.outcome = zerorouter_engine::records::Outcome::Failed);
        return fail(400, "0router: the request names no model");
    };
    let ir = match request::decode(&client, &inc.body) {
        Ok(ir) => ir,
        Err(e) => {
            engine.records.update(&id, |r| r.outcome = zerorouter_engine::records::Outcome::Failed);
            return fail(400, &format!("0router: {e}"));
        }
    };
    let wants_stream = stream(m, inc.path, &inc.body);
    let cancel = CancellationToken::new();
    let req = TextRequest {
        id: id.clone(),
        arrived: inc.arrived,
        client: client.clone(),
        body: inc.body.clone(),
        ir,
        headers: inc.headers,
        agent: inc.agent,
        target,
        stream: wants_stream,
        cancel: cancel.clone(),
        media: None,
        count: false,
    };
    // Until the body is handed to the client, dropping this handler cancels the request.
    let guard = cancel.clone().drop_guard();
    let answer = match engine.text(st, req).await {
        Ok(a) => a,
        Err(f) => return crate::serve::style_failure(m, &f, &id),
    };
    match answer {
        Answer::Whole { status, content_type, raw, answer } => match *answer {
            ForClient::AsReceived { .. } => relay::as_received(status, content_type.as_deref(), raw, &id),
            ForClient::Rebuilt { body, .. } => relay::json(status, &body, &id),
        },
        Answer::Events { rx, forced: true } => match attempt::collect(&client, &inc.body, rx).await {
            Ok(r) => match response::encode(&client, &r, unix_now()) {
                Ok(body) => relay::json(200, &body, &id),
                Err(e) => fail(502, &format!("0router: {e}")),
            },
            Err(e) => fail(e.status.unwrap_or(502), &e.message),
        },
        Answer::Events { rx, forced: false } => {
            let framing = client.text().map_or(Framing::SseData, |t| t.framing);
            let (tx, out) = mpsc::channel::<Result<Bytes, Infallible>>(attempt::CHANNEL);
            let written = Written { engine: engine.clone(), id: id.clone(), arrived: inc.arrived };
            tokio::spawn(write_stream(client, framing, inc.body, written, rx, tx));
            let mut out = out;
            let body = futures_util::stream::poll_fn(move |cx| out.poll_recv(cx));
            // The streamed body now owns the cancellation.
            guard.disarm();
            relay::stream(200, content_type(framing), body, cancel, &id)
        }
        Answer::Media(_) | Answer::Count { .. } => fail(500, "0router: a text request got a non-text answer"),
    }
}

/// Where a stream writer reports its write times: the record's TTFT is the first content
/// handed to the client's socket, its total the last byte (T105).
struct Written {
    engine: Arc<Engine>,
    id: String,
    arrived: Instant,
}

impl Written {
    fn ms(&self) -> f64 {
        self.arrived.elapsed().as_secs_f64() * 1000.0
    }
}

/// Writes the answer as the client's stream bytes until it ends or the client goes. The
/// writer closes the stream only if it wrote it: relayed frames carry their own ending.
async fn write_stream(
    client: Arc<Style>,
    framing: Framing,
    body: Value,
    written: Written,
    mut rx: mpsc::Receiver<Piece>,
    tx: mpsc::Sender<Result<Bytes, Infallible>>,
) {
    let Ok(mut w) = StreamWriter::new(&client, &body, &written.id, "", unix_now()) else { return };
    let (mut relayed, mut first) = (false, true);
    while let Some(piece) = rx.recv().await {
        let (out, content) = match piece {
            Piece::Event(ev) => {
                // Written content after relayed frames (a resumed answer): the writer ends it.
                relayed &= !ev.is_output();
                (w.write(&ev), ev.is_output())
            }
            Piece::Frame(f, events) => {
                w.observe(&events);
                relayed = true;
                (f.to_bytes(framing).unwrap_or_default(), events.iter().any(|e| e.is_output()))
            }
            Piece::Restart => {
                relayed = false;
                (w.restart(), true)
            }
        };
        if !out.is_empty() && tx.send(Ok(Bytes::from(out))).await.is_err() {
            return;
        }
        if content && first {
            first = false;
            let at = written.ms();
            written.engine.records.update(&written.id, |r| r.ttft_ms = Some(at));
        }
    }
    let out = if relayed { String::new() } else { w.end() };
    if !out.is_empty() && tx.send(Ok(Bytes::from(out))).await.is_err() {
        return;
    }
    let at = written.ms();
    written.engine.records.update(&written.id, |r| r.total_ms = Some(at));
}

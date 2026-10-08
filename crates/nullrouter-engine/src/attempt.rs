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
//! upstream work, including a backoff sleep. A break after content is resumed by the
//! next attempt as [`breaks`](crate::breaks) decides.

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use indexmap::IndexMap;
use nullrouter_registry::Resolution;
use nullrouter_registry::schema::{
    BodyEncoding, BreakBehaviour, Endpoint, ErrorRule, ForcedParam, Framing, InputSemantics, ModelType, RouteOp,
};
use nullrouter_registry::template::{FieldPath, Template};
use nullrouter_wire::codec::request::{self, Edits};
use nullrouter_wire::codec::response::{self, ForClient};
use nullrouter_wire::codec::types::{self, JobStatus, TypeCodec, TypeValue};
use nullrouter_wire::codec::{Dropped, Style};
use nullrouter_wire::error_body::{self, Tried};
use nullrouter_wire::estimate;
use nullrouter_wire::ir::{self, ErrorEvent, Event};
use nullrouter_wire::primitives::{body as encodings, session};
use nullrouter_wire::stream::{Frame, Framer, StreamReader, StreamWriter, usage_unasked};
use nullrouter_wire::template::Bindings;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{Map, Value};
use tokio::sync::{mpsc, oneshot};
use tokio::time;
use tokio_util::sync::CancellationToken;

use crate::accounts;
use crate::breaks::{self, Broken, Resume, Seen};
use crate::classify::{self, Verdict};
use crate::forwarding;
use crate::identity::{self, FillContext};
use crate::inband;
use crate::jobs::Job;
use crate::keys::AgentId;
use crate::plan::{self, Candidate, Step};
use crate::records::{
    AdapterOutcome, AdapterRun, FailReason, InvalidOutputRule, NotRunReason,
    Attempt, AttemptKind, AttemptOutcome, AttemptPlacement, BreakHandling, ErrorClass, JobRef, Outcome, ServedBy, Usage,
};
use crate::response_side::{Clean, ResponseSide};
use crate::routing::{CandidateKey, PlacementReason, WhyNot};
use crate::signin::refresh::Refreshed;
use crate::state::{Engine, EngineState};
use crate::upstream::{self, RequestParts, SignedIn};

/// Events buffered between the upstream reader and the client relay.
pub const CHANNEL: usize = 64;

/// One generation request, decoded: text, or a non-text type when `media` is set.
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
    /// A non-text request (research R16); `ir` is then unused.
    pub media: Option<Media>,
    /// A token count rather than a generation (research R14).
    pub count: bool,
}

/// A non-text request, decoded by the client style's codec.
pub struct Media {
    pub ty: ModelType,
    /// The client style's codec for the type, with the route's variant applied.
    pub codec: TypeCodec,
    /// The route's variant, if any: a variant body is never forwarded as it came.
    pub variant: Option<String>,
    pub input: TypeValue,
    /// The voice a TTS target named as its last segment (`provider/model/voice`).
    pub voice: Option<String>,
    /// A job submit (video).
    pub job: bool,
}

/// A non-text answer.
pub enum MediaAnswer {
    /// Decoded; the server encodes it in the client's style.
    Value(TypeValue),
    /// The provider's bytes as they arrive, for a client whose style answers with the raw
    /// body too. An `Err` ends the body early.
    Bytes { content_type: String, rx: mpsc::Receiver<Result<Bytes, String>> },
    /// A submitted job under its `vj_` id.
    Job { id: String, status: JobStatus, bindings: Bindings },
}

/// What the provider answered.
pub enum Answer {
    /// A non-stream answer: the provider's bytes, and what the client gets from them.
    Whole {
        status: u16,
        content_type: Option<String>,
        raw: Bytes,
        answer: Box<ForClient>,
    },
    /// A streamed answer. `forced`: the endpoint streams but the client didn't ask to;
    /// collect it with [`collect`].
    Events {
        rx: mpsc::Receiver<Piece>,
        forced: bool,
    },
    Media(MediaAnswer),
    /// A token count: from the provider, or 0router's estimate.
    Count {
        input_tokens: u64,
        estimated: bool,
    },
}

/// An answer, with the provider headers the serving plugin forwards to the client
/// (`forwarding.to_client`, after the floor).
pub struct Reply {
    pub answer: Answer,
    pub headers: Vec<(HeaderName, HeaderValue)>,
    /// The key's harness adapter, when it reads responses: the client's writer runs it on each
    /// stream event (or a collected answer). A whole answer has already been through it.
    pub response: Option<ResponseSide>,
}

/// One piece of a streamed answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    /// An IR event, for the client's stream writer.
    Event(Event),
    /// A provider frame for a client streaming in the wire's own style: relayed with its
    /// event name and data unchanged. The writer only observes its events, so the counters
    /// hold if a later segment is written after a break.
    Frame(Frame, Vec<Event>),
    /// The answer restarts after a break (research R9): the writer closes the open block
    /// and writes the note; a collector starts over.
    Restart,
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
    /// Every target tried, for the `nullrouter` details (research R11).
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

/// Writes the plugin's forced parameters into `body` (research R8, spec FR-004c): the
/// endpoint's, then the model's, which win. `include` is appended to the client's list
/// without duplicates; the others replace. Nothing else in the body changes. Returns each
/// parameter as written, for the attempt record.
fn force(body: &mut Value, c: &Candidate<'_>, st: &EngineState) -> Vec<(String, Value)> {
    let model = st.registry.model(&c.provider.id, &c.requested).ok().and_then(|m| m.model);
    let mut params: IndexMap<ForcedParam, &toml::Value> = c.endpoint.force.iter().collect();
    params.extend(model.into_iter().flat_map(|m| m.force.iter()));
    write_forced(body, params)
}

/// [`force`]'s writes, for `params` already merged.
fn write_forced<'v>(
    body: &mut Value,
    params: impl IntoIterator<Item = (ForcedParam, &'v toml::Value)>,
) -> Vec<(String, Value)> {
    let params = params.into_iter();
    let Some(root) = body.as_object_mut() else { return Vec::new() };
    let mut forced = Vec::new();
    for (p, v) in params {
        let Ok(v) = serde_json::to_value(v) else { continue };
        let Some((last, parents)) = p.path().split_last() else { continue };
        let mut at = &mut *root;
        for k in parents {
            let slot = at.entry(*k).or_insert_with(|| Value::Object(Map::new()));
            if !slot.is_object() {
                *slot = Value::Object(Map::new());
            }
            let Value::Object(next) = slot else { unreachable!("made an object above") };
            at = next;
        }
        if p.appends() {
            let slot = at.entry(*last).or_insert_with(|| Value::Array(Vec::new()));
            if !slot.is_array() {
                *slot = Value::Array(Vec::new());
            }
            if let Value::Array(list) = slot {
                for item in v.as_array().into_iter().flatten() {
                    if !list.contains(item) {
                        list.push(item.clone());
                    }
                }
            }
        } else {
            at.insert((*last).to_owned(), v.clone());
        }
        forced.push((p.to_string(), v));
    }
    forced
}

/// The client-style request an attempt sends from: the body as received and its IR, or, when the
/// key's harness adapter edited it, the edited body and the IR decoded from that.
pub(crate) struct Source<'a> {
    pub body: &'a Value,
    pub ir: &'a ir::Request,
}

/// What the adapter made of the request for one candidate.
struct Adapted {
    body: Value,
    /// Decoded only for a cross-style attempt, the one that encodes from the IR.
    ir: Option<ir::Request>,
}

/// The upstream body for `c` and the client keys it couldn't carry.
fn body_for(
    req: &TextRequest,
    src: &Source<'_>,
    c: &Candidate<'_>,
    wire: &Style,
    upstream_stream: bool,
) -> Result<(Value, Vec<Dropped>), Failure> {
    let usage_switch = Edits { include_usage: upstream_stream, ..Edits::default() };
    let carry = |e: nullrouter_wire::codec::CodecError| {
        Failure::new(400, format!("0router: {} can't take this request: {e}", c.provider.id))
    };
    if c.same_style(&req.client.id) {
        let edits = Edits {
            model: Some(&c.upstream_id),
            stream: c.endpoint.force_stream.then_some(true),
            include_usage: upstream_stream,
        };
        return Ok((request::forward(src.body, wire, &edits).map_err(carry)?, Vec::new()));
    }
    let mut ir = src.ir.clone();
    ir.model.clone_from(&c.upstream_id);
    ir.stream = upstream_stream;
    let enc = request::encode(&ir, wire, &req.client.id).map_err(carry)?;
    Ok((request::forward(&enc.body, wire, &usage_switch).map_err(carry)?, enc.dropped))
}

/// The count body for `c`: the client's own, with the model replaced, when the endpoint
/// speaks the client's style; else encoded without generation parameters.
fn count_body(
    req: &TextRequest,
    src: &Source<'_>,
    c: &Candidate<'_>,
    wire: &Style,
) -> Result<(Value, Vec<Dropped>), Failure> {
    let carry = |e: nullrouter_wire::codec::CodecError| {
        Failure::new(400, format!("0router: {} can't take this request: {e}", c.provider.id))
    };
    if c.same_style(&req.client.id) {
        let edits = Edits { model: Some(&c.upstream_id), ..Edits::default() };
        return Ok((request::forward(src.body, wire, &edits).map_err(carry)?, Vec::new()));
    }
    let mut ir = src.ir.clone();
    ir.model.clone_from(&c.upstream_id);
    let enc = request::encode_count(&ir, wire, &req.client.id).map_err(carry)?;
    Ok((enc.body, enc.dropped))
}

/// What goes upstream for one candidate.
struct Outbound {
    body: Bytes,
    content_type: String,
    voice: Option<String>,
    dropped: Vec<Dropped>,
    /// The forced parameters the body carries (research R8).
    forced: Vec<(String, Value)>,
}

/// An inline endpoint's response mapping (IR field → path); empty = a binary answer.
fn mapping(e: &Endpoint) -> Result<Vec<(String, FieldPath)>, String> {
    let Some(toml::Value::Table(t)) = &e.response else { return Ok(Vec::new()) };
    t.iter()
        .map(|(k, v)| {
            let path = v.as_str().ok_or_else(|| format!("0router: response field {k} isn't a path"))?;
            Ok((k.clone(), FieldPath::parse(path).map_err(|e| format!("0router: response field {k}: {e}"))?))
        })
        .collect()
}

/// The upstream body for a non-text candidate: the client's own body with the model
/// replaced when the endpoint speaks the client's style, else encoded from the IR by the
/// wire's codec or the endpoint's inline template.
fn media_body(req: &TextRequest, m: &Media, c: &Candidate<'_>, wire: Option<&Style>) -> Result<Outbound, String> {
    let carry = |e: &dyn std::fmt::Display| format!("0router: {} can't take this request: {e}", c.provider.id);
    let voice = m.input.str("input.voice").or_else(|| m.voice.clone()).or_else(|| c.endpoint.voices.first().cloned());
    let mut input = m.input.clone();
    input.scalars.set("model", c.upstream_id.clone());
    input.scalars.set("model.upstream_id", c.upstream_id.clone());
    if let Some(v) = &voice {
        input.scalars.set("input.voice", v.clone());
    }
    // Forwarding keeps the client's own fields, unless a voice has to be added to them.
    let forward = m.variant.is_none() && (voice.is_none() || m.input.str("input.voice").is_some());
    let (value, encoding) = match wire {
        Some(w) if w.id == req.client.id && forward => {
            let mut body = req.body.clone();
            if let Some(o) = body.as_object_mut()
                && o.contains_key("model")
            {
                o.insert("model".into(), Value::String(c.upstream_id.clone()));
            }
            (body, m.codec.encoding)
        }
        Some(w) => {
            let codec = w.type_codec(m.ty).map_err(|e| carry(&e))?;
            (codec.encode_request(&input, &Bindings::new()), codec.encoding)
        }
        None => {
            let decl = c.endpoint.body.as_ref().ok_or_else(|| carry(&"the endpoint declares no body"))?;
            let t = Template::parse(decl).map_err(|e| carry(&e))?;
            (types::encode(&t, &input, &Bindings::new()), c.endpoint.encoding.unwrap_or(BodyEncoding::Json))
        }
    };
    let (body, content_type) = encodings::encode(encoding, &value);
    Ok(Outbound { body: body.into(), content_type, voice, dropped: Vec::new(), forced: Vec::new() })
}

/// A count as the record's usage.
fn count_usage(n: u64, estimated: bool) -> Usage {
    Usage {
        input: Some(n),
        output: None,
        cache_read: None,
        cache_write: None,
        reasoning: None,
        input_semantics: InputSemantics::IncludesCache,
        estimated,
    }
}

/// Usage a non-text answer reported.
fn media_usage(v: &TypeValue) -> Option<Usage> {
    let (input, output) = (v.usage("input").or_else(|| v.usage("total")), v.usage("output"));
    (input.is_some() || output.is_some()).then_some(Usage {
        input,
        output,
        cache_read: None,
        cache_write: None,
        reasoning: None,
        input_semantics: InputSemantics::ExcludesCache,
        estimated: false,
    })
}

/// The provider's error message: the usual JSON places, else the start of the text.
fn error_message(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let v: Option<Value> = serde_json::from_str(&text).ok();
    let found = v.as_ref().and_then(|v| {
        [
            v.pointer("/error/message"),
            v.pointer("/message"),
            v.pointer("/error"),
            v.pointer("/detail/message"),
            v.pointer("/detail"),
        ]
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
    /// What the cut answer reported, for the record.
    usage: Option<Usage>,
}

impl Fail {
    fn transport(class: ErrorClass, reason: String, after_output: bool) -> Self {
        Fail {
            status: None,
            verdict: classify::transport(class),
            message: reason.clone(),
            reason,
            indicated: None,
            after_output,
            usage: None,
        }
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
    first: Option<oneshot::Sender<Result<Reply, Failure>>>,
    /// The provider headers the client gets with the answer.
    forward: Vec<(HeaderName, HeaderValue)>,
    /// The client stream, once a provider started one.
    tx: Option<mpsc::Sender<Piece>>,
    n: u32,
    /// What the client has seen of the current answer.
    seen: Seen,
    /// A break after output that the next attempt resumes.
    broken: Option<Broken>,
    /// A break happened: later segments are written as events, never relayed as frames.
    segmented: bool,
    /// Usage of the segments cut by breaks, added to the answer's.
    carried: Option<Usage>,
    /// The running attempt's `(provider, account, upstream model)`, for the traffic tally
    /// (research R15); `None` for an attempt without an account.
    sent: Option<(String, String, String)>,
    /// The placement of this request, once decided.
    routed: Option<crate::route::Routed>,
    /// The step being walked: why the placement chose it and where it ranked.
    placing: Option<AttemptPlacement>,
    /// A whole (non-stream) answer, kept until the request's `close` line is written so the
    /// client never has the end of the body before the journal has the record (FR-037).
    held: Option<Reply>,
    /// The harness adapter's edited request for the candidate being tried, if it edited.
    adapted: Option<Adapted>,
    /// What the adapter did for the candidate being tried; every attempt it starts records it.
    adapter_run: Option<AdapterRun>,
    /// The response side of the adapter for the candidate being tried, if it reads responses.
    response: Option<ResponseSide>,
}

fn cooldown_key<'c>(c: &'c Candidate<'_>) -> (&'c str, &'c str, &'c str) {
    (&c.provider.id, c.account.map_or("", |a| a.name.as_str()), &c.upstream_id)
}

/// A failure an endpoint's `errors` rule read from a 2xx answer: classified by its status,
/// with class `in_band`.
fn inband_fail(ib: inband::InBand, raw: &str, after_output: bool, st: &EngineState) -> Fail {
    let mut verdict = classify::upstream(ib.status, raw);
    verdict.class = ErrorClass::InBand;
    let message = st.redactor.redact(&ib.message).into_owned();
    Fail {
        status: Some(ib.status),
        verdict,
        reason: message.clone(),
        message,
        indicated: None,
        after_output,
        usage: None,
    }
}

/// Text in a 403's message that reads as a rejected token rather than a refused model.
const AUTH_REJECTION: [&str; 6] =
    ["token", "expired", "unauthenticated", "authentication", "invalid credentials", "invalid_api_key"];

/// Whether `f` rejected a sign-in account's token (research R9): a 401, or a 403 whose
/// message reads as an auth rejection. Neither when the plugin's `[[signin.refused]]` rules
/// match (the account is refused, not expired) nor after output.
fn token_rejected(c: &Candidate<'_>, f: &Fail) -> bool {
    let Some(status) = f.status.filter(|_| !f.after_output) else { return false };
    if c.provider.signin.as_ref().is_some_and(|d| d.refuses(status, &f.message)) {
        return false;
    }
    match status {
        401 => true,
        403 => {
            let m = f.message.to_lowercase();
            f.verdict.class == ErrorClass::Auth && AUTH_REJECTION.iter().any(|t| m.contains(t))
        }
        _ => false,
    }
}

/// Whether `f` refused a sign-in account (FR-004b, research R10), with the provider's
/// reason: a response matching the plugin's `[[signin.refused]]` rules, or, once the token
/// was refreshed for this request (`refreshed`), a fresh token rejected again. A 403 that
/// reads as model access, not as a rejected token, is neither.
fn refusal(c: &Candidate<'_>, f: &Fail, refreshed: bool) -> Option<String> {
    let status = f.status.filter(|_| !f.after_output)?;
    let ruled = c.provider.signin.as_ref().is_some_and(|d| d.refuses(status, &f.message));
    (ruled || refreshed && token_rejected(c, f)).then(|| f.message.clone())
}

fn class_name(class: ErrorClass) -> Option<String> {
    serde_json::to_value(class).ok().and_then(|v| v.as_str().map(str::to_owned))
}

impl Engine {
    /// Runs one text generation request against `st`.
    pub async fn text(self: &Arc<Self>, st: Arc<EngineState>, req: TextRequest) -> Result<Answer, Failure> {
        self.reply(st, req).await.map(|r| r.answer)
    }

    /// [`Engine::text`], with the provider headers forwarded to the client.
    pub async fn reply(self: &Arc<Self>, st: Arc<EngineState>, req: TextRequest) -> Result<Reply, Failure> {
        let (first, answer) = oneshot::channel();
        let run = Run {
            engine: self.clone(),
            req,
            first: Some(first),
            forward: Vec::new(),
            tx: None,
            n: 0,
            seen: Seen::default(),
            broken: None,
            segmented: false,
            carried: None,
            sent: None,
            routed: None,
            placing: None,
            held: None,
            adapted: None,
            adapter_run: None,
            response: None,
        };
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
        let walked = self.walk(&st).await;
        // The `close` line is queued; the client waits until it is written. A journal that can't
        // write never fails the request: the ack comes back with an error and the answer goes on.
        let _ = self.engine.journal.written().wait().await;
        if let Some(reply) = self.held.take()
            && let Some(first) = self.first.take()
        {
            let _ = first.send(Ok(reply));
        }
        let Err(f) = walked else { return };
        if let Some(first) = self.first.take() {
            let _ = first.send(Err(f));
        } else if let Some(tx) = self.tx.take() {
            if let Some(b) = self.broken.take() {
                self.engine.records.update(self.id(), |r| {
                    if r.break_handling == BreakHandling::None {
                        r.break_handling = BreakHandling::ErrorEvent { reason: b.reason };
                    }
                });
            }
            let ev = Event::Error(ErrorEvent { status: Some(f.status), kind: None, message: f.message, raw: None });
            // The open block closes before the error event.
            let _ = tx.send(Piece::Event(Event::BlockStop)).await;
            let _ = tx.send(Piece::Event(ev)).await;
        }
    }

    async fn walk(&mut self, st: &EngineState) -> Result<(), Failure> {
        // The `open` line is on disk before the first upstream call.
        let _ = self.engine.journal.written().wait().await;
        let req = &self.req;
        let ty = req.media.as_ref().map_or(ModelType::Text, |m| m.ty);
        let op = match (&req.media, req.count) {
            (_, true) => RouteOp::CountTokens,
            (Some(m), _) if m.job => RouteOp::JobSubmit,
            _ => RouteOp::Generate,
        };
        self.engine.records.update(&req.id, |r| {
            r.op = Some(op);
            r.model_type = Some(ty);
            r.target = Some(crate::records::plain(&req.target));
        });
        let (mut target, client_style) = (req.target.clone(), req.client.id.clone());
        // 9router's `provider/model/voice` form for a TTS target: the prefix names a model
        // declared as TTS and the whole target doesn't (an undeclared id would pass through).
        let declared = |t: &str| match st.registry.resolve_with(t, |p, m| st.live_models.has(p, m)) {
            Ok(Resolution::Direct { provider, requested, .. }) => {
                st.registry.model(&provider.id, requested).is_ok_and(|m| m.kind.is_some())
                    || st.live_models.has(&provider.id, requested)
            }
            Ok(Resolution::Unified(_)) => true,
            Err(_) => false,
        };
        if ty == ModelType::Tts
            && !declared(&target)
            && let Some((model, voice)) = target.rsplit_once('/')
            && model.contains('/')
            && declared(model)
        {
            let (model, voice) = (model.to_owned(), voice.to_owned());
            if let Some(m) = self.req.media.as_mut() {
                m.voice = Some(voice);
            }
            target = model;
        }
        let plan = match plan::plan(&st.registry, &st.accounts, &st.tokens, &st.live_models, &target, ty, &client_style)
        {
            Ok(p) => p,
            Err(e) => {
                self.end_request(Outcome::Failed, None);
                return Err(Failure::new(e.status(), format!("0router: {e}")));
            }
        };
        let unified = plan.unified.clone();
        self.engine.records.update(&self.req.id, |r| r.unified_model = unified);
        let routed = crate::route::decide(&self.engine, st, &self.req, &plan, SystemTime::now());
        let (order, decision) = (routed.order.clone(), routed.decision.clone());
        self.engine.records.update(&self.req.id, |r| r.decision = Some(decision));
        self.routed = Some(routed);
        let mut tried = Vec::new();
        let mut rested: Vec<(&str, &str, &str)> = Vec::new();
        let mut prev: Option<&str> = None;
        // What can't be tried at all is recorded first; the rest follows the placement's order.
        for step in &plan.steps {
            if let Step::Skip(s) = step {
                self.skip(&s.provider, s.account.clone(), &s.model, &s.reason, s.class, &mut tried);
            }
        }
        for slot in &order {
            let Step::Try(c) = &plan.steps[slot.step] else { continue };
            let key = cooldown_key(c);
            if let Some(until) = self.engine.cooldowns.cooling(key.0, key.1, key.2) {
                rested.push(key);
                let secs = until.saturating_duration_since(time::Instant::now()).as_secs_f64().ceil();
                self.skip(
                    &c.provider.id,
                    c.account.map(|a| a.name.clone()),
                    &c.upstream_id,
                    &format!("cooling down for {secs} s"),
                    None,
                    &mut tried,
                );
                continue;
            }
            let kind = match prev {
                None => AttemptKind::Initial,
                Some(p) if p == c.provider.id => AttemptKind::NextAccount,
                Some(_) => AttemptKind::NextMember,
            };
            prev = Some(&c.provider.id);
            self.placing = Some(AttemptPlacement { reason: slot.reason, rank: slot.rank });
            if self.candidate(st, c, kind, &mut tried).await? {
                return Ok(());
            }
            rested.push(key);
        }
        // Everyone the placement left out (priority 0, a window at its floor with something else
        // to try, …) is named with its reason: an error that lists only what was tried would hide
        // why the rest never were (spec edge case "Every account blocked").
        let left_out: Vec<_> = self
            .routed
            .as_ref()
            .map(|r| {
                let d = &r.decision;
                d.candidates
                    .iter()
                    .enumerate()
                    .filter(|(i, row)| !d.order.contains(i) && row.why_not.is_some_and(|w| w != WhyNot::OutOfService))
                    .map(|(_, row)| (row.provider.clone(), row.account.clone(), row.model.clone(), row.why_not))
                    .collect()
            })
            .unwrap_or_default();
        for (provider, account, model, why) in left_out {
            let reason = format!("not tried: {}", why.map_or(String::new(), |w| w.to_string()));
            self.skip(&provider, (!account.is_empty()).then_some(account), &model, &reason, None, &mut tried);
        }
        self.end_request(Outcome::Failed, None);
        let summary = format!("0router: no provider could serve {}", self.req.target);
        let retry_after = self
            .engine
            .cooldowns
            .earliest_end(rested)
            .map(|u| u.saturating_duration_since(time::Instant::now()).as_secs_f64().ceil().max(1.0) as u64);
        let message = error_body::message(&summary, self.id(), &tried);
        Err(Failure { status: 503, message, retry_after, tried })
    }

    /// Tries `c` with its same-account retries. `Ok(true)`: answered.
    async fn candidate(
        &mut self,
        st: &EngineState,
        c: &Candidate<'_>,
        kind: AttemptKind,
        tried: &mut Vec<Tried>,
    ) -> Result<bool, Failure> {
        let account = c.account.map(|a| a.name.clone());
        let skip = |run: &mut Self, reason: String, tried: &mut Vec<Tried>| {
            run.skip(&c.provider.id, account.clone(), &c.upstream_id, &reason, None, tried);
            Ok(false)
        };
        let wire = match c.endpoint.wire.as_deref() {
            None => None,
            Some(w) => match st.style(w) {
                Some(s) => Some(s.clone()),
                None => {
                    return skip(
                        self,
                        format!("0router: provider {} names a wire that isn't loaded", c.provider.id),
                        tried,
                    );
                }
            },
        };
        // A count goes to the endpoint's `[token_count]` URL, or is estimated without one.
        let (count_endpoint, counted);
        let c = match (self.req.count, &c.endpoint.token_count) {
            (false, _) => c,
            (true, None) if self.estimate(st, c, kind) => return Ok(true),
            (true, None) => {
                return skip(self, "0router: the request can't be put in the Messages shape to estimate".into(), tried);
            }
            (true, Some(tc)) => {
                count_endpoint = Endpoint { url: tc.url.clone(), force_stream: false, ..c.endpoint.clone() };
                counted = Candidate { endpoint: &count_endpoint, ..c.clone() };
                &counted
            }
        };
        self.adapt(st, c).await;
        let outbound = match (&self.req.media, &wire) {
            (Some(m), wire) => match media_body(&self.req, m, c, wire.as_deref()) {
                Ok(o) => o,
                Err(reason) => return skip(self, reason, tried),
            },
            (None, None) => {
                return skip(
                    self,
                    format!("0router: provider {} has a text endpoint with no wire", c.provider.id),
                    tried,
                );
            }
            (None, Some(wire)) if self.req.count => match count_body(&self.req, &self.source(), c, wire) {
                Ok((body, dropped)) => Outbound {
                    body: Bytes::from(body.to_string()),
                    content_type: "application/json".into(),
                    voice: None,
                    dropped,
                    forced: Vec::new(),
                },
                Err(f) => return skip(self, f.message, tried),
            },
            (None, Some(wire)) => {
                let upstream_stream = self.req.stream || c.endpoint.force_stream;
                match body_for(&self.req, &self.source(), c, wire, upstream_stream) {
                    Ok((mut body, dropped)) => {
                        let forced = force(&mut body, c, st);
                        Outbound {
                            body: Bytes::from(body.to_string()),
                            content_type: "application/json".into(),
                            voice: None,
                            dropped,
                            forced,
                        }
                    }
                    Err(f) => return skip(self, f.message, tried),
                }
            }
        };
        let mut kind = kind;
        let mut retries = 0;
        let mut budget = None;
        // A rejected sign-in token gets one refresh and one retry here (research R9).
        let mut refreshed = false;
        let signin = c.account.filter(|a| a.is_signin());
        loop {
            let prefilled;
            let ob = match self.broken.as_ref().map(|b| b.reason.clone()) {
                None => &outbound,
                Some(reason) => match wire.as_deref().map(|w| breaks::continuation(&self.req, &self.source(), c, w, &self.seen)) {
                    Some(Ok((mut body, dropped))) => {
                        kind = AttemptKind::Continuation;
                        self.resume_with(Resume::Continue);
                        let forced = force(&mut body, c, st);
                        prefilled = Outbound {
                            body: Bytes::from(body.to_string()),
                            content_type: "application/json".into(),
                            voice: None,
                            dropped,
                            forced,
                        };
                        &prefilled
                    }
                    _ if st.break_behaviour(&self.req.agent.key) == BreakBehaviour::Restart => {
                        kind = AttemptKind::Restart;
                        self.resume_with(Resume::Restart);
                        &outbound
                    }
                    _ => return Err(self.broke_off(c, reason, tried)),
                },
            };
            // Use-time freshness: a token about to expire is refreshed before it is sent; a
            // failed refresh leaves the cell valid or out of service, which `outgoing` reads.
            if let Some(a) = signin
                && self.wait(self.engine.fresh_for_use(&a.provider, &a.name)).await.is_none()
            {
                self.end_request(Outcome::Cancelled, None);
                return Err(Failure::new(499, "0router: the client went away"));
            }
            let sent = signin.and_then(|a| st.tokens.get(&a.provider, &a.name));
            let out = match self.outgoing(st, c, ob) {
                Ok(o) => o,
                Err((reason, class)) => {
                    self.skip(&c.provider.id, account.clone(), &c.upstream_id, &reason, class, tried);
                    return Ok(false);
                }
            };
            self.start_attempt(c, kind, ob.dropped.clone(), ob.forced.clone());
            let f = match self.once(st, c, wire.as_ref(), out).await {
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
            let refused = signin.and_then(|_| refusal(c, &f, refreshed));
            let class = if refused.is_some() { ErrorClass::Refused } else { f.verdict.class };
            self.end_attempt(AttemptOutcome::Failed { status: f.status, class, reason: f.reason.clone() }, f.usage);
            self.carry(f.usage);
            // A refused account goes out of service and the request falls back, as for any
            // non-serving account; no cooldown: the state keeps it out.
            if let (Some(a), Some(sent), Some(why)) = (signin, sent.as_deref(), refused) {
                self.engine.refuse(&a.provider, &a.name, &why, sent).await;
                let w = accounts::Withheld::Refused {
                    provider: a.provider.clone(),
                    name: a.name.clone(),
                    reason: crate::signin::refresh::short(&st.redactor.redact(&why)),
                };
                tried.push(Tried {
                    provider: c.provider.id.clone(),
                    account: account.clone(),
                    model: c.upstream_id.clone(),
                    status: f.status,
                    class: class_name(ErrorClass::Refused),
                    reason: w.to_string(),
                    retries,
                });
                if !self.keepalive().await {
                    self.end_request(Outcome::Cancelled, None);
                    return Err(Failure::new(499, "0router: the client went away"));
                }
                return Ok(false);
            }
            if let (Some(a), Some(sent), false) = (signin, sent.as_deref(), refreshed)
                && token_rejected(c, &f)
            {
                refreshed = true;
                let Some(r) = self.wait(self.engine.refresh_rejected(&a.provider, &a.name, sent)).await else {
                    self.end_request(Outcome::Cancelled, None);
                    return Err(Failure::new(499, "0router: the client went away"));
                };
                // The account isn't at fault for the model: no cooldown; the refresher retries.
                let (class, reason) = match r {
                    // The client sees only the retried answer; no fallback is counted.
                    Refreshed::Fresh => {
                        kind = AttemptKind::SameAccountRetry;
                        continue;
                    }
                    Refreshed::Transient(why) => (
                        ErrorClass::TokenRefreshing,
                        format!("{}; the token refresh failed ({why}), retrying", f.reason),
                    ),
                    Refreshed::Permanent(_) => (
                        ErrorClass::NeedsSignIn,
                        accounts::Withheld::NeedsSignIn { provider: a.provider.clone(), name: a.name.clone() }
                            .to_string(),
                    ),
                };
                tried.push(Tried {
                    provider: c.provider.id.clone(),
                    account: account.clone(),
                    model: c.upstream_id.clone(),
                    status: f.status,
                    class: class_name(class),
                    reason,
                    retries,
                });
                if !self.keepalive().await {
                    self.end_request(Outcome::Cancelled, None);
                    return Err(Failure::new(499, "0router: the client went away"));
                }
                return Ok(false);
            }
            let line = |reason: String, retries| Tried {
                provider: c.provider.id.clone(),
                account: account.clone(),
                model: c.upstream_id.clone(),
                status: f.status,
                class: class_name(f.verdict.class),
                reason,
                retries,
            };
            if f.after_output && !self.req.stream {
                // A collected answer: the client saw nothing, so the collector starts over.
                if !self.send(Piece::Restart).await {
                    self.end_request(Outcome::Cancelled, None);
                    return Err(Failure::new(499, "0router: the client went away"));
                }
                self.seen = Seen::default();
            } else if f.after_output {
                self.segmented = true;
                if self.seen.open == breaks::Open::ToolCall {
                    self.engine.cooldowns.fail(p, a, m, &f.verdict);
                    tried.push(line(f.reason.clone(), retries));
                    let reason = format!("the stream broke while a tool call was being sent: {}", f.reason);
                    return Err(self.broke_off(c, reason, tried));
                }
                self.broken.get_or_insert_with(|| Broken { reason: f.reason.clone(), resume: None });
            }
            if !f.verdict.fallback {
                tried.push(line(f.reason.clone(), retries));
                self.end_request(Outcome::Failed, None);
                let message = error_body::message(&f.message, self.id(), tried);
                return Err(Failure {
                    status: f.status.unwrap_or(502),
                    message,
                    retry_after: None,
                    tried: tried.clone(),
                });
            }
            let b =
                *budget.get_or_insert_with(|| classify::budget(f.status, &f.verdict, f.indicated, &c.endpoint.retry));
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
    ///
    /// A sign-in account's token comes from its cell in one atomic load ([`accounts::release`]),
    /// so a refresh never waits for a reload; its request gets the `[signin] auth` placement
    /// and the filled `[identity]` headers (research R6, R7). The body is not touched here.
    fn outgoing(
        &self,
        st: &EngineState,
        c: &Candidate<'_>,
        ob: &Outbound,
    ) -> Result<upstream::Outgoing, (String, Option<ErrorClass>)> {
        let plain = |e: String| (e, None);
        let released = c
            .account
            .map(|a| accounts::release(a, c.provider, &st.tokens))
            .transpose()
            // An out-of-service account reads as the plan's skip does (research R10).
            .map_err(|w| match w.class() {
                Some(class) => (w.to_string(), Some(class)),
                None => (format!("0router: {w}"), None),
            })?;
        let token = released.as_ref().and_then(accounts::Released::token);
        let signin = match (token, &c.provider.signin) {
            (Some(view), Some(decl)) => {
                let identity = match &c.provider.identity {
                    None => Vec::new(),
                    Some(d) => {
                        let session_id = self.engine.sessions.id_for_upstream(&self.req.agent, &st.redactor.current());
                        let ctx = FillContext {
                            session_id: &session_id,
                            request_id: &identity::uuid_v4(),
                            turns: identity::user_turns(&self.req.ir),
                            upstream_model: &c.upstream_id,
                            claims: Some(&view.entry.claims),
                            install_id: self.engine.install_id().map_err(|e| plain(format!("0router: {e}")))?,
                        };
                        identity::headers(d, &ctx)
                    }
                };
                Some(SignedIn { auth: &decl.auth, identity })
            }
            _ => None,
        };
        let redactor = st.redactor.current();
        let parts = RequestParts {
            provider: c.provider,
            endpoint: c.endpoint,
            floor: st.registry.floor(),
            redactor: &redactor,
            secret: released.as_ref().map(accounts::Released::secret),
            client_style: &self.req.client.id,
            client_headers: &self.req.headers,
            model: &c.upstream_id,
            voice: ob.voice.as_deref(),
            content_type: Some(&ob.content_type),
            body: ob.body.clone(),
            signin,
        };
        let mut out = upstream::build_request(parts).map_err(|e| plain(format!("0router: {e}")))?;
        upstream::check_ip_host(&out.url, st.registry.runtime().allow_private_endpoints)
            .map_err(|e| plain(format!("0router: {e}")))?;
        if let Some(s) = &c.provider.session
            && !st.registry.floor().blocks(&s.header)
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
    async fn once(
        &mut self,
        st: &EngineState,
        c: &Candidate<'_>,
        wire: Option<&Arc<Style>>,
        out: upstream::Outgoing,
    ) -> Ended {
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
        let content_type =
            resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(str::to_owned);
        let is_stream =
            content_type.as_deref().is_some_and(|c| c.starts_with("text/event-stream") || c.contains("ndjson"));
        let stall = upstream::stall_timeout(c.endpoint);
        let ok = (200..300).contains(&status);
        if ok {
            let allow = c.provider.forwarding.as_ref().map_or(&[][..], |f| &f.to_client.headers[..]);
            self.forward =
                forwarding::provider_headers(allow, resp.headers(), st.registry.floor(), &st.redactor.current());
        }

        if !ok {
            let indicated = classify::indicated_wait(resp.headers(), SystemTime::now());
            let raw = match self.read_all(resp, stall, st).await {
                Ok(b) => b,
                Err(e) => return e,
            };
            let message = st.redactor.redact(&error_message(&raw)).into_owned();
            let verdict = classify::upstream(status, &String::from_utf8_lossy(&raw));
            return Ended::Failed(Fail {
                status: Some(status),
                verdict,
                reason: message.clone(),
                message,
                indicated,
                after_output: false,
                usage: None,
            });
        }
        if self.req.media.is_some() {
            return self.media_once(st, c, wire, resp, content_type, stall).await;
        }
        if self.req.count {
            return self.count_once(wire, &c.endpoint.errors.body, resp, stall, st).await;
        }
        let Some(wire) = wire else {
            return Ended::Failed(Fail::transport(
                ErrorClass::InBand,
                "0router: a text endpoint with no wire".into(),
                false,
            ));
        };
        if !is_stream {
            let raw = match self.read_all(resp, stall, st).await {
                Ok(b) => b,
                Err(e) => return e,
            };
            let in_band = |reason: String| Ended::Failed(Fail::transport(ErrorClass::InBand, reason, false));
            let value: Value = match serde_json::from_slice(&raw) {
                Ok(v) => v,
                Err(e) => return in_band(format!("answered non-JSON: {e}")),
            };
            if let Some(ib) = inband::body(&c.endpoint.errors.body, &value) {
                return Ended::Failed(inband_fail(ib, &String::from_utf8_lossy(&raw), false, st));
            }
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
                // The channel stays open: it closes when `run` has the close ack.
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
            let (raw, answer) = self.adapt_whole(raw, answer, &value).await;
            self.answer(Answer::Whole { status, content_type, raw, answer: Box::new(answer) });
            return Ended::Ok(usage);
        }

        let frames = !self.segmented
            && self.req.stream
            && c.same_style(&self.req.client.id)
            && wire.text().is_ok_and(|t| t.framing != Framing::JsonArray);
        let relay = Relay { frames, drop_usage: frames && usage_unasked(&self.req.client, &self.req.body) };
        self.commit();
        self.pump(resp, wire, &c.endpoint.errors.stream, relay, stall, st).await
    }

    /// A non-text answer with a 2xx status: a job, the provider's bytes relayed as they
    /// arrive, or a decoded value.
    async fn media_once(
        &mut self,
        st: &EngineState,
        c: &Candidate<'_>,
        wire: Option<&Arc<Style>>,
        resp: reqwest::Response,
        content_type: Option<String>,
        stall: Duration,
    ) -> Ended {
        let in_band = |reason: String| Ended::Failed(Fail::transport(ErrorClass::InBand, reason, false));
        let Some(m) = &self.req.media else { return in_band("0router: not a non-text request".into()) };
        let (ty, job, client_binary) = (m.ty, m.job, m.codec.binary_response().is_some());
        let wire_codec = match wire.map(|w| w.type_codec(ty)) {
            None => None,
            Some(Ok(codec)) => Some(codec.clone()),
            Some(Err(e)) => return in_band(format!("0router: {e}")),
        };
        let map = match mapping(c.endpoint) {
            Ok(m) => m,
            Err(e) => return in_band(e),
        };
        let upstream_binary = wire_codec.as_ref().map_or(map.is_empty(), |w| w.binary_response().is_some());
        let json = content_type.as_deref().is_some_and(|c| c.contains("json"));

        if job {
            let raw = match self.read_all(resp, stall, st).await {
                Ok(b) => b,
                Err(e) => return e,
            };
            let (Some(codec), Some(wire)) = (&wire_codec, wire) else {
                return in_band("0router: an endpoint without a wire can't run jobs".into());
            };
            let Ok(v) = serde_json::from_slice::<Value>(&raw) else {
                return in_band("the job answer isn't JSON".into());
            };
            if let Some(ib) = inband::body(&c.endpoint.errors.body, &v) {
                return Ended::Failed(inband_fail(ib, &String::from_utf8_lossy(&raw), false, st));
            }
            // An endpoint's declared job mapping reads the answer, else the wire's job shape.
            let decoded = match &c.endpoint.job {
                Some(m) => crate::jobs::decode_mapped(m, &v, true).map(|j| (j.bindings, j.status)),
                None => codec.decode_job(&v).ok_or_else(|| "the job answer doesn't have the wire's shape".to_owned()),
            };
            let (bindings, status) = match decoded {
                Ok(d) => d,
                Err(e) => return in_band(e),
            };
            let Some(upstream_id) = bindings.str("job.id").map(str::to_owned) else {
                return in_band("the job answer has no id".into());
            };
            let url = upstream::endpoint_url(&c.endpoint.url, &c.upstream_id, None)
                .map_or_else(|_| c.endpoint.url.clone(), |u| u.to_string());
            let nullrouter_job_id = self.engine.jobs.insert(Job {
                record: self.req.id.clone(),
                provider: c.provider.id.clone(),
                account: c.account.map(|a| a.name.clone()),
                url,
                wire: wire.id.clone(),
                model: c.upstream_id.clone(),
                upstream_id: upstream_id.clone(),
                target: self.req.target.clone(),
                agent: self.req.agent.key.clone(),
                content_url: None,
            });
            let job = JobRef { nullrouter_job_id: nullrouter_job_id.clone(), upstream_id };
            self.engine.records.update(&self.req.id, |r| r.job = Some(job));
            let id = nullrouter_job_id;
            self.ttft();
            self.answer(Answer::Media(MediaAnswer::Job { id, status, bindings }));
            return Ended::Ok(None);
        }

        if client_binary && upstream_binary && !json {
            let (tx, rx) = mpsc::channel(CHANNEL);
            let content_type = content_type.unwrap_or_else(|| "application/octet-stream".into());
            self.ttft();
            self.answer(Answer::Media(MediaAnswer::Bytes { content_type, rx }));
            // Committed: a break from here on reaches the client as a cut body.
            let mut resp = resp;
            loop {
                let chunk = tokio::select! {
                    _ = self.req.cancel.cancelled() => return Ended::Cancelled,
                    c = time::timeout(stall, resp.chunk()) => c,
                };
                let reason = match chunk {
                    Ok(Ok(Some(b))) => {
                        let sent = tokio::select! {
                            _ = self.req.cancel.cancelled() => return Ended::Cancelled,
                            r = tx.send(Ok(b)) => r.is_ok(),
                        };
                        if !sent {
                            return Ended::Cancelled;
                        }
                        continue;
                    }
                    Ok(Ok(None)) => return Ended::Ok(None),
                    Err(_) => (ErrorClass::Stall, format!("no byte for {} ms", stall.as_millis())),
                    Ok(Err(e)) => {
                        (ErrorClass::Network, format!("stream read failed: {}", st.redactor.redact(&e.to_string())))
                    }
                };
                let _ = tx.send(Err(reason.1.clone())).await;
                return Ended::Failed(Fail::transport(reason.0, reason.1, true));
            }
        }

        let raw = match self.read_all(resp, stall, st).await {
            Ok(b) => b,
            Err(e) => return e,
        };
        if json
            && let Ok(v) = serde_json::from_slice::<Value>(&raw)
            && let Some(ib) = inband::body(&c.endpoint.errors.body, &v)
        {
            return Ended::Failed(inband_fail(ib, &String::from_utf8_lossy(&raw), false, st));
        }
        let value = match &wire_codec {
            Some(codec) => codec.decode_response(&raw, content_type.as_deref()),
            None => types::decode_mapped(&map, &raw, content_type.as_deref()),
        };
        let value = match value {
            Ok(v) => v,
            Err(e) => return in_band(format!("0router: {e}")),
        };
        let usage = media_usage(&value);
        self.ttft();
        self.answer(Answer::Media(MediaAnswer::Value(value)));
        Ended::Ok(usage)
    }

    /// A count answer with a 2xx status, read with the wire's `[text.count_tokens]`.
    async fn count_once(
        &mut self,
        wire: Option<&Arc<Style>>,
        errors: &[ErrorRule],
        resp: reqwest::Response,
        stall: Duration,
        st: &EngineState,
    ) -> Ended {
        let in_band = |reason: String| Ended::Failed(Fail::transport(ErrorClass::InBand, reason, false));
        let raw = match self.read_all(resp, stall, st).await {
            Ok(b) => b,
            Err(e) => return e,
        };
        let Some(t) = wire.and_then(|w| w.text().ok()).and_then(|t| t.count_response.as_ref()) else {
            return in_band("0router: the wire has no count_tokens response".into());
        };
        let v = serde_json::from_slice::<Value>(&raw).ok();
        if let Some(ib) = v.as_ref().and_then(|v| inband::body(errors, v)) {
            return Ended::Failed(inband_fail(ib, &String::from_utf8_lossy(&raw), false, st));
        }
        let n =
            v.as_ref().and_then(|v| nullrouter_wire::template::match_value(t, v)).and_then(|b| b.u64("count.input"));
        let Some(n) = n else { return in_band("the count answer doesn't have the wire's shape".into()) };
        self.ttft();
        self.answer(Answer::Count { input_tokens: n, estimated: false });
        Ended::Ok(Some(count_usage(n, false)))
    }

    /// Answers a count with 9router's estimate, on the request in the Messages shape, for
    /// `c`, whose endpoint declares no counting. `false`: the request can't be estimated.
    fn estimate(&mut self, st: &EngineState, c: &Candidate<'_>, kind: AttemptKind) -> bool {
        const MESSAGES: &str = "anthropic-messages";
        let n = if self.req.client.id == MESSAGES {
            Some(estimate::messages_body(&self.req.body))
        } else {
            st.style(MESSAGES).and_then(|m| estimate::estimate(&self.req.ir, m, &self.req.client.id).ok())
        };
        let Some(n) = n else { return false };
        self.start_attempt(c, kind, Vec::new(), Vec::new());
        self.succeed(c, Some(count_usage(n, true)));
        self.answer(Answer::Count { input_tokens: n, estimated: true });
        true
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
    async fn pump(
        &mut self,
        mut resp: reqwest::Response,
        wire: &Arc<Style>,
        errors: &[ErrorRule],
        relay: Relay,
        stall: Duration,
        st: &EngineState,
    ) -> Ended {
        let Ok(t) = wire.text() else {
            return Ended::Failed(Fail::transport(
                ErrorClass::InBand,
                "0router: the wire has no text section".into(),
                false,
            ));
        };
        let mut framer = Framer::new(t.framing);
        let Ok(mut reader) = StreamReader::new(wire) else {
            return Ended::Failed(Fail::transport(
                ErrorClass::InBand,
                "0router: the wire's stream can't be read".into(),
                false,
            ));
        };
        let mut usage = ir::Usage::default();
        let mut failed: Option<ErrorEvent> = None;
        let mut held: Vec<(Piece, Vec<Event>)> = Vec::new();
        let mut output = false;
        // A continuation's first text block merges into the one the client has open.
        let mut merge = self.broken.as_ref().is_some_and(|b| b.resume == Some(Resume::Continue))
            && self.seen.open == breaks::Open::Text;
        let cut = |u: &ir::Usage| (!u.is_empty()).then(|| Usage::reported(u, t.usage.semantics));
        loop {
            let chunk = tokio::select! {
                _ = self.req.cancel.cancelled() => return Ended::Cancelled,
                c = time::timeout(stall, resp.chunk()) => c,
            };
            let (frames, eof) = match chunk {
                Err(_) => {
                    let reason = format!("no byte for {} ms", stall.as_millis());
                    return Ended::Failed(Fail {
                        usage: cut(&usage),
                        ..Fail::transport(ErrorClass::Stall, reason, output)
                    });
                }
                Ok(Err(e)) => {
                    let reason = format!("stream read failed: {}", st.redactor.redact(&e.to_string()));
                    return Ended::Failed(Fail {
                        usage: cut(&usage),
                        ..Fail::transport(ErrorClass::Network, reason, output)
                    });
                }
                Ok(Ok(Some(b))) => (framer.feed(&b), false),
                Ok(Ok(None)) => (framer.finish(), true),
            };
            // Each piece with the events it carries.
            let mut items: Vec<(Option<Piece>, Vec<Event>)> = Vec::new();
            for f in frames {
                // A frame the endpoint declares an error is never relayed (FR-024).
                if let Some(ib) = inband::frame(errors, f.event.as_deref(), &f.data) {
                    if !output {
                        return Ended::Failed(inband_fail(ib, &f.data, false, st));
                    }
                    failed = Some(ErrorEvent { status: Some(ib.status), kind: None, message: ib.message, raw: None });
                    continue;
                }
                // A frame the wire's templates can't read carries no events; relayed
                // unchanged, it still reaches a native client.
                let events = reader.read(&f).unwrap_or_default();
                if relay.frames {
                    let only_usage = !events.is_empty() && events.iter().all(|e| matches!(e, Event::Usage(_)));
                    let piece = (!(relay.drop_usage && only_usage)).then(|| Piece::Frame(f, events.clone()));
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
                if merge && let Some(first) = events.first().filter(|e| e.is_output()) {
                    merge = false;
                    if matches!(first, Event::BlockStart(ir::BlockKind::Text)) {
                        continue;
                    }
                }
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
                    return Ended::Failed(Fail {
                        status,
                        verdict,
                        reason: message.clone(),
                        message,
                        indicated: None,
                        after_output: false,
                        usage: None,
                    });
                }
                // After content, the provider's error is a break the next attempt resumes.
                let Some(piece) = piece.filter(|_| failed.is_none()) else { continue };
                if !output && !content {
                    held.push((piece, events));
                    continue;
                }
                if !output {
                    output = true;
                    self.ttft();
                    if !self.resume().await {
                        return Ended::Cancelled;
                    }
                    for (p, evs) in std::mem::take(&mut held) {
                        if !self.relay(p, &evs).await {
                            return Ended::Cancelled;
                        }
                    }
                }
                if !self.relay(piece, &events).await {
                    return Ended::Cancelled;
                }
            }
            if eof || reader.saw_done() {
                if let Some(e) = failed.take() {
                    let reason = st.redactor.redact(&e.message).into_owned();
                    let f = Fail {
                        status: e.status,
                        usage: cut(&usage),
                        ..Fail::transport(ErrorClass::InBand, reason, true)
                    };
                    return Ended::Failed(f);
                }
                for (p, evs) in std::mem::take(&mut held) {
                    if !self.relay(p, &evs).await {
                        return Ended::Cancelled;
                    }
                }
                return Ended::Ok((!usage.is_empty()).then(|| Usage::reported(&usage, t.usage.semantics)));
            }
        }
    }

    /// Hands the client its answer, once.
    fn answer(&mut self, answer: Answer) {
        if self.first.is_none() {
            return;
        }
        let reply = Reply { answer, headers: std::mem::take(&mut self.forward), response: self.response.clone() };
        // A stream starts at once. A whole answer waits for the `close` line (see `run`).
        let whole = !matches!(reply.answer, Answer::Events { .. } | Answer::Media(MediaAnswer::Bytes { .. }));
        if whole {
            self.held = Some(reply);
        } else if let Some(first) = self.first.take() {
            let _ = first.send(Ok(reply));
        }
    }

    /// Hands the client a stream, once.
    fn commit(&mut self) {
        if self.tx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel(CHANNEL);
        self.tx = Some(tx);
        self.answer(Answer::Events { rx, forced: !self.req.stream });
    }

    /// Sends one piece of the answer and notes what the client saw.
    async fn relay(&mut self, piece: Piece, events: &[Event]) -> bool {
        if !self.send(piece).await {
            return false;
        }
        for e in events {
            self.seen.see(e);
        }
        true
    }

    /// Applies the pending break's resumption at the new segment's first content: a
    /// restart sends the note and starts a new answer; a continuation just goes on.
    async fn resume(&mut self) -> bool {
        let Some(b) = self.broken.take() else { return true };
        let handling = match b.resume {
            Some(Resume::Restart) => {
                if !self.send(Piece::Restart).await {
                    return false;
                }
                self.seen = Seen::default();
                BreakHandling::Restarted
            }
            Some(Resume::Continue) => BreakHandling::Continued,
            None => return true,
        };
        self.engine.records.update(self.id(), |r| r.break_handling = handling);
        true
    }

    fn resume_with(&mut self, how: Resume) {
        if let Some(b) = &mut self.broken {
            b.resume = Some(how);
        }
    }

    /// Ends a broken answer with the error event (sent by [`Run::run`]).
    fn broke_off(&mut self, c: &Candidate<'_>, reason: String, tried: &[Tried]) -> Failure {
        self.broken = None;
        self.engine.records.update(self.id(), |r| r.break_handling = BreakHandling::ErrorEvent { reason });
        self.end_request(Outcome::Failed, self.carried);
        let summary = format!("0router: the answer from {} broke off", c.provider.id);
        Failure {
            status: 502,
            message: error_body::message(&summary, self.id(), tried),
            retry_after: None,
            tried: tried.to_vec(),
        }
    }

    fn carry(&mut self, u: Option<Usage>) {
        match (&mut self.carried, u) {
            (Some(c), Some(u)) => c.add(&u),
            (c @ None, u) => *c = u,
            (Some(_), None) => {}
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

    fn start_attempt(
        &mut self,
        c: &Candidate<'_>,
        kind: AttemptKind,
        dropped: Vec<Dropped>,
        forced: Vec<(String, Value)>,
    ) {
        self.n += 1;
        self.sent = c.account.map(|a| (c.provider.id.clone(), a.name.clone(), c.upstream_id.clone()));
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
            forced,
            adapter: self.adapter_run.clone(),
            placement: match kind {
                AttemptKind::SameAccountRetry => {
                    self.placing.map(|p| AttemptPlacement { reason: PlacementReason::Retry, rank: p.rank })
                }
                AttemptKind::Continuation | AttemptKind::Restart => None,
                _ => self.placing,
            },
        };
        self.engine.records.update(self.id(), |r| r.attempts.push(a));
        if let (Some(routed), Some(account)) = (&self.routed, c.account) {
            let at = CandidateKey::new(&c.provider.id, &account.name, &c.upstream_id);
            crate::route::start(&self.engine, routed, &at, SystemTime::now());
        }
    }

    /// Ends the running attempt with its provider-reported usage, and tallies it on the
    /// account it was sent through (FR-024). An estimate sent nothing; a count consumes no
    /// tokens.
    fn end_attempt(&self, outcome: AttemptOutcome, usage: Option<Usage>) {
        let at = self.now();
        if let Some((p, a, m)) = &self.sent
            && !usage.is_some_and(|u| u.estimated)
        {
            if self.req.count {
                self.engine.history.tally.attempt_without_tokens(p, a, m);
            } else {
                self.engine.history.tally.attempt(p, a, m, usage.as_ref());
            }
        }
        if let Some(routed) = &self.routed {
            let tokens = usage.as_ref().map_or(0, crate::route::plain_tokens);
            crate::route::finish(
                &self.engine,
                routed,
                matches!(outcome, AttemptOutcome::Ok),
                tokens,
                SystemTime::now(),
            );
        }
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

    /// The request an attempt sends from: the adapter's edited body when it edited, else the
    /// request as received.
    fn source(&self) -> Source<'_> {
        match &self.adapted {
            Some(a) => Source { body: &a.body, ir: a.ir.as_ref().unwrap_or(&self.req.ir) },
            None => Source { body: &self.req.body, ir: &self.req.ir },
        }
    }

    /// Runs the key's harness adapter on the client-style request for candidate `c`, before the
    /// upstream body is built (research R2). A fallback or resume runs it again against the
    /// original body. Edits that no longer decode are dropped, and the run says so.
    async fn adapt(&mut self, st: &EngineState, c: &Candidate<'_>) {
        self.adapted = None;
        self.adapter_run = None;
        self.response = None;
        let Some(runner) = self.engine.runner_for(st, &self.req.agent.key) else { return };
        let runner = runner.with_client(self.req.client.clone());
        let ctx = nullrouter_adapter_kit::Context {
            direction: nullrouter_adapter_kit::Direction::Request,
            provider: c.provider.id.clone(),
            target_style: c.endpoint.wire.clone().unwrap_or_else(|| "custom".into()),
            same_style: c.same_style(&self.req.client.id),
            model: c.upstream_id.clone(),
            model_type: "text".into(),
            capabilities: nullrouter_adapter_kit::Capabilities { vision: Some(c.endpoint.vision), ..Default::default() },
            stream: self.req.stream,
            attempt: self.n + 1,
        };
        if self.req.media.is_none() && !self.req.count && runner.reads_responses() {
            let redactor = st.redactor.clone();
            let clean: Clean = Arc::new(move |s: &str| redactor.redact(&crate::records::plain(s)).into_owned());
            self.response = Some(ResponseSide::new(runner.clone(), ctx.clone(), clean));
        }
        let mut run = if self.req.media.is_some() {
            runner.not_run(&self.req.body, NotRunReason::MediaRequest).run
        } else {
            let out = runner.run_request(&ctx, &self.req.body).await;
            let mut run = out.run;
            if let std::borrow::Cow::Owned(body) = out.body {
                let ir = if ctx.same_style {
                    Ok(None)
                } else {
                    nullrouter_wire::codec::request::decode(&self.req.client, &body).map(Some)
                };
                match ir {
                    Ok(ir) => self.adapted = Some(Adapted { body, ir }),
                    Err(_) => {
                        run.outcome = AdapterOutcome::Failed {
                            reason: FailReason::InvalidOutput { rule: InvalidOutputRule::Undecodable },
                        };
                        run.changes.clear();
                    }
                }
            }
            run
        };
        run.clean_with(|s| st.redactor.redact(&crate::records::plain(s)).into_owned());
        self.adapter_run = Some(run);
    }

    /// Runs the adapter's response side on a whole client-style answer and records it. An
    /// edited answer goes out as the adapter left it; otherwise the provider's bytes stand.
    async fn adapt_whole(&self, raw: Bytes, answer: ForClient, value: &Value) -> (Bytes, ForClient) {
        let Some(side) = &self.response else { return (raw, answer) };
        let client = match &answer {
            ForClient::AsReceived { .. } => value,
            ForClient::Rebuilt { body, .. } => body,
        };
        let (edited, run) = side.whole(client).await;
        self.engine.records.update(&self.req.id, |r| r.response_adapter = Some(run));
        match (edited, answer) {
            (Some(body), ForClient::AsReceived { read }) => (Bytes::from(body.to_string()), ForClient::AsReceived { read }),
            (Some(body), ForClient::Rebuilt { read, .. }) => (raw, ForClient::Rebuilt { read, body }),
            (None, answer) => (raw, answer),
        }
    }

    /// A plan entry that can't be tried: a `skipped` attempt and an attempt line.
    fn skip(
        &mut self,
        provider: &str,
        account: Option<String>,
        model: &str,
        reason: &str,
        class: Option<ErrorClass>,
        tried: &mut Vec<Tried>,
    ) {
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
            outcome: Some(AttemptOutcome::Skipped { reason: reason.to_owned(), class }),
            usage: None,
            dropped: Vec::new(),
            forced: Vec::new(),
            placement: None,
            adapter: None,
        };
        self.engine.records.update(self.id(), |r| r.attempts.push(a));
        tried.push(Tried {
            provider: provider.to_owned(),
            account,
            model: model.to_owned(),
            status: None,
            class: class.and_then(class_name),
            reason: reason.to_owned(),
            retries: 0,
        });
    }

    /// The answer completed: the record, the cooldown and the warm account.
    fn succeed(&self, c: &Candidate<'_>, usage: Option<Usage>) {
        self.end_attempt(AttemptOutcome::Ok, usage);
        // A resumed answer: the cut segments count too.
        let usage = match (self.carried, usage) {
            (Some(mut total), Some(u)) => {
                total.add(&u);
                Some(total)
            }
            (carried, u) => u.or(carried),
        };
        let account = c.account.map(|a| a.name.clone());
        let served =
            ServedBy { provider: c.provider.id.clone(), account: account.clone(), model: c.upstream_id.clone() };
        self.engine.records.update(self.id(), |r| r.served_by = Some(served));
        if self.req.media.as_ref().is_some_and(|m| m.job) {
            // In progress until the job fails or its content is delivered.
            self.engine.records.update(self.id(), |r| r.usage = usage);
        } else {
            self.end_request(Outcome::Succeeded, usage);
        }
        let (p, a, m) = cooldown_key(c);
        self.engine.cooldowns.succeed(p, a, m);
        if let Some(routed) = &self.routed {
            let at = CandidateKey::new(&c.provider.id, account.as_deref().unwrap_or(""), &c.upstream_id);
            let cache = crate::route::cache_of(c.provider, c.account);
            crate::route::learn(
                &self.engine,
                &self.req.agent.key,
                routed,
                &at,
                cache,
                usage.as_ref(),
                SystemTime::now(),
            );
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
            Piece::Restart => w = StreamWriter::new(client, body, "", "", 0).expect("compiled above"),
            Piece::Frame(..) => {}
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
    fn forced_parameters_touch_only_their_paths() {
        let v = |s: &str| toml::Value::try_from(serde_json::from_str::<Value>(s).unwrap()).unwrap();
        let (store, summary, effort, include) = (v("false"), v("\"concise\""), v("\"high\""), v("[\"a\", \"b\"]"));
        let mut body = serde_json::json!({"model": "m", "reasoning": null, "include": ["b", "c"], "input": "hi"});
        let forced = write_forced(
            &mut body,
            [
                (ForcedParam::Store, &store),
                (ForcedParam::ReasoningSummary, &summary),
                (ForcedParam::ReasoningEffort, &effort),
                (ForcedParam::Include, &include),
            ],
        );
        assert_eq!(
            body.to_string(),
            r#"{"model":"m","reasoning":{"summary":"concise","effort":"high"},"include":["b","c","a"],"input":"hi","store":false}"#
        );
        let names: Vec<&str> = forced.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["store", "reasoning.summary", "reasoning.effort", "include"]);
        assert_eq!(forced[3].1, serde_json::json!(["a", "b"]), "recorded as declared");
        let mut list = serde_json::json!([1]);
        assert!(write_forced(&mut list, [(ForcedParam::Store, &store)]).is_empty(), "not an object: untouched");
    }

    #[test]
    fn the_session_input_is_the_key_then_the_session() {
        assert_eq!(session_input(&AgentId::new("ak_1", None)), "ak_1");
        assert_eq!(session_input(&AgentId::new("ak_1", Some("sess-9"))), "ak_1:sess-9");
    }
}

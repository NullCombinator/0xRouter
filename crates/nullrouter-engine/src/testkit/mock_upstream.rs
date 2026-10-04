//! A scripted upstream on `127.0.0.1:0`. Each request takes the next [`Step`] queued for
//! its path (longest matching prefix), else the next default step, else the responder's
//! answer, else a 404.
//!
//! It records every request, counts accepted TCP connections and notes when a client
//! drops a streamed body before its end.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::Response;
use axum::serve::ListenerExt;
use futures_util::stream;
use tokio::task::JoinHandle;

/// One scripted reply.
#[derive(Debug, Clone)]
pub enum Step {
    /// A whole response.
    Reply { status: u16, headers: Vec<(String, String)>, body: Bytes },
    /// A streamed 200-ish response: `frames` sent `every` apart. With `cut`, the
    /// connection drops after the last frame instead of ending cleanly.
    Stream { status: u16, headers: Vec<(String, String)>, frames: Vec<Bytes>, every: Duration, cut: bool },
    /// Headers, then `frames`, then no more bytes for `hold` (a stall).
    StallAfter { frames: Vec<Bytes>, hold: Duration },
    /// No response headers for `hold`.
    StallHeaders { hold: Duration },
}

impl Step {
    pub fn json(status: u16, body: serde_json::Value) -> Self {
        Self::Reply {
            status,
            headers: vec![("content-type".into(), "application/json".into())],
            body: body.to_string().into(),
        }
    }

    /// A `429` with `retry-after: <secs>`.
    pub fn rate_limited(secs: u64, body: serde_json::Value) -> Self {
        let Self::Reply { status, mut headers, body } = Self::json(429, body) else { unreachable!() };
        headers.push(("retry-after".into(), secs.to_string()));
        Self::Reply { status, headers, body }
    }

    /// A `200` whose body is an error (in-band).
    pub fn in_band_error(body: serde_json::Value) -> Self {
        Self::json(200, body)
    }

    pub fn binary(content_type: &str, bytes: impl Into<Bytes>) -> Self {
        Self::Reply { status: 200, headers: vec![("content-type".into(), content_type.into())], body: bytes.into() }
    }

    /// `202 Accepted` with a job body; queue the poll replies after it.
    pub fn accepted(body: serde_json::Value) -> Self {
        Self::json(202, body)
    }

    /// An SSE stream of `data:` lines (each `v` serialised), optionally named events.
    pub fn sse(events: &[(Option<&str>, serde_json::Value)], done: bool) -> Self {
        let mut frames: Vec<Bytes> = events
            .iter()
            .map(|(name, v)| match name {
                Some(n) => format!("event: {n}\ndata: {v}\n\n").into(),
                None => format!("data: {v}\n\n").into(),
            })
            .collect();
        if done {
            frames.push(Bytes::from_static(b"data: [DONE]\n\n"));
        }
        Self::Stream {
            status: 200,
            headers: vec![("content-type".into(), "text/event-stream".into())],
            frames,
            every: Duration::ZERO,
            cut: false,
        }
    }

    /// The same reply with one more response header.
    pub fn with_header(self, name: &str, value: &str) -> Self {
        match self {
            Self::Reply { status, mut headers, body } => {
                headers.push((name.into(), value.into()));
                Self::Reply { status, headers, body }
            }
            Self::Stream { status, mut headers, frames, every, cut } => {
                headers.push((name.into(), value.into()));
                Self::Stream { status, headers, frames, every, cut }
            }
            other => other,
        }
    }

    /// The same stream, dropped after its first `n` frames.
    pub fn cut_after(self, n: usize) -> Self {
        match self {
            Self::Stream { status, headers, mut frames, every, .. } => {
                frames.truncate(n);
                Self::Stream { status, headers, frames, every, cut: true }
            }
            other => other,
        }
    }
}

/// A request the mock received.
#[derive(Debug, Clone)]
pub struct Received {
    pub method: Method,
    pub path_and_query: String,
    pub headers: HeaderMap,
    pub body: Bytes,
    pub at: Instant,
}

impl Received {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
    }
}

/// Answers any request no queued step is left for.
pub type Responder = Arc<dyn Fn(&Received) -> Step + Send + Sync>;

#[derive(Default)]
struct Inner {
    by_path: Mutex<Vec<(String, VecDeque<Step>)>>,
    default: Mutex<VecDeque<Step>>,
    responder: Mutex<Option<Responder>>,
    received: Mutex<Vec<Received>>,
    connections: AtomicUsize,
    disconnects: Mutex<Vec<Instant>>,
}

pub struct MockUpstream {
    addr: SocketAddr,
    inner: Arc<Inner>,
    task: JoinHandle<()>,
}

impl Drop for MockUpstream {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl MockUpstream {
    pub async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let inner = Arc::new(Inner::default());
        let counted = inner.clone();
        let listener = listener.tap_io(move |_| {
            counted.connections.fetch_add(1, Ordering::SeqCst);
        });
        let app = axum::Router::new().fallback(handle).with_state(inner.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self { addr, inner, task }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// `http://127.0.0.1:<port><path>`.
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    /// Queues steps for any path.
    pub fn push(&self, steps: impl IntoIterator<Item = Step>) {
        lock(&self.inner.default).extend(steps);
    }

    /// Queues steps for paths starting with `prefix`.
    pub fn on(&self, prefix: &str, steps: impl IntoIterator<Item = Step>) {
        let mut by = lock(&self.inner.by_path);
        match by.iter_mut().find(|(p, _)| p == prefix) {
            Some((_, q)) => q.extend(steps),
            None => {
                by.push((prefix.to_owned(), steps.into_iter().collect()));
                by.sort_by_key(|(p, _)| std::cmp::Reverse(p.len()));
            }
        }
    }

    /// Answers every request that no queued step is left for, however many arrive and in
    /// whatever order (real clients).
    pub fn respond(&self, f: impl Fn(&Received) -> Step + Send + Sync + 'static) {
        *lock(&self.inner.responder) = Some(Arc::new(f));
    }

    pub fn received(&self) -> Vec<Received> {
        lock(&self.inner.received).clone()
    }

    pub fn connections(&self) -> usize {
        self.inner.connections.load(Ordering::SeqCst)
    }

    /// When clients dropped streamed bodies before their end.
    pub fn disconnects(&self) -> Vec<Instant> {
        lock(&self.inner.disconnects).clone()
    }
}

fn next_step(inner: &Inner, path: &str) -> Option<Step> {
    let mut by = lock(&inner.by_path);
    if let Some((_, q)) = by.iter_mut().find(|(p, q)| path.starts_with(p.as_str()) && !q.is_empty()) {
        return q.pop_front();
    }
    drop(by);
    lock(&inner.default).pop_front()
}

fn head(status: u16, headers: &[(String, String)]) -> axum::http::response::Builder {
    let mut b = Response::builder().status(StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR));
    for (k, v) in headers {
        if let (Ok(k), Ok(v)) = (HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(v)) {
            b = b.header(k, v);
        }
    }
    b
}

/// Notes a disconnect if dropped before `finished`.
struct Watch {
    inner: Arc<Inner>,
    finished: bool,
}

impl Watch {
    fn finish(mut self) {
        self.finished = true;
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        if !self.finished {
            lock(&self.inner.disconnects).push(Instant::now());
        }
    }
}

async fn handle(State(inner): State<Arc<Inner>>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let body = axum::body::to_bytes(body, usize::MAX).await.unwrap_or_default();
    let path_and_query = parts.uri.path_and_query().map_or_else(|| parts.uri.path().to_owned(), |p| p.to_string());
    let received = Received {
        method: parts.method,
        path_and_query: path_and_query.clone(),
        headers: parts.headers,
        body,
        at: Instant::now(),
    };
    let responder = lock(&inner.responder).clone();
    let step = next_step(&inner, parts.uri.path()).or_else(|| responder.map(|f| f(&received)));
    lock(&inner.received).push(received);
    let Some(step) = step else {
        return head(404, &[])
            .body(Body::from(format!("mock upstream: no step scripted for {path_and_query}")))
            .expect("response");
    };
    match step {
        Step::Reply { status, headers, body } => head(status, &headers).body(Body::from(body)).expect("response"),
        Step::StallHeaders { hold } => {
            tokio::time::sleep(hold).await;
            head(504, &[]).body(Body::empty()).expect("response")
        }
        Step::Stream { status, headers, frames, every, cut } => {
            let watch = Watch { inner: inner.clone(), finished: false };
            let s =
                stream::unfold((frames.into_iter(), watch, false), move |(mut it, mut watch, started)| async move {
                    if started && !every.is_zero() {
                        tokio::time::sleep(every).await;
                    }
                    match it.next() {
                        Some(f) => Some((Ok::<Bytes, std::io::Error>(f), (it, watch, true))),
                        None if cut => {
                            watch.finished = true;
                            Some((
                                Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "scripted cut")),
                                (it, watch, true),
                            ))
                        }
                        None => {
                            watch.finish();
                            None
                        }
                    }
                });
            head(status, &headers).body(Body::from_stream(s)).expect("response")
        }
        Step::StallAfter { frames, hold } => {
            let watch = Watch { inner: inner.clone(), finished: false };
            let s = stream::unfold((frames.into_iter(), watch), move |(mut it, watch)| async move {
                match it.next() {
                    Some(f) => Some((Ok::<Bytes, std::io::Error>(f), (it, watch))),
                    None => {
                        tokio::time::sleep(hold).await;
                        watch.finish();
                        None
                    }
                }
            });
            head(200, &[("content-type".into(), "text/event-stream".into())])
                .body(Body::from_stream(s))
                .expect("response")
        }
    }
}

/// What one account has been served so far, in the mock's token estimate (4 bytes a token).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Served {
    pub requests: u64,
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
}

/// A prompt cache and rate limits over a [`MockUpstream`] (spec 006, T016).
///
/// Install with [`MockUpstream::simulate_cache`]. Accounts are told apart by the bearer
/// token (`authorization: Bearer` or `x-api-key`) registered with [`account`](Self::account).
/// Per `(account, model)` it remembers the prompt prefixes it was sent, by content hash and
/// in the mock only, and answers with the usage a real provider would report: `cache_read`
/// for the longest prefix it already holds within the lifetime, `cache_write` for the newly
/// cached rest, `input` for the part past the last cache boundary.
///
/// - **Boundaries.** Where a Messages-style body marks blocks with `cache_control`, only
///   those blocks end a cacheable prefix. Otherwise every message or block does (automatic
///   caching).
/// - **Time** is the sim's own: tests move it with [`set_now`](Self::set_now), so a week
///   of traffic runs without waiting and the lifetime follows it.
/// - **Rate limits**, scripted per account: [`rate_limit`](Self::rate_limit).
#[derive(Clone)]
pub struct CacheSim {
    inner: Arc<Mutex<SimState>>,
}

type ServedListener = Arc<dyn Fn(&str, &Served) + Send + Sync>;

struct SimState {
    lifetime: Duration,
    output_tokens: u64,
    now: Duration,
    tokens: Vec<(String, String)>,
    /// `(label, model)` → cumulative prefix hash → (tokens, last used).
    cache: std::collections::HashMap<(String, String), std::collections::HashMap<u64, (u64, Duration)>>,
    served: std::collections::BTreeMap<String, Served>,
    limits: Vec<RateLimit>,
    listeners: Vec<ServedListener>,
}

struct RateLimit {
    label: String,
    /// Counts requests of the account, 1-based, from this one on.
    from_nth: u64,
    from: Option<Duration>,
    duration: Duration,
}

impl CacheSim {
    pub fn new(lifetime: Duration) -> Self {
        Self {
            inner: Arc::new(Mutex::new(SimState {
                lifetime,
                output_tokens: 10,
                now: Duration::ZERO,
                tokens: Vec::new(),
                cache: Default::default(),
                served: Default::default(),
                limits: Vec::new(),
                listeners: Vec::new(),
            })),
        }
    }

    /// Requests carrying `secret` belong to the account named `label`.
    pub fn account(&self, secret: &str, label: &str) -> &Self {
        lock(&self.inner).tokens.push((secret.to_owned(), label.to_owned()));
        self
    }

    pub fn set_lifetime(&self, lifetime: Duration) {
        lock(&self.inner).lifetime = lifetime;
    }

    /// Output tokens reported for every reply.
    pub fn set_output_tokens(&self, n: u64) {
        lock(&self.inner).output_tokens = n;
    }

    /// The sim's clock: where cache lifetimes and rate limits are measured.
    pub fn set_now(&self, now: Duration) {
        lock(&self.inner).now = now;
    }

    pub fn advance(&self, by: Duration) {
        lock(&self.inner).now += by;
    }

    /// Answers `429` for `duration` (sim time) from the account's `nth` request on. The window
    /// opens at the first request that falls in it.
    pub fn rate_limit(&self, label: &str, nth: u64, duration: Duration) {
        lock(&self.inner).limits.push(RateLimit { label: label.to_owned(), from_nth: nth, from: None, duration });
    }

    /// Told of every served request with its usage (the mock quota follows this).
    pub fn on_served(&self, f: impl Fn(&str, &Served) + Send + Sync + 'static) {
        lock(&self.inner).listeners.push(Arc::new(f));
    }

    pub fn served(&self, label: &str) -> Served {
        lock(&self.inner).served.get(label).copied().unwrap_or_default()
    }

    pub fn requests(&self, label: &str) -> u64 {
        self.served(label).requests
    }

    pub fn cache_reads(&self, label: &str) -> u64 {
        self.served(label).cache_read
    }

    fn answer(&self, r: &Received) -> Step {
        let secret = r
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.strip_prefix("Bearer ").unwrap_or(v).to_owned())
            .or_else(|| r.headers.get("x-api-key").and_then(|v| v.to_str().ok()).map(str::to_owned))
            .unwrap_or_default();
        let body = r.json();
        let model = body.get("model").and_then(|m| m.as_str()).unwrap_or_default().to_owned();
        let anthropic = r.path_and_query.contains("/messages");
        let mut st = lock(&self.inner);
        let label = st.tokens.iter().find(|(t, _)| *t == secret).map_or_else(|| "unknown".to_owned(), |(_, l)| l.clone());
        let count = st.served.entry(label.clone()).or_default();
        count.requests += 1;
        let nth = count.requests;
        let now = st.now;
        if let Some(l) = st.limits.iter_mut().find(|l| {
            l.label == label && nth >= l.from_nth && l.from.is_none_or(|from| now < from + l.duration)
        }) {
            let from = *l.from.get_or_insert(now);
            let left = (from + l.duration).saturating_sub(now).as_secs().max(1);
            return Step::rate_limited(left, serde_json::json!({ "error": { "message": "rate limited" } }));
        }
        let segments = segments(&body);
        let explicit = segments.iter().any(|s| s.marker);
        let mut total = 0u64;
        let mut cumulative = Vec::with_capacity(segments.len());
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for s in &segments {
            use std::hash::Hash;
            s.text.hash(&mut hasher);
            total += s.tokens;
            cumulative.push((std::hash::Hasher::finish(&hasher), total, !explicit || s.marker));
        }
        let lifetime = st.lifetime;
        let slot = st.cache.entry((label.clone(), model)).or_default();
        slot.retain(|_, (_, at)| now.saturating_sub(*at) <= lifetime);
        let read = cumulative.iter().rev().find(|(h, ..)| slot.contains_key(h)).map_or(0, |(_, t, _)| *t);
        let boundary = cumulative.iter().rev().find(|(.., b)| *b).map_or(0, |(_, t, _)| *t);
        for (h, t, is_boundary) in &cumulative {
            // A hit refreshes the prefixes it covers; new boundaries are written.
            if (*is_boundary && *t <= boundary) || *t <= read {
                slot.insert(*h, (*t, now));
            }
        }
        let write = boundary.saturating_sub(read);
        let input = total.saturating_sub(read + write);
        let output = st.output_tokens;
        let delta = Served { requests: 1, input, cache_read: read, cache_write: write, output };
        let totals = st.served.get_mut(&label).expect("counted above");
        totals.input += input;
        totals.cache_read += read;
        totals.cache_write += write;
        totals.output += output;
        let listeners = st.listeners.clone();
        drop(st);
        for f in listeners {
            f(&label, &delta);
        }
        let streaming = body.get("stream").and_then(|s| s.as_bool()).unwrap_or(false);
        reply(anthropic, streaming, &delta)
    }
}

struct Segment {
    text: String,
    tokens: u64,
    marker: bool,
}

/// The request's prompt as ordered pieces: tools, system, instructions, then messages (one
/// piece per content block).
fn segments(body: &serde_json::Value) -> Vec<Segment> {
    use serde_json::Value;
    fn piece(out: &mut Vec<Segment>, v: &Value) {
        let marker = v.get("cache_control").is_some();
        let text = v.get("text").and_then(Value::as_str).map_or_else(|| v.to_string(), str::to_owned);
        out.push(Segment { tokens: (text.len() as u64).div_ceil(4), text, marker });
    }
    fn content(out: &mut Vec<Segment>, v: &Value) {
        match v {
            Value::String(s) => out.push(Segment { tokens: (s.len() as u64).div_ceil(4), text: s.clone(), marker: false }),
            Value::Array(blocks) => blocks.iter().for_each(|b| piece(out, b)),
            Value::Null => {}
            other => piece(out, other),
        }
    }
    let mut out = Vec::new();
    if let Some(Value::Array(tools)) = body.get("tools") {
        tools.iter().for_each(|t| piece(&mut out, t));
    }
    for key in ["system", "instructions"] {
        if let Some(v) = body.get(key) {
            content(&mut out, v);
        }
    }
    for key in ["messages", "input"] {
        match body.get(key) {
            Some(Value::Array(messages)) => {
                for m in messages {
                    match m.get("content") {
                        Some(c) => {
                            let role = m.get("role").and_then(Value::as_str).unwrap_or_default();
                            let first = out.len();
                            content(&mut out, c);
                            if let Some(s) = out.get_mut(first) {
                                s.text = format!("{role}:{}", s.text);
                            }
                        }
                        None => piece(&mut out, m),
                    }
                }
            }
            Some(other) => content(&mut out, other),
            None => {}
        }
    }
    out
}

fn reply(anthropic: bool, streaming: bool, u: &Served) -> Step {
    use serde_json::json;
    if anthropic {
        let usage = json!({
            "input_tokens": u.input,
            "cache_read_input_tokens": u.cache_read,
            "cache_creation_input_tokens": u.cache_write,
            "output_tokens": u.output,
        });
        if streaming {
            return Step::sse(
                &[
                    (Some("message_start"), json!({"type": "message_start", "message": {"id": "msg_sim", "type": "message", "role": "assistant", "content": [], "model": "sim", "usage": usage}})),
                    (Some("content_block_start"), json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}})),
                    (Some("content_block_delta"), json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "ok"}})),
                    (Some("content_block_stop"), json!({"type": "content_block_stop", "index": 0})),
                    (Some("message_delta"), json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": u.output}})),
                    (Some("message_stop"), json!({"type": "message_stop"})),
                ],
                false,
            );
        }
        return Step::json(
            200,
            json!({"id": "msg_sim", "type": "message", "role": "assistant", "model": "sim", "content": [{"type": "text", "text": "ok"}], "stop_reason": "end_turn", "usage": usage}),
        );
    }
    let usage = json!({
        "prompt_tokens": u.input + u.cache_read + u.cache_write,
        "completion_tokens": u.output,
        "total_tokens": u.input + u.cache_read + u.cache_write + u.output,
        "prompt_tokens_details": {"cached_tokens": u.cache_read},
    });
    if streaming {
        return Step::sse(
            &[
                (None, json!({"id": "c1", "object": "chat.completion.chunk", "model": "sim", "choices": [{"index": 0, "delta": {"role": "assistant", "content": "ok"}, "finish_reason": null}]})),
                (None, json!({"id": "c1", "object": "chat.completion.chunk", "model": "sim", "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": usage})),
            ],
            true,
        );
    }
    Step::json(
        200,
        json!({"id": "c1", "object": "chat.completion", "model": "sim", "choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}], "usage": usage}),
    )
}

impl MockUpstream {
    /// Answers every request no step is queued for with [`CacheSim`]'s usage.
    pub fn simulate_cache(&self, sim: &CacheSim) {
        let sim = sim.clone();
        self.respond(move |r| sim.answer(r));
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn scripts_replies_counts_connections_and_sees_disconnects() {
        let m = MockUpstream::start().await;
        m.on(
            "/v1/messages",
            [Step::rate_limited(3, json!({ "error": "slow down" })), Step::json(200, json!({ "ok": true }))],
        );
        let Step::Stream { status, headers, frames, cut, .. } =
            Step::sse(&[(None, json!({ "a": 1 })), (None, json!({ "a": 2 }))], true).cut_after(1)
        else {
            unreachable!()
        };
        m.push([Step::Stream { status, headers, frames, every: Duration::from_millis(20), cut }]);
        let c = reqwest::Client::new();

        let r = c.post(m.url("/v1/messages")).header("x-test", "1").body("{\"q\":1}").send().await.unwrap();
        assert_eq!(r.status(), 429);
        assert_eq!(r.headers()["retry-after"], "3");
        let r = c.post(m.url("/v1/messages")).send().await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&r.bytes().await.unwrap()).unwrap(),
            json!({ "ok": true })
        );
        assert_eq!(m.connections(), 1, "sequential requests reuse one connection");

        let mut r = c.get(m.url("/stream")).send().await.unwrap();
        assert_eq!(&r.chunk().await.unwrap().unwrap()[..], b"data: {\"a\":1}\n\n");
        assert!(r.chunk().await.is_err(), "the scripted cut surfaces as a body error");
        assert_eq!(c.get(m.url("/none")).send().await.unwrap().status(), 404);

        let got = m.received();
        assert_eq!(got.len(), 4);
        assert_eq!(got[0].json(), json!({ "q": 1 }));
        assert_eq!(got[0].headers["x-test"], "1");
        assert_eq!(m.connections(), 2, "a cut connection isn't reused");
    }

    #[tokio::test]
    async fn client_drop_is_observed() {
        let m = MockUpstream::start().await;
        let frames = (0..100).map(|i| Bytes::from(format!("data: {i}\n\n"))).collect();
        m.push([Step::Stream { status: 200, headers: vec![], frames, every: Duration::from_millis(20), cut: false }]);
        let mut r = reqwest::get(m.url("/s")).await.unwrap();
        assert!(r.chunk().await.unwrap().is_some());
        drop(r);
        for _ in 0..50 {
            if !m.disconnects().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(m.disconnects().len(), 1);
    }

    fn sim_body(model: &str, system: &str, turns: &[&str]) -> serde_json::Value {
        let mut messages = vec![json!({ "role": "system", "content": system })];
        for t in turns {
            messages.push(json!({ "role": "user", "content": t }));
        }
        json!({ "model": model, "messages": messages })
    }

    async fn body_json(r: reqwest::Response) -> serde_json::Value {
        serde_json::from_slice(&r.bytes().await.unwrap()).unwrap()
    }

    async fn post(c: &reqwest::Client, m: &MockUpstream, key: &str, body: &serde_json::Value) -> reqwest::Response {
        c.post(m.url("/v1/chat/completions")).bearer_auth(key).body(body.to_string()).send().await.unwrap()
    }

    #[tokio::test]
    async fn the_cache_sim_reads_a_held_prefix_and_forgets_it_past_the_lifetime() {
        let m = MockUpstream::start().await;
        let sim = CacheSim::new(Duration::from_secs(300));
        sim.account("sk-a", "a").account("sk-b", "b");
        m.simulate_cache(&sim);
        let c = reqwest::Client::new();
        let system = "s".repeat(4000);
        let first = sim_body("m", &system, &["one"]);
        assert_eq!(post(&c, &m, "sk-a", &first).await.status(), 200);
        assert_eq!(sim.cache_reads("a"), 0, "nothing held yet");
        let second = sim_body("m", &system, &["one", "two"]);
        let r = body_json(post(&c, &m, "sk-a", &second).await).await;
        assert_eq!(r["usage"]["prompt_tokens_details"]["cached_tokens"], 1001, "system and first turn");
        // Another account holds nothing; another model neither.
        post(&c, &m, "sk-b", &second).await;
        post(&c, &m, "sk-a", &sim_body("other", &system, &["one", "two"])).await;
        assert_eq!((sim.cache_reads("b"), sim.requests("a")), (0, 3));
        // Past the lifetime the prefix is cold again; inside it the clock refreshes it.
        sim.advance(Duration::from_secs(200));
        post(&c, &m, "sk-a", &second).await;
        sim.advance(Duration::from_secs(200));
        let r = body_json(post(&c, &m, "sk-a", &second).await).await;
        assert!(r["usage"]["prompt_tokens_details"]["cached_tokens"].as_u64().unwrap() > 1000, "refreshed by the hit");
        sim.advance(Duration::from_secs(400));
        let r = body_json(post(&c, &m, "sk-a", &second).await).await;
        assert_eq!(r["usage"]["prompt_tokens_details"]["cached_tokens"], 0, "idle past the lifetime");
    }

    #[tokio::test]
    async fn explicit_markers_cache_only_up_to_the_last_marker() {
        let m = MockUpstream::start().await;
        let sim = CacheSim::new(Duration::from_secs(300));
        sim.account("sk-a", "a");
        m.simulate_cache(&sim);
        let c = reqwest::Client::new();
        let body = |tail: &str| {
            json!({
                "model": "m",
                "system": [{ "type": "text", "text": "x".repeat(400), "cache_control": { "type": "ephemeral" } }],
                "messages": [{ "role": "user", "content": [{ "type": "text", "text": tail }] }],
            })
        };
        let post = |b: serde_json::Value| {
            let c = c.clone();
            let url = m.url("/v1/messages");
            async move {
                let r = c.post(url).header("x-api-key", "sk-a").body(b.to_string()).send().await.unwrap();
                serde_json::from_slice::<serde_json::Value>(&r.bytes().await.unwrap()).unwrap()
            }
        };
        let r = post(body("first question")).await;
        assert_eq!(r["usage"]["cache_creation_input_tokens"], 100, "up to the marker");
        assert_eq!(r["usage"]["cache_read_input_tokens"], 0);
        let r = post(body("another question")).await;
        assert_eq!(r["usage"]["cache_read_input_tokens"], 100);
        assert_eq!(r["usage"]["cache_creation_input_tokens"], 0);
        assert!(r["usage"]["input_tokens"].as_u64().unwrap() > 0, "the unmarked tail is plain input");
    }

    #[tokio::test]
    async fn scripted_limits_start_at_the_nth_request_and_end_on_the_sim_clock() {
        let m = MockUpstream::start().await;
        let sim = CacheSim::new(Duration::from_secs(300));
        sim.account("sk-a", "a").account("sk-b", "b");
        sim.rate_limit("a", 2, Duration::from_secs(60));
        m.simulate_cache(&sim);
        let c = reqwest::Client::new();
        let body = sim_body("m", "sys", &["q"]);
        assert_eq!(post(&c, &m, "sk-a", &body).await.status(), 200);
        let limited = post(&c, &m, "sk-a", &body).await;
        assert_eq!(limited.status(), 429);
        assert_eq!(limited.headers()["retry-after"], "60");
        assert_eq!(post(&c, &m, "sk-b", &body).await.status(), 200, "only that account");
        sim.advance(Duration::from_secs(61));
        assert_eq!(post(&c, &m, "sk-a", &body).await.status(), 200);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        sim.on_served(move |label, d| lock(&log).push((label.to_owned(), d.output)));
        post(&c, &m, "sk-b", &body).await;
        assert_eq!(*lock(&seen), [("b".to_owned(), 10)]);
    }
}

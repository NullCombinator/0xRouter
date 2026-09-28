//! A scripted upstream on `127.0.0.1:0`. Each request takes the next [`Step`] queued for
//! its path (longest matching prefix), else the next default step, else a 404.
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
        Self::Reply { status, headers: vec![("content-type".into(), "application/json".into())], body: body.to_string().into() }
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

#[derive(Default)]
struct Inner {
    by_path: Mutex<Vec<(String, VecDeque<Step>)>>,
    default: Mutex<VecDeque<Step>>,
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
    lock(&inner.received).push(Received {
        method: parts.method,
        path_and_query: path_and_query.clone(),
        headers: parts.headers,
        body,
        at: Instant::now(),
    });
    let Some(step) = next_step(&inner, parts.uri.path()) else {
        return head(404, &[]).body(Body::from(format!("mock upstream: no step scripted for {path_and_query}"))).expect("response");
    };
    match step {
        Step::Reply { status, headers, body } => head(status, &headers).body(Body::from(body)).expect("response"),
        Step::StallHeaders { hold } => {
            tokio::time::sleep(hold).await;
            head(504, &[]).body(Body::empty()).expect("response")
        }
        Step::Stream { status, headers, frames, every, cut } => {
            let watch = Watch { inner: inner.clone(), finished: false };
            let s = stream::unfold((frames.into_iter(), watch, false), move |(mut it, mut watch, started)| async move {
                if started && !every.is_zero() {
                    tokio::time::sleep(every).await;
                }
                match it.next() {
                    Some(f) => Some((Ok::<Bytes, std::io::Error>(f), (it, watch, true))),
                    None if cut => {
                        watch.finished = true;
                        Some((Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "scripted cut")), (it, watch, true)))
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
            head(200, &[("content-type".into(), "text/event-stream".into())]).body(Body::from_stream(s)).expect("response")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn scripts_replies_counts_connections_and_sees_disconnects() {
        let m = MockUpstream::start().await;
        m.on("/v1/messages", [Step::rate_limited(3, json!({ "error": "slow down" })), Step::json(200, json!({ "ok": true }))]);
        let Step::Stream { status, headers, frames, cut, .. } = Step::sse(&[(None, json!({ "a": 1 })), (None, json!({ "a": 2 }))], true).cut_after(1) else {
            unreachable!()
        };
        m.push([Step::Stream { status, headers, frames, every: Duration::from_millis(20), cut }]);
        let c = reqwest::Client::new();

        let r = c.post(m.url("/v1/messages")).header("x-test", "1").body("{\"q\":1}").send().await.unwrap();
        assert_eq!(r.status(), 429);
        assert_eq!(r.headers()["retry-after"], "3");
        let r = c.post(m.url("/v1/messages")).send().await.unwrap();
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&r.bytes().await.unwrap()).unwrap(), json!({ "ok": true }));
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
}

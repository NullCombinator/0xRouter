//! Spike for spec 013 research R3: can a `connector_layer` whose future reads a task-local
//! at completion attribute connect time to the right attempt?
//!
//! (1) the measured span includes the proxy CONNECT and the TLS handshake;
//! (2) a connect that loses the pool's checkout race never sees the task-local;
//! (3) sequential requests read new then reused, and on HTTP/2 concurrent requests read one
//!     new connection and the rest reused.

use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tower::{Layer, Service};

/// What a layer completion stored for one attempt.
#[derive(Default)]
struct Clock {
    connected: Mutex<Option<Duration>>,
}

tokio::task_local! {
    static ATTEMPT: Arc<Clock>;
}

#[derive(Default)]
struct Shared {
    /// Completions that found a task-local.
    scoped: AtomicUsize,
    /// Completions that did not (a connect finished on a background task).
    unscoped: AtomicUsize,
}

#[derive(Clone, Default)]
struct ConnectClock(Arc<Shared>);

impl<S> Layer<S> for ConnectClock {
    type Service = ConnectClockService<S>;
    fn layer(&self, inner: S) -> Self::Service {
        ConnectClockService { inner, shared: self.0.clone() }
    }
}

#[derive(Clone)]
struct ConnectClockService<S> {
    inner: S,
    shared: Arc<Shared>,
}

impl<S, R> Service<R> for ConnectClockService<S>
where
    S: Service<R>,
    S::Future: Send + 'static,
    S::Response: 'static,
    S::Error: 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<S::Response, S::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: R) -> Self::Future {
        let start = Instant::now();
        let fut = self.inner.call(req);
        let shared = self.shared.clone();
        Box::pin(async move {
            let out = fut.await;
            let span = start.elapsed();
            if out.is_ok() {
                let stored = ATTEMPT.try_with(|c| *c.connected.lock().unwrap() = Some(span));
                match stored {
                    Ok(()) => shared.scoped.fetch_add(1, Ordering::SeqCst),
                    Err(_) => shared.unscoped.fetch_add(1, Ordering::SeqCst),
                };
            }
            out
        })
    }
}

type Delay = Arc<dyn Fn(usize) -> Duration + Send + Sync>;

fn none() -> Delay {
    Arc::new(|_| Duration::ZERO)
}

/// A TLS server on loopback. `tls_delay(conn)` waits after the TCP accept, before the
/// handshake; `resp_delay(request)` waits before the response.
async fn tls_server(h2: bool, tls_delay: Delay, resp_delay: Delay) -> SocketAddr {
    let key = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string()]).unwrap();
    let cert = CertificateDer::from(key.cert.der().to_vec());
    let pk = PrivateKeyDer::try_from(key.key_pair.serialize_der()).unwrap();
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut cfg = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert], pk)
        .unwrap();
    cfg.alpn_protocols = if h2 { vec![b"h2".to_vec()] } else { vec![b"http/1.1".to_vec()] };
    let acceptor = TlsAcceptor::from(Arc::new(cfg));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let conns = Arc::new(AtomicUsize::new(0));
    let reqs = Arc::new(AtomicUsize::new(0));
    tokio::spawn(async move {
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            let idx = conns.fetch_add(1, Ordering::SeqCst);
            let (acceptor, tls_delay, resp_delay, reqs) =
                (acceptor.clone(), tls_delay.clone(), resp_delay.clone(), reqs.clone());
            tokio::spawn(async move {
                tokio::time::sleep(tls_delay(idx)).await;
                let Ok(tls) = acceptor.accept(tcp).await else { return };
                let svc = service_fn(move |_req: Request<Incoming>| {
                    let (resp_delay, reqs) = (resp_delay.clone(), reqs.clone());
                    async move {
                        let n = reqs.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(resp_delay(n)).await;
                        Ok::<_, Infallible>(Response::new(Full::new(Bytes::from("ok"))))
                    }
                });
                let _ = auto::Builder::new(TokioExecutor::new())
                    .serve_connection(TokioIo::new(tls), svc)
                    .await;
            });
        }
    });
    addr
}

/// An HTTP CONNECT proxy that waits `delay` before it answers the CONNECT.
async fn connect_proxy(delay: Duration) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut down, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if down.read(&mut byte).await.unwrap_or(0) == 0 {
                        return;
                    }
                    head.push(byte[0]);
                }
                let head = String::from_utf8_lossy(&head).into_owned();
                let target = head.split_whitespace().nth(1).unwrap_or_default().to_string();
                tokio::time::sleep(delay).await;
                let Ok(mut up) = TcpStream::connect(&target).await else { return };
                let _ = down.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").await;
                let _ = tokio::io::copy_bidirectional(&mut down, &mut up).await;
            });
        }
    });
    addr
}

fn client(layer: &ConnectClock, proxy: Option<SocketAddr>) -> reqwest::Client {
    let mut b = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .connector_layer(layer.clone());
    b = match proxy {
        Some(p) => b.proxy(reqwest::Proxy::all(format!("http://{p}")).unwrap()),
        None => b.no_proxy(),
    };
    b.build().unwrap()
}

/// One request inside its own attempt scope; returns what the layer stored for it.
async fn attempt(c: &reqwest::Client, url: &str) -> Option<Duration> {
    let clock = Arc::new(Clock::default());
    let resp = ATTEMPT.scope(clock.clone(), c.get(url).send()).await.unwrap();
    resp.bytes().await.unwrap();
    let out = *clock.connected.lock().unwrap();
    out
}

#[tokio::test]
async fn span_includes_proxy_connect_and_tls_handshake() {
    let delay = Duration::from_millis(400);

    let server = tls_server(false, none(), none()).await;
    let proxy = connect_proxy(delay).await;
    let layer = ConnectClock::default();
    let span = attempt(&client(&layer, Some(proxy)), &format!("https://{server}/")).await;
    let span = span.expect("a new connection through the proxy reads a span");
    assert!(span >= delay.mul_f32(0.9), "proxy CONNECT delay missing from the span: {span:?}");

    let server = tls_server(false, Arc::new(move |_| delay), none()).await;
    let layer = ConnectClock::default();
    let span = attempt(&client(&layer, None), &format!("https://{server}/")).await;
    let span = span.expect("a new connection reads a span");
    assert!(span >= delay.mul_f32(0.9), "TLS handshake delay missing from the span: {span:?}");
}

#[tokio::test]
async fn a_connect_that_loses_the_checkout_race_is_never_attributed() {
    // Connection 0 is fast; later ones wait 600 ms before the handshake. Request 0 holds
    // connection 0 for 200 ms. Request B starts meanwhile and begins a slow connect, then
    // takes connection 0 when it goes idle. B's own connect finishes on a background task.
    let server = tls_server(
        false,
        Arc::new(|conn| if conn == 0 { Duration::ZERO } else { Duration::from_millis(600) }),
        Arc::new(|req| if req == 0 { Duration::from_millis(200) } else { Duration::ZERO }),
    )
    .await;
    let url = format!("https://{server}/");
    let layer = ConnectClock::default();
    let c = client(&layer, None);

    let a = tokio::spawn({
        let (c, url) = (c.clone(), url.clone());
        async move { attempt(&c, &url).await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let b = attempt(&c, &url).await;
    let a = a.await.unwrap();

    assert!(a.is_some(), "request A made the first connection");
    assert!(b.is_none(), "request B reused A's connection; its lost connect must not be read as its own");

    // The abandoned connect finishes later, on a background task, without a task-local.
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert_eq!(layer.0.scoped.load(Ordering::SeqCst), 1, "only A's connect was scoped");
    assert_eq!(layer.0.unscoped.load(Ordering::SeqCst), 1, "B's lost connect completed unscoped");
}

#[tokio::test]
async fn sequential_requests_read_new_then_reused() {
    let server = tls_server(false, none(), none()).await;
    let url = format!("https://{server}/");
    let layer = ConnectClock::default();
    let c = client(&layer, None);
    assert!(attempt(&c, &url).await.is_some(), "first request: new connection");
    assert!(attempt(&c, &url).await.is_none(), "second request: reused");
}

#[tokio::test]
async fn http2_concurrent_requests_read_one_new_connection() {
    let server = tls_server(true, none(), Arc::new(|_| Duration::from_millis(200))).await;
    let url = format!("https://{server}/");
    let layer = ConnectClock::default();
    let c = client(&layer, None);
    let tasks: Vec<_> = (0..3)
        .map(|_| {
            let (c, url) = (c.clone(), url.clone());
            tokio::spawn(async move { attempt(&c, &url).await })
        })
        .collect();
    let mut new = 0;
    for t in tasks {
        if t.await.unwrap().is_some() {
            new += 1;
        }
    }
    assert_eq!(new, 1, "one request made the connection, the others reused it");
}

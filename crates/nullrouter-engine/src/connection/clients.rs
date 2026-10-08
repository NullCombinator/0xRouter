//! The client cache and the connector layer that times connects (spec 013, research R3, R5).

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::sync::Mutex;
use std::time::Duration;

use tower::{BoxError, Layer, Service};

use super::proxy::Proxies;
use crate::timing::{ATTEMPT, DEFAULT_CONNECT_TIMEOUT};

/// How a client talks HTTP to the upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HttpMode {
    /// HTTP/2 where the server offers it, else HTTP/1.1.
    Negotiate,
    Http1Only,
}

/// What distinguishes one cached client from another (research R5).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClientKey {
    /// A proxy name from `proxies.toml`; `None` is a direct connection.
    pub proxy: Option<String>,
    pub http: HttpMode,
    /// Whether idle connections are kept for the next request.
    pub reuse: bool,
}

impl ClientKey {
    /// Today's client: direct, negotiating, reusing.
    pub const PLAIN: ClientKey = ClientKey { proxy: None, http: HttpMode::Negotiate, reuse: true };
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("proxy {0:?} is not defined in proxies.toml")]
    UnknownProxy(String),
    #[error("proxy {name:?}: {reason}")]
    Proxy { name: String, reason: String },
    #[error("the HTTP client could not be built: {0}")]
    Build(String),
}

/// The clients of one engine snapshot, built when first asked for. A reload makes a new
/// snapshot, so a changed proxy definition or setting gets fresh clients.
#[derive(Debug)]
pub struct Clients {
    allow_private: bool,
    proxies: Proxies,
    cache: Mutex<HashMap<ClientKey, reqwest::Client>>,
}

impl Clients {
    pub fn new(allow_private: bool, proxies: Proxies) -> Self {
        Self { allow_private, proxies, cache: Mutex::new(HashMap::new()) }
    }

    pub fn proxies(&self) -> &Proxies {
        &self.proxies
    }

    /// How many clients have been built.
    pub fn len(&self) -> usize {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The client for `key`, built on first use. Clones share one pool.
    pub fn get(&self, key: &ClientKey) -> Result<reqwest::Client, ClientError> {
        if let Some(c) = self.cache.lock().unwrap_or_else(|e| e.into_inner()).get(key) {
            return Ok(c.clone());
        }
        let built = self.build(key)?;
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        Ok(cache.entry(key.clone()).or_insert(built).clone())
    }

    fn build(&self, key: &ClientKey) -> Result<reqwest::Client, ClientError> {
        // Through a proxy only the proxy's own host is resolved here, and the operator chose it,
        // so it may be private; the target is the proxy's to resolve.
        let mut b = crate::upstream::builder(self.allow_private || key.proxy.is_some());
        if let Some(name) = &key.proxy {
            let def = self.proxies.get(name).ok_or_else(|| ClientError::UnknownProxy(name.clone()))?;
            let bad = |e: reqwest::Error| ClientError::Proxy { name: name.clone(), reason: e.to_string() };
            let mut proxy = reqwest::Proxy::all(&def.url).map_err(bad)?;
            if let Some(user) = &def.username {
                let password = def.secret.as_ref().map(|s| s.with_exposed(str::to_owned)).unwrap_or_default();
                proxy = proxy.basic_auth(user, &password);
            }
            b = b.proxy(proxy);
        }
        if key.http == HttpMode::Http1Only {
            b = b.http1_only();
        }
        if !key.reuse {
            b = b.pool_max_idle_per_host(0);
        }
        b.build().map_err(|e| ClientError::Build(e.to_string()))
    }
}

/// A connect that took longer than the attempt's connect timeout.
#[derive(Debug, Clone, Copy)]
pub struct ConnectTimedOut(pub Duration);

impl fmt::Display for ConnectTimedOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "connect timed out after {} ms", self.0.as_millis())
    }
}

impl std::error::Error for ConnectTimedOut {}

/// Marks the running attempt's clock when a connection is ready, and enforces its connect
/// timeout. Install it with `reqwest::ClientBuilder::connector_layer`.
///
/// It reads the `ATTEMPT` task-local when the connect *completes*. A connect that lost the
/// pool's checkout race finishes on a background task, where the task-local is absent, so it
/// is never attributed to an attempt (research R3, proved by `tests/connect_attribution.rs`).
#[derive(Clone, Copy, Default)]
pub struct ConnectClock;

impl<S> Layer<S> for ConnectClock {
    type Service = ConnectClockService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ConnectClockService { inner }
    }
}

#[derive(Clone)]
pub struct ConnectClockService<S> {
    inner: S,
}

impl<S, R> Service<R> for ConnectClockService<S>
where
    S: Service<R, Error = BoxError>,
    S::Future: Send + 'static,
    S::Response: 'static,
{
    type Response = S::Response;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<S::Response, BoxError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), BoxError>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: R) -> Self::Future {
        let fut = self.inner.call(req);
        Box::pin(async move {
            let _ = ATTEMPT.try_with(|c| c.mark_connecting());
            // Read at poll time, like the completion mark, so both see the same scope.
            let limit = ATTEMPT.try_with(|c| c.connect_timeout()).unwrap_or(DEFAULT_CONNECT_TIMEOUT);
            let conn = match tokio::time::timeout(limit, fut).await {
                Ok(res) => res?,
                Err(_) => return Err(ConnectTimedOut(limit).into()),
            };
            let _ = ATTEMPT.try_with(|c| c.mark_connected());
            Ok(conn)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Instant;

    use super::*;
    use crate::timing::AttemptClock;

    /// A connector that never finishes.
    #[derive(Clone)]
    struct Hang;

    impl Service<()> for Hang {
        type Response = ();
        type Error = BoxError;
        type Future = std::future::Pending<Result<(), BoxError>>;

        fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), BoxError>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _: ()) -> Self::Future {
            std::future::pending()
        }
    }

    /// A connector that is ready at once.
    #[derive(Clone)]
    struct Ready;

    impl Service<()> for Ready {
        type Response = ();
        type Error = BoxError;
        type Future = std::future::Ready<Result<(), BoxError>>;

        fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), BoxError>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _: ()) -> Self::Future {
            std::future::ready(Ok(()))
        }
    }

    /// A connector that takes `0` to connect.
    #[derive(Clone)]
    struct Slow(Duration);

    impl Service<()> for Slow {
        type Response = ();
        type Error = BoxError;
        type Future = Pin<Box<dyn Future<Output = Result<(), BoxError>> + Send>>;

        fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), BoxError>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _: ()) -> Self::Future {
            let d = self.0;
            Box::pin(async move {
                tokio::time::sleep(d).await;
                Ok(())
            })
        }
    }

    #[tokio::test]
    async fn a_slow_connect_is_the_span_between_the_two_marks() {
        let clock = Arc::new(AttemptClock::new(Instant::now(), Duration::from_secs(5)));
        let mut svc = ConnectClock.layer(Slow(Duration::from_millis(80)));
        ATTEMPT.scope(clock.clone(), svc.call(())).await.unwrap();
        let span = clock.connected().unwrap() - clock.connecting().unwrap();
        assert!((75.0..250.0).contains(&span), "connect took {span:.1} ms");
    }

    #[tokio::test(start_paused = true)]
    async fn a_connect_past_the_attempts_timeout_fails_as_a_connect_timeout() {
        let clock = Arc::new(AttemptClock::new(Instant::now(), Duration::from_millis(50)));
        let mut svc = ConnectClock.layer(Hang);
        let err = ATTEMPT.scope(clock.clone(), svc.call(())).await.unwrap_err();
        assert_eq!(err.downcast_ref::<ConnectTimedOut>().map(|e| e.0), Some(Duration::from_millis(50)));
        assert_eq!(clock.connected(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn outside_an_attempt_the_built_in_default_applies() {
        let mut svc = ConnectClock.layer(Hang);
        let err = svc.call(()).await.unwrap_err();
        assert_eq!(err.downcast_ref::<ConnectTimedOut>().map(|e| e.0), Some(DEFAULT_CONNECT_TIMEOUT));
    }

    #[tokio::test]
    async fn a_finished_connect_marks_the_clock_inside_a_scope_only() {
        let clock = Arc::new(AttemptClock::new(Instant::now(), DEFAULT_CONNECT_TIMEOUT));
        let mut svc = ConnectClock.layer(Ready);
        svc.call(()).await.unwrap();
        assert_eq!(clock.connected(), None);
        ATTEMPT.scope(clock.clone(), svc.call(())).await.unwrap();
        assert!(clock.connected().is_some());
    }

    fn proxies() -> Proxies {
        Proxies::parse(
            "schema = 1\n[[proxy]]\nname = \"eu\"\nurl = \"socks5://127.0.0.1:1080\"\nusername = \"u\"\npassword = \"p\"\n[[proxy]]\nname = \"web\"\nurl = \"http://127.0.0.1:3128\"\n",
            std::path::Path::new("proxies.toml"),
            |_| None,
        )
        .unwrap()
    }

    #[test]
    fn a_client_is_built_once_per_key_and_shared() {
        let c = Clients::new(false, proxies());
        assert!(c.is_empty());
        c.get(&ClientKey::PLAIN).unwrap();
        c.get(&ClientKey::PLAIN).unwrap();
        assert_eq!(c.len(), 1);
        let eu = ClientKey { proxy: Some("eu".into()), ..ClientKey::PLAIN };
        let web = ClientKey { proxy: Some("web".into()), ..ClientKey::PLAIN };
        let h1 = ClientKey { http: HttpMode::Http1Only, ..ClientKey::PLAIN };
        let no_reuse = ClientKey { reuse: false, ..ClientKey::PLAIN };
        for k in [&eu, &web, &h1, &no_reuse, &eu] {
            c.get(k).unwrap();
        }
        assert_eq!(c.len(), 5, "plain, eu, web, http1 and no-reuse: one each");
    }

    #[test]
    fn an_undefined_proxy_is_an_error_and_builds_nothing() {
        let c = Clients::new(false, proxies());
        let key = ClientKey { proxy: Some("nope".into()), ..ClientKey::PLAIN };
        let e = c.get(&key).unwrap_err();
        assert!(matches!(e, ClientError::UnknownProxy(ref n) if n == "nope"), "{e}");
        assert!(c.is_empty());
    }
}

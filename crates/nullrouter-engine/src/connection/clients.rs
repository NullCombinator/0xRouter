//! The client cache and the connector layer that times connects (spec 013, research R3, R5).

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tower::{BoxError, Layer, Service};

use crate::timing::{ATTEMPT, DEFAULT_CONNECT_TIMEOUT};

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
        let limit = ATTEMPT.try_with(|c| c.connect_timeout()).unwrap_or(DEFAULT_CONNECT_TIMEOUT);
        let fut = self.inner.call(req);
        Box::pin(async move {
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
}

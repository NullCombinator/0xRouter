//! Bounded work (research R8): at most two page builds at once, a third waits up to five seconds
//! and is then told to reload, each build has ten seconds, and each runs in its own task so a
//! panic is that page's error and nothing else's.
//!
//! A timed-out build's page is told at once, but the build keeps its permit until it ends: its
//! reads run on the blocking pool (`views::run_in_process`), where an abort can't stop them, so
//! freeing the permit early let slow reads pile up past two (security-review.md M1). A build
//! still running at [`BACKSTOP`] times its limit is aborted, so one that never ends can't hold a
//! permit forever.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use tokio::sync::Semaphore;

const PERMITS: usize = 2;
const WAIT: Duration = Duration::from_secs(5);
const LIMIT: Duration = Duration::from_secs(10);
/// How many limits a timed-out build may go on running before it is aborted.
pub const BACKSTOP: u32 = 6;

/// Why a page was not built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildError {
    /// Both permits stayed taken for the whole wait.
    Busy,
    /// The build ran past its limit.
    TimedOut(Duration),
    /// The build's task panicked; the message is the panic's, cut to 200 characters.
    Panicked(String),
}

impl BuildError {
    pub fn status(&self) -> StatusCode {
        match self {
            Self::Busy => StatusCode::SERVICE_UNAVAILABLE,
            Self::TimedOut(_) | Self::Panicked(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// The page's text, shown inside the frame.
    pub fn text(&self) -> String {
        match self {
            Self::Busy => "Busy; reload.".into(),
            Self::TimedOut(limit) => format!(
                "This page could not be built: it took longer than {} s. Other pages and client requests are unaffected.",
                limit.as_secs()
            ),
            Self::Panicked(reason) => {
                format!("This page could not be built: {reason}. Other pages and client requests are unaffected.")
            }
        }
    }
}

impl IntoResponse for BuildError {
    fn into_response(self) -> Response {
        (self.status(), [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], self.text() + "\n").into_response()
    }
}

/// The permits and limits every page build goes through.
#[derive(Clone)]
pub struct Guard {
    permits: Arc<Semaphore>,
    wait: Duration,
    limit: Duration,
}

impl Default for Guard {
    fn default() -> Self {
        Self::with_limits(PERMITS, WAIT, LIMIT)
    }
}

impl Guard {
    pub fn with_limits(permits: usize, wait: Duration, limit: Duration) -> Self {
        Self { permits: Arc::new(Semaphore::new(permits)), wait, limit }
    }

    /// Runs `build` under a permit, in its own task.
    pub async fn run<T, Fut>(&self, build: impl FnOnce() -> Fut + Send + 'static) -> Result<T, BuildError>
    where
        Fut: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let permit = match tokio::time::timeout(self.wait, self.permits.clone().acquire_owned()).await {
            Ok(Ok(permit)) => permit,
            _ => return Err(BuildError::Busy),
        };
        let mut task = tokio::spawn(async move {
            let _permit = permit;
            build().await
        });
        match tokio::time::timeout(self.limit, &mut task).await {
            Ok(Ok(page)) => Ok(page),
            Ok(Err(e)) => Err(BuildError::Panicked(panic_text(e))),
            Err(_) => {
                let backstop = self.limit * BACKSTOP;
                tokio::spawn(async move {
                    if tokio::time::timeout(backstop, &mut task).await.is_err() {
                        task.abort();
                    }
                });
                Err(BuildError::TimedOut(self.limit))
            }
        }
    }
}

fn panic_text(e: tokio::task::JoinError) -> String {
    if !e.is_panic() {
        return "the build was cancelled".into();
    }
    let payload = e.into_panic();
    let text = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "the build panicked".into());
    text.chars().take(200).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::Notify;

    #[tokio::test]
    async fn a_build_returns_its_page() {
        let guard = Guard::default();
        assert_eq!(guard.run(|| async { 7 }).await, Ok(7));
    }

    #[tokio::test]
    async fn a_third_build_waits_then_is_told_to_reload() {
        let guard = Guard::with_limits(2, Duration::from_millis(60), Duration::from_secs(5));
        let release = Arc::new(Notify::new());
        let held = |g: &Guard| {
            let release = release.clone();
            let g = g.clone();
            tokio::spawn(async move { g.run(move || async move { release.notified().await }).await })
        };
        let (a, b) = (held(&guard), held(&guard));
        tokio::time::sleep(Duration::from_millis(20)).await;

        let started = std::time::Instant::now();
        let third = guard.run(|| async { 1 }).await;
        assert_eq!(third, Err(BuildError::Busy));
        assert!(started.elapsed() >= Duration::from_millis(50), "it waited for a permit first");
        assert_eq!(BuildError::Busy.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(BuildError::Busy.text(), "Busy; reload.");

        release.notify_waiters();
        assert_eq!(a.await.unwrap(), Ok(()));
        assert_eq!(b.await.unwrap(), Ok(()));
        assert_eq!(guard.run(|| async { 2 }).await, Ok(2), "permits come back");
    }

    #[tokio::test]
    async fn a_waiting_build_runs_once_a_permit_frees() {
        let guard = Guard::with_limits(1, Duration::from_secs(2), Duration::from_secs(5));
        let slow = {
            let g = guard.clone();
            tokio::spawn(async move { g.run(|| async { tokio::time::sleep(Duration::from_millis(50)).await }).await })
        };
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(guard.run(|| async { "second" }).await, Ok("second"));
        slow.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn a_build_past_its_limit_is_told_at_once_and_keeps_its_permit_until_it_ends() {
        let guard = Guard::with_limits(1, Duration::from_millis(100), Duration::from_millis(40));
        let r = guard.run(|| async { tokio::time::sleep(Duration::from_millis(400)).await }).await;
        assert_eq!(r, Err(BuildError::TimedOut(Duration::from_millis(40))));
        assert_eq!(guard.run(|| async { 3 }).await, Err(BuildError::Busy), "the slow build still holds it");
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(guard.run(|| async { 3 }).await, Ok(3), "and frees it when it ends");
    }

    #[tokio::test]
    async fn a_build_that_never_ends_is_aborted_at_the_backstop() {
        let guard = Guard::with_limits(1, Duration::from_millis(50), Duration::from_millis(20));
        let r = guard.run(|| async { tokio::time::sleep(Duration::from_secs(30)).await }).await;
        assert_eq!(r, Err(BuildError::TimedOut(Duration::from_millis(20))));
        tokio::time::sleep(Duration::from_millis(20) * BACKSTOP + Duration::from_millis(100)).await;
        assert_eq!(guard.run(|| async { 4 }).await, Ok(4));
    }

    #[tokio::test]
    async fn a_panic_is_that_pages_error_only() {
        let guard = Guard::default();
        let r: Result<(), _> = guard.run(|| async { panic!("the view broke") }).await;
        let err = r.unwrap_err();
        assert_eq!(err, BuildError::Panicked("the view broke".into()));
        assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            err.text(),
            "This page could not be built: the view broke. Other pages and client requests are unaffected."
        );
        assert_eq!(guard.run(|| async { 4 }).await, Ok(4), "the next page builds");
    }
}

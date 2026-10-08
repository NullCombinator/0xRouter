//! Per-attempt marks (spec 013, research R1–R3).
//!
//! An `AttemptClock` holds the marks of the running attempt. The attempt loop, the connector
//! layer (through the `ATTEMPT` task-local) and the live table share it as an `Arc`; marks are
//! atomic slots, so none of them takes a lock on the request path. `to_timing` folds it into
//! the record's `AttemptTiming` when the attempt ends.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::records::{AttemptTiming, Connection, TimeoutHit};

tokio::task_local! {
    /// The running attempt's clock, set around the upstream send (research R3).
    pub static ATTEMPT: Arc<AttemptClock>;
}

/// The connect timeout outside an attempt scope and when nothing sets one.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

const UNSET: u64 = f64::NAN.to_bits();

/// A mark in ms from the request's arrival. First write wins; NaN means unset.
struct Mark(AtomicU64);

impl Mark {
    fn new() -> Self {
        Self(AtomicU64::new(UNSET))
    }

    fn set(&self, ms: f64) {
        let _ = self.0.compare_exchange(UNSET, ms.to_bits(), Relaxed, Relaxed);
    }

    fn get(&self) -> Option<f64> {
        let v = f64::from_bits(self.0.load(Relaxed));
        (!v.is_nan()).then_some(v)
    }
}

/// A span that adds up, in nanoseconds.
#[derive(Default)]
struct Span(AtomicU64);

impl Span {
    fn add(&self, d: Duration) {
        self.0.fetch_add(d.as_nanos().min(u64::MAX as u128) as u64, Relaxed);
    }

    fn ms(&self) -> f64 {
        self.0.load(Relaxed) as f64 / 1e6
    }

    fn nonzero_ms(&self) -> Option<f64> {
        (self.0.load(Relaxed) > 0).then(|| self.ms())
    }
}

pub struct AttemptClock {
    arrival: Instant,
    connect_timeout: Duration,
    connecting: Mark,
    connected: Mark,
    headers: Mark,
    first_output: Mark,
    upstream_done: Mark,
    blocked: Span,
    retry_wait: Span,
    refresh: Span,
    closing: Mark,
    merged_wait: AtomicBool,
    /// A response came back, whatever its status.
    answered: AtomicBool,
    /// 0 unset, 1 HTTP/1.1, 2 HTTP/2.
    http: AtomicU8,
    proxy: Mutex<Option<String>>,
    timeout: Mutex<Option<TimeoutHit>>,
}

impl AttemptClock {
    /// `arrival` is the request's arrival instant; every mark counts from it.
    pub fn new(arrival: Instant, connect_timeout: Duration) -> Self {
        Self {
            arrival,
            connect_timeout,
            connecting: Mark::new(),
            connected: Mark::new(),
            headers: Mark::new(),
            first_output: Mark::new(),
            upstream_done: Mark::new(),
            blocked: Span::default(),
            retry_wait: Span::default(),
            refresh: Span::default(),
            closing: Mark::new(),
            merged_wait: AtomicBool::new(false),
            answered: AtomicBool::new(false),
            http: AtomicU8::new(0),
            proxy: Mutex::new(None),
            timeout: Mutex::new(None),
        }
    }

    pub fn now_ms(&self) -> f64 {
        self.arrival.elapsed().as_secs_f64() * 1e3
    }

    pub fn connect_timeout(&self) -> Duration {
        self.connect_timeout
    }

    /// The connector layer began a connect: this attempt is not on a pooled connection.
    pub fn mark_connecting(&self) {
        self.connecting.set(self.now_ms());
    }

    pub fn mark_connected(&self) {
        self.connected.set(self.now_ms());
    }

    pub fn mark_headers(&self) {
        self.headers.set(self.now_ms());
    }

    pub fn mark_first_output(&self) {
        self.first_output.set(self.now_ms());
    }

    pub fn mark_upstream_done(&self) {
        self.upstream_done.set(self.now_ms());
    }

    /// Sets a mark at an exact time, for callers that already hold the reading.
    pub fn set_connected(&self, ms: f64) {
        self.connected.set(ms);
    }

    pub fn set_headers(&self, ms: f64) {
        self.headers.set(ms);
    }

    pub fn set_first_output(&self, ms: f64) {
        self.first_output.set(ms);
    }

    pub fn set_upstream_done(&self, ms: f64) {
        self.upstream_done.set(ms);
    }

    pub fn set_closing(&self, ms: f64) {
        self.closing.set(ms);
    }

    pub fn add_blocked(&self, d: Duration) {
        self.blocked.add(d);
    }

    pub fn add_retry_wait(&self, d: Duration) {
        self.retry_wait.add(d);
    }

    pub fn add_refresh(&self, d: Duration) {
        self.refresh.add(d);
    }

    /// A response came back. A non-2xx one doesn't mark `headers`: its time, the error body
    /// included, is the headers phase the attempt ended in.
    pub fn mark_answered(&self) {
        self.answered.store(true, Relaxed);
    }

    pub fn set_merged_wait(&self) {
        self.merged_wait.store(true, Relaxed);
    }

    /// `"1.1"` or `"2"`; anything else is ignored.
    pub fn set_http(&self, version: &str) {
        let v = match version {
            "1.1" => 1,
            "2" => 2,
            _ => return,
        };
        self.http.store(v, Relaxed);
    }

    pub fn set_proxy(&self, name: &str) {
        *self.proxy.lock().unwrap_or_else(|e| e.into_inner()) = Some(name.to_owned());
    }

    pub fn set_timeout(&self, hit: TimeoutHit) {
        *self.timeout.lock().unwrap_or_else(|e| e.into_inner()) = Some(hit);
    }

    pub fn headers(&self) -> Option<f64> {
        self.headers.get()
    }

    pub fn first_output(&self) -> Option<f64> {
        self.first_output.get()
    }

    pub fn connecting(&self) -> Option<f64> {
        self.connecting.get()
    }

    pub fn connected(&self) -> Option<f64> {
        self.connected.get()
    }

    pub fn upstream_done(&self) -> Option<f64> {
        self.upstream_done.get()
    }

    /// Folds the clock into the record's form. A connection is `new` if the connector layer
    /// saw one, `reused` if headers arrived without it, and `none` before that.
    pub fn to_timing(&self) -> AttemptTiming {
        let connected = self.connected.get();
        let headers = self.headers.get();
        let connection = match (connected, headers.is_some() || self.answered.load(Relaxed)) {
            (Some(_), _) => Connection::New,
            (None, true) => Connection::Reused,
            (None, false) => Connection::None,
        };
        AttemptTiming {
            retry_wait_ms: self.retry_wait.nonzero_ms(),
            refresh_ms: self.refresh.nonzero_ms(),
            connected,
            connection,
            http: match self.http.load(Relaxed) {
                1 => Some("1.1".to_owned()),
                2 => Some("2".to_owned()),
                _ => None,
            },
            proxy: self.proxy.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            headers,
            first_output: self.first_output.get(),
            merged_wait: self.merged_wait.load(Relaxed),
            upstream_done: self.upstream_done.get(),
            blocked_ms: self.blocked.ms(),
            closing_ms: self.closing.get(),
            timeout: *self.timeout.lock().unwrap_or_else(|e| e.into_inner()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::{Source, SourceBy, SourceLevel, TimeoutKind};

    fn clock() -> Arc<AttemptClock> {
        Arc::new(AttemptClock::new(Instant::now(), DEFAULT_CONNECT_TIMEOUT))
    }

    #[tokio::test]
    async fn marks_set_from_several_tasks_are_read_back_exactly() {
        let c = clock();
        let tasks = [
            tokio::spawn({
                let c = c.clone();
                async move { c.set_connected(10.5) }
            }),
            tokio::spawn({
                let c = c.clone();
                async move { c.set_headers(20.25) }
            }),
            tokio::spawn({
                let c = c.clone();
                async move { c.set_first_output(30.0) }
            }),
            tokio::spawn({
                let c = c.clone();
                async move { c.set_upstream_done(40.125) }
            }),
        ];
        for t in tasks {
            t.await.unwrap();
        }
        let t = c.to_timing();
        assert_eq!(t.connected, Some(10.5));
        assert_eq!(t.headers, Some(20.25));
        assert_eq!(t.first_output, Some(30.0));
        assert_eq!(t.upstream_done, Some(40.125));
    }

    #[test]
    fn the_first_write_to_a_mark_wins() {
        let c = clock();
        c.set_headers(5.0);
        c.set_headers(9.0);
        assert_eq!(c.to_timing().headers, Some(5.0));
    }

    #[test]
    fn blocked_and_retry_wait_spans_accumulate() {
        let c = clock();
        c.add_blocked(Duration::from_millis(30));
        c.add_blocked(Duration::from_millis(12));
        c.add_retry_wait(Duration::from_millis(250));
        c.add_retry_wait(Duration::from_millis(250));
        let t = c.to_timing();
        assert!((t.blocked_ms - 42.0).abs() < 1e-6);
        assert!((t.retry_wait_ms.unwrap() - 500.0).abs() < 1e-6);
        assert_eq!(t.refresh_ms, None);
    }

    #[test]
    fn a_reused_connection_has_no_connected_mark() {
        let c = clock();
        c.set_headers(12.0);
        let t = c.to_timing();
        assert_eq!(t.connection, Connection::Reused);
        assert_eq!(t.connected, None);
    }

    #[test]
    fn an_error_response_counts_as_answered_without_headers() {
        let c = clock();
        c.mark_answered();
        let t = c.to_timing();
        assert_eq!((t.connection, t.headers), (Connection::Reused, None));
    }

    #[test]
    fn connection_follows_the_marks() {
        let c = clock();
        assert_eq!(c.to_timing().connection, Connection::None);
        c.set_connected(3.0);
        assert_eq!(c.to_timing().connection, Connection::New);
        let c = clock();
        c.set_connected(3.0);
        c.set_headers(8.0);
        let t = c.to_timing();
        assert_eq!((t.connection, t.connected), (Connection::New, Some(3.0)));
    }

    #[test]
    fn http_proxy_merge_and_timeout_carry_over() {
        let c = clock();
        c.set_http("2");
        c.set_http("3");
        c.set_proxy("corp");
        c.set_merged_wait();
        let hit = TimeoutHit {
            which: TimeoutKind::Headers,
            ms: 5000,
            source: Source { by: SourceBy::BuiltIn, level: SourceLevel::Default },
        };
        c.set_timeout(hit);
        let t = c.to_timing();
        assert_eq!(t.http.as_deref(), Some("2"));
        assert_eq!(t.proxy.as_deref(), Some("corp"));
        assert!(t.merged_wait);
        assert_eq!(t.timeout, Some(hit));
    }

    #[tokio::test]
    async fn the_task_local_reaches_code_inside_the_scope_only() {
        let c = clock();
        assert!(ATTEMPT.try_with(|_| ()).is_err());
        ATTEMPT.scope(c.clone(), async { ATTEMPT.with(|a| a.set_connected(1.0)) }).await;
        assert_eq!(c.connected(), Some(1.0));
    }
}

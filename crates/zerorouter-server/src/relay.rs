//! Responses to the client (research R5).
//!
//! Streams pass through as the bytes the wire writer framed for the client's style
//! (SSE, NDJSON or a JSON array), each chunk sent as it's produced; nothing is buffered.
//! Axum's `Sse` isn't used because it would frame those bytes a second time.
//! A [`CancelOnDrop`] travels with every streamed body, so a client that goes away cancels
//! the request's upstream work.

use std::pin::Pin;
use std::task::{Context, Poll};

use axum::body::{Body, Bytes};
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::response::Response;
use futures_util::Stream;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

pub const REQUEST_ID: HeaderName = HeaderName::from_static("x-0router-request-id");

/// Cancels its token when dropped.
#[derive(Debug)]
pub struct CancelOnDrop(pub CancellationToken);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// A stream that owns a [`CancelOnDrop`].
struct Guarded<S> {
    inner: S,
    _guard: CancelOnDrop,
}

impl<S: Stream + Unpin> Stream for Guarded<S> {
    type Item = S::Item;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<S::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}

pub fn stamp(resp: &mut Response, request_id: &str) {
    if let Ok(v) = HeaderValue::from_str(request_id) {
        resp.headers_mut().insert(REQUEST_ID, v);
    }
}

/// Adds the provider headers the serving plugin forwards (`forwarding.to_client`, already
/// past the floor). The core's own headers win.
pub fn forward(mut resp: Response, headers: Vec<(HeaderName, HeaderValue)>) -> Response {
    let core: Vec<HeaderName> = resp.headers().keys().cloned().collect();
    for (name, value) in headers {
        if !core.contains(&name) {
            resp.headers_mut().append(name, value);
        }
    }
    resp
}

pub fn json(status: u16, body: &Value, request_id: &str) -> Response {
    let mut resp = Response::new(Body::from(body.to_string()));
    *resp.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    resp.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    stamp(&mut resp, request_id);
    resp
}

/// A same-style non-stream answer: the provider's bytes as received (research R27).
pub fn as_received(status: u16, content_type: Option<&str>, body: Bytes, request_id: &str) -> Response {
    let mut resp = Response::new(Body::from(body));
    *resp.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
    let ct = content_type
        .and_then(|c| HeaderValue::from_str(c).ok())
        .unwrap_or(HeaderValue::from_static("application/json"));
    resp.headers_mut().insert(header::CONTENT_TYPE, ct);
    stamp(&mut resp, request_id);
    resp
}

/// A streamed body; `token` is cancelled when the body is dropped (finished or abandoned).
pub fn stream<S, E>(status: u16, content_type: &str, body: S, token: CancellationToken, request_id: &str) -> Response
where
    S: Stream<Item = Result<Bytes, E>> + Send + Unpin + 'static,
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    let guarded = Guarded { inner: body, _guard: CancelOnDrop(token) };
    let mut resp = Response::new(Body::from_stream(guarded));
    *resp.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
    let h = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(content_type) {
        h.insert(header::CONTENT_TYPE, v);
    }
    if content_type.starts_with("text/event-stream") {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        h.insert(HeaderName::from_static("x-accel-buffering"), HeaderValue::from_static("no"));
    }
    stamp(&mut resp, request_id);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;

    #[tokio::test]
    async fn dropping_the_body_cancels_and_every_response_has_an_id() {
        let token = CancellationToken::new();
        let body = stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(b"data: 1\n\n"))]);
        let resp = stream(200, "text/event-stream", body, token.clone(), "rq_1");
        assert_eq!(resp.headers()[REQUEST_ID], "rq_1");
        assert_eq!(resp.headers()[header::CACHE_CONTROL], "no-cache");
        assert!(!token.is_cancelled());
        drop(resp);
        assert!(token.is_cancelled());
        assert_eq!(json(401, &serde_json::json!({}), "rq_2").headers()[REQUEST_ID], "rq_2");
    }

    #[tokio::test]
    async fn a_same_style_body_goes_out_byte_for_byte() {
        let raw = Bytes::from_static(br#"{"id":"m1",  "context_management":{"applied":[]} ,"x":1}"#);
        let resp = as_received(200, Some("application/json; charset=utf-8"), raw.clone(), "rq_3");
        assert_eq!(resp.headers()[header::CONTENT_TYPE], "application/json; charset=utf-8");
        assert_eq!(resp.headers()[REQUEST_ID], "rq_3");
        assert_eq!(axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap(), raw);
    }
}

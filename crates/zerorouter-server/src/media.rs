//! Non-text routes (T086): embeddings, image, TTS, STT, and video jobs.
//!
//! The client style's codec decodes the body into the IR, the engine runs it like a text
//! request (retry, fallback, cooldowns), and the answer is encoded back in the client's
//! style. Audio that both sides send as a raw body is relayed as it arrives. A video job
//! gets a `vj_` id; polls and the content fetch go to the account that took it.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Bytes;
use axum::http::HeaderMap;
use axum::response::Response;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use zerorouter_engine::attempt::{Answer, Media, MediaAnswer, TextRequest};
use zerorouter_engine::records::Outcome;
use zerorouter_engine::state::{Engine, EngineState};
use zerorouter_registry::schema::RouteOp;
use zerorouter_wire::codec::types::JobStatus;
use zerorouter_wire::template::Bindings;

use crate::relay;
use crate::router::Matched;
use crate::serve::{style_error, style_failure};
use crate::text::{self, Incoming};

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// A body read from a channel of chunks; an `Err` cuts it.
fn body_of(
    mut rx: mpsc::Receiver<Result<Bytes, String>>,
) -> impl futures_util::Stream<Item = Result<Bytes, std::io::Error>> + Send + Unpin {
    futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx).map(|o| o.map(|r| r.map_err(std::io::Error::other))))
}

/// Where the client fetches a finished job's content: its style's content route.
fn content_url(m: &Matched<'_>, headers: &HeaderMap, id: &str) -> Option<String> {
    let route = m
        .entry
        .style
        .file
        .routes
        .iter()
        .find(|r| r.op == RouteOp::JobContent && r.model_type == m.entry.route.model_type)?;
    let path = route.path.replace("{id}", id);
    Some(match headers.get("host").and_then(|h| h.to_str().ok()) {
        Some(host) => format!("http://{host}{path}"),
        None => path,
    })
}

/// A job status body in the client's style.
fn job_body(
    m: &Matched<'_>,
    headers: &HeaderMap,
    id: &str,
    mut bindings: Bindings,
    status: JobStatus,
    target: &str,
    rid: &str,
) -> Response {
    bindings.set("job.id", id);
    bindings.set("job.model", target);
    if status == JobStatus::Completed
        && let Some(url) = content_url(m, headers, id)
    {
        bindings.set("job.content_url", url);
    }
    let Some(codec) = m.entry.style.codec.type_codec(m.entry.route.model_type).ok() else {
        return style_error(m, 500, "0router: the style has no job shape", rid);
    };
    match codec.encode_job(&bindings, status) {
        Some(body) => relay::json(200, &body, rid),
        None => style_error(m, 500, "0router: the style has no job shape", rid),
    }
}

/// Runs one non-text generation or job submit and answers in the client's style.
pub async fn generate(
    engine: &Arc<Engine>,
    st: Arc<EngineState>,
    m: &Matched<'_>,
    inc: Incoming<'_>,
    job: bool,
) -> Response {
    let client = m.entry.style.codec.clone();
    let route = &m.entry.route;
    let id = inc.id.clone();
    let fail = |status: u16, message: &str| {
        engine.records.update(&id, |r| r.outcome = Outcome::Failed);
        style_error(m, status, message, &id)
    };
    let Some(target) = text::model(m, &inc.body) else { return fail(400, "0router: the request names no model") };
    let codec = match client.type_codec(route.model_type) {
        Ok(c) => c.variant(route.variant.as_deref()).into_owned(),
        Err(_) => return fail(501, &format!("0router: this style doesn't carry {} requests", route.model_type)),
    };
    let input = match codec.decode_request(&inc.body) {
        Ok(v) => v,
        Err(e) => return fail(400, &format!("0router: {e}")),
    };
    let headers = inc.headers.clone();
    let cancel = CancellationToken::new();
    let req = TextRequest {
        id: id.clone(),
        arrived: inc.arrived,
        client: client.clone(),
        body: inc.body,
        ir: Default::default(),
        headers: inc.headers,
        agent: inc.agent,
        target: target.clone(),
        stream: false,
        cancel: cancel.clone(),
        media: Some(Media {
            ty: route.model_type,
            codec: codec.clone(),
            variant: route.variant.clone(),
            input,
            voice: None,
            job,
        }),
    };
    let guard = cancel.clone().drop_guard();
    let answer = match engine.text(st, req).await {
        Ok(Answer::Media(a)) => a,
        Ok(_) => return fail(500, "0router: a non-text request got a text answer"),
        Err(f) => return style_failure(m, &f, &id),
    };
    match answer {
        MediaAnswer::Value(v) => {
            let ctx = Bindings::new().with("response.model", target.as_str()).with("response.created", unix_now());
            let (bytes, ctype) = codec.encode_response(&v, &ctx);
            relay::as_received(200, Some(&ctype), Bytes::from(bytes), &id)
        }
        MediaAnswer::Bytes { content_type, rx } => {
            guard.disarm();
            relay::stream(200, &content_type, body_of(rx), cancel, &id)
        }
        MediaAnswer::Job { id: vj, status, bindings } => job_body(m, &headers, &vj, bindings, status, &target, &id),
    }
}

/// A job poll: one upstream request, the status in the client's style. The response
/// carries the submit's record id.
pub async fn job_get(engine: &Arc<Engine>, st: Arc<EngineState>, m: &Matched<'_>, inc: Incoming<'_>) -> Response {
    let vj = m.capture("id").unwrap_or_default().to_owned();
    match engine.job_get(&st, &vj, &inc.agent.key, &m.entry.style.file.id).await {
        Ok((job, bindings, status)) => job_body(m, &inc.headers, &vj, bindings, status, &job.target, &job.record),
        Err(f) => style_failure(m, &f, &inc.id),
    }
}

/// A finished job's content, relayed as it arrives.
pub async fn job_content(engine: &Arc<Engine>, st: Arc<EngineState>, m: &Matched<'_>, inc: Incoming<'_>) -> Response {
    let vj = m.capture("id").unwrap_or_default().to_owned();
    let record = engine.jobs.get(&vj).map(|j| j.record).unwrap_or_else(|| inc.id.clone());
    match engine.job_content(&st, &vj, &inc.agent.key, &m.entry.style.file.id).await {
        Ok((content_type, rx)) => relay::stream(200, &content_type, body_of(rx), CancellationToken::new(), &record),
        Err(f) => style_failure(m, &f, &inc.id),
    }
}

//! Token counting routes (T113, FR-026): the target resolves as for generation, and the
//! engine walks the same plan. A provider whose text endpoint declares `[token_count]`
//! answers the count; any other is estimated with 9router's estimator, and the answer
//! carries `x-0router-estimate: true`.

use std::sync::Arc;

use axum::http::HeaderValue;
use axum::response::Response;
use nullrouter_engine::attempt::{Answer, TextRequest};
use nullrouter_engine::records::Outcome;
use nullrouter_engine::state::{Engine, EngineState};
use nullrouter_wire::codec::request;
use nullrouter_wire::template::{Bindings, render};
use tokio_util::sync::CancellationToken;

use crate::relay;
use crate::router::Matched;
use crate::text::{self, Incoming};

pub const ESTIMATE: &str = "x-0router-estimate";

/// Runs one count request and answers in the client's style.
pub async fn count(engine: &Arc<Engine>, st: Arc<EngineState>, m: &Matched<'_>, inc: Incoming<'_>) -> Response {
    let client = m.entry.style.codec.clone();
    let id = inc.id.clone();
    let fail = |status: u16, message: &str| {
        engine.records.update(&id, |r| r.outcome = Outcome::Failed);
        crate::serve::style_error(m, status, message, &id)
    };
    let Some(target) = text::model(m, &inc.body) else {
        return fail(400, "0router: the request names no model");
    };
    let target = target.strip_prefix("models/").unwrap_or(&target).to_owned();
    // Gemini's countTokens takes either `contents` or a whole `generateContentRequest`.
    let body = match inc.body.get("generateContentRequest") {
        Some(inner) if inc.body.get("contents").is_none() => inner.clone(),
        _ => inc.body,
    };
    let ir = match request::decode(&client, &body) {
        Ok(ir) => ir,
        Err(e) => return fail(400, &format!("0router: {e}")),
    };
    let Some(shape) = client.text().ok().and_then(|t| t.count_response.clone()) else {
        return fail(501, "0router: this style declares no count response");
    };
    let cancel = CancellationToken::new();
    let req = TextRequest {
        id: id.clone(),
        arrived: inc.arrived,
        client: client.clone(),
        body,
        ir,
        headers: inc.headers,
        agent: inc.agent,
        target,
        stream: false,
        cancel: cancel.clone(),
        media: None,
        count: true,
    };
    let _guard = cancel.drop_guard();
    let (n, estimated) = match engine.text(st, req).await {
        Ok(Answer::Count { input_tokens, estimated }) => (input_tokens, estimated),
        Ok(_) => return fail(500, "0router: a count request got another answer"),
        Err(f) => return crate::serve::style_failure(m, &f, &id),
    };
    let answer = render(&shape, &Bindings::new().with("count.input", n));
    let mut r = relay::json(200, &answer, &id);
    if estimated {
        r.headers_mut().insert(ESTIMATE, HeaderValue::from_static("true"));
    }
    r
}

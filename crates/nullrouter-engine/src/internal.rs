//! Internal requests (spec 004, T058): the core's own model calls, such as the adapter review.
//!
//! An [`InternalRequest`] is built here as an openai-chat body and goes through [`Engine::text`]
//! like any request: the normal plan and attempt loop, on the operator's accounts. It has no
//! HTTP route, no key check, and no adapter: the adapter runner is found by key id, and the
//! agent label is not a key, so [`Engine::runner_for`] finds none. The record's agent is the
//! label, which is what `records show` renders (unknown ids show as given).

use std::sync::Arc;
use std::time::Instant;

use reqwest::header::HeaderMap;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::attempt::{self, Answer, TextRequest};
use crate::keys::AgentId;
use crate::records::{Outcome, RequestRecord};
use crate::state::Engine;
use nullrouter_wire::codec::request;
use nullrouter_wire::codec::response::ForClient;

const STYLE: &str = "openai-chat";

/// One system-and-user exchange the core makes for itself.
#[derive(Debug, Clone)]
pub struct InternalRequest {
    /// Shown as the record's agent, e.g. `review:acme@v1.0.0-xx`. Must not be a key id.
    pub agent_label: String,
    /// A provider-qualified model or unified model name, as a client would send it.
    pub model: String,
    pub system: String,
    pub user: String,
    pub max_tokens: u32,
}

/// What the caller needs of the answer and its record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InternalAnswer {
    pub text: String,
    pub record_id: String,
    pub tokens_in: Option<u64>,
    pub tokens_out: Option<u64>,
    /// Who served it, when the record already says (it is written as the attempt settles).
    pub provider: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InternalError {
    #[error("0router: the {STYLE} style isn't loaded")]
    NoStyle,
    #[error("0router: the internal request failed ({status}): {message}")]
    Failed { status: u16, message: String },
}

impl Engine {
    /// Runs `req` through the attempt loop and returns the assistant text.
    pub async fn internal(self: &Arc<Self>, req: InternalRequest) -> Result<InternalAnswer, InternalError> {
        let st = self.snapshot();
        let client = st.style(STYLE).ok_or(InternalError::NoStyle)?.clone();
        let body = json!({
            "model": req.model,
            "max_tokens": req.max_tokens,
            "stream": false,
            "messages": [
                {"role": "system", "content": req.system},
                {"role": "user", "content": req.user},
            ],
        });
        let ir = request::decode(&client, &body)
            .map_err(|e| InternalError::Failed { status: 500, message: e.to_string() })?;
        let id = crate::records::new_id();
        let mut record = RequestRecord::new(id.clone(), crate::clock::now_rfc3339_millis(), client.id.clone());
        let agent = AgentId::new(req.agent_label, None);
        record.agent = Some(agent.clone());
        self.records.insert(record);
        let text_req = TextRequest {
            id: id.clone(),
            arrived: Instant::now(),
            client: client.clone(),
            body: body.clone(),
            ir,
            headers: HeaderMap::new(),
            agent,
            target: req.model,
            stream: false,
            cancel: CancellationToken::new(),
            media: None,
            count: false,
            pin: None,
            test: None,
        };
        let failed = |status: u16, message: String| {
            self.records.update(&id, |r| {
                if r.outcome == Outcome::InProgress {
                    r.outcome = Outcome::Failed;
                }
            });
            InternalError::Failed { status, message }
        };
        let read = match self.text(st, text_req).await {
            Ok(Answer::Whole { answer, .. }) => match *answer {
                ForClient::AsReceived { read } => read,
                ForClient::Rebuilt { read, .. } => Some(read),
            },
            Ok(Answer::Events { rx, .. }) => match attempt::collect(&client, &body, rx).await {
                Ok(r) => Some(r),
                Err(e) => return Err(failed(e.status.unwrap_or(502), e.message)),
            },
            Ok(Answer::Media(_) | Answer::Count { .. }) => {
                return Err(failed(500, "a text request got a non-text answer".into()));
            }
            Err(f) => return Err(InternalError::Failed { status: f.status, message: f.message }),
        };
        let Some(read) = read else {
            return Err(failed(502, "the answer didn't read as a chat completion".into()));
        };
        let served = self.records.get(&id).and_then(|r| r.served_by);
        Ok(InternalAnswer {
            text: read.text(),
            record_id: id,
            tokens_in: read.usage.as_ref().and_then(|u| u.input),
            tokens_out: read.usage.as_ref().and_then(|u| u.output),
            provider: served.as_ref().map(|s| s.provider.clone()),
            model: served.map(|s| s.model),
        })
    }
}

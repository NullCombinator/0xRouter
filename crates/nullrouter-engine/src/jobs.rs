//! Video jobs: 0router job ids mapped to the account that took them (research R16).
//!
//! A client never sees a provider's job id. Submitting a job hands it a `vj_` id; polling
//! and fetching the content go back to the same provider, account and wire, one upstream
//! request each, with no fallback: another account can't see the job. The entry holds no
//! secret, only the account's name, and lives in memory for [`TTL`].

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use bytes::Bytes;
use nullrouter_registry::schema::{Endpoint, ModelType};
use nullrouter_wire::codec::types::JobStatus;
use nullrouter_wire::template::Bindings;
use reqwest::header::HeaderMap;
use tokio::sync::mpsc;
use tokio::time;

use crate::accounts;
use crate::attempt::{CHANNEL, Failure};
use crate::records::Outcome;
use crate::state::{Engine, EngineState};
use crate::upstream::{self, RequestParts};

/// The bound on one poll or content request's headers and each body chunk.
pub const POLL_TIMEOUT: Duration = Duration::from_secs(60);

/// How long a job stays reachable after its last use.
pub const TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    /// The request record the submit created; polls and the content fetch add to it.
    pub record: String,
    pub provider: String,
    pub account: Option<String>,
    /// The endpoint URL the job was submitted to (polls go to `{url}/{upstream_id}`).
    pub url: String,
    pub wire: String,
    pub model: String,
    /// The provider's own job id.
    pub upstream_id: String,
    /// The target as the client named it.
    pub target: String,
    /// The agent key that submitted it; no other key reaches it.
    pub agent: String,
}

#[derive(Debug, Default)]
pub struct JobMap {
    inner: Mutex<HashMap<String, (Job, Instant)>>,
}

/// A fresh `vj_` id.
pub fn new_id() -> String {
    crate::records::new_id().replacen("rq_", "vj_", 1)
}

impl JobMap {
    /// Remembers `job`; returns its `vj_` id.
    pub fn insert(&self, job: Job) -> String {
        let id = new_id();
        let mut m = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        m.retain(|_, (_, used)| now.duration_since(*used) < TTL);
        m.insert(id.clone(), (job, now));
        id
    }

    pub fn get(&self, id: &str) -> Option<Job> {
        let mut m = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let (job, used) = m.get_mut(id)?;
        if used.elapsed() >= TTL {
            m.remove(id);
            return None;
        }
        *used = Instant::now();
        Some(job.clone())
    }
}

fn failure(status: u16, message: impl Into<String>) -> Failure {
    Failure { status, message: message.into(), retry_after: None, tried: Vec::new() }
}

impl Engine {
    /// The job `id` for `agent`: a 404 for an unknown id or another key's job.
    fn job(&self, id: &str, agent: &str) -> Result<Job, Failure> {
        self.jobs
            .get(id)
            .filter(|j| j.agent == agent)
            .ok_or_else(|| failure(404, format!("0router: no video job {id}")))
    }

    /// One GET under the job's endpoint, on the account that took it.
    async fn job_request(
        &self,
        st: &EngineState,
        job: &Job,
        suffix: &str,
        client_style: &str,
    ) -> Result<reqwest::Response, Failure> {
        let provider = st
            .registry
            .provider(&job.provider)
            .map_err(|_| failure(502, format!("0router: provider {} is no longer installed", job.provider)))?;
        let base = provider
            .endpoints
            .get(&ModelType::Video)
            .and_then(|e| e.0.iter().find(|e| e.wire.as_deref() == Some(job.wire.as_str())))
            .ok_or_else(|| {
                failure(502, format!("0router: provider {} no longer has the job's endpoint", job.provider))
            })?;
        let endpoint =
            Endpoint { url: format!("{}/{}{suffix}", job.url, job.upstream_id), method: "GET".into(), ..base.clone() };
        let account = match &job.account {
            None => None,
            Some(name) => Some(
                st.accounts
                    .for_provider(&job.provider)
                    .find(|a| &a.name == name)
                    .ok_or_else(|| failure(502, format!("0router: account {}/{name} is gone", job.provider)))?,
            ),
        };
        let secret = account
            .map(|a| accounts::release(a, provider))
            .transpose()
            .map_err(|w| failure(502, format!("0router: {w}")))?;
        let headers = HeaderMap::new();
        let parts = RequestParts {
            provider,
            endpoint: &endpoint,
            floor: st.registry.floor(),
            redactor: &st.redactor,
            secret,
            client_style,
            client_headers: &headers,
            model: &job.model,
            voice: None,
            content_type: None,
            body: Bytes::new(),
        };
        let out = upstream::build_request(parts).map_err(|e| failure(502, format!("0router: {e}")))?;
        upstream::check_ip_host(&out.url, st.registry.runtime().allow_private_endpoints)
            .map_err(|e| failure(502, format!("0router: {e}")))?;
        let resp = time::timeout(POLL_TIMEOUT, out.into_request(&st.http).send())
            .await
            .map_err(|_| {
                failure(
                    504,
                    format!("0router: {} sent no response headers within {} s", job.provider, POLL_TIMEOUT.as_secs()),
                )
            })?
            .map_err(|e| failure(502, format!("0router: network error: {}", st.redactor.redact(&e.to_string()))))?;
        let status = resp.status().as_u16();
        if (200..300).contains(&status) {
            return Ok(resp);
        }
        let raw = time::timeout(POLL_TIMEOUT, resp.bytes()).await.ok().and_then(Result::ok).unwrap_or_default();
        let text = st.redactor.redact(&String::from_utf8_lossy(&raw)).chars().take(500).collect::<String>();
        Err(failure(status, format!("{}: {text}", job.provider)))
    }

    /// Polls the job once: its bindings in the IR (`job.id` is the `vj_` id) and state. A
    /// failed job fails its record.
    pub async fn job_get(
        &self,
        st: &EngineState,
        id: &str,
        agent: &str,
        client_style: &str,
    ) -> Result<(Job, Bindings, JobStatus), Failure> {
        let job = self.job(id, agent)?;
        let resp = self.job_request(st, &job, "", client_style).await?;
        let raw = time::timeout(POLL_TIMEOUT, resp.bytes())
            .await
            .map_err(|_| failure(504, "0router: the job status stalled"))?
            .map_err(|e| failure(502, format!("0router: network error: {}", st.redactor.redact(&e.to_string()))))?;
        let wire =
            st.style(&job.wire).ok_or_else(|| failure(502, format!("0router: style {} isn't loaded", job.wire)))?;
        let codec = wire.type_codec(ModelType::Video).map_err(|e| failure(502, format!("0router: {e}")))?;
        let value: serde_json::Value =
            serde_json::from_slice(&raw).map_err(|_| failure(502, "0router: the job status isn't JSON"))?;
        let (mut bindings, status) = codec
            .decode_job(&value)
            .ok_or_else(|| failure(502, "0router: the job status doesn't have the wire's shape"))?;
        bindings.set("job.id", id);
        bindings.0.remove("job.content_url");
        if status == JobStatus::Failed {
            self.records.update(&job.record, |r| r.outcome = Outcome::Failed);
        }
        Ok((job, bindings, status))
    }

    /// The finished job's content, relayed as it arrives. The record succeeds when the
    /// body has been read to its end.
    pub async fn job_content(
        self: &std::sync::Arc<Self>,
        st: &EngineState,
        id: &str,
        agent: &str,
        client_style: &str,
    ) -> Result<(String, mpsc::Receiver<Result<Bytes, String>>), Failure> {
        let job = self.job(id, agent)?;
        let mut resp = self.job_request(st, &job, "/content", client_style).await?;
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_owned();
        let (tx, rx) = mpsc::channel(CHANNEL);
        let (engine, redactor) = (self.clone(), st.redactor.clone());
        tokio::spawn(async move {
            loop {
                let item = match time::timeout(POLL_TIMEOUT, resp.chunk()).await {
                    Ok(Ok(Some(b))) => Ok(b),
                    Ok(Ok(None)) => {
                        engine.records.update(&job.record, |r| r.outcome = Outcome::Succeeded);
                        return;
                    }
                    Err(_) => Err(format!("no byte for {} s", POLL_TIMEOUT.as_secs())),
                    Ok(Err(e)) => Err(format!("content read failed: {}", redactor.redact(&e.to_string()))),
                };
                let failed = item.is_err();
                if tx.send(item).await.is_err() || failed {
                    return;
                }
            }
        });
        Ok((content_type, rx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_is_found_by_its_vj_id() {
        let m = JobMap::default();
        let job = Job {
            record: "rq_1".into(),
            provider: "p".into(),
            account: Some("main".into()),
            url: "https://x.example/v1/videos".into(),
            wire: "openai-chat".into(),
            model: "v".into(),
            upstream_id: "up_1".into(),
            target: "p/v".into(),
            agent: "ak_1".into(),
        };
        let id = m.insert(job.clone());
        assert!(id.starts_with("vj_"));
        assert_eq!(m.get(&id), Some(job));
        assert_eq!(m.get("vj_nope"), None);
    }
}

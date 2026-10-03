//! Video jobs: 0router job ids mapped to the account that took them (research R16).
//!
//! A client never sees a provider's job id. Submitting a job hands it a `vj_` id; polling
//! and fetching the content go back to the same provider, account and wire, one upstream
//! request each, with no fallback: another account can't see the job. The entry holds no
//! secret, only the account's name, and lives in memory for [`TTL`].
//!
//! A video endpoint may declare where polls go (`poll_url`) and how its job bodies read
//! (`job`), for providers whose jobs don't have the wire's shape (xAI). A declared
//! `job.content_url` makes the content a download from that URL: its host is checked like
//! any upstream host, redirects are never followed, and the account's secret goes only to
//! a host the account is bound to.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use bytes::Bytes;
use nullrouter_registry::schema::{Endpoint, JobMapping, JobState, ModelType, ProviderEntity};
use nullrouter_registry::template::FieldPath;
use nullrouter_wire::codec::types::{self, JobStatus};
use nullrouter_wire::template::{self, Bindings};
use reqwest::Url;
use reqwest::header::HeaderMap;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::time;

use crate::accounts;
use crate::attempt::{CHANNEL, Failure};
use crate::identity::{self, FillContext};
use crate::keys::AgentId;
use crate::records::Outcome;
use crate::state::{Engine, EngineState};
use crate::upstream::{self, RequestParts, SignedIn};

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
    /// The finished video's download URL, once a poll has read it (`job.content_url`).
    pub content_url: Option<String>,
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

    /// Remembers the finished job's download URL.
    fn set_content_url(&self, id: &str, url: &str) {
        let mut m = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((job, _)) = m.get_mut(id) {
            job.content_url = Some(url.to_owned());
        }
    }
}

/// A job body read through an endpoint's declared `job` mapping.
#[derive(Debug, Clone, PartialEq)]
pub struct MappedJob {
    /// `job.id` (the provider's id, when the body carries it), `job.status`, `job.error`.
    pub bindings: Bindings,
    pub status: JobStatus,
    pub content_url: Option<String>,
}

fn path(p: &str) -> Result<FieldPath, String> {
    FieldPath::parse(p).map_err(|e| format!("0router: job path {p}: {e}"))
}

fn text(v: &Value) -> String {
    v.as_str().map_or_else(|| v.to_string(), str::to_owned)
}

/// Reads `v` through `m`. A submit answer must carry the id and may lack a status (it is
/// then queued); a poll answer must carry the status.
pub fn decode_mapped(m: &JobMapping, v: &Value, submit: bool) -> Result<MappedJob, String> {
    let one =
        |p: &str| -> Result<Option<&Value>, String> { Ok(template::select_one(&path(p)?, v).filter(|v| !v.is_null())) };
    let mut bindings = Bindings::new();
    match one(&m.id)? {
        Some(id) => bindings.set("job.id", text(id)),
        None if submit => return Err("the job answer has no id".into()),
        None => {}
    }
    let status = match one(&m.status)?.map(text) {
        None if submit => JobStatus::Queued,
        None => return Err("the job status has no status".into()),
        Some(raw) => {
            let status = match m.status_map.get(&raw) {
                Some(JobState::Queued) => JobStatus::Queued,
                Some(JobState::InProgress) => JobStatus::InProgress,
                Some(JobState::Completed) => JobStatus::Completed,
                Some(JobState::Failed) => JobStatus::Failed,
                None => types::job_status(&raw, None),
            };
            bindings.set("job.status", raw);
            status
        }
    };
    if let Some(e) = m.error.as_deref().map(one).transpose()?.flatten() {
        bindings.set("job.error", text(e));
    }
    let content_url = m.content_url.as_deref().map(one).transpose()?.flatten().map(text);
    Ok(MappedJob { bindings, status, content_url })
}

/// What one job request asks for.
#[derive(Debug, Clone, Copy)]
enum Target<'a> {
    /// The job's status.
    Poll,
    /// The `{poll}/content` route.
    Content,
    /// A declared download URL.
    Download(&'a str),
}

/// The URL a job is polled at: the endpoint's `poll_url` with the id, else `{url}/{id}`.
fn poll_url(endpoint: &Endpoint, job: &Job) -> Result<String, Failure> {
    match &endpoint.poll_url {
        None => Ok(format!("{}/{}", job.url, job.upstream_id)),
        Some(p) => upstream::segment(&job.upstream_id)
            .map(|id| p.replace("{id}", &id))
            .map_err(|e| failure(502, format!("0router: {e}"))),
    }
}

/// Whether `url`'s host is one the account's secret may go to: the hosts a key account
/// was added for, or the hosts a sign-in account's token is bound to.
fn bound(url: &Url, account: Option<&crate::accounts::Account>, released: Option<&accounts::Released>) -> bool {
    let Some(host) = url.host_str() else { return false };
    match (released, account) {
        (Some(accounts::Released::Token(view)), _) => view.entry.hosts.contains(host),
        (Some(accounts::Released::Key(_)), Some(a)) => a.hosts.contains(host),
        _ => false,
    }
}

/// A download URL the core may fetch: http(s), https unless private endpoints are allowed.
fn download_url(raw: &str, allow_private: bool) -> Result<Url, Failure> {
    let url = Url::parse(raw).map_err(|_| failure(502, "0router: the job's content URL doesn't parse"))?;
    let ok = match url.scheme() {
        "https" => true,
        "http" => allow_private,
        _ => false,
    };
    if !ok || url.host_str().is_none() {
        return Err(failure(502, "0router: the job's content URL isn't an https URL"));
    }
    upstream::check_ip_host(&url, allow_private).map_err(|e| failure(502, format!("0router: {e}")))?;
    Ok(url)
}

fn video_endpoint<'p>(provider: &'p ProviderEntity, job: &Job) -> Result<&'p Endpoint, Failure> {
    provider
        .endpoints
        .get(&ModelType::Video)
        .and_then(|e| e.0.iter().find(|e| e.wire.as_deref() == Some(job.wire.as_str())))
        .ok_or_else(|| failure(502, format!("0router: provider {} no longer has the job's endpoint", job.provider)))
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

    /// One GET for the job, on the account that took it.
    async fn job_request(
        &self,
        st: &EngineState,
        job: &Job,
        target: Target<'_>,
        client_style: &str,
    ) -> Result<reqwest::Response, Failure> {
        let provider = st
            .registry
            .provider(&job.provider)
            .map_err(|_| failure(502, format!("0router: provider {} is no longer installed", job.provider)))?;
        let base = video_endpoint(provider, job)?;
        let url = match target {
            Target::Poll => poll_url(base, job)?,
            Target::Content => format!("{}/content", poll_url(base, job)?),
            Target::Download(u) => u.to_owned(),
        };
        let endpoint = Endpoint { url, method: "GET".into(), ..base.clone() };
        let account = match &job.account {
            None => None,
            Some(name) => Some(
                st.accounts
                    .for_provider(&job.provider)
                    .find(|a| &a.name == name)
                    .ok_or_else(|| failure(502, format!("0router: account {}/{name} is gone", job.provider)))?,
            ),
        };
        let released = account
            .map(|a| accounts::release(a, provider, &st.tokens))
            .transpose()
            .map_err(|w| failure(502, format!("0router: {w}")))?;
        let allow_private = st.registry.runtime().allow_private_endpoints;
        // A download URL off the account's hosts gets a bare GET: no secret, no identity.
        if let Target::Download(raw) = target {
            let url = download_url(raw, allow_private)?;
            if !bound(&url, account, released.as_ref()) {
                let req = st.http.get(url);
                return self.job_send(st, job, req).await;
            }
        }
        let secret = released.as_ref().map(accounts::Released::secret);
        // A sign-in account's poll carries its token where `[signin] auth` says, and the
        // identity headers, as its submit did (research R6, R7).
        let signin = match (released.as_ref().and_then(accounts::Released::token), &provider.signin) {
            (Some(view), Some(decl)) => {
                let identity = match &provider.identity {
                    None => Vec::new(),
                    Some(d) => {
                        let session_id = self.sessions.id_for(&AgentId::new(job.agent.clone(), None));
                        let ctx = FillContext {
                            session_id: &session_id,
                            request_id: &identity::uuid_v4(),
                            turns: 0,
                            upstream_model: &job.model,
                            claims: Some(&view.entry.claims),
                            install_id: self.install_id().map_err(|e| failure(502, format!("0router: {e}")))?,
                        };
                        identity::headers(d, &ctx)
                    }
                };
                Some(SignedIn { auth: &decl.auth, identity })
            }
            _ => None,
        };
        let redactor = st.redactor.current();
        let headers = HeaderMap::new();
        let parts = RequestParts {
            provider,
            endpoint: &endpoint,
            floor: st.registry.floor(),
            redactor: &redactor,
            secret,
            client_style,
            client_headers: &headers,
            model: &job.model,
            voice: None,
            content_type: None,
            body: Bytes::new(),
            signin,
        };
        let out = upstream::build_request(parts).map_err(|e| failure(502, format!("0router: {e}")))?;
        upstream::check_ip_host(&out.url, allow_private).map_err(|e| failure(502, format!("0router: {e}")))?;
        self.job_send(st, job, out.into_request(&st.http)).await
    }

    /// Sends one job request: a 2xx answer, else the provider's status and redacted text.
    /// A redirect is never followed.
    async fn job_send(
        &self,
        st: &EngineState,
        job: &Job,
        req: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, Failure> {
        let resp = time::timeout(POLL_TIMEOUT, req.send())
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
        if (300..400).contains(&status) {
            return Err(failure(
                502,
                format!("0router: {} answered with a redirect, which isn't followed", job.provider),
            ));
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
        let (bindings, status, _) = self.poll(st, id, &job, client_style).await?;
        Ok((job, bindings, status))
    }

    /// One poll: the bindings, the IR status and, through a declared mapping, the
    /// download URL (remembered for the content fetch).
    async fn poll(
        &self,
        st: &EngineState,
        id: &str,
        job: &Job,
        client_style: &str,
    ) -> Result<(Bindings, JobStatus, Option<String>), Failure> {
        let resp = self.job_request(st, job, Target::Poll, client_style).await?;
        let raw = time::timeout(POLL_TIMEOUT, resp.bytes())
            .await
            .map_err(|_| failure(504, "0router: the job status stalled"))?
            .map_err(|e| failure(502, format!("0router: network error: {}", st.redactor.redact(&e.to_string()))))?;
        let value: Value =
            serde_json::from_slice(&raw).map_err(|_| failure(502, "0router: the job status isn't JSON"))?;
        let mapping = st.registry.provider(&job.provider).ok().and_then(|p| video_endpoint(p, job).ok()?.job.clone());
        let (mut bindings, status, content_url) = match mapping {
            Some(m) => {
                let j = decode_mapped(&m, &value, false).map_err(|e| failure(502, format!("0router: {e}")))?;
                (j.bindings, j.status, j.content_url)
            }
            None => {
                let wire = st
                    .style(&job.wire)
                    .ok_or_else(|| failure(502, format!("0router: style {} isn't loaded", job.wire)))?;
                let codec = wire.type_codec(ModelType::Video).map_err(|e| failure(502, format!("0router: {e}")))?;
                let (b, s) = codec
                    .decode_job(&value)
                    .ok_or_else(|| failure(502, "0router: the job status doesn't have the wire's shape"))?;
                (b, s, None)
            }
        };
        bindings.set("job.id", id);
        bindings.0.remove("job.content_url");
        let content_url = content_url.filter(|_| status == JobStatus::Completed);
        if let Some(u) = &content_url {
            self.jobs.set_content_url(id, u);
        }
        if status == JobStatus::Failed {
            self.records.update(&job.record, |r| r.outcome = Outcome::Failed);
        }
        Ok((bindings, status, content_url))
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
        let declared = st
            .registry
            .provider(&job.provider)
            .ok()
            .and_then(|p| video_endpoint(p, &job).ok()?.job.as_ref().map(|m| m.content_url.is_some()))
            .unwrap_or(false);
        let mut resp = if declared {
            let url = match job.content_url.clone() {
                Some(u) => u,
                None => match self.poll(st, id, &job, client_style).await? {
                    (_, _, Some(u)) => u,
                    (_, status, None) => {
                        return Err(failure(
                            409,
                            format!("0router: video job {id} has no content ({})", status.as_str()),
                        ));
                    }
                },
            };
            self.job_request(st, &job, Target::Download(&url), client_style).await?
        } else {
            self.job_request(st, &job, Target::Content, client_style).await?
        };
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
            content_url: None,
        };
        let id = m.insert(job.clone());
        assert!(id.starts_with("vj_"));
        assert_eq!(m.get(&id), Some(job));
        assert_eq!(m.get("vj_nope"), None);
    }

    #[test]
    fn a_mapping_reads_xai_bodies() {
        let m: JobMapping = toml::from_str(
            r#"id = "request_id"
status = "status"
status_map = { processing = "in_progress", done = "completed" }
content_url = "video.url"
error = "error.message""#,
        )
        .unwrap();
        let submit = decode_mapped(&m, &serde_json::json!({"request_id": "req-1"}), true).unwrap();
        assert_eq!((submit.bindings.str("job.id"), submit.status), (Some("req-1"), JobStatus::Queued));
        assert!(decode_mapped(&m, &serde_json::json!({}), true).is_err(), "a submit needs its id");
        let done = serde_json::json!({"status": "done", "video": {"url": "https://cdn.x.ai/v.mp4"}});
        let done = decode_mapped(&m, &done, false).unwrap();
        assert_eq!(done.status, JobStatus::Completed);
        assert_eq!(done.content_url.as_deref(), Some("https://cdn.x.ai/v.mp4"));
        let pending = decode_mapped(&m, &serde_json::json!({"status": "pending"}), false).unwrap();
        assert_eq!(pending.status, JobStatus::Queued, "unmapped values read as the common words");
        let failed = serde_json::json!({"status": "expired", "error": {"message": "too late"}});
        let failed = decode_mapped(&m, &failed, false).unwrap();
        assert_eq!((failed.status, failed.bindings.str("job.error")), (JobStatus::Failed, Some("too late")));
        assert!(decode_mapped(&m, &serde_json::json!({"progress": 3}), false).is_err(), "a poll needs its status");
    }

    #[test]
    fn download_urls_are_https_and_public() {
        assert!(download_url("https://cdn.x.ai/v.mp4", false).is_ok());
        assert!(download_url("http://cdn.x.ai/v.mp4", false).is_err());
        assert!(download_url("http://127.0.0.1:9/v.mp4", true).is_ok());
        assert!(download_url("https://169.254.169.254/latest", false).is_err());
        assert!(download_url("file:///etc/passwd", true).is_err());
    }
}

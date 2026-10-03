//! The device authorization grant (RFC 8628), used by grok-cli (research R3).
//!
//! The device request sends `client_id`, `scope` and the declared parameters; the token
//! endpoint is polled at `interval` (default 5 s, at least 1 s). `authorization_pending`
//! keeps waiting, `slow_down` adds 5 s, `expired_token` and `access_denied` end the sign-in.
//! A transient failure while polling (timeout, 5xx, 429) is retried at the next interval.
//! No PKCE.

use std::time::{Duration, Instant, SystemTime};

use nullrouter_registry::SecretString;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::{Flow, MAX_EXPIRES_IN, SignInError, SignInHttp, encode_body, error_code, pkce, token_request};
use crate::tokens::TokenEntry;

pub const GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(5);
pub const MIN_INTERVAL: Duration = Duration::from_secs(1);
pub const SLOW_DOWN_STEP: Duration = Duration::from_secs(5);
/// Used when the device response has no `expires_in`.
pub const DEFAULT_EXPIRES_IN: Duration = Duration::from_secs(600);

/// A device-code sign-in, waiting for the operator to approve it.
#[derive(Debug)]
pub struct DeviceSignIn {
    flow: Flow,
    /// The code the operator confirms on the provider's page.
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    /// When the device code expires, for display.
    pub expires_at: SystemTime,
    device_code: SecretString,
    interval: Duration,
    deadline: Instant,
}

/// One poll's outcome.
#[derive(Debug)]
pub enum DevicePoll {
    Pending,
    /// The interval grew by 5 s.
    SlowDown,
    Done(Box<TokenEntry>),
}

fn secs(v: &Value) -> Option<Duration> {
    match v {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
    // A provider's number is clamped (security review L8): no clock arithmetic overflows.
    .map(|s| Duration::from_secs(s.min(MAX_EXPIRES_IN.as_secs())))
}

impl DeviceSignIn {
    pub(crate) async fn start(http: &SignInHttp, flow: Flow) -> Result<Self, SignInError> {
        const WHAT: &str = "device endpoint";
        let decl = &flow.decl;
        let url = decl.device_url.as_deref().ok_or_else(|| SignInError::BadResponse {
            what: WHAT,
            reason: "the plugin declares no device_url".into(),
        })?;
        let scope = decl.scopes.join(" ");
        let params: Vec<(&str, String)> = decl.params.iter().map(|(k, v)| (k.as_str(), pkce::param_value(v))).collect();
        let mut fields = vec![("client_id", decl.client_id.as_str())];
        if !scope.is_empty() {
            fields.push(("scope", &scope));
        }
        fields.extend(params.iter().map(|(k, v)| (*k, v.as_str())));
        let body = encode_body(decl.body, &fields);
        let (status, bytes) = http.send(WHAT, reqwest::Method::POST, url, flow.headers.clone(), Some(body)).await?;
        if !(200..300).contains(&status) {
            return Err(SignInError::Rejected { what: WHAT, status, code: error_code(status, &bytes) });
        }
        let bad = |reason: &str| SignInError::BadResponse { what: WHAT, reason: reason.into() };
        let v: Value = serde_json::from_slice(&bytes).map_err(|_| bad("not JSON"))?;
        let text = |k: &str| v[k].as_str().filter(|s| !s.is_empty()).map(str::to_owned);
        let device_code = text("device_code").map(SecretString::new).ok_or_else(|| bad("no device_code"))?;
        let user_code = text("user_code").ok_or_else(|| bad("no user_code"))?;
        let verification_uri =
            text("verification_uri").or_else(|| text("verification_url")).ok_or_else(|| bad("no verification_uri"))?;
        let expires_in = secs(&v["expires_in"]).filter(|d| !d.is_zero()).unwrap_or(DEFAULT_EXPIRES_IN);
        let interval = secs(&v["interval"]).unwrap_or(DEFAULT_INTERVAL).max(MIN_INTERVAL);
        Ok(Self {
            user_code,
            verification_uri,
            verification_uri_complete: text("verification_uri_complete"),
            expires_at: SystemTime::now().checked_add(expires_in).unwrap_or_else(SystemTime::now),
            device_code,
            interval,
            deadline: Instant::now().checked_add(expires_in).unwrap_or_else(Instant::now),
            flow,
        })
    }

    /// The page to show: `verification_uri_complete` when given, else `verification_uri`
    /// (shown with [`user_code`](Self::user_code)).
    pub fn link(&self) -> &str {
        self.verification_uri_complete.as_deref().unwrap_or(&self.verification_uri)
    }

    /// The current wait between polls.
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// Polls the token endpoint once for account `name`.
    pub async fn poll_once(&mut self, http: &SignInHttp, name: &str) -> Result<DevicePoll, SignInError> {
        if Instant::now() >= self.deadline {
            return Err(SignInError::Expired);
        }
        let decl = &self.flow.decl;
        let body = self.device_code.with_exposed(|dc| {
            encode_body(
                decl.body,
                &[("grant_type", GRANT_TYPE), ("device_code", dc), ("client_id", decl.client_id.as_str())],
            )
        });
        match token_request(http, &self.flow.token_url, self.flow.headers.clone(), body).await {
            Ok(grant) => Ok(DevicePoll::Done(Box::new(self.flow.finish(http, name, grant).await))),
            Err(SignInError::Rejected { code, .. }) if code == "authorization_pending" => Ok(DevicePoll::Pending),
            Err(SignInError::Rejected { code, .. }) if code == "slow_down" => {
                self.interval = self.interval.saturating_add(SLOW_DOWN_STEP);
                Ok(DevicePoll::SlowDown)
            }
            Err(SignInError::Rejected { code, .. }) if code == "expired_token" => Err(SignInError::Expired),
            Err(SignInError::Rejected { code, .. }) if code == "access_denied" => Err(SignInError::Denied(code)),
            Err(e) if e.is_transient() => {
                tracing::debug!(error = %e, "device sign-in poll failed; retrying at the next interval");
                Ok(DevicePoll::Pending)
            }
            Err(e) => Err(e),
        }
    }

    /// Polls until the sign-in is approved, refused or expired, or `cancel` fires.
    pub async fn poll(
        &mut self,
        http: &SignInHttp,
        name: &str,
        cancel: &CancellationToken,
    ) -> Result<TokenEntry, SignInError> {
        loop {
            let left = self.deadline.saturating_duration_since(Instant::now());
            tokio::select! {
                () = cancel.cancelled() => return Err(SignInError::Cancelled),
                () = tokio::time::sleep(self.interval.min(left)) => {}
            }
            if Instant::now() >= self.deadline {
                return Err(SignInError::Expired);
            }
            if let DevicePoll::Done(e) = self.poll_once(http, name).await? {
                return Ok(*e);
            }
        }
    }
}

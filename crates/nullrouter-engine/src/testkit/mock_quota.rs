//! Quota endpoints on `127.0.0.1:0` (research R11, R12), each answering a settable
//! response. Defaults are built from 9router's parser cases
//! (`open-sse/services/usage/{claude,grok-cli,grokCliQuotaFrame,opencode-go}.js`,
//! `tests/unit/opencode-go-usage.test.js`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use serde_json::{Value, json};

use super::mock_upstream::{MockUpstream, Received, Step};

/// One quota route of the mock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QuotaRoute {
    /// anthropic `GET /api/oauth/usage`.
    AnthropicUsage,
    /// grok-cli `GET /v1/billing?format=credits`.
    GrokBilling,
    /// grok-cli `GET /v1/user?include=subscription`.
    GrokUser,
    /// grok-cli gRPC-web `POST /grok_api_v2.GrokBuildBilling/GetGrokCreditsConfig`.
    GrokCredits,
    /// opencode-go `GET /zen/go/v1/usage`.
    OpencodeGo,
    /// opencode-zen `GET /zen/v1/usage`.
    OpencodeZen,
    /// `GET /v1/models`, optionally with `x-ratelimit-*` headers (xai, live check L2).
    Models,
    /// `GET /sim/quota`: per-account windows a test sets (spec 006, [`SimQuota`]).
    Sim,
}

impl QuotaRoute {
    pub const ALL: [Self; 8] = [
        Self::AnthropicUsage,
        Self::GrokBilling,
        Self::GrokUser,
        Self::GrokCredits,
        Self::OpencodeGo,
        Self::OpencodeZen,
        Self::Models,
        Self::Sim,
    ];

    /// The route's path (no query).
    pub fn path(self) -> &'static str {
        match self {
            Self::AnthropicUsage => "/api/oauth/usage",
            Self::GrokBilling => "/v1/billing",
            Self::GrokUser => "/v1/user",
            Self::GrokCredits => "/grok_api_v2.GrokBuildBilling/GetGrokCreditsConfig",
            Self::OpencodeGo => "/zen/go/v1/usage",
            Self::OpencodeZen => "/zen/v1/usage",
            Self::Models => "/v1/models",
            Self::Sim => "/sim/quota",
        }
    }

    /// The path with the query the provider's client sends.
    pub fn path_and_query(self) -> &'static str {
        match self {
            Self::GrokBilling => "/v1/billing?format=credits",
            Self::GrokUser => "/v1/user?include=subscription",
            other => other.path(),
        }
    }

    fn of(path: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.path() == path)
    }

    /// The default answer: a healthy response shaped as the provider sends it.
    pub fn sample(self) -> Step {
        match self {
            Self::AnthropicUsage => Step::json(200, samples::anthropic_usage()),
            Self::GrokBilling => Step::json(200, samples::grok_billing()),
            Self::GrokUser => Step::json(200, samples::grok_user()),
            Self::GrokCredits => samples::grpc_web(&samples::grok_credits_frame(0.35, 1_790_000_000)),
            Self::OpencodeGo | Self::OpencodeZen => Step::json(200, samples::opencode_usage()),
            Self::Models => {
                Step::json(200, json!({ "object": "list", "data": [{ "id": "grok-4", "object": "model" }] }))
            }
            Self::Sim => Step::json(200, json!({ "windows": [] })),
        }
    }
}

/// Sample bodies.
pub mod samples {
    use super::*;

    /// claude.js: `five_hour`, `seven_day`, `seven_day_<model>`, `limits[]` (`weekly_scoped`).
    pub fn anthropic_usage() -> Value {
        json!({
            "five_hour": { "utilization": 87, "resets_at": "2026-10-02T18:00:00Z" },
            "seven_day": { "utilization": 40, "resets_at": "2026-10-07T00:00:00Z" },
            "seven_day_opus": { "utilization": 12.5, "resets_at": "2026-10-07T00:00:00Z" },
            "seven_day_oauth_apps": null,
            "extra_usage": null,
            "limits": [
                { "kind": "weekly_scoped", "percent": 30, "resets_at": "2026-10-07T00:00:00Z",
                  "scope": { "model": { "display_name": "Fable" } } },
                { "kind": "other", "percent": 99 }
            ]
        })
    }

    /// grok-cli.js: protobuf-JSON `{ val }` numbers under `config`.
    pub fn grok_billing() -> Value {
        json!({
            "config": {
                "currentPeriod": { "type": "USAGE_PERIOD_TYPE_WEEKLY",
                                   "start": "2026-09-28T00:00:00Z", "end": "2026-10-05T00:00:00Z" },
                "monthlyLimit": { "val": 2500 },
                "includedUsed": { "val": 1200 },
                "onDemandCap": { "val": 1000 },
                "onDemandUsed": { "val": 250 },
                "prepaidBalance": { "val": 500 },
                "isUnifiedBillingUser": true,
                "billingPeriodStart": "2026-10-01T00:00:00Z",
                "billingPeriodEnd": "2026-11-01T00:00:00Z"
            }
        })
    }

    pub fn grok_user() -> Value {
        json!({
            "userId": "user-0001",
            "email": "user@example.com",
            "subscriptionTier": "SuperGrok",
            "hasGrokCodeAccess": true
        })
    }

    /// tests/unit/opencode-go-usage.test.js, first case.
    pub fn opencode_usage() -> Value {
        json!({
            "usage": {
                "rolling": { "status": "ok", "percent": 13, "resetsAt": "2026-09-04T14:28:02.617Z" },
                "weekly": { "status": "ok", "percent": 5, "resetsAt": "2026-09-07T00:00:00.617Z" },
                "monthly": { "status": "ok", "percent": 2, "resetsAt": "2026-10-02T12:14:24.617Z" }
            }
        })
    }

    fn varint(mut v: u64, out: &mut Vec<u8>) {
        while v >= 0x80 {
            out.push((v as u8) | 0x80);
            v >>= 7;
        }
        out.push(v as u8);
    }

    /// grokCliQuotaFrame.js: field 1 { field 1 fixed32 float used ratio, field 5
    /// Timestamp { seconds } }, as one gRPC-web data frame followed by an OK trailer.
    pub fn grok_credits_frame(ratio: f32, reset_secs: u64) -> Vec<u8> {
        let mut ts = vec![0x08];
        varint(reset_secs, &mut ts);
        let mut info = vec![0x0d];
        info.extend(ratio.to_le_bytes());
        info.push(0x2a);
        varint(ts.len() as u64, &mut info);
        info.extend(ts);
        let mut msg = vec![0x0a];
        varint(info.len() as u64, &mut msg);
        msg.extend(info);
        let mut out = vec![0x00];
        out.extend((msg.len() as u32).to_be_bytes());
        out.extend(msg);
        let trailer = b"grpc-status:0\r\n";
        out.push(0x80);
        out.extend((trailer.len() as u32).to_be_bytes());
        out.extend(trailer);
        out
    }

    /// A gRPC-web response carrying `bytes`.
    pub fn grpc_web(bytes: &[u8]) -> Step {
        Step::binary("application/grpc-web+proto", bytes.to_vec())
    }
}

pub struct MockQuota {
    upstream: MockUpstream,
    answers: Arc<Mutex<HashMap<QuotaRoute, Step>>>,
    sim: Arc<Mutex<Option<SimQuota>>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl MockQuota {
    /// Every route answers its [`QuotaRoute::sample`] until set.
    pub async fn start() -> Self {
        let upstream = MockUpstream::start().await;
        let answers: Arc<Mutex<HashMap<QuotaRoute, Step>>> =
            Arc::new(Mutex::new(QuotaRoute::ALL.into_iter().map(|r| (r, r.sample())).collect()));
        let a = answers.clone();
        let sim: Arc<Mutex<Option<SimQuota>>> = Arc::default();
        let simmed = sim.clone();
        upstream.respond(move |r: &Received| {
            let path = r.path_and_query.split('?').next().unwrap_or_default();
            match QuotaRoute::of(path) {
                Some(QuotaRoute::Sim) if lock(&simmed).is_some() => {
                    lock(&simmed).as_ref().map_or_else(|| QuotaRoute::Sim.sample(), |s| s.answer(r))
                }
                Some(route) => lock(&a).get(&route).cloned().unwrap_or_else(|| route.sample()),
                None => Step::json(404, json!({ "error": format!("mock quota: nothing at {path}") })),
            }
        });
        Self { upstream, answers, sim }
    }

    /// `/sim/quota` answers from `sim`, per account.
    pub fn simulate(&self, sim: &SimQuota) {
        *lock(&self.sim) = Some(sim.clone());
    }

    /// `http://127.0.0.1:<port>` + the route's path and query.
    pub fn url(&self, route: QuotaRoute) -> String {
        self.upstream.url(route.path_and_query())
    }

    /// The base URL, `http://127.0.0.1:<port>`.
    pub fn base(&self) -> String {
        self.upstream.url("")
    }

    /// Sets the route's answer for every later call.
    pub fn set(&self, route: QuotaRoute, step: Step) {
        lock(&self.answers).insert(route, step);
    }

    /// Sets a JSON answer.
    pub fn set_json(&self, route: QuotaRoute, status: u16, body: Value) {
        self.set(route, Step::json(status, body));
    }

    /// Sets the gRPC-web credits body to raw `bytes`.
    pub fn set_grpc_bytes(&self, bytes: impl Into<Bytes>) {
        self.set(QuotaRoute::GrokCredits, Step::binary("application/grpc-web+proto", bytes));
    }

    /// `/v1/models` answers with these `x-ratelimit-*` (or any) headers.
    pub fn set_models_headers(&self, headers: &[(&str, &str)]) {
        let step = headers.iter().fold(QuotaRoute::Models.sample(), |s, (k, v)| s.with_header(k, v));
        self.set(QuotaRoute::Models, step);
    }

    /// Requests received on `route`.
    pub fn received(&self, route: QuotaRoute) -> Vec<Received> {
        self.upstream
            .received()
            .into_iter()
            .filter(|r| r.path_and_query.split('?').next() == Some(route.path()))
            .collect()
    }

    pub fn calls(&self, route: QuotaRoute) -> usize {
        self.received(route).len()
    }

    /// The underlying scripted upstream (one-off queued steps, all requests).
    pub fn upstream(&self) -> &MockUpstream {
        &self.upstream
    }
}

/// One window the simulated provider reports.
#[derive(Debug, Clone, PartialEq)]
pub struct SimWindow {
    pub name: String,
    /// `tokens`, `requests`, `credits` or `percent` (the `[quota]` units).
    pub unit: &'static str,
    pub used: f64,
    pub limit: f64,
    pub resets_at: String,
    /// What a served token costs in this window: `[input, cache_read, cache_write, output]`.
    pub weights: [f64; 4],
    /// For a `percent` window, the tokens that make 100%.
    pub capacity: f64,
}

impl SimWindow {
    pub fn new(name: &str, unit: &'static str, limit: f64, resets_at: &str) -> Self {
        Self {
            name: name.to_owned(),
            unit,
            used: 0.0,
            limit,
            resets_at: resets_at.to_owned(),
            weights: [1.0; 4],
            capacity: limit,
        }
    }

    pub fn weights(mut self, input: f64, cache_read: f64, cache_write: f64, output: f64) -> Self {
        self.weights = [input, cache_read, cache_write, output];
        self
    }

    pub fn capacity(mut self, tokens: f64) -> Self {
        self.capacity = tokens;
        self
    }

    pub fn used(mut self, used: f64) -> Self {
        self.used = used;
        self
    }

    /// What `d` adds to `used`.
    fn cost(&self, d: &super::mock_upstream::Served) -> f64 {
        if self.unit == "requests" {
            return d.requests as f64;
        }
        let [i, r, w, o] = self.weights;
        let tokens = d.input as f64 * i + d.cache_read as f64 * r + d.cache_write as f64 * w + d.output as f64 * o;
        if self.unit == "percent" { 100.0 * tokens / self.capacity } else { tokens }
    }
}

/// The provider side of a quota the router polls: windows per account, set by the test and
/// drawn down by what a [`CacheSim`](super::mock_upstream::CacheSim) serves, so "the estimate
/// equals the provider's own figure at every poll" is checkable (SC-008).
#[derive(Clone, Default)]
pub struct SimQuota {
    state: Arc<Mutex<SimQuotaState>>,
}

#[derive(Default)]
struct SimQuotaState {
    tokens: Vec<(String, String)>,
    windows: std::collections::BTreeMap<String, Vec<SimWindow>>,
    /// Answer every request with this status.
    failing: Option<u16>,
}

impl SimQuota {
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests carrying `secret` (bearer or `x-api-key`) are the account `label`.
    pub fn account(&self, secret: &str, label: &str) -> &Self {
        lock(&self.state).tokens.push((secret.to_owned(), label.to_owned()));
        self
    }

    /// Every request fails with `status` until called again with `None`.
    pub fn fail(&self, status: Option<u16>) {
        lock(&self.state).failing = status;
    }

    pub fn set(&self, label: &str, windows: Vec<SimWindow>) {
        lock(&self.state).windows.insert(label.to_owned(), windows);
    }

    pub fn windows(&self, label: &str) -> Vec<SimWindow> {
        lock(&self.state).windows.get(label).cloned().unwrap_or_default()
    }

    pub fn used(&self, label: &str, window: &str) -> Option<f64> {
        self.windows(label).iter().find(|w| w.name == window).map(|w| w.used)
    }

    /// The window reset: nothing used, a new reset time.
    pub fn reset(&self, label: &str, window: &str, resets_at: &str) {
        let mut st = lock(&self.state);
        if let Some(w) = st.windows.get_mut(label).and_then(|ws| ws.iter_mut().find(|w| w.name == window)) {
            w.used = 0.0;
            w.resets_at = resets_at.to_owned();
        }
    }

    /// Draws every window of an account down by what `sim` serves it.
    pub fn follow(&self, sim: &super::mock_upstream::CacheSim) {
        let this = self.clone();
        sim.on_served(move |label, d| {
            if let Some(ws) = lock(&this.state).windows.get_mut(label) {
                ws.iter_mut().for_each(|w| w.used += w.cost(d));
            }
        });
    }

    fn answer(&self, r: &Received) -> Step {
        let secret = r
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.strip_prefix("Bearer ").unwrap_or(v).to_owned())
            .or_else(|| r.headers.get("x-api-key").and_then(|v| v.to_str().ok()).map(str::to_owned))
            .unwrap_or_default();
        let st = lock(&self.state);
        if let Some(status) = st.failing {
            return Step::json(status, json!({ "error": "mock quota: failing" }));
        }
        let Some(label) = st.tokens.iter().find(|(t, _)| *t == secret).map(|(_, l)| l) else {
            return Step::json(401, json!({ "error": "mock quota: unknown account" }));
        };
        let windows: Vec<Value> = st
            .windows
            .get(label)
            .into_iter()
            .flatten()
            .map(|w| {
                json!({ "name": w.name, "unit": w.unit, "used": w.used, "limit": w.limit, "resets_at": w.resets_at })
            })
            .collect();
        Step::json(200, json!({ "windows": windows }))
    }

    /// The `[quota]` section a test plugin declares to read this endpoint: one rule per
    /// `(name, unit)`, since a rule's unit is fixed.
    pub fn quota_toml(url: &str, windows: &[(&str, &str)]) -> String {
        let mut out = format!("[quota]\naccounts = \"any\"\nrequest = {{ url = \"{url}\" }}\n");
        for (name, unit) in windows {
            out += &format!(
                "[[quota.window]]\npath = \"windows[*]\"\nwhere = {{ name = \"{name}\" }}\nname = \"{name}\"\nunit = \"{unit}\"\nused = \"used\"\nlimit = \"limit\"\nresets_at = \"resets_at\"\nresets_format = \"rfc3339\"\n"
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn routes_answer_their_samples_until_set() {
        let q = MockQuota::start().await;
        let c = reqwest::Client::new();
        let get = |u: String| c.get(u).send();
        let body = |b: Bytes| serde_json::from_slice::<Value>(&b).unwrap();

        let r = get(q.url(QuotaRoute::AnthropicUsage)).await.unwrap();
        assert_eq!(body(r.bytes().await.unwrap())["five_hour"]["utilization"], 87);
        q.set_json(QuotaRoute::OpencodeGo, 403, json!({ "error": { "type": "EntitlementError" } }));
        assert_eq!(get(q.url(QuotaRoute::OpencodeGo)).await.unwrap().status(), 403);
        assert_eq!(get(q.url(QuotaRoute::OpencodeZen)).await.unwrap().status(), 200);

        q.set_models_headers(&[("x-ratelimit-remaining-requests", "99")]);
        let r = get(q.url(QuotaRoute::Models)).await.unwrap();
        assert_eq!(r.headers()["x-ratelimit-remaining-requests"], "99");

        let r = c.post(q.url(QuotaRoute::GrokCredits)).body(vec![0u8; 5]).send().await.unwrap();
        let b = r.bytes().await.unwrap();
        assert_eq!(b[0], 0);
        let len = u32::from_be_bytes([b[1], b[2], b[3], b[4]]) as usize;
        assert_eq!(&b[5..8], &[0x0a, (len - 2) as u8, 0x0d]);
        assert_eq!(f32::from_le_bytes([b[8], b[9], b[10], b[11]]), 0.35);
        q.set_grpc_bytes(vec![1, 2, 3]);
        let r = c.post(q.url(QuotaRoute::GrokCredits)).send().await.unwrap();
        assert_eq!(&r.bytes().await.unwrap()[..], &[1, 2, 3]);

        assert_eq!(q.calls(QuotaRoute::GrokCredits), 2);
        assert_eq!(q.received(QuotaRoute::OpencodeGo).len(), 1);
        assert_eq!(get(q.url(QuotaRoute::GrokBilling)).await.unwrap().status(), 200);
        assert_eq!(q.received(QuotaRoute::GrokBilling)[0].path_and_query, "/v1/billing?format=credits");
        assert_eq!(get(q.upstream().url("/nope")).await.unwrap().status(), 404);
    }

    #[tokio::test]
    async fn sim_windows_follow_served_usage_and_reset() {
        use super::super::mock_upstream::CacheSim;
        let q = MockQuota::start().await;
        let sim = SimQuota::new();
        sim.account("sk-a", "a");
        sim.set(
            "a",
            vec![
                SimWindow::new("5h", "tokens", 1000.0, "2026-10-04T05:00:00Z").weights(1.0, 0.1, 1.25, 5.0),
                SimWindow::new("daily", "requests", 10.0, "2026-10-05T00:00:00Z"),
                SimWindow::new("weekly", "percent", 100.0, "2026-10-11T00:00:00Z").capacity(2000.0),
            ],
        );
        q.simulate(&sim);
        let up = MockUpstream::start().await;
        let cache = CacheSim::new(std::time::Duration::from_secs(300));
        cache.account("sk-a", "a").set_output_tokens(10);
        up.simulate_cache(&cache);
        sim.follow(&cache);
        let c = reqwest::Client::new();
        let body = json!({ "model": "m", "messages": [{ "role": "user", "content": "x".repeat(400) }] });
        c.post(up.url("/v1/chat/completions")).bearer_auth("sk-a").body(body.to_string()).send().await.unwrap();
        // A cold prompt of 100 tokens is written to the cache (weight 1.25); 10 output at 5.
        assert_eq!(sim.used("a", "5h"), Some(175.0));
        assert_eq!(sim.used("a", "daily"), Some(1.0));
        assert!((sim.used("a", "weekly").unwrap() - 100.0 * 110.0 / 2000.0).abs() < 1e-9);

        let get = |key: &str| c.get(q.url(QuotaRoute::Sim)).bearer_auth(key).send();
        let r = get("sk-a").await.unwrap();
        let b: Value = serde_json::from_slice(&r.bytes().await.unwrap()).unwrap();
        assert_eq!(
            b["windows"][0],
            json!({ "name": "5h", "unit": "tokens", "used": 175.0, "limit": 1000.0, "resets_at": "2026-10-04T05:00:00Z" })
        );
        assert_eq!(get("nobody").await.unwrap().status(), 401);
        sim.reset("a", "5h", "2026-10-04T10:00:00Z");
        assert_eq!(sim.windows("a")[0].used, 0.0);
        assert!(SimQuota::quota_toml("http://x/sim/quota", &[("5h", "tokens")]).contains("where = { name = \"5h\" }"));
    }
}

# Skill: js-to-rust-patterns

## Description

Quick-reference pattern cards for common JavaScript idioms found in 9router and their idiomatic Rust equivalents. Load this alongside `port-js-to-rust` when you need a fast lookup during translation work.

## Triggers

Load when you encounter a JS idiom and need the Rust mapping immediately, without running the full port workflow. This is the lookup companion to `port-js-to-rust`.

---

## Quick Reference Cards

### Nullish coalescing / optional chaining

```js
const baseUrl = credentials?.providerSpecificData?.baseUrl || OPENAI_COMPAT_BASE;
```
```rust
let base_url = credentials
    .provider_specific_data
    .as_ref()
    .and_then(|d| d.base_url.as_deref())
    .unwrap_or(OPENAI_COMPAT_BASE);
```

---

### Object spread / partial override

```js
const headers = { "Content-Type": "application/json", ...this.config.headers };
```
```rust
let mut headers = HeaderMap::new();
headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
for (k, v) in &self.config.headers {
    headers.insert(HeaderName::try_from(k.as_str())?, HeaderValue::try_from(v.as_str())?);
}
```

---

### Dynamic delete of header keys

```js
delete headers["anthropic-dangerous-direct-browser-access"];
```
```rust
headers.remove("anthropic-dangerous-direct-browser-access");
```

---

### String.startsWith guard

```js
if (this.provider?.startsWith?.("openai-compatible-")) { ... }
```
```rust
if self.provider.starts_with("openai-compatible-") { ... }
```
Or with an enum: `matches!(self.provider, Provider::OpenAICompatible(_))`

---

### Object.fromEntries / filter / map pipeline

```js
const REFRESH_GRANTS = Object.fromEntries(
  Object.entries(PROVIDER_OAUTH)
    .filter(([, o]) => o.refresh)
    .map(([id, o]) => [id, buildGrant(o)])
);
```
```rust
let refresh_grants: HashMap<ProviderId, RefreshGrant> = PROVIDER_OAUTH
    .iter()
    .filter(|(_, o)| o.refresh.is_some())
    .map(|(id, o)| (*id, build_grant(o)))
    .collect();
```

---

### Promise.all (concurrent, all must succeed)

```js
const [a, b] = await Promise.all([fetchA(), fetchB()]);
```
```rust
let (a, b) = tokio::try_join!(fetch_a(), fetch_b())?;
```

---

### Promise.race (first wins)

```js
const result = await Promise.race([fetchA(), timeout(5000)]);
```
```rust
tokio::select! {
    res = fetch_a() => res?,
    _ = tokio::time::sleep(Duration::from_secs(5)) => return Err(Error::Timeout),
}
```

---

### setTimeout / clearTimeout (connect timeout)

```js
const timer = setTimeout(() => ctrl.abort(), timeoutMs);
try { const r = await fetch(..., { signal }); clearTimeout(timer); return r; }
catch (e) { clearTimeout(timer); throw e; }
```
```rust
tokio::time::timeout(Duration::from_millis(timeout_ms), do_fetch(url, req))
    .await
    .map_err(|_| ExecutorError::ConnectTimeout)?
```

---

### Regex test

```js
if (/^claude-/.test(model)) { ... }
```
```rust
if model.starts_with("claude-") { ... }
// or, for a real regex:
static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^claude-").unwrap());
if RE.is_match(model) { ... }
```

---

### URLSearchParams (form body)

```js
body: new URLSearchParams({ grant_type: "refresh_token", client_id: cfg.clientId })
```
```rust
let form = [("grant_type", "refresh_token"), ("client_id", &cfg.client_id)];
let body = serde_urlencoded::to_string(&form)?;
// header: Content-Type: application/x-www-form-urlencoded
```

---

### btoa / atob (base64)

```js
const basicAuth = btoa(`${clientId}:${clientSecret}`);
```
```rust
use base64::{engine::general_purpose::STANDARD, Engine};
let basic_auth = STANDARD.encode(format!("{}:{}", client_id, client_secret));
```

---

### try/catch → return null (fail-open)

```js
try { return doThing(); } catch { return null; }
```
```rust
do_thing().ok()  // Result<T, E> → Option<T>
// or:
do_thing().unwrap_or_else(|_| default_value)
```

---

### JSON.stringify / JSON.parse

```js
const bodyStr = JSON.stringify(transformedBody);
const obj = JSON.parse(text);
```
```rust
let body_str = serde_json::to_string(&transformed_body)?;
let obj: MyType = serde_json::from_str(&text)?;
// or dynamically:
let obj: serde_json::Value = serde_json::from_str(&text)?;
```

---

### Array.isArray + guard

```js
if (!Array.isArray(body.messages)) return body;
```
```rust
// If messages is typed as Vec<Message>, no runtime check needed.
// If it's a serde_json::Value:
let Some(messages) = body.get("messages").and_then(|v| v.as_array()) else {
    return body;
};
```

---

### typeof check

```js
if (typeof model === "string" && /^claude-/.test(model)) { ... }
```
```rust
// In Rust, model is already typed. The typeof check disappears.
if model.starts_with("claude-") { ... }
```

---

### Constructor (factory function)

```js
// 9router executors do not expose a static create() factory —
// they are constructed inline per-request.
function makeExecutor(provider, config) { return new DefaultExecutor(provider, config); }
```
```rust
impl DefaultExecutor {
    pub fn new(provider: BuiltinProvider, config: ProviderConfig) -> Self {
        Self { provider, config }
    }
}
```

---

## Crate recommendations

| Need | JS | Rust crate |
|---|---|---|
| HTTP client | `fetch` | `reqwest` |
| SSE server | `res.write` | `axum` + `axum::response::sse` |
| JSON | `JSON.*` | `serde_json` |
| URL encoding | `URLSearchParams` | `serde_urlencoded` |
| Base64 | `btoa`/`atob` | `base64` |
| Regex | `RegExp` | `regex` (with `LazyLock`) |
| Async runtime | Node event loop | `tokio` |
| Async streams | `Transform` / `pipe` | `futures::StreamExt` |
| Cancellation | `AbortController` | `tokio_util::sync::CancellationToken` |
| Config/env | `process.env` | `config` crate or `std::env` |
| SQLite | `better-sqlite3` / `sql.js` | `sqlx` with `sqlite` feature |
| Logging | `console.log` / custom | `tracing` |
| Intra-binary registration | import side-effects | `inventory` crate (built-ins only; never for plugins) |

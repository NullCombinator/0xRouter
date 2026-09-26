# Skill: port-js-to-rust

## Description

Systematic workflow for porting a JavaScript (ESM) module from the 9router codebase to idiomatic Rust for 0router. Covers the full pattern catalog, file-by-file decision checklist, and validation gate.

## Triggers

Load this skill when:
- Translating any `open-sse/`, `src/`, or `cli/` file from 9router to Rust
- Designing a Rust module that corresponds to a JS counterpart
- Choosing between trait objects vs. generics for something that was a JS class hierarchy
- Deciding how to represent a JS config-driven registry in Rust

---

## Pattern Catalog

### 1. Class inheritance → Trait with default methods

**JS pattern (BaseExecutor)**
```js
class BaseExecutor {
  buildHeaders(credentials, stream) { ... }
  transformRequest(model, body, stream, credentials) { return body; } // override in subclass
  async execute({ model, body, stream, credentials, signal, log }) { ... }
}
class DefaultExecutor extends BaseExecutor {
  transformRequest(model, body) { /* override */ }
}
```

**Rust translation**
```rust
use async_trait::async_trait;

// async fn in a trait is NOT dyn-compatible without async_trait (or a boxed future).
// async_trait rewrites the async fn to return Pin<Box<dyn Future + Send>>.
#[async_trait]
pub trait Executor: Send + Sync {
    fn build_url(&self, model: &str, stream: bool, url_index: usize, creds: &Credentials) -> String;
    fn build_headers(&self, creds: &Credentials, stream: bool) -> HeaderMap;
    fn transform_request(&self, model: &str, body: RequestBody) -> RequestBody { body } // default = passthrough
    async fn execute(&self, req: ExecuteRequest) -> Result<ExecuteResult, ExecutorError>;
}

pub struct DefaultExecutor { pub provider: String, pub config: ProviderConfig }
#[async_trait]
impl Executor for DefaultExecutor { ... }
```

**Decision rule**: If the override set is closed at compile time → use an enum. If runtime dispatch is needed → `Box<dyn Executor>` with `async_trait`. For 0router's built-in executor set, prefer an enum; `Box<dyn Executor>` is reserved for future plugin ABIs that genuinely require runtime dispatch (not today). Plugins never contribute Rust code.

---

### 2. Dynamic dispatch maps → enum dispatch

**JS pattern**
```js
const refreshers = {
  claude: () => this.refreshFromGrant(credentials, proxyOptions),
  kiro:   () => this.refreshKiro(credentials.refreshToken, proxyOptions),
  gemini: () => this.refreshFromGrant(credentials, proxyOptions),
};
const refresher = refreshers[this.provider];
if (!refresher) return null;
```

**Rust translation**
```rust
pub enum BuiltinProvider { Claude, Kiro, Gemini, ... }

impl BuiltinProvider {
    pub async fn refresh_credentials(&self, creds: &Credentials, proxy: Option<&ProxyConfig>)
        -> Result<Option<TokenPair>, OAuthError>
    {
        match self {
            Self::Claude | Self::Gemini => self.refresh_from_grant(creds, proxy).await,
            Self::Kiro => self.refresh_kiro(&creds.refresh_token, proxy).await,
            ...
        }
    }
}
```

**Rule**: Prefer `match` over trait objects for closed provider sets. Use `Option::None` (not `null`) for "provider has no refresher".

---

### 3. AbortController / AbortSignal → CancellationToken

**JS pattern**
```js
const connectCtrl = new AbortController();
const timer = setTimeout(() => connectCtrl.abort(), timeoutMs);
const mergedSignal = AbortSignal.any([signal, connectCtrl.signal]);
const response = await fetch(url, { signal: mergedSignal, ... });
clearTimeout(timer);
```

**Rust translation (tokio)**
```rust
use tokio_util::sync::CancellationToken;
use tokio::time::timeout;

// JS times out only the connect/headers phase and maps it to a retryable 502.
// It then clears the timer after headers arrive (base.js:137,150).
// Wrap only the connect+headers call, not the full streaming response.
let connect_result = tokio::select! {
    r = timeout(Duration::from_millis(connect_timeout_ms), connect_and_get_headers(url, &child)) => {
        match r {
            // Elapsed: map to 502 (retryable) so the retry loop can try the next URL
            Err(_elapsed) => Err(ExecutorError::StatusCode(502)),
            Ok(inner) => inner,
        }
    }
    _ = caller_token.cancelled() => return Err(ExecutorError::Cancelled),
};
// Once headers are received, stream the body without a per-chunk timeout.
```

**Rule**: Map `AbortSignal` parameters to `CancellationToken` passed by value. `AbortSignal.any([a, b])` → `select!` over two `.cancelled()` futures. The connect timeout maps to a retryable 502, not `Err(Elapsed)` — the retry loop needs a status code, not an error type.

---

### 4. Side-effect self-registration → static table (or `inventory` for intra-binary use)

**JS pattern**
```js
// translator/request/openai-to-claude.js
register("openai", "claude", reqFn, resFn); // runs on import as side effect

// translator/index.js
import "./request/openai-to-claude.js";     // triggers registration
```

**Rust translation**
```rust
// Explicit static table (preferred)
pub fn builtin_translators() -> Vec<TranslatorEntry> {
    vec![
        TranslatorEntry { from: Format::OpenAI, to: Format::Claude, ... },
        TranslatorEntry { from: Format::Claude, to: Format::Kiro, ... },
    ]
}

// inventory (intra-binary only, when a static table would span many files)
// NOTE: inventory uses link-time registration — it can only register code
// compiled into the same binary. It cannot accept contributions from plugins.
// Plugins are TOML data; they never contribute Rust functions via inventory.
use inventory;
inventory::submit!(TranslatorRegistration {
    from: Format::OpenAI, to: Format::Claude,
    translate_req: openai_to_claude_req,
    translate_res: openai_to_claude_res,
});
// Somewhere in the crate root: inventory::collect!(TranslatorRegistration);
```

**Rule**: Prefer the explicit static table. Use `inventory` only for intra-binary registration where the table would otherwise span many files inconveniently. `inventory` MUST NOT be used for plugin extension points — plugins are data, not code, and cannot contribute link-time registrations.

---

### 5. SSE streaming → axum SSE + futures::Stream

**JS pattern**
```js
// returns res.write("data: ...\n\n") style
res.setHeader("Content-Type", "text/event-stream");
upstreamResponse.body.pipe(transformStream).pipe(res);
```

**Rust translation (axum)**
```rust
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::{self, StreamExt};

pub async fn chat_handler(...) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = upstream_stream
        .map(|chunk| translate_chunk(chunk))
        .map(|ev| Ok(Event::default().data(ev)));
    Sse::new(stream).keep_alive(KeepAlive::default())
}
```

**Rule**: Never buffer a full streaming response. Chain `Stream` adaptors. Use `Bytes` not `String` until the final SSE `Event::data()` call.

---

### 6. Config-driven JS objects → serde structs

**JS pattern**
```js
const PROVIDERS = {
  claude: { baseUrl: "https://api.anthropic.com/v1/messages", format: "claude", headers: {...} },
  openai: { baseUrl: "https://api.openai.com/v1/chat/completions", format: "openai" },
};
```

**Rust translation**
```rust
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfig {
    pub base_url: String,
    pub format: ProviderFormat,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub quirks: ProviderQuirks,
}

// Embed as TOML/JSON in the binary for builtin providers
const BUILTIN_PROVIDERS: &str = include_str!("../config/providers.toml");
```

**Rule**: Never match on string keys at runtime for known providers. Use a typed enum + serde for the config layer, then convert to the enum at deserialization time.

---

### 7. Fail-open middleware → `Option<T>` pipeline

**Note on rtk**: 9router's `rtk/` (compressMessages, headroom, pxpipe, caveman, ponytail)
is a token-optimization suite that runs *before* the routing decision. init.md places
token optimization in a separate upstream hop. **Do not port rtk to 0router.**

The fail-open *pattern* (try → null on error, caller uses original) appears elsewhere in
9router (e.g. body transforms, optional feature hooks). The pattern applies there.

**JS pattern (general fail-open hook)**
```js
// A hook that may transform a value; returns null on any failure (fail-open).
async function maybeTransform(value) {
  try {
    return await doTransform(value);
  } catch {
    return null; // caller uses original if null
  }
}
const transformed = await maybeTransform(value);
const out = transformed ?? value;
```

**Rust translation**
```rust
pub async fn maybe_transform(value: Payload) -> Option<Payload> {
    transform_internal(value).await.ok()  // Any error → None
}

// Caller:
let value = maybe_transform(value.clone()).await.unwrap_or(value);
```

**Rule**: Fail-open hooks return `Option<T>`. Never propagate errors out of them. The `.ok()` combinator on `Result<T, E>` is the idiomatic bridge. The key invariant: if the hook fails for any reason, the pipeline continues with the original value unmodified — it never fails closed.

---

### 8. Retry loop with URL fallback → custom retry combinator

**JS pattern** (from `open-sse/executors/base.js:83–183`, `runtimeConfig.js:71–93`)

There are **two distinct mechanisms** — keep them separate or the semantics are wrong:

```js
// Mechanism 1 — tryRetry(urlIndex, statusKey): same-URL retry.
//   Reads DEFAULT_RETRY_CONFIG[statusKey].attempts from runtimeConfig.js.
//   Default retry config (runtimeConfig.js:79–82):
//     429 → { attempts: 0, delayMs: 0 }   ← ZERO same-URL retries
//     502 → { attempts: 3, delayMs: 3000 } ← 3 same-URL retries, 3 s gap
//     503 → { attempts: 3, delayMs: 2000 } ← 3 same-URL retries, 2 s gap
//     504 → { attempts: 2, delayMs: 3000 } ← 2 same-URL retries, 3 s gap
//   Returns true → caller does `urlIndex--; continue` (outer for-loop undoes the ++)
//   Returns false (attempts==0 or exhausted) → falls through to mechanism 2.

// Mechanism 2 — shouldRetry(status, urlIndex): 429-only URL advance.
//   Returns true ONLY for RATE_LIMITED (429) when more fallback URLs remain.
//   Returns true → `continue` (outer for-loop advances urlIndex).
//   Despite the name, this does NOT retry the same URL; it moves to the next one.

shouldRetry(status, urlIndex) {
  return status === HTTP_STATUS.RATE_LIMITED && urlIndex + 1 < this.getFallbackCount();
}

// In execute():
const response = await fetch(fallbackUrls[urlIndex], ...);
if (await tryRetry(urlIndex, response.status, ...)) { urlIndex--; continue; } // same URL (502/503/504)
if (this.shouldRetry(response.status, urlIndex))   { continue; }              // next URL (429 only)
return response;

// Network errors use 502's retry config, then advance URL:
// if (await tryRetry(urlIndex, HTTP_STATUS.BAD_GATEWAY, ...)) { urlIndex--; continue; } // same URL
// if (urlIndex + 1 < fallbackCount) continue;                                           // advance URL
// throw error;
```

**Rust translation**
```rust
pub async fn execute_with_fallback(
    urls: &[Url],
    req: &Request,
    retry_cfg: &RetryConfig,
    cancel: CancellationToken,
) -> Result<Response, ExecutorError> {
    let mut per_url_attempts: Vec<u32> = vec![0; urls.len()];
    let mut url_index = 0;

    while url_index < urls.len() {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ExecutorError::Cancelled),
            result = attempt(&urls[url_index], req) => {
                match result {
                    Ok(resp) => {
                        // Mechanism 1: same-URL retry for 502/503/504 (attempts > 0)
                        let entry = retry_cfg.for_status(resp.status());
                        if entry.attempts > 0 && per_url_attempts[url_index] < entry.attempts {
                            per_url_attempts[url_index] += 1;
                            tokio::time::sleep(Duration::from_millis(entry.delay_ms)).await;
                            continue; // same URL
                        }
                        // Mechanism 2: 429 → URL advance (attempts == 0 by default)
                        if resp.status() == StatusCode::TOO_MANY_REQUESTS
                            && url_index + 1 < urls.len()
                        {
                            url_index += 1;
                            continue; // next URL
                        }
                        return Ok(resp);
                    }
                    Err(e) if is_network_error(&e) => {
                        // Network errors use 502 retry config for same-URL retries
                        let entry = retry_cfg.for_status(StatusCode::BAD_GATEWAY);
                        if entry.attempts > 0 && per_url_attempts[url_index] < entry.attempts {
                            per_url_attempts[url_index] += 1;
                            tokio::time::sleep(Duration::from_millis(entry.delay_ms)).await;
                            continue; // same URL
                        }
                        if url_index + 1 < urls.len() {
                            url_index += 1;
                            continue; // advance URL on network error after attempts exhausted
                        }
                        return Err(ExecutorError::Network(e));
                    }
                    Err(e) => return Err(ExecutorError::Network(e)),
                }
            }
        }
    }
    Err(ExecutorError::AllUrlsFailed)
}
```

**Rule**: 429 advances to the **next URL** by default — it has zero same-URL attempts in the default config (`runtimeConfig.js:79`). 502, 503, and 504 retry the **same URL** (3×, 3×, 2× respectively). A provider can override these via `config.retry` merged at `base.js:107`. Keep the two mechanisms separate: `tryRetry` for same-URL retries, `shouldRetry` for URL advance.

---

### 9. SQLite adapter chain → sqlx with feature flags

**JS pattern**
```js
// driver.js — tries bun:sqlite → better-sqlite3 → node:sqlite → sql.js
async function openDb(path) {
  return tryBun(path) ?? tryBetterSqlite3(path) ?? tryNodeSqlite(path) ?? trySqlJs(path);
}
```

**Rust translation**
```rust
// Cargo.toml
[dependencies]
sqlx = { version = "0.8", features = ["sqlite", "runtime-tokio"] }

// db.rs — single driver, no runtime fallback needed
pub async fn open(path: &Path) -> Result<SqlitePool, DbError> {
    SqlitePoolOptions::new()
        .connect_with(SqliteConnectOptions::new().filename(path).create_if_missing(true))
        .await
        .map_err(DbError::from)
}
```

**Rule**: Rust doesn't need a runtime adapter chain. Ship one driver (`sqlx` + `sqlite` feature). The JS chain existed because JS runtimes differ; in Rust the target is fixed at compile time.

---

### 10. OpenAI-pivot translator → type-state with format enums

**JS pattern**
```js
// Pivot: client format → OpenAI → provider format (lossy double-hop)
// Direct route: registered source:target pair skips pivot (e.g. claude:kiro)
```

**Rust translation**
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format { OpenAI, Claude, Gemini, Kiro, /* ... */ }

pub struct TranslateKey { pub from: Format, pub to: Format }

pub fn translate_request(key: TranslateKey, body: RequestBody) -> RequestBody {
    if let Some(direct) = DIRECT_ROUTES.get(&key) {
        return direct(body);  // skip pivot
    }
    let openai = to_openai(key.from, body);
    from_openai(key.to, openai)
}
```

**Rule**: Register direct routes for format pairs that are lossy through the OpenAI pivot (thinking blocks, tool IDs, non-base64 images, `is_error`). Check `open-sse/AGENTS.md` "Pitfalls" section for the current lossy-pair list.

---

## Porting Workflow

For each JS file being ported:

1. **Identify the pattern** — which of the 10 patterns above apply?
2. **Map data shapes** — JS `{}` objects → Rust structs + enums. Document each field's optionality.
3. **Map async boundaries** — `async function` → `async fn`. `Promise.all` → `tokio::join!`. `Promise.race` → `tokio::select!`.
4. **Map error handling** — `try/catch + return null` (fail-open) → `Option<T>`. `throw` (propagating) → `Result<T, E>`.
5. **Identify state** — JS class `this.field` state → Rust struct fields. Shared mutable state → `Arc<Mutex<T>>` or `Arc<RwLock<T>>`.
6. **Write the Rust module** — use `rust-engineer` agent for implementation details.
7. **Run parity audit** — invoke `/rust-parity-audit` skill before closing the port.

### Build order (reference-informed, not 9router import order)

9router's import graph has cycles (translator → executors → config/kiroConstants →
translator/concerns), so its file order is not a valid build order for 0router.
Build the first testable slice instead:

1. **Provider + model registry** — typed Rust structs/enums for provider entities,
   unified models, and error classification; parity-test against 9router's
   `tests/__baseline__/snapshot-providers.mjs` and `verify-alias.mjs`.
2. **Translator schema** — Rust enums for ROLE, BLOCK types, FORMAT; pure conversions.
3. **Translator concerns + request/response** — pure translation functions; test against
   9router's unit test suite for each translator pair.
4. **Executor trait + DefaultExecutor** — `Executor` trait (with `async_trait`),
   `DefaultExecutor` impl; retry loop; fail-open error paths.
5. **Provider-specific executors** — one `impl Executor` per provider with special auth
   or request-shape quirks.
6. **chatCore equivalent** — request handler composing translator + executor + routing.
7. **Axum router** — SSE handler, keep-alive, cancellation via `CancelOnDrop`.

**rtk is not in this list.** rtk (token compression) is upstream of 0router. Do not port it.

---

## Checklist before marking a port done

- [ ] All JS `null` returns on error converted to `Option::None`
- [ ] All `throw` on unrecoverable converted to `Err(...)` with a typed error
- [ ] No `.unwrap()` on `Option`/`Result` outside tests
- [ ] Streaming path uses `Stream`, not a buffered `Vec`
- [ ] Config fields use typed enums, not string matching
- [ ] Retry loop preserves the per-URL attempt counter semantics
- [ ] Direct translator routes registered for all lossy pairs
- [ ] `cargo clippy --all-targets -- -D warnings` passes
- [ ] Unit tests cover the fail-open path (middleware returns `None`)

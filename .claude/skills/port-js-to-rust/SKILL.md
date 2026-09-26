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
pub trait Executor: Send + Sync {
    fn build_url(&self, model: &str, stream: bool, url_index: usize, creds: &Credentials) -> String;
    fn build_headers(&self, creds: &Credentials, stream: bool) -> HeaderMap;
    fn transform_request(&self, model: &str, body: RequestBody) -> RequestBody { body } // default = passthrough
    async fn execute(&self, req: ExecuteRequest) -> Result<ExecuteResult, ExecutorError>;
}

pub struct DefaultExecutor { pub provider: String, pub config: ProviderConfig }
impl Executor for DefaultExecutor { ... }
```

**Decision rule**: If the override set is closed at compile time → use an enum. If plugins add executors at runtime → use `Box<dyn Executor>`. For 0router, providers are declared as data-not-code plugins, so the core set is compile-time: prefer an enum.

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

let token = CancellationToken::new();
let child = token.child_token();

let result = tokio::select! {
    res = timeout(Duration::from_millis(timeout_ms), do_request(url, child)) => res?,
    _ = caller_token.cancelled() => return Err(ExecutorError::Cancelled),
};
```

**Rule**: Map `AbortSignal` parameters to `CancellationToken` passed by value. `AbortSignal.any([a, b])` → `select!` over two `.cancelled()` futures.

---

### 4. Side-effect self-registration → inventory crate (or build-time table)

**JS pattern**
```js
// translator/request/openai-to-claude.js
register("openai", "claude", reqFn, resFn); // runs on import as side effect

// translator/index.js
import "./request/openai-to-claude.js";     // triggers registration
```

**Rust translation**
```rust
// Option A — inventory (distributed registration, good for plugins)
use inventory;
inventory::submit!(TranslatorRegistration {
    from: Format::OpenAI, to: Format::Claude,
    translate_req: openai_to_claude_req,
    translate_res: openai_to_claude_res,
});

// Option B — explicit table (simpler, no proc-macro dep)
pub fn builtin_translators() -> Vec<TranslatorEntry> {
    vec![
        TranslatorEntry { from: Format::OpenAI, to: Format::Claude, ... },
        TranslatorEntry { from: Format::Claude, to: Format::Kiro, ... },
    ]
}
```

**Rule**: For 0router's declarative plugin design (data-not-code), prefer Option B with a static table. Reserve `inventory` for a future plugin ABI where third-party crates contribute translators.

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

### 7. Fail-open RTK middleware → `Option<T>` pipeline

**JS pattern**
```js
// rtk/index.js — mutates in-place, returns null on error (fail-open)
async function compressMessages(body) {
  try {
    return doCompress(body);
  } catch {
    return null; // caller uses original if null
  }
}
const compressed = await compressMessages(body);
const outBody = compressed ?? body;
```

**Rust translation**
```rust
pub async fn compress_messages(body: RequestBody) -> Option<RequestBody> {
    compress_internal(body).await.ok()  // Any error → None
}

// Caller:
let body = compress_messages(body.clone()).await.unwrap_or(body);
```

**Rule**: Fail-open hooks return `Option<T>`. Never propagate errors out of them. The `.ok()` combinator on `Result<T, E>` is the idiomatic bridge.

---

### 8. Retry loop with URL fallback → custom retry combinator

**JS pattern**
```js
for (let urlIndex = 0; urlIndex < fallbackCount; urlIndex++) {
  try {
    const response = await fetch(urls[urlIndex], ...);
    if (shouldRetry(response.status)) { urlIndex--; continue; }
    return response;
  } catch (e) {
    if (urlIndex + 1 >= fallbackCount) throw e;
  }
}
```

**Rust translation**
```rust
pub async fn execute_with_fallback(urls: &[Url], req: &Request) -> Result<Response, ExecutorError> {
    let mut last_err = None;
    for url in urls {
        match attempt(url, req).await {
            Ok(resp) if resp.status() == StatusCode::TOO_MANY_REQUESTS => {
                last_err = Some(ExecutorError::RateLimited);
                continue;  // try next URL
            }
            Ok(resp) => return Ok(resp),
            Err(e) => { last_err = Some(e); continue; }
        }
    }
    Err(last_err.unwrap_or(ExecutorError::NoUrls))
}
```

**Rule**: The JS `urlIndex--; continue` trick (retry same URL) maps to a recursive call or an explicit inner loop. Keep the outer loop for URL progression and a separate retry counter per URL.

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

### File priority order (recommended)

Port in dependency order, bottom-up:

1. `config/` → typed Rust config structs + enums (no logic, easy start)
2. `translator/schema/` → Rust enums for ROLE, BLOCK types, FORMAT
3. `translator/concerns/` → pure functions, easiest to test
4. `translator/request/` + `translator/response/` → translation functions
5. `executors/base.js` → `Executor` trait
6. `executors/default.js` → `DefaultExecutor` impl
7. `executors/*.js` → one `impl Executor` per special provider
8. `rtk/` → fail-open middleware pipeline
9. `handlers/chatCore.js` → main request handler
10. `src/sse/` + routing glue → axum router

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

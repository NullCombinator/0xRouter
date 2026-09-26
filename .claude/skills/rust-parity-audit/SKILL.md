# Skill: rust-parity-audit

## Description

Verifies that a newly written Rust module is behaviorally equivalent to its JavaScript counterpart in 9router. Structured as a read-only audit that produces a findings list — it does not fix code.

## Triggers

Load this skill when:
- A JS→Rust port is complete and needs validation before merging
- A behavior discrepancy is suspected between 0router (Rust) and 9router (JS)
- Reviewing a PR that replaces a JS module with Rust

---

## Audit Protocol

### Step 1: Establish the scope

Identify the exact JS file(s) this Rust module replaces.

```
JS source:  ref/9router/open-sse/<path>
Rust port:  src/<crate>/<path>.rs
```

For each pair, record:
- Module purpose (one sentence)
- Public interface: functions, methods, types
- Side effects: mutations, logging, DB writes, network calls

---

### Step 2: Interface parity check

For every public function/method in the JS file:

| JS signature | Rust signature | Notes |
|---|---|---|
| `buildUrl(model, stream, urlIndex, credentials)` | `fn build_url(&self, model: &str, stream: bool, url_index: usize, creds: &Credentials) -> String` | ✓ |
| `transformRequest(model, body)` | `fn transform_request(&self, model: &str, body: RequestBody) -> RequestBody` | ✓ |

Flag any:
- Missing functions (JS has it, Rust doesn't)
- Signature narrowing that drops an optional field
- Return type widening (JS returned nullable, Rust returns non-optional — did the caller handle null?)

---

### Step 3: Error contract parity

For each function, compare the JS and Rust error contracts:

| Scenario | JS behavior | Rust behavior | Match? |
|---|---|---|---|
| Network timeout | Returns null (fail-open) | Returns `None` | ✓ |
| Upstream 429 | `urlIndex--; continue` | `continue` to next URL | ✓ |
| Bad credentials | Throws ExecutorError | `Err(ExecutorError::BadCredentials)` | ✓ |
| AbortSignal fired | Throws AbortError, propagated | `Err(ExecutorError::Cancelled)` | ✓ |

Special attention to **fail-open** paths (RTK, headroom, caveman, ponytail) — these must return `None`/original on any error, never panic or return `Err`.

---

### Step 4: State and mutation parity

- JS mutates request body in-place (RTK `compressMessages`). Rust clone-on-write: original untouched if middleware returns `None`.
- JS class fields (`this.provider`, `this.config`) → Rust struct fields. Confirm no field was dropped.
- Shared mutable state (`Map`, `Set` in closures) → confirm Rust uses `Arc<Mutex<T>>` or `Arc<RwLock<T>>` with equivalent access semantics.

---

### Step 5: Streaming contract parity

If the module touches SSE:
- [ ] JS `res.write` side effects → Rust `Stream::poll_next` yields equivalent events
- [ ] Back-pressure: JS `pipe` → Rust `Stream` + axum's built-in back-pressure
- [ ] Cancellation: JS `signal.abort()` → Rust `CancellationToken::cancel()` observed in the stream loop
- [ ] `[DONE]` sentinel forwarded correctly in both

---

### Step 6: Configuration surface parity

For each constant/config from `open-sse/config/`:
- [ ] All constants present in Rust `config/` module
- [ ] Default values match (timeouts, retry counts, status codes)
- [ ] Provider-specific quirks flags (`dropClientMetadata`, `urlSuffix`) present and respected

---

### Step 7: Translator route coverage

Compare `open-sse/translator/index.js` imports vs. Rust `DIRECT_ROUTES` + pivot table:

- [ ] Every `register(from, to, ...)` in JS has a corresponding Rust entry
- [ ] All lossy pairs (claude:kiro, openai:cursor, etc.) have a direct route in Rust (not pivot)
- [ ] No pivot route used where JS had a direct route

---

### Step 8: Test coverage gate

- [ ] At least one unit test per fail-open path
- [ ] At least one unit test for each retry scenario (429, network error, timeout)
- [ ] At least one test for the streaming cancellation path
- [ ] Property tests for translator round-trips (if fragile format pairs exist)

---

## Output format

Produce a findings table:

| # | File | Pattern | Finding | Severity |
|---|---|---|---|---|
| 1 | `executors/default.rs` | Error contract | `applyJsonSchemaFallback` returns empty messages on edge case; JS returned original body | High |
| 2 | `rtk/mod.rs` | Fail-open | `compress_messages` panics on malformed UTF-8; should return `None` | Critical |
| 3 | `translator/request/openai_to_claude.rs` | Direct route | Missing direct route for openai→claude pair; falls through lossy pivot | High |

Severity:
- **Critical** — behavioral difference that will surface as an error or data loss in production
- **High** — behavioral difference that affects correctness in edge cases
- **Medium** — API surface mismatch (missing option, narrowed type) that limits future use
- **Low** — cosmetic / naming inconsistency

Any Critical or High findings must be resolved before the port is merged.

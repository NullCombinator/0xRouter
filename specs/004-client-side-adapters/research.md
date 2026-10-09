# Research: Client Side, Harness Adapters (slice 004)

Phase 0 output of `/speckit-plan`. Every item is a technical decision Claude made; technical
choices are Claude's to make (user direction, 2026-09-27). Items that change something the user
can see are marked **(user-visible)** and are reported to the user with the plan.

Sources:
- `ref/9router` at the slice 003 pin, judged on chatCore's request path;
- the scope brief `specs/briefs/2026-09-28-client-side-adapters.md`, including its clarify
  additions and P notes;
- slice 003's plan, research (R1–R27) and code on branch `003-request-pipeline`;
- crates.io metadata fetched on 2026-09-28 (versions and MSRVs below).

No NEEDS CLARIFICATION remains. One fact needs a live check with operator keys, which is not
possible while planning: which chosen providers reject hermes's echoed reasoning (R4). The plan
fixes the rule and the fallback; the check is a task.

**Dependency on slice 003.** 0router's attempt loop (003 T048), style translators (T049–T056)
and records display (T106) are not built yet. This slice hooks into them, so it is implemented
after them (spec Assumptions).

---

## R1. Toolchain and crates

**Decision**:
- **WASM runtime: `wasmtime` 45.x** on the host, with `cranelift`, `async` and
  `pooling-allocator`, and without `wasi`, `component-model`, `cache`, `threads` or `gc`.
  wasmtime 45 needs rustc 1.93, and 1.93.1 is installed. wasmtime 46 and later need 1.94 or
  newer, and 49 needs 1.96.
- **Workspace MSRV rises from 1.89 to 1.93.** Only the crates that link the sandbox need it, but
  the `nullrouter` binary links them all.
- **Guest target: `wasm32-unknown-unknown`**, not WASI. The guest has no imports beyond the kit's
  host ABI (R2), so it has no system interface to misuse.
- **Source tooling: `syn` 2 (`full`, `visit`) and `prettyplease` 0.2** for the gate (R7) and the
  scrambler (R10). `flate2` and `tar` unpack catalogue archives (R12). `sha2` is already a
  workspace dependency.
- **The kit's guest-side dependencies are `serde` and `serde_json` only.** The builder vendors
  them (R8).

**Rationale**:
- wasmtime compiles each module to native code once, when it loads, and runs it at near-native
  speed. An adapter runs on every request of its key, so this matters. SC-010 allows 5 ms p95,
  and a Claude Code body can hold hundreds of KB.
- wasmtime has epoch interruption and a resource limiter (R3), and it is the most-audited
  runtime.
- `Module::new` compiles from WASM bytes through the safe API. The core never calls the unsafe
  `Module::deserialize`, which would trust precompiled native code from outside.

**Alternatives**:
- `wasmi` 2.0: a pure-Rust interpreter with fuel metering and MSRV 1.86. It is 5–20× slower,
  which puts SC-010 at risk on large bodies. It is kept as the fallback if wasmtime's build cost
  proves too high.
- The component model with WIT and `wit-bindgen`: the adapter would have to invoke a procedural
  macro, which FR-011 forbids. A core module with a small ABI needs none.
- WASI preview 2: it adds a system interface that we would then have to deny piece by piece.

## R2. Where an adapter runs, and what it sees

**Decision**:
- **Placement.** An adapter runs once per attempt, on the **client-style body**, before slice
  003's `forward` (same-style) or `decode`/`encode` (cross-style). The attempt context tells it
  the target:
  - provider id;
  - the endpoint's wire style;
  - `same_style`;
  - upstream model id;
  - model type;
  - the model's declared capabilities (vision, file input).

  On the way back it runs on the **client-style response**:
  - once per stream event, after the event is re-encoded for the client;
  - once for a non-stream body.

  A fallback attempt re-runs it against the original client body, with the new target in the
  context (spec Edge Cases, "fallback").
- **Selectors.** An adapter's manifest (`adapter.toml`, contracts/adapter-package.md) declares
  path patterns for requests and responses, for example `messages[*].images` or
  `messages[*].content[*].type`. `*` matches any array index or object key. The host walks the
  parsed body natively and sends the guest only the matching subtrees, each with its concrete
  path. The pattern `$` means the whole body.
- **Edits, not bodies.** The guest returns a list of edits: `remove(path)` or
  `replace(path, value)`. Each edit carries a `kind` (`removed` or `converted`) and a `reason`
  code. The host checks the edits (R5), applies them to a copy, and runs the guardrail (R6).
  Discarding an adapter's changes means not applying its edits. The original body is never
  touched.
- **Reason codes** form a closed set in the kit, for example `target_rejects_field`,
  `target_cannot_carry_block`, `foreign_block`, `format_conversion`, `empty_after_removal` and
  `param_unsupported_by_model`. The record stores the path, kind and reason code, and never an
  adapter-supplied string. An adapter sees prompt content and could otherwise write it into the
  record (FR-025).
- **Header edits** are not in this slice. No confirmed harness needs them, and the forwarding
  floor (003 R18) stays the only header policy.

**Update 2026-10-06** (the engine after slices 005–009):
- **Where in the code.** The request side runs in `attempt.rs` per candidate, before
  `body_for` and `count_body`.
  - Same style: `forward` takes the edited body.
  - Cross style: the edited body is decoded again with the client's codec, and `encode` reads
    that IR.
  - No edits: the request's own body and IR are used, so a key without a harness, or an
    adapter that changes nothing, costs no extra decode.
- **Token counts** run the request side too: a harness's count call carries the same blocks
  its generation call does. There is no response side for counts.
- **Media requests** run no adapter. The record shows `not_run{media_request}` (FR-026).
- **Routing.** `route.rs` builds its prefix chain from the request as received, before any
  adapter. An adapter's edits are fixed for a given body, target and version, so equal client
  prefixes still map to equal upstream prefixes, and the warm lookup holds. A new adapter
  version can change that mapping once, which costs at most one cache miss.
- **Stream-break resume** (`breaks.rs`) is a new attempt. The adapter runs on the client body,
  then the core appends its continuation. That continuation is the core's own, not the
  adapter's, so the guardrail doesn't see it.
- **Pass-through frames.** When the provider's frames go out unchanged (same style, streamed),
  they are parsed for the response side only if the active adapter declares response
  selectors. hermes and the Claude Code adapter declare none.
- **Response event paths** (`event[N]`) count the events sent to the client across attempts,
  so a resumed stream continues the numbering.
- **Journal.** Slice 006 writes each attempt whole, so `Attempt.adapter` is kept, pruned and
  forgotten with its record. The close line names its fields, so it gains
  `response_adapter`, written only when present; older lines read unchanged. (CI found this
  on 2026-10-07; the first draft of this note said no journal change was needed.)

**Rationale**:
- Adapter authors know their own harness's format, which is the client style. Working
  pre-encode means an adapter never has to learn every provider's wire format.
- Selectors keep the data sent into the guest small, which is the main cost on large bodies,
  and they limit what an adapter can read. The reviewer and the operator see the selectors in
  the manifest.
- Edit lists make the content change notes (FR-025) exact and cheap, make "discard" free, and
  let the guardrail tell a removal from an addition by construction.

**Alternatives**:
- Whole-body in and out: a JSON parse and serialisation inside the guest on every request, and
  the host would have to diff two bodies to find the changes.
- Host-call accessors (`get(path)` / `set(path)` imports): many boundary crossings per request,
  and a larger ABI surface to secure.
- Running on the provider-style body after encode: an adapter would have to know every target
  wire format.

## R3. Sandbox limits

**Decision**:
- **Engine config:**
  - `epoch_interruption(true)`;
  - `consume_fuel(false)`;
  - static memory bounds;
  - the `wasm_threads`, `wasm_simd` relaxed and `wasm_multi_memory` proposals are off.
- **One `Engine`** for the process, with a pooling allocator sized for 64 concurrent instances.
- **Time.** A ticker task bumps the engine epoch every 1 ms. Each call gets an epoch deadline
  (default **20 ms** per request call, **2 ms** per stream-event call) and
  `epoch_deadline_async_yield_and_update`. The guest therefore yields to Tokio instead of
  blocking a worker (Constitution, Tokio rule), and it is trapped at its deadline.
- **Memory.** A `ResourceLimiter` caps linear memory at **64 MiB**, tables at 10,000
  elements and instances at 1.
- **Instances.** A fresh instance per call, from the pre-compiled `InstancePre`. With pooling
  this takes about a microsecond, and no state survives from one request to the next, so one
  agent's content can't leak into another's.
- **Imports.** The module may import only the kit's host functions: `zr_abi_version`, and
  `zr_log`, which is capped, rate-limited and redacted. Any other import refuses the module at
  load, with an alert.
- **Exports.** Required: `memory`, `zr_alloc`, `zr_on_request`. Optional: `zr_on_response`,
  `zr_on_event`.
- **Failure.** A trap, a deadline, an out-of-memory condition or malformed output continues the
  request without that adapter's changes. The record gives the reason and an alert is raised
  (FR-018). None of these marks the adapter as suspect: only the guardrail does (FR-016).

**Rationale**: no network, file, environment or secret access follows from the empty import
set, not from a policy check. A fresh instance per call removes cross-request state. Epoch
interruption costs less than fuel.

**Alternatives**:
- Fuel metering: deterministic, but it costs 10–30% at run time.
- Reusing an instance per key: faster, but state could carry across requests and sessions.

The limits are starting values, set in `config.toml` `[adapters]`.

## R4. hermes (built-in)

**Decision**: hermes is a core module (`nullrouter-adapters/src/builtin/hermes.rs`). It uses the
same edit list, records and selectors as a WASM adapter, but runs natively and is not
guardrailed (FR-015 covers third-party adapters). Its tests assert that it never touches tool
calls, definitions or results. hermes talks openai-chat.

- **Echoed reasoning** (FR-008):
  - Scope: `reasoning_content`, `reasoning` and `reasoning_details` on assistant messages.
  - Action: removed with `target_rejects_field` only when the target provider is in hermes's
    reject table for its wire style.
  - Where it acts: on same-style (openai-chat) targets. On cross-style targets, slice 003's
    encoder already drops unplaced keys and records them (003 R27), so the adapter leaves those
    attempts alone.
- **Reject table (user-visible).** It is filled by a live check (task) against anthropic,
  openrouter, opencode-zen, opencode-go, xai and grok-cli. grok-cli speaks openai-responses,
  so hermes's attempts there are cross-style. For a provider not in the table, the default is
  **keep**: forward as received (IV). openrouter uses `reasoning_details` to continue reasoning
  across tool calls, so a blanket removal would degrade it. 9router's rule
  (`paramSupport.js`: groq, mistral, cerebras) seeds the table for community providers.
- **Images** (FR-009):
  - hermes's Ollama-style `messages[i].images` holds raw base64 strings or objects with
    `data`/`url` and a MIME type.
  - Each image becomes an openai-chat content part:
    `{type:"image_url", image_url:{url:"data:<mime>;base64,<data>"}}`. The MIME type is
    sniffed from magic bytes when absent.
  - A string `content` becomes `[{type:"text",text}] + parts`.
  - Recorded as `converted` / `format_conversion` on `messages[i].content`, and as `removed` /
    `format_conversion` on the `messages[i].images` key it came from (R5 ties `converted` to
    `replace`). A key that still holds entries the adapter couldn't convert is `converted` to
    those entries.
  - A message with no `content` key is left alone: an edit can only replace a key that exists.
  - Slice 003's codec then carries the parts to any target style.
- **Attachments**:
  - `attachments` / `experimental_attachments` entries have the shape
    `{url|data, contentType|mediaType, name}`.
  - Images convert as above.
  - PDFs and other files convert to `{type:"file", file:{file_data, filename}}`.
  - A MIME type the target model can't read is left unconverted. Slice 003 then skips that
    target as `cannot_carry` (IV: content is never dropped).

**9router oracle (user-visible deviation)**: 9router does not convert hermes images or
attachments. `combo.js` only detects them to pick a vision-capable model, and `modality.js`
**deletes** them when the model can't read them. 0router converts them instead, and never
deletes them. The oracle for echoed reasoning is `paramSupport.js`.

**Alternatives**:
- Declaring rejected fields in provider plugins (schema 2 data): generic, but it would make the
  pipeline itself drop fields on same-style attempts, which IV forbids outside an adapter.
  Revisit if a second harness needs the same table.
- Blanket removal of echoed reasoning: breaks openrouter's reasoning continuity.

## R5. Checking an adapter's edits (before the guardrail)

**Decision**: edits are refused, and the adapter's output treated as invalid (FR-018, not
suspect), when any of these holds:
- a path is not under one of its declared selectors;
- a path doesn't exist in the body;
- two edits overlap;
- the list exceeds 1,024 edits;
- a `replace` value is over 4 MiB;
- the kind doesn't match the operation (`removed` must be `remove`);
- the reason code is unknown.

**Rationale**: an adapter can edit only what it declared it reads. This keeps the reviewer's
view of the manifest honest, and keeps the guardrail's search bounded.

## R6. The guardrail

Spec FR-015–FR-017, clarifications Q1–Q3.

**Decision**: a pure function in `nullrouter-adapters/src/guard.rs`, run on every third-party
adapter output that has at least one edit. It uses slice 003's style codecs, so it works for any
client style:

1. **Request.**
   - Decode the original body (already decoded by the pipeline) and the edited body with the
     client style.
   - Extract multisets of:
     - **tool calls**: id, name and canonical JSON arguments, from assistant messages;
     - **tool results**: tool-use id and canonical content;
     - **tool definitions**: name, description and canonical schema;
     - **opaque blocks**: the canonical JSON hash of each part the style can't read.
       Server-tool blocks such as `server_tool_use` are opaque in 0router's styles.
   - **Violation** if any after-multiset is not a sub-multiset of its before-multiset.
     Removals pass, and additions or changes fail.
   - **Violation** if the edited body has any `unplaced` key path (003 R27) that the original
     didn't have. Otherwise an adapter could add an unknown field, such as a tool list under a
     name the decoder ignores, that the provider reads on a same-style attempt.
2. **Non-stream response.** The same, with the response codec: tool calls, and opaque blocks
   in the answer.
3. **Stream event.** Each event decodes to IR stream events. It is a violation if the edited
   event carries a tool-call start, a tool-call argument delta or an opaque block that the
   original event didn't carry identically. Removing one is allowed.
4. **Undecodable output.** If the edited body or event no longer decodes in the client
   style, it is a failure, not a violation: `failed{invalid_output: undecodable}` (FR-018).
   The original goes on, an `adapter_failed` alert is raised, and the adapter is not marked
   suspect. It is safe because nothing edited leaves 0router. A suspect mark stays reserved
   for a proven tool, opaque or unplaced change.
5. **On violation:**
   - drop the edits and send the original;
   - set the adapter version to **suspect**, persisted, so it serves no key until cleared
     (FR-017);
   - raise an alert;
   - write a guardrail event on the attempt: direction, stage, what the adapter tried (which
     rule and the paths involved, never values), adapter and version.

   Mid-stream, the rest of the response goes out unedited. Events already sent are not
   recalled.

**Cost**: one extra decode of the edited body, only when there are edits. Decode is native Rust.
A 1 MB body is expected to take well under 5 ms, and a stream event microseconds. R14
benchmarks this.

**Alternatives**:
- Diffing raw JSON under tool-looking keys: style-specific and easy to evade.
- AI checks per request: forbidden (Constitution I).

## R7. Validation gate for adapter source

**Decision**: the gate runs in the core (`nullrouter-adapters/src/gate.rs`) on the unpacked
source tree, before anything is built. It collects every reason it finds, not just the first
(FR-011).

- **Allowed files**, within a 256 KiB total and 64-file cap:
  - `Cargo.toml` and `adapter.toml`;
  - `src/**/*.rs`;
  - optional `README.md`, `LICENSE*` and `CHANGELOG.md`.

  Refused:
  - any other file;
  - a symlink;
  - an absolute or `..` path;
  - a non-UTF-8 file or one with a NUL byte, as binary;
  - a `.rs` file over 64 KiB.
- **`Cargo.toml`**:
  - Required: `[package]` with `name`, `version` and `edition`.
  - `[dependencies]` must contain exactly `nullrouter-adapter-kit` with a plain version
    requirement.
  - Refused: `path`, `git` or `registry` keys; `build-dependencies`; `dev-dependencies`;
    `build`; `links`; `[lib] proc-macro`; `[patch]`; `[replace]`; `[workspace]`; `[features]`
    beyond `default = []`; `[[bin]]`; `[[example]]`; `[profile]`.
  - The builder sets `crate-type`; an author doesn't.
- **Rust source**, parsed with `syn` and walked with a visitor. Refused:
  - `extern crate` of anything but `std`, `core`, `alloc` or the kit;
  - `extern` blocks and `#[link]`;
  - `#[no_mangle]` and `#[export_name]`, since the kit's export macro owns the ABI;
  - `#[path]`;
  - these macros: `include!`, `include_str!`, `include_bytes!`, `env!`, `option_env!`,
    `asm!`, `global_asm!`, `concat_idents!`;
  - any `unsafe` block, fn, impl or trait. The builder also compiles with
    `-F unsafe_code`.
- **Opaque blobs**:
  - Refused: any string, byte-string or char-array literal, or any run of numeric array
    elements, whose longest base64-alphabet or hex-alphabet run exceeds 256 characters, or 128
    bytes as an array.
  - Refused: an integer-array literal with over 256 elements.
- **Harness name**: `adapter.toml` `harness` must match `^[a-z][a-z0-9-]{1,31}$` and must not
  be a built-in name (`hermes`, plus the reserved `opencode`, `grok-build`, `zcode`).

**Rationale**: the rules are exactly Constitution I's list. A syntax-level check can't be
fooled by formatting, and the builder's `-F unsafe_code` and `--offline` repeat the key checks
where they are enforced.

## R8. The builder

**Decision**:
- **What it is.** A separate binary, `nullrouter-builder`, in its own crate, shipped as an
  optional component (FR-013). The core never links it.
- **How it's invoked (user-visible, in operation).** On demand, as a child process: one JSON
  job on stdin, a JSON result on stdout. There is no daemon. "Next to the core" means same
  host, separate process and binary. When the builder is absent, an install stops at
  `queued`, with the reason "builder not installed".
- **Environment**:
  - a pinned toolchain from `rust-toolchain.toml` in the builder's home: rustc 1.93.1 with
    `wasm32-unknown-unknown`;
  - a local registry holding only the kit and its locked dependencies (see "Kit source" below);
  - the builder's own `Cargo.lock`, pinning the kit, `serde` and `serde_json`, copied into the
    build directory. Packages may not carry a lock file (layout in the package contract);
  - `cargo build --release --offline --locked --target wasm32-unknown-unknown`;
  - `CARGO_HOME` set to the builder's own directory, and `RUSTFLAGS` of
    `-F unsafe_code --remap-path-prefix=<tmp>=/src -C debuginfo=0 -C strip=symbols`;
  - an empty environment except `PATH` and the Cargo variables;
  - a fresh temp directory;
  - wall-time and memory limits of 120 s and 2 GiB (`setrlimit`).

  With no build scripts or procedural macros (R7), compiling runs no adapter code. Only rustc
  itself reads the source.
- **Reproducibility.** The builder compiles twice and requires identical WASM hashes. The result
  records:
  - `source_fp`: SHA-256 of the canonical source tree (sorted paths and contents);
  - `kit_abi`;
  - the toolchain id;
  - `wasm_hash`.
- **Kit source (user-visible for adapter authors).** The kit is **not published to crates.io
  in this slice**. This is Claude's technical decision, not yet confirmed by the user, and easy
  to reverse.
  - The builder binary embeds the kit's packaged `.crate` and the pinned `Cargo.lock` from this
    repository at its own build time.
  - `nullrouter-builder setup` unpacks the kit into `$NULLROUTER_HOME/builder/vendor/`, with its
    `.cargo-checksum.json`. It fetches `serde` and `serde_json` at the locked versions, the only
    network use, run once by the operator. It then points `crates-io` at that directory.
  - **Deviation (2026-10-09):** no one may run cargo locally, so `tools/package-kit.sh`, the
    committed `.crate` and `tests/kit_embed.rs` are replaced. The builder embeds the kit's live
    source files with `include_str!`, so it cannot drift from `crates/nullrouter-adapter-kit`.
    `setup` writes the kit into `vendor/` with a normalised manifest (no workspace
    inheritance) and a `.cargo-checksum.json` whose `package` value is also the kit's lock
    `checksum`. It vendors `serde`, `serde_json` and their closure with `cargo vendor` from
    the workspace `Cargo.lock`, embedded with `include_str!` (no copy to drift), and derives the pinned
    `Cargo.lock` from the result. The builder adds the adapter's own lock entry per build.
  - Adapter packages keep writing `nullrouter-adapter-kit = "1"`, so a future crates.io release
    needs no change to any package.
  - Authors build with `nullrouter-builder`. In this repository, `adapters/community/` carries a
    `.cargo/config.toml` that patches the kit to the workspace path, for host-side
    `cargo test`. It sits outside every package, so the gate never sees it and the builder never
    reads it.
  - Why not publish now: publishing is an outward-facing step, and it freezes the kit API while
    it is still settling. Name-squatting on crates.io can't reach 0router's builds, since they
    run offline against the embedded kit.
- **Source match (FR-012).** The core stores the source tree, the module and `build.json`.
  - Before loading a module, the core recomputes `source_fp` from the stored tree and
    `wasm_hash` from the stored module.
  - It runs the module only if both match the values the operator approved.
  - A mismatch refuses the module, raises an alert, and serves bound keys as plain clients.

**Alternatives**:
- A long-running builder daemon on a socket: more moving parts for an occasional job.
- Building inside the core process: forbidden, since the builder must be separate.
- Letting the builder emit precompiled native code (`.cwasm`): the core would have to trust
  native code through `unsafe` deserialisation.

## R9. Kit ABI and upgrades (FR-032)

**Decision**:
- The kit exports `KIT_ABI: u32`. A module imports `zr_abi_version` and embeds its ABI in a
  custom section, `nr.abi`.
- The core supports **the current ABI and the previous one**, with a translation shim for the
  previous.
- **At startup**, the core finds approved versions whose ABI it no longer supports:
  - It never runs their old build.
  - It asks the builder to rebuild the same stored source against the current kit.
  - A matching `source_fp` needs no new review: the source is unchanged.
  - On success, the new module and `build.json` are stored, and the version serves.
  - On failure, bound keys work as plain clients, and an alert gives the builder's error.
- **(User-visible.)** While a rebuild runs at startup, bound keys work as plain clients, and
  each record says "adapter rebuilding after upgrade". `nullrouter adapters rebuild` lets an
  operator rebuild before swapping binaries, so this window can be avoided.

**Rationale**: supporting two ABI versions makes incompatibility rare. A rebuild of the same
reviewed source keeps the trust chain: the source was reviewed, and the builder output is bound
to it by hash.

## R10. Review pipeline

**Decision**:
- **Scrambler** (`nullrouter-adapters/src/scramble.rs`):
  - Parse with `syn`. That drops `//` and `/* */` comments. Also strip `#[doc]` attributes,
    which carry `///` and `//!`.
  - Rename every identifier *defined in the adapter* (items, fields, variants, bindings,
    lifetimes, labels, generic parameters) to `v1`, `v2`, … in first-seen order.
  - Keep identifiers from `std`, `core`, `alloc` and the kit, so the reviewer can see what the
    adapter calls.
  - Keep string literals, since they are needed to judge behaviour.
  - Print with `prettyplease`, and put the manifest's selectors at the top.
- **Prompt injection through string literals.** The review prompt frames the source as untrusted
  data and asks for findings with locations. The operator decides, and the sandbox and guardrail
  hold whatever the report says.
- **Review call.** The review is an internal request through 0router's own pipeline (spec
  Assumption), to the unified model the operator set with `adapters review-settings`, on the
  operator's own accounts:
  - no tools declared, so the model can't run anything;
  - a system prompt fixed in the core;
  - the scrambled source as the user message;
  - `max_tokens` from the budget.

  It is recorded like any request, with `agent: review:<harness>@<version>`.
- **Budget (FR-022).** The budget is a token limit per review.
  - Before the call, the core computes slice 003's input estimate plus the reserved output. If
    that exceeds the budget, the adapter stays **quarantined**, and the reason gives both
    numbers.
  - The answer is capped by `max_tokens`.
  - The budget covers the whole review, retries included. A retry runs only if the tokens
    already used (from the first call's recorded usage) plus the retry's estimate and reserve
    fit the budget. Otherwise the adapter is **quarantined** with "budget exhausted", giving
    both numbers.
  - With no model set, the adapter is **quarantined** with the reason "no review model set".
- **Report.** The review model must answer JSON:
  `{risk: low|medium|high, summary, findings:[{location, concern}]}`. Anything else is one
  retry, then quarantine with "review failed". A provider error or exhausted budget also means
  quarantine (spec Edge Cases).
- **Queue.** Reviews run one at a time, in the background, started by install, update or
  `adapters review --retry`.

## R11. Store, states and hot apply

**Decision**:
- **Location.** `$NULLROUTER_HOME/adapters/`:
  - `index.toml`: harnesses, versions, states, active pointer, review settings;
  - `<harness>/<version-id>/source/`, `module.wasm`, `build.json`, `review.json`,
    `decision.json`;
  - `alerts.toml`.

  Files are mode 0600 and written atomically, like `keys.toml`.
- **States**: `queued`, `building`, `refused`, `in_review`, `quarantined`, `reported`,
  `approved`, `rejected`, `suspect`, `superseded`. data-model.md has the full state machine.
- **Serving.**
  - At most one approved version per harness serves: the `active` pointer.
  - Approving a new version makes it active, and the old one becomes `superseded`.
  - A suspect version isn't served, and keys fall back to plain-client handling, not to the
    previous version (FR-017).
- **Hot apply.** The adapter index is part of slice 003's `EngineState` snapshot (`ArcSwap`,
  003 R20). CLI changes reach the engine through the operator socket and apply from the next
  request.
  - In-flight requests keep the version they started with, which is why v1 finishes requests
    in flight (US4-3).
  - Compiled modules are cached by `wasm_hash` in the snapshot.
- **Key binding.** `keys.toml` `AgentKey` gains `adapter: Option<HarnessName>`. It is set by
  `keys issue --adapter` and changed by `keys set-adapter` or `keys set-adapter --clear`
  (FR-001).
  - Renamed from `harness` / `--harness` / `set-harness` on merging main, 2026-10-08. Slice 010
    (R7) had shipped `harness` as a free-text, display-only tag that "auth, routing, adapters
    and records never read" (the user's clarify Q5). One field for both would let a label start
    running code once an adapter of that name is installed, so the two stay separate: the tag
    names the client for people, the binding selects the adapter.
  - An unknown harness name is refused at issue time, unless it's a third-party harness that has
    no approved version yet (spec Edge Cases: the key works as a plain client until then).

## R12. Catalogue

Clarifications Q4–Q6.

**Decision**:
- **Where it lives (user-visible).** In **this repository**: `catalogue/index.toml`, with the
  Claude Code adapter's source at `adapters/community/claude-code/`. It is served from the
  repository's raw-file URL. A second repository isn't needed for "0router-owned public
  repository", and publishing is a pull request here.
- **Default URL.** `config.toml` `[adapters] catalogue_url` defaults to that raw URL. Operators
  may point it at a mirror.
- **Format.** contracts/catalogue.md. Each entry gives the harness and summary. Each version
  gives a `source` (an HTTPS URL to a `.tar.gz`) and its `sha256`.
- **Fetch.** Only on `catalogue list | show | install | check`, never in the background (FR-031).
  The core fetches with slice 003's `reqwest` client:
  - HTTPS only;
  - no redirects to a different host;
  - the SSRF rules of 003 R17;
  - 1 MiB index and 2 MiB archive caps.

  The archive's SHA-256 must match the entry before it is unpacked. Unpacking refuses symlinks,
  hard links, device files, absolute or `..` paths, and more than 64 entries or 256 KiB. Then
  the gate runs as for a local install.
- **`check`** compares each installed harness's newest version with the index and lists newer
  ones. It installs nothing (US4-4).

## R13. Records and alerts

**Decision**:
- Each attempt in slice 003's `RequestRecord` gains `adapter: Option<AdapterRun>`, covering the
  request side:
  - harness and version, or `builtin`;
  - `ran`, or a `not_run` reason: `no_approved_version`, `suspect`, `source_mismatch`,
    `rebuilding`, `rebuild_failed`, `removed`, or `no_selector_match`;
  - `changes: Vec<ContentChange{path, kind, reason}>`;
  - `failure`: `trap`, `deadline`, `memory`, or `invalid_output` with an R5 rule;
  - `guardrail: Option<GuardrailEvent>`.
- The record also gains `response_adapter` for the served attempt's response side: stream
  events are aggregated, and change paths are prefixed with the event index.
- **No content is ever stored** (FR-025). Paths and codes only. The redactor (003 R23) also runs
  over every string.
- **Alerts** are persisted in `alerts.toml` and written to the log at `warn`. Each alert has a
  kind (guardrail, failure, source mismatch, rebuild failed, quarantine), the adapter and
  version, the record id and the time. They stay until `alerts ack`.

## R14. Performance and benchmarks

**Decision**: new Criterion benches, with a baseline committed as `bench-baseline.md` (003
format):
- `adapters/guard_request`: 10 KB, 100 KB and 1 MB anthropic-messages bodies, with one removal
  edit;
- `adapters/guard_event`: an anthropic-messages content-block delta;
- `adapters/selector_extract`: `messages[*].content[*].type` over 1 MB;
- `sandbox/call_noop` and `sandbox/call_claude_code`: instance creation plus call, with a 100 KB
  body;
- `adapters/hermes_request`: a body with images.

Targets (SC-010): adapter plus guardrail ≤ 5 ms p95 per request and ≤ 1 ms per event. Any
regression blocks merge (Constitution gate).

## R15. Claude Code adapter

**Decision**: written as a normal third-party adapter at `adapters/community/claude-code/`
against the kit, in anthropic-messages style. It is listed in the catalogue and goes through
gate, build, review and decision with no special path (FR-027). Its selectors are:
`thinking`, `output_config`, `model`, `messages[*].role`, `messages[*].content`, `tools`.

Behaviour follows `normalizeClaudePassthrough` (`open-sse/translator/formats/claude.js:204`)
plus `dedupeTools`:

1. `thinking.type = "adaptive"` on a model matching `/haiku/i` becomes
   `{type:"enabled", budget_tokens:10000}`, recorded `converted` /
   `param_unsupported_by_model`.
2. `output_config.effort` on haiku is removed, along with an empty `output_config`.
3. A bare content-block object is wrapped as a one-element array (`converted`).
4. Mid-conversation `role: "system"` messages are folded into the neighbouring user turn
   (`converted`). This follows 9router's reason: hoisting would break the prompt cache.
5. On same-style Anthropic targets:
   - thinking blocks whose signature isn't Claude's are removed;
   - `server_tool_use` blocks with a foreign (non-`srvtoolu_`) id are removed, together with
     the tool results that reference them (`foreign_block`).
6. On targets that aren't Anthropic, `server_tool_use` and `web_search_tool_result` blocks are
   removed with `target_cannot_carry_block`. Without this, slice 003 skips every such target as
   `cannot_carry`, because those blocks are opaque. US5-2 depends on it. On Anthropic they stay
   (US5-3).
7. Empty text blocks, and messages left empty by the removals, are removed
   (`empty_after_removal`).
8. Duplicate built-in tools that have MCP equivalents are removed (`dedupeTools`). This is a
   tool-definition removal, allowed by FR-016.

**Deviations (user-visible)**:
- 9router **inserts a thinking placeholder** into an assistant turn that has a `tool_use` but no
  valid thinking block when thinking is enabled. That adds content, which IV doesn't allow an
  adapter to do. 0router doesn't insert it. Anthropic may then refuse such a turn, and slice
  003's classification treats it as a client error. This case arises only after a session
  switches providers mid-conversation with thinking on.
- 9router selects this handling by detecting Claude Code. 0router binds it to the key (brief P
  note).

## R16. Test strategy

**Decision**:
- **Gate corpus**: `crates/nullrouter-adapters/tests/gate/invalid/*/`. One directory per
  refusal reason, with a golden `.expected` message, plus multi-reason cases (SC-004).
- **Hostile corpus**: `crates/nullrouter-adapters/tests/hostile/*/`. Each is built by the real
  builder in CI when the toolchain is present, and precompiled `.wasm` fixtures are checked in
  for engine tests.
  - The kit's ABI has no network, file or environment access, so those attempts are written as
    raw imports the host doesn't provide. The module is refused at load, and a request through
    its key completes as a plain client.
  - Runaway loop, memory bomb, invalid output.
  - Guardrail cases: add a tool call; change arguments; add a tool definition; rewrite a tool
    result; add an unknown field; add a tool call in a stream event; add one in a non-stream
    response.
  - Legitimate removals (SC-002, SC-003).
- **Scrambler tests**: no comment and no original identifier survives. This is checked against
  a list of every identifier the source defines (SC-007).
- **Tamper tests**: edit the stored source or module after approval (SC-005).
- **Lifecycle test**: v1 serves through v2's queued, in-review, quarantined, rejected and
  approved states under load (SC-006).
- **hermes**:
  - Engine tests with a scripted mock upstream for each chosen provider's wire style.
  - Opt-in live checks with operator keys, never in CI (003 R25). These also fill the
    echoed-reasoning reject table (R4).
- **Claude Code**: slice 003's harness runner (`tests/harness/`), against an adapter installed
  through the real pipeline with a real review model. Opt-in.
- **No-adapter run**: slice 003's suite runs with the builder absent (SC-011).
- **Catalogue**: a local HTTPS mock serves the index and archives. A test asserts zero requests
  when no catalogue command runs (SC-012).

## R17. Deliberate deviations from 9router (summary)

| 9router | 0router | Why |
|---|---|---|
| Detects the client from headers or body | Harness named on the key | FR-002; brief ledger |
| Deletes hermes images the model can't read | Converts them; a target that can't read them is skipped | IV: content is never dropped |
| Strips echoed reasoning in core for groq, mistral, cerebras | The hermes adapter strips it, per its reject table | Only an adapter may change content (IV) |
| Inserts a thinking placeholder | Not inserted | IV: adapters remove or convert, never add |
| Claude handling runs only on native passthrough to Anthropic | Also removes server-tool blocks for other targets | Otherwise slice 003 skips those targets (US5-2) |

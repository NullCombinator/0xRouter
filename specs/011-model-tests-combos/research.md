# Research: Model Tests and Combos

Spec: [spec.md](spec.md). Brief: [2026-10-07-model-tests-combos.md](../briefs/2026-10-07-model-tests-combos.md).
Each decision names the code it builds on, as of `1f94e08`.

## R1. A test is an engine request pinned to one account

**Decision**: A test builds an ordinary `attempt::TextRequest` (or one with `media` set) and
runs it through `Engine::reply`, with two new fields on `TextRequest`:

- `pin: Option<Pin>`: `{ provider, account }`. `plan::plan` keeps only that account's steps
  for the target, so there is no fallback to another account or member. The routing decision
  (`route::decide`) still runs over the one candidate, so the record keeps its usual placement
  block.
- `test: Option<TestTag>`: `{ run_id, source }` with source `operator` or `retest`. It marks the
  record (R9), bypasses priority 0 and the reserve floor for `operator` tests only (clarify
  Q4), and keeps the warm-state `route::learn` from running (FR-021).

Same-account retries stay as `classify::budget` sets them, within the type's test timeout
(R6): a retry after a 503 is cheaper than a retest a minute later.

**Rationale**: FR-002 asks for the same path a client request takes. Pinning reuses plan,
placement, upstream, records, quota tally and pacing without a second executor. Test calls
count against quota and pacing because they go through `route::start`/`finish` like any call.

**Alternatives considered**: a separate "ping" executor as 9router has (`ping.js` posts to its
own HTTP endpoints). Rejected: it would bypass the record and tally paths the spec requires,
and would need an access key, which a test has none of.

## R2. The smallest request per type

**Decision**: a fixed body per type, built in the IR / `TypeValue` so every wire encodes it:

| Type | Request | PASS when |
|---|---|---|
| text (and untyped) | one user message `"hi"`, `max_tokens` 1024, no stream | a response with at least one content part or a finish reason |
| embedding | input `"test"` | at least one vector with at least one number |
| tts | text `"test"`, the model's first declared voice, else none | non-empty audio bytes |
| stt | 250 ms of 16 kHz mono silence, WAV, built in memory | a decoded answer with a `text` field (empty text is fine: the input is silence) |
| image | prompt `"a red dot"`, the smallest size the model declares, one image | at least one image (URL or bytes) |
| video | prompt `"a red dot"`, the shortest duration and smallest size the model declares | the job reaches `completed` and the first content bytes arrive |

**Rationale**: 9router's `ping.js` is the oracle for "smallest call" (brief P note). We keep its
1024-token text budget (its comment: reasoning models spend a small budget on thinking and give
a false "no choices") and its silent WAV. 9router fails STT on empty text; here silence
answering with empty text is a PASS, since the provider accepted the model and returned a
transcript.

**Alternatives considered**: `max_tokens` 1 to cut cost. Rejected for the same false-failure
reason as 9router.

## R3. What counts as a definitive rejection

**Decision**: a new pure function `verdict::judge(failure, provider) -> Judged` in the engine,
used only for tests (client traffic keeps `classify` unchanged, FR-010). Order:

1. Not definitive, whatever the text says: no HTTP status (network, timeout, stall, break), 408,
   429, any 5xx, and an auth rejection as `attempt::token_rejected` or `[[signin.refused]]`
   already decide it (FR-009: the account is marked, the model isn't).
2. The plugin's `[[rejections]]` rules (R4), first match wins.
3. The core list. The status must be 400, 403, 404, 405 or 422, and the provider's message
   (lower-cased, as `classify::text` reads it) must name the model and say why:

   | Reason | Message must contain `model` and one of |
   |---|---|
   | `model_not_found` | `not found`, `does not exist`, `not_found`, `unknown model`, `no such model`, `invalid model`, `not a valid model` |
   | `model_not_available` (403 only, or any listed status with) | `access to`, `not available`, `not allowed`, `not enabled`, `permission`, `not entitled`, `your plan`, `tier` |
   | `type_not_supported` | `does not support`, `not supported`, `unsupported`, `only supports`, `is not a chat model`, `not a text model` |

4. Anything else is UNKNOWN with the provider's status and message as the reason.

A bare 404 with no model wording is UNKNOWN: a wrong path or a moved endpoint answers 404 too,
and Constitution VII forbids guessing.

**Rationale**: `classify::text` treats every 404 as `NotFound` and moves on, which is right for
fallback but too loose for BROKEN. Requiring the word "model" plus a reason phrase matches the
rejections of the bundled providers (Anthropic `not_found_error … model: x`, OpenAI-style
`The model 'x' does not exist`, OpenRouter `… is not a valid model ID`) without matching a
path 404. A provider that phrases it differently declares a rule (R4).

**Alternatives considered**: (a) treat any 404 as BROKEN: violates VII. (b) parse structured
error codes per style: the bundled styles disagree on the field, and plugins can already name
codes with `body_contains`.

## R4. Plugin-declared rejection signals

**Decision**: a top-level `[[rejections]]` array in the provider plugin (schema 2), shaped like
`[[signin.refused]]` (`schema::signin::RefusedRule`):

```toml
[[rejections]]
status = [400, 404]                 # one status or a list; required
body_contains = "model_retired"     # optional; matched case-sensitively like signin.refused
reason = "model_not_found"          # model_not_found | model_not_available | type_not_supported
```

The validation gate refuses a rule whose status list contains 408, 429 or anything outside
400–499, naming the rule. Rules are data the core matches; no plugin code runs.

**Rationale**: reuses an existing, reviewed matcher shape. Restricting statuses at validation is
how the spec's edge case ("a plugin that declares a rate limit … fails validation") is enforced.

## R5. Verdict storage and durability

**Decision**: verdicts live in memory in the engine (`verdict::Board`, a map from
`(provider, account, model)` to `Verdict`) and on disk in `routing/verdicts.jsonl`, written by
the existing journal writer thread (`journal::writer`, new `Target::Verdicts`). Each change
appends one line; a line with `"cleared": true` removes the pair. At start the file is replayed
(last line per pair wins) and compacted when it holds more than 4× the live pairs, as
`journal::state` does for `warm.jsonl`.

**Rationale**: FR-018 asks for slice 006's durability. Using the same writer gives the same
fsync cadence, crash tolerance and disk-full handling (spec edge case) with no new I/O path.

## R6. Timeouts per type

**Decision**: a test runs under `tokio::time::timeout(settings.timeout[type])` around the
whole engine call, video included (submit, polls, first content bytes). On expiry the request's
`CancellationToken` is cancelled, so the upstream call stops, and the result is UNKNOWN
`timeout after 30 s`.

**Rationale**: the engine already cancels upstream work through the token; the test only adds
the deadline.

## R7. Resetting verdicts when the account or plugin changes

**Decision**: each verdict stores a `basis`: `{ secret, signed_in_at, plugin }`.

- `secret`: SHA-256 of `install-id ‖ 0x00 ‖ key` (the existing `sha2` crate; no `hmac` in the
  workspace), as `sha256:<hex>`; `None` for sign-in and no-auth accounts. The salt keeps the
  digest from matching `keys.toml`-style unsalted digests; the key itself never reaches the file.
  For an `--env` account the value read at load is hashed.
- `signed_in_at`: the token entry's `signed_in_at` (`tokens.rs`), which a new sign-in sets and a
  refresh keeps (FR-019: a routine refresh is not a change).
- `plugin`: SHA-256 of the provider's plugin source bytes, recorded by the registry at load
  (new `Registry::plugin_digest(id)`; bundled plugins hash their embedded text).

On start and after every reload or token swap, the board drops each verdict whose basis no
longer matches the current one and appends a `cleared` line (reason `account changed` or
`plugin changed`). A removed account or provider drops all its pairs.

**Rationale**: compares state rather than hooking every command that can change it (accounts
add/replace, signin, plugins install/uninstall, hand edits), so a hand edit applied at the next
start is caught too.

**Alternatives considered**: clearing from each CLI command. Rejected: misses hand edits and
`plugins/` changes picked up at start.

## R8. Retests run from the maintenance queue

**Decision**: a new `maintenance::JobKind::Retest`, one slot per pair that is due:

- UNKNOWN: due at `became_unknown + schedule[step]`; after the last step, every `schedule.last()`.
  A test's new UNKNOWN restarts at step 0 (FR-012).
- BROKEN from a test: due every `broken_retest` when that setting is on; never for
  operator-set BROKEN (FR-013).
- Held while the account can't serve or couldn't take cold work: not `active`, cooling down, or
  a quota window at its reserve floor (`routing` view data). The verdict list shows `waiting:
  <reason>` (clarify Q4).
- After a restart, overdue retests are spread: the n-th overdue pair waits `n × 10 s`.
- Retests count against `MAX_JOBS` like other maintenance jobs, and the test concurrency limit
  (R10) as well.

**Rationale**: the maintenance queue already rebuilds slots per wake, drops removed accounts, and
runs jobs with child cancellation tokens; a new kind needs only `due` and `run` arms.

## R9. Test records

**Decision**: `RequestRecord` gains `test: Option<TestMark>` (`{ run, source }`), and test
records use the pseudo-agent `test` (no key id). Records already keep no prompt; the test also
keeps no output: the attempt loop's `ForClient` answer is dropped after `judge` reads it.
`records list --test` / `--no-test` filter them; by default `records list` shows them with a
`test` tag.

## R10. Running tests from the CLI

**Decision**: two operator-socket ops (contracts/operator-socket.md):

- `test.plan` returns the pairs a request would test and their count by type, with no call.
- `test.run` streams one NDJSON line per finished pair, then a final summary line. The socket
  call stays open for the run; the CLI closing it cancels calls not yet sent (spec edge case).

Calls run at most `tests.concurrency` (default 4) at once per server, shared with retests.
The CLI calls `test.plan` first, prints the counts and asks `make N billed calls? [y/N]` when N
> 1 and `--yes` wasn't given (FR-005).

**Rationale**: the existing `operator::call` is one line each way. A run of image and video
tests can last minutes; per-pair lines let the CLI print results as they finish.

## R11. Skipping BROKEN pairs in routing

**Decision**: `plan::plan` takes the verdict board (as it takes `tokens` and `live`) and turns
a step whose `(provider, account, upstream model)` is BROKEN into `Step::Skip` with a new
`ErrorClass::Broken` and reason `BROKEN since <time>: <reason>`. When every step of a plan is a
`Broken` skip, `walk` fails before any upstream call, and the informational error lists the
skips (FR-011). Model lists are not touched (clarify Q2).

The board is read through an `ArcSwap` snapshot, so the hot path pays one hash lookup per step.

## R12. Combos in the registry

**Decision**: `config.toml` gains `[[combo]]` with `name` and `members` (names). The registry
loads combos after unified models and checks, reporting each as a `config.toml` error with
`file:line:col path: rule` (FR-024):

- the name clashes with a unified model or another combo;
- a member names neither a unified model nor a combo;
- a cycle (DFS over member names; the error prints `a → b → a`);
- members' kinds conflict (untyped members never conflict, as for unified models);
- `members` is empty.

A combo whose member was dropped because a plugin was skipped is dropped and reported, as
unified models are. `Registry::resolve` gains `Resolution::Combo(&Combo)`; a combo has a
`kind` derived from its members. Plugins have no `combo` key; the plugin schema's
`deny_unknown_fields` already refuses one.

## R13. Combos at request time

**Decision**: the attempt loop walks a combo as a list of member plans. The combo is flattened
at plan time into an ordered list of unified models (depth-first, members in order), with each
unified model kept once: a second appearance is dropped (spec US5 scenario 6). Each unified model
gets its own `plan` and its own `route::decide`, run when its turn comes, so placement and warm
state work per unified model as today. The loop moves to the next unified model only when the
current one's `walk` failed with `fallback = true` and no output has reached the client
(`after_output == false`); a `fallback = false` failure (a request error) ends the combo
(FR-026). Records name the combo in a new `combo` field and each attempt's `member` path
(`coder › fallback-chain › gpt`).

**Rationale**: the existing unified-model walk already does retries, accounts and members; a
combo adds an outer loop. Flattening at plan time makes nested combos cost nothing extra at
request time.

## R14. Combo tests

**Decision**: a combo test is `test.run` with `combo`: one `TextRequest` (or media) of the
combo's kind, target the combo's name, `test` tag set, no pin. Each attempt that ended in PASS
or a definitive rejection (by `judge`) updates its pair; other failures appear only in the
output (clarify Q3). The combo verdict: PASS when a member answered; BROKEN when every unified
model it reached was skipped as BROKEN or definitively rejected on every account; UNKNOWN
otherwise. The output nests attempts under each combo level from the record's `member` path.

## R15. Settings

**Decision**: `config.toml` `[tests]`:

```toml
[tests]
retest = ["1m", "5m", "30m", "6h"]   # the last repeats
broken_retest = "off"                # or an interval, e.g. "24h"
concurrency = 4                      # 1–32
[tests.timeout]
text = "30s"; embedding = "30s"; tts = "30s"; stt = "30s"; image = "5m"; video = "5m"
```

Each value has a floor (retest steps ≥ 30 s, timeouts 5 s–30 min). The CLI writes them with
`nullrouter verdicts settings …`, then asks for a reload, as `routing window` does (FR-015).

## R16. Performance gate

**Decision**: extend `nullrouter-engine/benches/engine.rs` with a plan bench over a unified
model of 4 members × 4 accounts with 0 and 1000 verdicts on the board, and a 3-level combo.
Target: within 5 % of today's plan time without verdicts (SC-009). `registry/benches/resolve.rs`
gains a combo resolve case.

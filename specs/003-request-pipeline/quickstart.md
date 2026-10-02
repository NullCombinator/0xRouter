# Quickstart: Request Pipeline (slice 003)

Runnable checks that prove the slice works end to end. Formats and messages are defined in
the [contracts](contracts/); this guide says what to run and what to expect.

## Prerequisites

```bash
cd ~/Desktop/0router
export CARGO_HOME=$PWD/.cargo-home
cargo build --workspace
export NULLROUTER_HOME=$(mktemp -d)       # clean operator home; keep the path short:
                                          # run/operator.sock must fit in 108 bytes
alias nr=./target/debug/nullrouter
```

Harness tests also need Python 3 with `openai`, `anthropic` and `google-genai`, plus Node
≥ 22 with `openai`, `@anthropic-ai/sdk` and `@google/genai`. Claude Code and Codex CLI are
optional for automated runs, but at least two harnesses must pass per run (SC-001).

## 0. Whole suite (mock upstreams, no network)

```bash
cargo test --workspace
```

This covers every scenario below that doesn't say "live". Each section also names the
narrower test target.

## 1. Setup: account, key, server (US1, US5)

```bash
printf '%s' "$ANTHROPIC_API_KEY" | nr accounts add anthropic main    # secret on stdin
nr accounts list                  # anthropic  main  0  …last4  active
nr keys issue laptop              # prints 0r-… once; copy it
export NR_KEY=0r-…
nr serve &                        # 127.0.0.1:20129
```

Expected:
- `accounts.toml` and `keys.toml` are mode 0600;
- `keys.toml` holds a digest and never the key;
- making `keys.toml` world-readable (`chmod 644`) makes `serve` refuse to start and name
  the file.

## 2. One key, four styles, any provider (US1, SC-001)

Automated: `cargo test -p nullrouter-server --test styles` and `tests/harness/run.sh`
(both use mock upstreams).

By hand against the running server, one request per style for the same model:

```bash
curl -s localhost:20129/v1/chat/completions -H "authorization: Bearer $NR_KEY" \
  -d '{"model":"anthropic/claude-sonnet-4-20250514","stream":true,"messages":[{"role":"user","content":"hi"}]}'
curl -s localhost:20129/v1/messages -H "x-api-key: $NR_KEY" -H 'anthropic-version: 2023-06-01' \
  -d '{"model":"anthropic/claude-sonnet-4-20250514","max_tokens":64,"messages":[{"role":"user","content":"hi"}]}'
curl -s localhost:20129/v1/responses -H "authorization: Bearer $NR_KEY" \
  -d '{"model":"anthropic/claude-sonnet-4-20250514","input":"hi"}'
curl -s "localhost:20129/v1beta/models/anthropic/claude-sonnet-4-20250514:generateContent" \
  -H "x-goog-api-key: $NR_KEY" -d '{"contents":[{"role":"user","parts":[{"text":"hi"}]}]}'
```

Expected:
- each reply is a valid body in that style;
- every response carries `x-0router-request-id`;
- a missing or wrong key returns 401 in the style's shape, and the mock upstream receives
  nothing.

Harness runners (operator-run when the tool isn't installed):

```bash
ANTHROPIC_BASE_URL=http://127.0.0.1:20129 ANTHROPIC_API_KEY=$NR_KEY claude -p "say ok"
OPENAI_BASE_URL=http://127.0.0.1:20129/v1 OPENAI_API_KEY=$NR_KEY codex exec "say ok"
```

Both should complete. With a tool call, Claude Code must run the tool and continue.

## 3. Failures don't reach the client (US2, SC-002, SC-003, SC-009)

`cargo test -p nullrouter-engine --test retry --test fallback --test stay_warm --test timeouts`
and `cargo test -p nullrouter-server --test errors`

The scripted mocks cover:

| Script | Expected client result | Expected record |
|---|---|---|
| main: 503, 503, 200 | 200 | 3 attempts on main, `same_account_retry` |
| main: 429 `retry-after: 30`; backup: 200 | 200 | main `rate_limited`, backup ok |
| main and backup 502 ×4; openrouter member 200 (unified target) | 200 | `next_member` |
| same, but a direct `anthropic/…` target | 503 + `retry-after`, informational body | no member attempt |
| every candidate fails, one test per style | informational error in that style ([client-surface](contracts/client-surface.md#informational-error-body)) | id in header and message |
| after a move to backup, main recovers | the agent's next request goes to backup, where its cache is warm | warm hit |

## 4. Every model type (US3, SC-007)

`cargo test -p nullrouter-server --test types` (mocks) covers:
- text;
- embeddings;
- image;
- TTS (elevenlabs, streamed audio);
- STT (elevenlabs multipart `scribe_v2`);
- video (openrouter submit → poll → content, through a `vj_` id).

Each is tested through the OpenAI routes and, where the Gemini style has a route, through
Gemini too. A type mismatch (an embeddings model on `/v1/chat/completions`) returns 400
naming both types.

Live (opt-in, operator keys): `NR_LIVE=1 cargo test -p nullrouter-engine --test live -- types`

## 5. Stream breaks (US4, SC-008)

`cargo test -p nullrouter-engine --test breaks`

| Script | Expected client stream |
|---|---|
| cut after 20 text deltas; the next target declares continuation | one uninterrupted answer, no marker, `break continued` |
| same, no continuation target, default behaviour | the open block closes, a note block `— connection lost, answer restarted —` appears, the full answer follows with new block indexes |
| same, key overridden with `nr keys set-break laptop error_event` | the style's stream error event, then a clean end |
| cut while tool-call arguments are streaming | error event, even under restart |
| cut before any content | invisible to the client; a normal retry |

For the Messages style, the restart stream is replayed through the Anthropic SDK stream
parser and must parse with no error. Claude Code is run against a mock that cuts once.

Live (opt-in): `NR_LIVE=1 … --test live -- continuation` confirms prefill continuation on
the catalogued pre-4.6 anthropic models and the 400 on 4.6+.

## 6. Records (US5, SC-004, SC-005, SC-006)

```bash
nr records list --provider anthropic
nr records list --model claude-sonnet
nr records show rq_…                # format: contracts/operator-cli.md
```

`cargo test -p nullrouter-wire --test usage`, `-p nullrouter-engine --test usage_records`,
`-p nullrouter-server --test timing --test secrets` and `-p nullrouter-cli --test operator`:
- **Usage**: for each provider and style pair, recorded input, output, cache-read and
  cache-write match the mock's usage exactly. A field the provider omits shows
  `not reported`.
- **Timing**: TTFT and total are within 10 ms of what the test client measured.
- **Secrets**: sentinel secrets never appear in logs, records, errors, CLI output,
  forwarded headers or anything a plugin can see.

## 7. Model lists and token counts (US6)

The estimate still needs an account for the target provider (any secret will do; no
request is sent):

```bash
printf 'sk-or-placeholder' | nr accounts add openrouter main    # prints "applied"
curl -s localhost:20129/v1/models -H "authorization: Bearer $NR_KEY"
curl -s localhost:20129/v1/models -H "x-api-key: $NR_KEY" -H 'anthropic-version: 2023-06-01'
curl -s localhost:20129/v1beta/models -H "x-goog-api-key: $NR_KEY"
curl -si localhost:20129/v1/messages/count_tokens -H "x-api-key: $NR_KEY" -H 'anthropic-version: 2023-06-01' \
  -d '{"model":"openrouter/openai/gpt-5","messages":[{"role":"user","content":"hi"}]}'
```

Expected:
- each list uses its style's shape and includes unified and direct models of every type;
- a count on a provider without counting support returns an estimate with
  `x-0router-estimate: true`;
- a count on anthropic is the provider's own number.

## 8. Specifics as data, security floor (US7)

`cargo test -p nullrouter-registry --test gate` and
`cargo test -p nullrouter-engine --test forwarding --test inband`:
- every file in `crates/nullrouter-registry/tests/gate/invalid/styles/` and `…/invalid/providers/` is
  rejected with its expected diagnostic;
- a plugin declaring `authorization` in `forwarding.to_upstream` loads with that entry
  stripped (strict mode: rejected);
- a client's `anthropic-beta` header reaches the anthropic mock;
- a client's `x-api-key` never reaches any mock.

## 9. Community plugins (US8, SC-012)

```bash
nr plugins list --community       # 5 bundled ("loaded") + 116 community, each "fits" or "unsupported"
nr plugins install qoder          # exit 3, message per contracts/provider-schema-v2.md#fit-check
nr plugins install groq           # fits; installed; appears in nr providers
```

`cargo test -p nullrouter-registry --test community` sweeps all 116 plugins:
- each one either loads or is refused with every unsupported part listed;
- no refused plugin contributes anything to the snapshot;
- the `tests/gate/unsupported/` goldens match.

## 10. Cancellation, reuse, performance (SC-010, SC-011, SC-013)

```bash
cargo test -p nullrouter-server --test cancel --test reuse
cargo bench -p nullrouter-wire -p nullrouter-engine -p nullrouter-server
```

Expected:
- when the client drops mid-stream, the mock sees its connection closed within 1 s;
- N sequential requests to one mock host use one accepted connection;
- the `engine` bench shows 0router's added time to first byte at p95 ≤ 10 ms;
- the summary is recorded in `bench-baseline.md`.

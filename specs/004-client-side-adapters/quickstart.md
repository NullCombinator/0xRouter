# Quickstart: Client Side, Harness Adapters (slice 004)

Runnable checks that prove the slice works end to end. Formats and messages are defined in
the [contracts](contracts/). This guide says what to run and what to expect. It assumes
slice 003's quickstart passes.

## Prerequisites

```bash
cd ~/Desktop/0router
export CARGO_HOME=$PWD/.cargo-home
cargo build --workspace
export NULLROUTER_HOME=$(mktemp -d)
alias nr=./target/debug/nullrouter
```

For builder checks (sections 3 onwards), the builder needs its pinned toolchain:

```bash
rustup target add wasm32-unknown-unknown --toolchain 1.93.1
cargo build -p nullrouter-builder
./target/debug/nullrouter-builder setup      # unpacks the embedded kit; fetches serde once
```

## 0. Whole suite (no network, no builder needed)

```bash
cargo test --workspace
```

This covers the gate corpus, the guardrail matrix, the hostile corpus (checked-in `.wasm`
fixtures), the scrambler audit, tamper tests, the update lifecycle and hermes on mock upstreams.

## 1. No adapters, no builder (FR-013, SC-011)

```bash
PATH=/usr/bin:/bin cargo test -p nullrouter-server     # the slice 003 suite, builder not on PATH
nr serve &                                             # starts; `adapters list` shows hermes only
```

**Expected**: the slice 003 suite passes, and `serve` logs no builder or review-model
warning.

## 2. hermes (US1, FR-006–FR-009, SC-001)

```bash
nr keys issue hermes-desktop --adapter hermes         # prints the key once
cargo test -p nullrouter-adapters --test hermes        # images, attachments, echoed reasoning
```

**Expected**:
- Images and attachments arrive at the mock as `image_url` and `file` parts.
- Echoed reasoning is removed only for mock providers in the reject table.
- Each change is in the record by path.

Live (opt-in, needs operator accounts): run
`NR_LIVE=1 cargo test -p nullrouter-server --test harness_hermes -- --ignored`. It runs
hermes against anthropic, openrouter, opencode-zen, opencode-go, xai and grok-cli (signed in), with tools, reasoning
and images, streamed and not streamed. It also fills in or confirms the reject table.

## 3. Install, review, decide (US2, FR-019–FR-022)

```bash
nr adapters install crates/nullrouter-adapters/tests/fixtures/noop   # → queued → building → in_review
nr adapters show noop                                  # quarantined: no_review_model
nr adapters review-settings --model claude-sonnet --budget 60000
nr adapters review noop <version> --retry              # → reported (live model)
nr adapters approve noop <version>
nr keys issue test-agent --adapter noop
```

**Expected**:
- Nothing serves before `approve`.
- The review request appears in `records list` with the agent `review:noop@…`.
- `adapters show` prints the report.

## 4. Gate refusals (FR-010, FR-011, SC-004)

```bash
cargo test -p nullrouter-adapters --test gate
nr adapters install crates/nullrouter-adapters/tests/gate/invalid/multi_reason   # exit 3
```

**Expected**: one line per reason, matching `multi_reason/.expected`. The expected codes
include `foreign_dependency`, `build_script` and `opaque_blob`.

## 5. Hostile adapters and the guardrail (US3, SC-002, SC-003)

```bash
cargo test -p nullrouter-server --test hostile
cargo test -p nullrouter-adapters --test guard
```

**Expected**:
- Modules with network, file or environment imports are refused at load, and requests
  complete as a plain client.
- A runaway loop gives `failed{deadline}`, and a memory bomb gives `failed{memory}`.
- Each tool-call, tool-definition or tool-result addition or change, in requests, non-stream
  responses and stream events, sends the original, marks the adapter suspect, raises an alert
  and writes a guardrail event.
- Legitimate removal cases never trip the guardrail.

## 6. Tampering (FR-012, SC-005)

```bash
cargo test -p nullrouter-adapters --test tamper
```

**Expected**: editing `source/src/lib.rs` or `module.wasm` after approval causes a load
refusal and a `source_mismatch` alert, and bound keys work as plain clients.

## 7. Updates never break a working setup (US4, FR-023, SC-006)

```bash
cargo test -p nullrouter-server --test adapter_lifecycle
```

**Expected**: under constant load, v1 serves 100% of requests while v2 is queued, in review,
quarantined, rejected and then approved. From the next request after approval, v2 serves.

## 8. Kit upgrade (FR-032, SC-013)

```bash
cargo test -p nullrouter-adapters --test kit_upgrade
```

**Expected**:
- A version built for an unsupported ABI never runs.
- The same `source_fp` is rebuilt with no review, and serves again.
- A fixture that no longer compiles raises `rebuild_failed`.

## 9. Catalogue (FR-031, SC-012)

```bash
cargo test -p nullrouter-adapters --test catalogue     # local HTTPS mock
```

**Expected**:
- `catalogue install` and a local install of the same source take identical steps.
- A hash or fingerprint mismatch is refused before the gate.
- A full `serve` run with no catalogue command makes zero catalogue requests.

## 10. Claude Code through the full pipeline (US5, FR-027, FR-028, SC-008)

Live, opt-in, with the builder and a review model:

```bash
nr catalogue install claude-code          # or: nr adapters install adapters/community/claude-code
nr adapters review claude-code <version> --retry
nr adapters approve claude-code <version>
nr keys issue cc-laptop --adapter claude-code
NR_LIVE=1 cargo test -p nullrouter-server --test harness_claude_code -- --ignored
```

**Expected**:
- Claude Code sessions with tools and web search complete against each chosen text provider.
- For targets other than Anthropic, records show `server_tool_use` and
  `web_search_tool_result` removed with `target_cannot_carry_block`.
- For Anthropic, foreign thinking blocks are removed with `foreign_block`.

## 11. Benchmarks (FR-030, SC-010)

```bash
cargo bench -p nullrouter-adapters -p nullrouter-sandbox -- --save-baseline slice-004
```

**Expected**: adapter plus guardrail at p95 ≤ 5 ms on the 1 MB request bench, and ≤ 1 ms
per event. Record the summary in `bench-baseline.md`.

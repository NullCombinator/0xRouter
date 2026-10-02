# Quickstart: Request Execution Walking Skeleton

This guide shows how to validate slice 003 end to end. It is a run guide, not an
implementation guide.

Contracts: [http-api](contracts/http-api.md) · [keys-file](contracts/keys-file.md) ·
[operator-cli](contracts/operator-cli.md) · [outbound-parity](contracts/outbound-parity.md)

## Prerequisites

```bash
cd ~/Desktop/0router
export CARGO_HOME=$PWD/.cargo-home
cargo build -p nullrouter-cli            # binary: target/debug/nullrouter-cli
export PATH=$PWD/target/debug:$PATH
export NULLROUTER_HOME=$(mktemp -d)      # a scratch home, so ~/.0router is untouched
```

- Node 22 or later is needed only to regenerate fixtures
  (`node tools/gen-bundled/generate.mjs`).

## 1. Automated validation (no network)

```bash
cargo test -p nullrouter-server --test parity     # SC-001, SC-005, SC-008, SC-010, SC-011
cargo test -p nullrouter-server --test e2e        # US1–US5 against in-process mock upstreams
cargo test -p nullrouter-server --test e2e cancel        # SC-004 (50 runs, < 1 s)
cargo test -p nullrouter-server --test e2e concurrency   # SC-007 (100 streams)
cargo test -p nullrouter-server --test e2e secrets       # SC-006
cargo bench -p nullrouter-server --bench relay            # SC-003: TTFT delta, median < 5 ms
cargo test --workspace                                    # 002 suites stay green
```

Expected result: every suite passes. Each deliberate deviation (research
[R15](research.md#r15-deliberate-deviations-from-9router)) has a named test that passes.

## 2. Keys file

```bash
cat > $NULLROUTER_HOME/keys.toml <<'EOF'
schema = 1

[[access_key]]
agent = "laptop"
key   = { env = "NR_KEY" }

[[connection]]
provider = "anthropic"
name     = "personal"
api_key  = { env = "ANTHROPIC_API_KEY" }

[[connection]]
provider = "deepseek"
name     = "main"
api_key  = { env = "DEEPSEEK_API_KEY" }
EOF
export NR_KEY=nr-local-$(head -c 12 /dev/urandom | base64 | tr -dc a-zA-Z0-9)
nullrouter-cli check        # expect: 2 connections, 1 access key; no values printed
```

**Negative checks**:

| Change | Expected result |
|---|---|
| Make a key a literal and `chmod 644` the file | `check` fails with the permission error |
| Set `provider = "azure"` | `not executable in this slice (specialized executor)` |
| Unset `DEEPSEEK_API_KEY` | `environment variable DEEPSEEK_API_KEY is not set` |

## 3. Serve

```bash
nullrouter-cli serve &                  # 127.0.0.1:20129
```

**Auth (US5 scenario 6)**:

```bash
curl -s localhost:20129/v1/models                                   # 401 Missing API key
curl -s localhost:20129/v1/models -H "Authorization: Bearer nope"   # 401 Invalid API key
curl -s localhost:20129/v1/models -H "x-api-key: $NR_KEY" | jq '.data[].id' | head
```

The listing shows only anthropic and deepseek models, plus unified models backed by
them.

## 4. Chat with real providers (SC-002)

**OpenAI format, streamed (US1 scenario 1)**:

```bash
curl -N localhost:20129/v1/chat/completions -H "Authorization: Bearer $NR_KEY" \
  -H 'content-type: application/json' \
  -d '{"model":"deepseek/deepseek-chat","stream":true,"messages":[{"role":"user","content":"hi"}]}'
```

Expected: `data:` events arrive incrementally, then `data: [DONE]`.

**Anthropic format with Claude Code (US1 scenarios 2 and 8)**:

```bash
ANTHROPIC_BASE_URL=http://127.0.0.1:20129 ANTHROPIC_API_KEY=$NR_KEY \
ANTHROPIC_MODEL=anthropic/claude-sonnet-4-5 claude -p "say hi"
```

Expected: the reply streams. `nullrouter-cli obs --provider anthropic` shows agent
`laptop`, a `claude:<uuid>` session, and native pair = true.

**Format mismatch (edge case)**:

```bash
curl -s localhost:20129/v1/messages -H "x-api-key: $NR_KEY" -H 'content-type: application/json' \
  -d '{"model":"groq/llama-3.3-70b-versatile","max_tokens":8,"messages":[{"role":"user","content":"hi"}]}'
```

Expected: an Anthropic-shaped error for a provider that has no connection (404
`no_usable_connection`). Once a groq connection is added, the same request gives 400
`format_mismatch`.

## 5. Observations (US3)

```bash
nullrouter-cli obs --provider deepseek
nullrouter-cli obs --agent laptop --since 15m --json | jq '.summary'
```

Expected:

- One row per request, with its outcome, TTFT, and total.
- Cache-read and cache-write counts appear for Anthropic.
- For OpenAI streams sent without `stream_options.include_usage`, tokens show `—` (not
  reported, research [R11](research.md#r11-usage-extraction-fr-023)).

## 6. Reload (US5 scenario 3)

1. Add a second anthropic connection before `personal` in `keys.toml`.
2. Run `nullrouter-cli reload`. The report prints, and the next request runs on the new
   connection.
3. Introduce a typo in the provider name, then run `nullrouter-cli reload`.

Expected after step 3: exit 1 with the error line, and requests keep using the previous
connections.

## 7. Clean up

```bash
kill %1; rm -rf $NULLROUTER_HOME
```

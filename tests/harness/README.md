# Harness tests (spec 003)

Real client SDKs and coding harnesses against a running `zerorouter serve` backed by
mock upstreams. They prove that standard clients accept 0router's bodies, streams and
errors (SC-001, SC-009, US4 replay).

## Setup

```bash
python3 -m venv tests/harness/.venv
tests/harness/.venv/bin/pip install openai anthropic google-genai
npm_config_cache=/tmp/npm-cache-0router npm install --prefix tests/harness openai @anthropic-ai/sdk @google/genai
```

`.venv/`, `node_modules/` and the npm manifest files are git-ignored.

## Run

```bash
ZR_HARNESS=1 cargo test -p zerorouter-server --test harness -- --nocapture
```

The test starts the server over a scripted OpenAI-compatible provider (`mockco/m1`,
answering "Hello"), then calls `run.sh`. Against a server you run yourself:

```bash
ZR_BASE=http://127.0.0.1:20129 ZR_KEY=0r-… ZR_MODEL=anthropic/claude-sonnet-4-20250514 tests/harness/run.sh
```

| Harness | Script | Styles |
|---|---|---|
| Python SDKs | `py/*.py` | openai chat and responses, anthropic messages, google-genai |
| Node SDKs | `node/*.mjs` | the same four |
| Claude Code | `claude.sh` (`claude -p`, throwaway `CLAUDE_CONFIG_DIR`) | anthropic messages |
| Codex CLI | `codex.sh` (`codex exec`, throwaway `CODEX_HOME`) | openai responses |

Each sends one whole and one streamed request and expects the text "Hello" (Claude Code
and Codex send what they send). A missing tool is skipped with a message; `run.sh` fails
on any failure, or if fewer than two harnesses ran (SC-001).

## Tool availability (checked 2026-09-27, T008)

| Tool | Status |
|---|---|
| Python SDKs `openai` 3.19.2, `anthropic` 1.8.0, `google-genai` | installed in `.venv` |
| Node SDKs `openai`, `@anthropic-ai/sdk`, `@google/genai` | installed; npm needs `npm_config_cache` under `/tmp` in the sandbox |
| Claude Code (`claude`) | on `PATH` |
| Codex CLI (`codex`) | not on `PATH`: its runner is skipped with a message |

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
ZR_HARNESS=1 cargo test -p zerorouter-server --test harness
```

The test starts the server and the mocks, then calls `run.sh`.

## Tool availability (checked 2026-09-27, T008)

| Tool | Status |
|---|---|
| Python SDKs `openai` 3.19.2, `anthropic` 1.8.0, `google-genai` | installed in `.venv` |
| Node SDKs `openai`, `@anthropic-ai/sdk`, `@google/genai` | installed; npm needs `npm_config_cache` under `/tmp` in the sandbox |
| Claude Code (`claude`) | on `PATH` |
| Codex CLI (`codex`) | not on `PATH`: its runner is skipped with a message |

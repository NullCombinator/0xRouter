# Harness tests (spec 003)

Real client SDKs and coding harnesses against a running `nullrouter serve` backed by
mock upstreams. They prove that standard clients accept 0router's bodies, streams and
errors (SC-001, SC-009, US4 replay).

## Setup

```bash
python3 -m venv tests/harness/.venv
tests/harness/.venv/bin/pip install openai anthropic google-genai
tests/harness/.venv/bin/pip install 'headroom-ai[proxy]==0.37.0'   # optional: the headroom chain
npm_config_cache=/tmp/npm-cache-0router npm install --prefix tests/harness openai @anthropic-ai/sdk @google/genai
```

`.venv/`, `node_modules/` and the npm manifest files are git-ignored.

## Run

```bash
NR_HARNESS=1 cargo test -p nullrouter-server --test harness -- --nocapture
```

The test starts the server over a scripted OpenAI-compatible provider (`mockco/m1`,
answering "Hello"), then calls `run.sh`. Against a server you run yourself:

```bash
NR_BASE=http://127.0.0.1:20129 NR_KEY=0r-… NR_MODEL=anthropic/claude-sonnet-4-20250514 tests/harness/run.sh
```

| Harness | Script | Styles |
|---|---|---|
| Python SDKs | `py/*.py` | openai chat and responses, anthropic messages, google-genai |
| Node SDKs | `node/*.mjs` | the same four |
| Claude Code | `claude.sh` (`claude -p`, throwaway `CLAUDE_CONFIG_DIR`) | anthropic messages |
| Codex CLI | `codex.sh` (`codex exec`, throwaway `CODEX_HOME`) | openai responses |
| headroom chain | `headroom.sh` → `py/headroom_chain.py` | anthropic and openai SDKs → `headroom proxy` → 0router |

Each sends one whole and one streamed request and expects the text "Hello". With
`NR_MODEL_FAIL` set (the test sets `broken/m1`, whose every attempt gets a 401), each SDK
also expects its own API error, not a parse error, with the record id in the message (Claude Code
and Codex send what they send). A missing tool is skipped with a message; `run.sh` fails
on any failure, or if fewer than two harnesses ran (SC-001).

Signed-in accounts (spec 005 SC-002): a second test in the same file starts the server
with sign-in accounts on the bundled anthropic, xai and grok-cli plugins (hosts pointed at
the mock, tokens in `tokens.toml`) and runs `run.sh` once per provider with `NR_MODEL` set
to `anthropic/claude-sonnet-4-20250514`, `xai/grok-4` and `grok-cli/grok-4.5`. Every SDK
style and Claude Code must pass, every upstream request must carry the account's token as
its bearer (never an `x-api-key`), and every record must succeed on that account.

The headroom chain (SC-014, US1-11) runs when `NR_MODEL_MESSAGES` names a messages-wire
model (the cargo test sets `multi/m-messages`) and a headroom with its proxy extras is in
`.venv` or on `PATH`; otherwise it is skipped and doesn't count toward SC-001. Each SDK
sends to a same-style and a cross-style model, marking every request with an unknown body
field `x_chain` and header `x-chain-marker`. The cargo test then checks that the marks
reached the provider on same-style routes, never on cross-style ones, and that each
cross-style record lists `x_chain` as dropped. headroom 0.37 forwards short prompts
unchanged and adds nothing of its own, so the marks stand in for an optimizer's additions.

## Tool availability (checked 2026-09-27, T008)

| Tool | Status |
|---|---|
| Python SDKs `openai` 3.19.2, `anthropic` 1.8.0, `google-genai` | installed in `.venv` |
| Node SDKs `openai`, `@anthropic-ai/sdk`, `@google/genai` | installed; npm needs `npm_config_cache` under `/tmp` in the sandbox |
| Claude Code (`claude`) | on `PATH` |
| Codex CLI (`codex`) | not on `PATH`: its runner is skipped with a message |
| headroom 0.37.0 with `[proxy]` | installed in `.venv` (2026-09-28); the `headroom` on `PATH` lacks the proxy extras |

## hermes (spec 004, T028 and T031)

`tests/harness/hermes/run.sh` drives the real `hermes chat` against a running server, with
a throwaway `HERMES_HOME` whose `config.yaml` points the `custom` provider at `NR_BASE`. One
model per run; four turns: a plain answer, a tool call (`-t terminal`, checked for a
sentinel the tool printed), a `--continue` follow-up that replays the earlier turns, and an
`--image` turn. Each turn must exit 0 and print something.

`crates/nullrouter-server/tests/harness_hermes.rs` runs it for each model in `NR_MODELS`
(default: the six chosen text providers). It is `#[ignore]` and spends real quota, so it is
operator-run:

```text
nullrouter keys issue hermes-live --adapter hermes
NR_LIVE=1 NR_KEY=0r-… cargo test -p nullrouter-server --test harness_hermes -- --ignored --nocapture
```

What it does not cover: whether hermes streams. Its `streaming` option is a display
setting, so the API call mode is hermes's own choice and the script does not control it.

A failure whose output names `reasoning_content`, `reasoning` or `reasoning_details` with a
400 or 422 is flagged: that provider goes into `REJECTS_ECHOED_REASONING` (T031), with the
run date in a comment.

Results: not run yet.

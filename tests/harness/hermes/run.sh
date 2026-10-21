#!/usr/bin/env bash
# The real hermes agent against 0router (spec 004, T028). One model per run:
#   NR_BASE=http://127.0.0.1:20129 NR_KEY=0r-… NR_MODEL=xai/grok-4 tests/harness/hermes/run.sh
# NR_KEY must belong to a key issued with `--adapter hermes`. hermes gets a throwaway
# HERMES_HOME, so nothing of the operator's own hermes setup is read or written.
# Turns: a plain answer, a tool call, a follow-up that replays the first turns (echoed
# reasoning rides on it), and an image. Each must exit 0 and print something; the tool
# turn must also print what the tool returned.
set -euo pipefail
: "${NR_BASE:?}" "${NR_KEY:?}" "${NR_MODEL:?}"

home=$(mktemp -d)
trap 'rm -rf "$home"' EXIT
export HERMES_HOME=$home
# hermes keeps its model config in config.yaml; `custom` is its OpenAI-compatible provider.
cat >"$home/config.yaml" <<YAML
model:
  default: $NR_MODEL
  provider: custom
  base_url: ${NR_BASE%/}/v1
  api_key: $NR_KEY
  api_mode: chat_completions
YAML

hermes_chat() { timeout 300 hermes chat -Q --yolo --accept-hooks --ignore-rules "$@" </dev/null; }

fail() { echo "FAIL $NR_MODEL: $1" >&2; echo "$2" >&2; exit 1; }

out=$(hermes_chat -q "Reply with one short sentence saying hello.") || fail "plain turn" "$out"
[ -n "$out" ] || fail "plain turn printed nothing" ""

marker="nr-tool-$RANDOM"
out=$(hermes_chat -t terminal -q "Run this exact shell command with the terminal tool and tell me what it printed: echo $marker") \
  || fail "tool turn" "$out"
grep -q "$marker" <<<"$out" || fail "tool turn did not report the tool's output" "$out"

# The follow-up resumes the session, so hermes replays the earlier turns (and any reasoning
# the model returned for them) in the request.
out=$(hermes_chat --continue -q "In one sentence: what command did I ask you to run?") || fail "follow-up turn" "$out"
grep -q "$marker" <<<"$out" || fail "follow-up did not remember the earlier turn" "$out"

# A 1x1 PNG, written without any imaging library.
png="$home/pixel.png"
printf '%s' 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==' | base64 -d >"$png"
out=$(hermes_chat --image "$png" -q "Describe this image in one short sentence.") || fail "image turn" "$out"
[ -n "$out" ] || fail "image turn printed nothing" ""

echo "ok   hermes $NR_MODEL"

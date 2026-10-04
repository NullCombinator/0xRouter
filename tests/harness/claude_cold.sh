#!/usr/bin/env bash
# Claude Code in print mode: two independent one-shots against 0router, each in its own config
# directory, so neither has a prefix the other holds (spec 006, US3). The model is NR_MODEL_COLD.
set -euo pipefail
work=$(mktemp -d)
cfgs=()
trap 'rm -rf "$work" "${cfgs[@]}"' EXIT
cd "$work"
for prompt in "Say hello." "Name one colour."; do
  cfg=$(mktemp -d)
  cfgs+=("$cfg")
  out=$(CLAUDE_CONFIG_DIR=$cfg \
    ANTHROPIC_BASE_URL=$NR_BASE ANTHROPIC_API_KEY=$NR_KEY \
    ANTHROPIC_MODEL=$NR_MODEL_COLD ANTHROPIC_SMALL_FAST_MODEL=$NR_MODEL_COLD ANTHROPIC_DEFAULT_HAIKU_MODEL=$NR_MODEL_COLD \
    CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 DISABLE_AUTOUPDATER=1 \
    timeout 120 claude -p "$prompt" --model "$NR_MODEL_COLD" </dev/null)
  test -n "$out"
done

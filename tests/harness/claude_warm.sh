#!/usr/bin/env bash
# Claude Code in print mode: two turns of one conversation (the second with --continue) against
# 0router, so the second request's prefix is the first's (spec 006, US1). The model is
# NR_MODEL_WARM, a unified model behind several accounts.
set -euo pipefail
cfg=$(mktemp -d)
work=$(mktemp -d)
trap 'rm -rf "$cfg" "$work"' EXIT
cd "$work"
run() {
  CLAUDE_CONFIG_DIR=$cfg \
    ANTHROPIC_BASE_URL=$NR_BASE ANTHROPIC_API_KEY=$NR_KEY_WARM \
    ANTHROPIC_MODEL=$NR_MODEL_WARM ANTHROPIC_SMALL_FAST_MODEL=$NR_MODEL_WARM ANTHROPIC_DEFAULT_HAIKU_MODEL=$NR_MODEL_WARM \
    CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 DISABLE_AUTOUPDATER=1 \
    timeout 120 claude -p "$1" --model "$NR_MODEL_WARM" "${@:2}" </dev/null
}
first=$(run "Say hello.")
test -n "$first"
second=$(run "Say it once more." --continue)
test -n "$second"

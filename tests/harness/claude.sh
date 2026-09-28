#!/usr/bin/env bash
# Claude Code in print mode against 0router, with a throwaway config directory.
set -euo pipefail
cfg=$(mktemp -d)
trap 'rm -rf "$cfg"' EXIT
out=$(CLAUDE_CONFIG_DIR=$cfg \
  ANTHROPIC_BASE_URL=$ZR_BASE ANTHROPIC_API_KEY=$ZR_KEY \
  ANTHROPIC_MODEL=$ZR_MODEL ANTHROPIC_SMALL_FAST_MODEL=$ZR_MODEL ANTHROPIC_DEFAULT_HAIKU_MODEL=$ZR_MODEL \
  CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 DISABLE_AUTOUPDATER=1 \
  timeout 120 claude -p "Say hello." --model "$ZR_MODEL" </dev/null)
echo "$out"
grep -q Hello <<<"$out"

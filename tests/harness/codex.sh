#!/usr/bin/env bash
# Codex CLI (Responses) against 0router, with a throwaway home.
set -euo pipefail
home=$(mktemp -d)
trap 'rm -rf "$home"' EXIT
out=$(CODEX_HOME=$home OPENAI_BASE_URL=$ZR_BASE/v1 OPENAI_API_KEY=$ZR_KEY \
  timeout 120 codex exec --skip-git-repo-check -m "$ZR_MODEL" "Say hello." </dev/null)
echo "$out"
grep -q Hello <<<"$out"

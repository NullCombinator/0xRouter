#!/usr/bin/env bash
# Runs every available harness against a running `zerorouter serve` (see README.md).
# Needs ZR_BASE (http://host:port), ZR_KEY (an agent key) and ZR_MODEL. Each harness
# sends one whole and one streamed request and expects the text "Hello".
# Fails if any harness fails, or if fewer than two ran (SC-001).
set -u
here=$(cd "$(dirname "$0")" && pwd)
: "${ZR_BASE:?}" "${ZR_KEY:?}" "${ZR_MODEL:?}"
passed=0 failed=0
run() {
  local name=$1
  shift
  echo "== $name"
  if "$@"; then
    passed=$((passed + 1))
    echo "ok   $name"
  else
    failed=$((failed + 1))
    echo "FAIL $name"
  fi
}

styles="openai_chat openai_responses anthropic_messages genai"
if [ -x "$here/.venv/bin/python" ]; then
  for s in $styles; do run "python $s" "$here/.venv/bin/python" "$here/py/$s.py"; done
else
  echo "skip python: no tests/harness/.venv (see README.md)"
fi
if command -v node >/dev/null && [ -d "$here/node_modules" ]; then
  for s in $styles; do run "node $s" node "$here/node/$s.mjs"; done
else
  echo "skip node: node or tests/harness/node_modules missing (see README.md)"
fi
if command -v claude >/dev/null; then run "claude code" bash "$here/claude.sh"; else echo "skip claude code: claude not on PATH"; fi
if command -v codex >/dev/null; then run "codex" bash "$here/codex.sh"; else echo "skip codex: codex not on PATH"; fi

echo "passed $passed, failed $failed"
[ "$failed" -eq 0 ] && [ "$passed" -ge 2 ]

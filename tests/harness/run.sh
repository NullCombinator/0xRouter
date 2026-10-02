#!/usr/bin/env bash
# Runs every available harness against a running `nullrouter serve` (see README.md).
# Needs NR_BASE (http://host:port), NR_KEY (an agent key) and NR_MODEL; NR_MODEL_MESSAGES
# (a messages-wire model) adds the headroom chain (headroom.sh); NR_MODEL_CUT (a model
# whose first stream for a body is cut) with NR_KEY_STRICT (an error_event key) adds the
# break replay (py/breaks.py, node/breaks.mjs). Each harness
# sends one whole and one streamed request and expects the text "Hello". NR_MODEL_FAIL
# (a model whose every attempt fails) makes each SDK also expect its own API error with
# the record id.
# Fails if any harness fails, or if fewer than two ran (SC-001).
set -u
here=$(cd "$(dirname "$0")" && pwd)
: "${NR_BASE:?}" "${NR_KEY:?}" "${NR_MODEL:?}"
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
# Mid-stream breaks (US4): restart and error-event streams through every SDK parser, and
# Claude Code against a model whose first stream is cut.
if [ -n "${NR_MODEL_CUT:-}" ]; then
  [ -x "$here/.venv/bin/python" ] && run "python breaks" "$here/.venv/bin/python" "$here/py/breaks.py"
  command -v node >/dev/null && [ -d "$here/node_modules" ] && run "node breaks" node "$here/node/breaks.mjs"
  command -v claude >/dev/null && run "claude code, cut once" env NR_MODEL="$NR_MODEL_CUT" bash "$here/claude.sh"
fi
if command -v claude >/dev/null; then run "claude code" bash "$here/claude.sh"; else echo "skip claude code: claude not on PATH"; fi
if command -v codex >/dev/null; then run "codex" bash "$here/codex.sh"; else echo "skip codex: codex not on PATH"; fi
# The headroom chain counts only when it ran (77 = skipped).
if [ -n "${NR_MODEL_MESSAGES:-}" ]; then
  echo "== headroom chain"
  bash "$here/headroom.sh"
  case $? in
    0) passed=$((passed + 1)); echo "ok   headroom chain" ;;
    77) ;;
    *) failed=$((failed + 1)); echo "FAIL headroom chain" ;;
  esac
else
  echo "skip headroom chain: NR_MODEL_MESSAGES not set"
fi

echo "passed $passed, failed $failed"
[ "$failed" -eq 0 ] && [ "$passed" -ge 2 ]

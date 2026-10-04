#!/usr/bin/env bash
# The cold and overflow scenario (spec 006, US3, SC-009): the Python OpenAI SDK runs cold one-shots
# and an overflow burst, Claude Code runs two cold one-shots, and the Node SDK runs whole and
# streamed cold requests, all through NR_MODEL_COLD (a unified model behind subscription accounts and a pay-as-you-go
# key). Fails if any client errors; the cargo test then checks where each request was served.
set -u
here=$(cd "$(dirname "$0")" && pwd)
: "${NR_BASE:?}" "${NR_KEY:?}" "${NR_MODEL_COLD:?}"
failed=0
if [ -x "$here/.venv/bin/python" ]; then
  "$here/.venv/bin/python" "$here/py/cold.py" && echo "ok   python cold and overflow" || { echo "FAIL python cold and overflow"; failed=1; }
else
  echo "skip python cold and overflow: no tests/harness/.venv"
fi
if command -v claude >/dev/null; then
  bash "$here/claude_cold.sh" && echo "ok   claude code cold one-shots" || { echo "FAIL claude code cold one-shots"; failed=1; }
else
  echo "skip claude code cold one-shots: claude not on PATH"
fi
if command -v node >/dev/null && [ -d "$here/node_modules" ]; then
  node "$here/node/cold.mjs" && echo "ok   node cold requests" || { echo "FAIL node cold requests"; failed=1; }
else
  echo "skip node cold requests: node or tests/harness/node_modules missing"
fi
exit $failed

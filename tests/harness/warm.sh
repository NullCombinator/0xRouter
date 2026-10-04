#!/usr/bin/env bash
# The warm-session scenario (spec 006, US1, SC-009): the Python OpenAI SDK (chat) and Claude Code
# (messages) each run a multi-turn session through NR_MODEL_WARM, each under its own agent key
# (NR_KEY and NR_KEY_WARM). Fails if either errors; the cargo test then checks where they landed.
set -u
here=$(cd "$(dirname "$0")" && pwd)
: "${NR_BASE:?}" "${NR_KEY:?}" "${NR_KEY_WARM:?}" "${NR_MODEL_WARM:?}"
failed=0
if [ -x "$here/.venv/bin/python" ]; then
  "$here/.venv/bin/python" "$here/py/warm.py" && echo "ok   python warm session" || { echo "FAIL python warm session"; failed=1; }
else
  echo "skip python warm session: no tests/harness/.venv"
fi
if command -v claude >/dev/null; then
  bash "$here/claude_warm.sh" && echo "ok   claude code warm session" || { echo "FAIL claude code warm session"; failed=1; }
else
  echo "skip claude code warm session: claude not on PATH"
fi
exit $failed

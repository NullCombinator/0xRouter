#!/usr/bin/env bash
# The SDKs through a headroom proxy in front of 0router (SC-014, US1-11). Needs ZR_BASE,
# ZR_KEY, ZR_MODEL (a chat-wire model) and ZR_MODEL_MESSAGES (a messages-wire model).
# Exits 77 (skipped) when no headroom with its proxy extras is found.
set -u
here=$(cd "$(dirname "$0")" && pwd)
: "${ZR_BASE:?}" "${ZR_KEY:?}" "${ZR_MODEL:?}" "${ZR_MODEL_MESSAGES:?}"
hr=$here/.venv/bin/headroom
[ -x "$hr" ] || hr=$(command -v headroom) || { echo "skip headroom: not installed (see README.md)"; exit 77; }
py=$here/.venv/bin/python
[ -x "$py" ] || { echo "skip headroom: no tests/harness/.venv"; exit 77; }

port=$("$py" -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
home=$(mktemp -d)
log=$home/headroom.log
# A throwaway HOME: headroom keeps state under ~/.headroom.
HOME=$home "$hr" proxy --port "$port" --no-http2 \
  --anthropic-api-url "$ZR_BASE" --openai-api-url "$ZR_BASE/v1" >"$log" 2>&1 &
pid=$!
trap 'kill $pid 2>/dev/null; wait $pid 2>/dev/null; rm -rf "$home"' EXIT
for _ in $(seq 1 240); do
  curl -sf -o /dev/null "http://127.0.0.1:$port/health" && break
  kill -0 $pid 2>/dev/null || break
  sleep 0.25
done
if ! curl -sf -o /dev/null "http://127.0.0.1:$port/health"; then
  if grep -q "dependencies not installed" "$log"; then
    echo "skip headroom: its proxy extras are missing (pip install 'headroom-ai[proxy]')"
    exit 77
  fi
  echo "headroom didn't start:"; tail -20 "$log"; exit 1
fi
HR_BASE=http://127.0.0.1:$port "$py" "$here/py/headroom_chain.py"

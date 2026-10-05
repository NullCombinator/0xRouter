#!/bin/sh
# SessionStart hook. Cloud sessions (claude.ai/code, `claude --cloud`) clone
# only tracked files, so the gitignored ref/9router oracle is missing there:
# clone it at the SHA the parity fixtures were generated from. Locally the
# clone exists and this exits at once.
set -eu
cd "${CLAUDE_PROJECT_DIR:-.}"
[ -d ref/9router/.git ] && exit 0
sha=$(sed -n 's/.*ref\/9router@\([0-9a-f]*\).*/\1/p' tests/fixtures/9router/providers.json | head -n 1)
git clone -q --filter=blob:none https://github.com/decolua/9router.git ref/9router
git -C ref/9router checkout -q "$sha"
echo "cloned ref/9router at $sha"

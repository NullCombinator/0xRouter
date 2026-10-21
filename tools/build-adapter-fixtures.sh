#!/bin/sh
# Operator-run, outside the Claude session. Builds every adapter fixture source with the
# nullrouter-builder and writes module.wasm and build.json into the fixture directory.
#
#   - tests/hostile/<case>/source/  ->  tests/hostile/<case>/
#   - adapters/community/claude-code  ->  tests/fixtures/claude-code/
#
# Needs `nullrouter-builder setup` to have run (network, once). Run it whenever one of those
# sources changes, then commit the fixtures alone.
#
# Usage: tools/build-adapter-fixtures.sh [path-to-nullrouter-builder]
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
builder=${1:-nullrouter-builder}
tests="$root/crates/nullrouter-adapters/tests"

build_one() {
    src=$1
    out=$2
    printf 'building %s\n' "$src"
    printf '{"source_dir":"%s","out_dir":"%s","kit":"1","abi":1}' "$src" "$out" \
        | "$builder" build | tee /dev/stderr | grep -q '"ok":true' \
        || { printf 'FAILED: %s\n' "$src" >&2; exit 1; }
}

for source in "$tests"/hostile/*/source; do
    [ -d "$source" ] || continue
    build_one "$source" "$(dirname "$source")"
done

if [ -d "$root/adapters/community/claude-code" ]; then
    build_one "$root/adapters/community/claude-code" "$tests/fixtures/claude-code"
fi

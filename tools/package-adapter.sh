#!/usr/bin/env bash
# Builds the deterministic release archive of one adapter source directory.
#   tools/package-adapter.sh adapters/community/claude-code [out.tar.gz]
# The archive holds one top-level directory (the source dir's name), sorted entries, zeroed
# times and owners, fixed modes, and `gzip -n` (no name or time in the header). The output
# path defaults to <name>.tar.gz in the current directory. Needs GNU tar; the sha256 of the
# result is what catalogue/index.toml lists.
set -euo pipefail

dir=${1:?usage: package-adapter.sh <adapter-dir> [out.tar.gz]}
dir=${dir%/}
name=$(basename "$dir")
out=${2:-$PWD/$name.tar.gz}
case $out in /*) ;; *) out=$PWD/$out ;; esac

LC_ALL=C tar --sort=name --mtime=@0 --owner=0 --group=0 --numeric-owner \
  --format=ustar --mode='u=rwX,go=rX' \
  -C "$(dirname "$dir")" -cf - -- "$name" | gzip -n -9 > "$out"

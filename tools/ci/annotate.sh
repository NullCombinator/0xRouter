#!/bin/sh
# Turn test panics and compiler errors in the CI logs into annotations: the
# GitHub API serves annotations without a login, unlike job logs.
# Usage: tools/ci/annotate.sh test.log clippy.log
[ -f "$1" ] && awk '
  /^ *Running / { bin = $0; sub(/^ *Running /, "", bin); sub(/ \(.*/, "", bin); next }
  /^---- .* stdout ----$/ { name = $2; msg = ""; next }
  name != "" && /^$/ && msg == "" { next }
  name != "" && /^$/ { printf "::error title=%s %s::%s\n", bin, name, msg; name = ""; next }
  name != "" { gsub(/%/, "%25"); msg = msg (msg == "" ? "" : "%0A") $0; next }
  /^error(\[E[0-9]+\])?: / && !/could not compile/ { e = $0; getline loc; printf "::error title=build::%s%%0A%s\n", e, loc }
' "$1"
[ -f "$2" ] && sed -nE 's/^([^:]+):([0-9]+):([0-9]+): (warning|error): (.*)$/::error file=\1,line=\2,col=\3::\5/p' "$2"
exit 0

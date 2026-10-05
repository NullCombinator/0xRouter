#!/bin/sh
# Turn test panics and compiler errors in the CI logs into annotations: the
# GitHub API serves annotations without a login, unlike job logs.
# Usage: tools/ci/annotate.sh test.log clippy.log
[ -f "$1" ] && awk '
  function flush() {
    if (name == "") return
    # Lead with the panic; what the test printed before it is context.
    m = (panic != "") ? panic : out
    if (length(m) > 3000) m = substr(m, 1, 3000) "%0A…"
    printf "::error title=%s %s::%s\n", bin, name, m
    name = ""
  }
  function add(s, t) { return s (s == "" ? "" : "%0A") t }
  /^ *Running / { flush(); bin = $0; sub(/^ *Running /, "", bin); sub(/ \(.*/, "", bin); next }
  /^---- .* stdout ----$/ { flush(); name = $2; out = ""; panic = ""; next }
  /^failures:$/ || /^test result:/ { flush(); next }
  name != "" {
    gsub(/%/, "%25")
    if (/panicked at/) panic = $0; else if (panic != "") panic = add(panic, $0); else out = add(out, $0)
    next
  }
  /^error(\[E[0-9]+\])?: / && !/could not compile/ { e = $0; getline loc; printf "::error title=build::%s%%0A%s\n", e, loc }
  END { flush() }
' "$1"
[ -f "$2" ] && sed -nE 's/^([^:]+):([0-9]+):([0-9]+): (warning|error): (.*)$/::error file=\1,line=\2,col=\3::\5/p' "$2"
exit 0

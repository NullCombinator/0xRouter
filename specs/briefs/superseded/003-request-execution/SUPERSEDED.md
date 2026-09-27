# SUPERSEDED — see note

This draft (spec, clarifications, plan, tasks) was written on 2026-09-27 from a
`/speckit-specify` description that Claude suggested and the user pasted in trust. It
was never committed as a live spec. It is kept here, outside `specs/`, so speckit
numbering is free for the replacement slice.

**Why it drifted:**

1. "There is no retry and no fallback" came from Claude's suggested text, not from the
   user. The user requires retry and fallback in the core now.
2. The harness↔vendor "native pair" was treated as a core feature. The user's direction
   is that forwarding is plugin-declared data, interpreted by the core under a security
   floor.
3. The draft was text-first (image, audio and video endpoints out of scope) and
   Claude-Code-first. The user wants every model type first-class and any standard
   client served.
4. Several clarify answers (header forwarding, error bodies, count_tokens) were given
   inside the native-pair framing and do not carry over.

**Replacement:** the slice was reshaped with `/shape-spec` on 2026-09-27. See
`specs/briefs/2026-09-27-request-pipeline.md` for the confirmed ledger and the new
`/speckit-specify` text.

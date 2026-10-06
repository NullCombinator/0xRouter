# SUPERSEDED — see note

This slice (spec, clarifications, plan, research, contracts, tasks) was specified on 2026-10-05
from `specs/briefs/2026-10-05-dashboard-v1-superseded.md`. None of its dashboard was built. It is
kept here, outside `specs/`, as history; don't plan or implement from it.

**Why it was replaced:**

1. After the mockups in `docs/dashboard/mockups`, the user overrode its four-page layout (FR-019,
   FR-033): the dashboard now uses 9router's sidebar and page names, under the title
   "0Router Proxy".
2. The user asked for the whole mockup, cut into two slices in build order. Latency and usage
   summaries, Est. Cost and the traffic landscape move to the second dashboard slice.
3. Its shared view layer and the CLI reads `unified`, `behaviour show` and
   `records list --before` were already moved to slice 008, read model, and shipped there.

**What carries over:**

- Task T011, a `subject` on each `check` item: now spec 009 FR-024.
- The plan's dashboard token and cookie decisions (research R6 to R8, open Low L1): to spec 009's
  plan.

**Replacement:** spec 009, `specs/009-dashboard/spec.md`, from the brief
`specs/briefs/2026-10-05-dashboard.md`.

# Scope brief: read model

Shaped with `/shape-spec` on 2026-10-05, from the kickstart note on `007-dashboard` (`5d63677`).
The user chose a slice of its own over a milestone inside 007 or all of 007, so that its
pass/fail does not depend on 007's page layout, which the open dashboard vision may rewrite. In
`/speckit-clarify` and `/speckit-plan`, an answer that contradicts a confirmed row below means
stop and revisit this brief. Don't accept it.

The spec directory is `specs/008-read-model`, on branch `008-read-model`, which starts at
`a17f8bf` (tip of `006-routing-decision`). This branch has no `specs/007-dashboard`, so speckit
would number the spec 007 unless it is told the directory.

## Core map (at `a17f8bf`, verified against the code on 2026-10-05)

| Capability | Status | Evidence |
|---|---|---|
| Provider plugins, unified models, four client API styles | shipped | 002, 003 |
| Request execution, retry, fallback, informational errors, non-text types | shipped (live checks T060, T087, T096 open) | 003 |
| Account sign-in, quota polling | shipped (live checks T045, T056, T078, T097 open) | 005 |
| Cache-aware routing, per-agent isolation, windowed amortization | shipped, not merged to `main` | 006; T085 and T091 open |
| Persistent request records with latency | shipped, not merged | 006 journal |
| CLI reads: accounts, quota, routing, records, providers, model, plugins, keys, check, resolve | shipped, each builds its own `--json` | `crates/nullrouter-cli/src/cmd/*.rs`, `quota_text.rs`, `routing_text.rs` |
| Operator socket the CLI reads through | shipped | `crates/nullrouter-server/src/operator.rs` (`operator::call`, `operator::handle`) |
| Shared read model | absent | no `views` module; → **this slice** |
| `unified` list, `behaviour show`, `records list --before` | absent | `behaviour` has only `set-break`; `records list` has no `--before`; → **this slice** |
| Dashboard (pages, token, listener, style guide) | specified, 0/65 tasks | `specs/007-dashboard` on `007-dashboard`; vision open in `docs/dashboard/mockups` |
| Latency summaries and trends | absent | → own slice after the dashboard |
| Model tests, combos | absent | → later |
| Harness adapters (hermes, WASM) | specified, 0/98 built | 004 |
| Agents panel, harness tags, plugin enable/disable/hide, housekeeping agent, Est. Cost, usage over a period | absent | mockups only |

## Playback (confirmed)

When this slice is done, every CLI command that shows you something gets its answer from one
shared place. That place gives the same answer whether it's asked through the server's socket or
from inside the server, so the future dashboard can show exactly what the CLI shows. Nothing you
see today changes: same text, same JSON, and it still works with the server stopped. You get
three new commands: a list of all unified models, a view of your break-behaviour setting, and
paging back through records 50 at a time, which stays fast with 100,000 of them. Nothing appears
in a browser yet. The slice changes nothing on disk except the code: no writes, no latency, usage,
cost, agents or dashboard sign-in. `validate` stays as it is. The work starts from 006's tip, and
007's documents get a note saying what moved.

## Ledger

| # | Tag | Claim | Source |
|---|---|---|---|
| 1 | U | One shared read model between the engine and both front ends, built first | "Agreed." (2026-10-05, kickstart) |
| 2 | C✓ | Its own slice and its own spec, not part of 007 | "Own slice, own spec" |
| 3 | C✓ | Every CLI read builds its `--json` in the read model: accounts, quota (incl. history), routing, records list/show, providers, model, plugins list, keys list, check, resolve | the failure answer "A read builds its own value", plus the validate answer |
| 4 | C✓ | Existing CLI text and `--json` are unchanged byte for byte | failure: "Any existing output changes" |
| 5 | C✓ | The socket path and the in-server path give identical values | failure: "Socket vs in-server disagree" |
| 6 | C✓ | Reads work with the server stopped, as today | premortem 2 |
| 7 | C✓ | `unified [NAME]`: kind, ordered members with upstream ids, limits notes, dropped models; an empty home says so (exit 0); an unknown NAME exits 2 with resolve's message | new reads; premortem 3 |
| 8 | C✓ | `behaviour show`: the value, marked "(default)" when unset | new reads; premortem 3 |
| 9 | C✓ | `records list --before <ID>`: the newest page is read without a full-journal scan; an unknown id is an error naming it | new reads; premortem 3 |
| 10 | C✓ | A paging benchmark on 100k records with a target | failure: "Paging slow on big journals" |
| 11 | M | Every dashboard fact has a CLI twin; add a read-only CLI view where none exists | 2026-10-05, 007 specify |
| 12 | M | The dashboard runs inside the `serve` process | 2026-10-05, dashboard brief |
| 13 | K | Read-only views never expose secrets beyond what the CLI shows today (`…last4`) | constitution I; 007 FR-019a |
| 14 | C✓ | Out: `validate` stays outside the read model | "validate stays out" |
| 15 | C✓ | Out: dashboard pages, token, listener, config and style guide → dashboard slice | "dashboard comes later" |
| 16 | C✓ | Out: no `subject` field on check items → dashboard slice | "add it with the pages" |
| 17 | C✓ | Out: no writes in this slice; whether the dashboard ever writes stays open | "read-only slice" |
| 18 | C✓ | Out: latency view → its own slice after the dashboard | "latency is its own slice" |
| 19 | C✓ | Out: agents panel and harness tags → later, with the vision | "later" |
| 20 | C✓ | Out: Est. Cost; whether prices are plugin data or core stays open → later | "later" |
| 21 | C✓ | Out: usage over a period → later, likely with latency | premortem 1 |
| 22 | C✓ | 007 gets a note on what moved and is otherwise not edited | "Note in 007" |
| 23 | C✓ | The branch starts from the tip of `006-routing-decision` | "From 006's tip" |

## Open decisions (not closed by this slice)

- Whether the dashboard ever writes (housekeeping agent, plugin enable/disable/hide, harness tags).
- Where prices come from (plugin-declared data or a core table).
- The dashboard's pages: 007's FR-033 (four pages of 0router's own) versus the mockups (9router's
  page structure: usage, quota, providers, combos, endpoint, settings, …).

## Notes for research.md (`P`)

- 007's research R1 already designs this: views in `nullrouter_server::views`, a `Live` source
  with a socket implementation (`operator::call`) and an in-process one (`operator::handle`);
  007's tasks T004-T012 (move), T040-T042 (newest-first read, `--before`, bench), T044/T046
  (`unified`) and T049 (`behaviour show`) are the starting point. Reuse them; don't re-derive.
  T011's `subject` field is out (row 16).
- 007's `contracts/cli.md` has the text and `--json` shapes for `unified` and `behaviour show`;
  `--before` there doesn't say what an unknown id does. Row 9 decides it: an error naming the id.
- The CLI's existing integration tests are the byte-for-byte gate (row 4). Where a read has no
  test pinning its text or `--json`, add one before moving it.
- `resolve` already builds unified-member JSON (`crates/nullrouter-cli/src/cmd/resolve.rs`
  `member`, `note_json`); `unified` reuses it from the read model.
- Some reads combine files with a live socket answer that may be missing (`accounts.rs` uses
  `unwrap_or(Value::Null)` on `accounts.state`). The in-process source must give the same value
  the socket gives, including when a live part is unavailable (rows 5, 6).
- The paging target: 007's SC-007 aimed at < 1 s per dashboard page on 100k records; plan picks
  this slice's target for the read itself.

## Final command

```text
/speckit-specify SPECIFY_FEATURE_DIRECTORY=specs/008-read-model

Read model: one shared place that builds what every operator read shows, so the CLI today and the dashboard later show the same facts from the same values.

Every CLI read command builds its JSON answer in this shared read model: accounts list, quota (including history), routing, records list and records show, providers, model, plugins list, keys list, check, and resolve. The CLI prints what the read model returns. Each read gives identical values whether it is asked through the operator socket or from inside the running server, because the dashboard will run inside the serve process and every fact it shows must have a CLI twin.

Nothing the operator sees today changes: every existing command's text and --json output stays byte for byte the same, and reads keep working with the server stopped exactly as they do now.

Three new read-only commands, each with --json:
- `unified [NAME]` lists every unified model with its kind, ordered members with their upstream ids, limits notes, and dropped models. With no unified models it says so and exits 0; an unknown NAME exits 2 with the message resolve gives.
- `behaviour show` shows the operator's break behaviour, marked "(default)" when it is not set.
- `records list --before <ID>` pages back through records older than that id. The newest page is read without scanning the whole journal, and an unknown id is an error naming it. A benchmark on a 100,000-record journal measures paging, against a target set in planning.

Constraints: read-only views show no more secret text than the CLI shows today (key and token endings only). Plugins stay data, never code.

The slice fails if any existing output changes, if any listed read still builds its own value outside the read model, if the socket and in-server answers differ, or if paging is slow on a large journal.

Out of scope:
- `validate` stays outside the read model; it checks a plugin file, not router state → stays as is.
- Dashboard pages, dashboard token and sign-in, listener, [dashboard] config, style guide → dashboard slice, shaped when the dashboard vision settles.
- A per-item subject field on check results → dashboard slice.
- Any command that changes state; whether the dashboard ever writes stays open → later.
- Latency view (median and 95th percentile, trends) → its own slice after the dashboard.
- Usage over a chosen period → later, likely with latency.
- Agents panel and harness tags on keys → later, with the dashboard vision.
- Estimated cost; whether prices are plugin data or core stays open → later.

Scope brief: specs/briefs/2026-10-05-read-model.md
```

## Trace

| Command sentence | Rows |
|---|---|
| Directory line | process (23: the branch has no `specs/007-dashboard`) |
| Purpose | 1, 2, 11 |
| Every CLI read command builds its JSON… / prints | 3 |
| Identical values, socket or in-server, because… | 5, 11, 12 |
| Nothing changes; byte for byte; offline | 4, 6 |
| `unified` | 7 |
| `behaviour show` | 8 |
| `records list --before`, benchmark | 9, 10 |
| Constraints: secrets | 13 |
| Constraints: plugins stay data | K (constitution I) |
| Failure sentence | 3, 4, 5, 10 |
| Out-of-scope lines, in order | 14, 15, 16, 17, 18, 21, 19, 20 |
| (not in the command) | 22: done on `007-dashboard` as housekeeping |

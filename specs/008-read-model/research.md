# Research: Read Model (slice 008)

Starts from slice 007's research (`specs/007-dashboard/research.md` on `007-dashboard`), R1 and
R9, which designed this part before it was carved out. Where this file disagrees with 007, this
file wins: the clarify answers of 2026-10-05 changed where facts come from (R2 below), and the
code has moved on since 007's R9 was written (R6).

## R1. Where the read model lives

**Decision**: a new module, `nullrouter_server::views`, one submodule per read. The CLI calls it
and prints the result. A future dashboard (inside `serve`) calls the same functions.

**Rationale**: the views need the operator socket's request and answer shapes, which the server
crate defines, and the CLI already depends on the server crate. 007 R1 rejected a separate crate
and the engine crate for the same reasons, and nothing has changed.

**Alternatives rejected**: a new `nullrouter-views` crate (one more crate for one module, and it
would depend on the server crate anyway); the engine crate (it doesn't know the operator ops).

## R2. Same sources by both routes (clarify Q1)

**Decision**: every view is a function of two inputs, and nothing else:

1. **Files**: the operator home, read by the view itself: `config.toml`, `accounts.toml`,
   `tokens.toml`, `keys.toml`, the plugin directories, `records/`, `quota/`. Registry-backed
   reads (`providers`, `model`, `plugins list`, `resolve`, `unified`, `check`) load the registry
   from the home's files the way the CLI does today, not from the engine's loaded snapshot.
2. **Live**: the answers to the operator ops the view needs (`accounts.state`, `quota.list`,
   `routing.view`, `routing.health`, `records.get`), or "no server" for each.

Each view declares the ops it needs. The caller fetches them, then calls the view:

- the CLI fetches over the socket (`operator::call`), as it does today;
- the in-server route fetches with `operator::handle(engine, req).await`, the function the socket
  itself calls, then runs the view with `tokio::task::spawn_blocking`, because views read files
  (constitution, Architecture Constraints: no blocking I/O on the executor).

Whether a server is running is part of the live input (today's `server_runs` check, used to name
unfinished records "in flight" or "cut short"). In-server it is always true.

**Rationale**: the user chose this (spec Clarifications, Q1). The two routes then differ only in
transport, so they agree by construction. It also keeps the CLI's offline behaviour exactly as
it is (FR-005): with no server, each live input is "no server", and the view says what it says
today.

**Alternatives rejected**: a `Live` trait object passed into views (007's sketch): views would
have to be async or block on the socket inside the server; fetching first keeps views sync,
pure over their inputs and testable without a server. Reading the engine's snapshot in-server:
the user rejected it in clarify (it would let the routes differ).

## R3. One answer, two renderings (clarify Q2)

**Decision**: each view returns a `View` value: the JSON the CLI prints with `--json` today,
plus, where the text shows facts the JSON lacks, a separate `extra` part. `--json` prints only
the first part, unchanged. The CLI's text renderer reads both parts. Known case: `records show`
names agent keys by name (`keys.toml`), while its JSON has the key id; the
view carries the id → name map in `extra`. Every read is audited for other cases while it moves
(tasks), and each one found is listed in `contracts/read-model.md`.

**Rationale**: FR-002 and FR-004 together: one answer per read, `--json` byte for byte the same.

**Alternatives rejected**: adding the facts to `--json` (breaks FR-004); letting each front end
look them up (rejected by the user in clarify Q2).

## R4. The byte-for-byte gate (FR-004, SC-001)

**Finding**: the CLI's integration tests (`crates/nullrouter-cli/tests/*.rs`) cover accounts,
keys, check, plugins, quota, records and routing, but they assert parts of the output, not the
whole of it, and `providers`, `model` and `resolve` have no full-output test.

**Decision**: before any code moves, add a characterization suite,
`crates/nullrouter-cli/tests/read_golden.rs`. It builds fixture homes that exercise every listed
read (every account kind and state, unified models including a dropped one and one with a limits
note, bundled, installed and community plugins, keys including a revoked one, quota history,
records including in-flight and cut-short ones), runs every listed read with and without
`--json`, with the server stopped and running, and compares stdout, stderr and the exit code with
committed golden files under `crates/nullrouter-cli/tests/golden/`. The goldens are generated
once from the unchanged code (`NR_BLESS=1`) and committed in their own commit, before the move.
After that, a golden may change only in the commit that adds a new read.

Values that change on their own (times, durations left, ULIDs of fresh records) are pinned by the
test clock and fixed fixture ids, not masked. Where that isn't possible (a cooldown counted from
the real clock in a running server), the suite masks only that field, by name, and lists it in
the golden's header.

**Rationale**: SC-001 asks for 0 differing bytes; partial assertions can't show that.

## R5. Same answer by both routes, tested (FR-003, SC-002)

**Decision**: `crates/nullrouter-server/tests/views_routes.rs` starts a real server
(`testkit`) on each fixture home, and for every view compares the value built from socket answers
with the value built from `operator::handle` answers. Values must be equal as JSON; fields that
change on their own between two reads are compared within the seconds that elapsed, as 007 R11
described.

## R6. Paging records (FR-012 to FR-015, SC-004, SC-005)

**Finding**: `records::read` (`crates/nullrouter-engine/src/journal/records.rs`) already reads
the daily segments (`records/YYYY-MM-DD.jsonl`) newest first and stops once `limit` records
match. But it reads and folds each segment whole. A busy day's segment is read entirely even for
a page of 50, and `records::get` searches segments newest first, testing each file's text for
the id.

**Decision**:

1. **Cursor**: `Filter` gains `before: Option<String>`. "Older than" is the listing's own order:
   record ids are ULIDs, sorted descending within a segment and segment by segment, so a record is
   older than the cursor when its id sorts below it.
2. **Finding the cursor**: the ULID's time part gives the instant the id was made, at arrival. The
   cursor's record is looked for in that day's segment and the day either side of it (a request
   arriving at a day boundary), not in every segment. If it isn't there, the command fails with
   `no record <ID>` (FR-013), the message `records show` gives today.
3. **Reading a segment from its end**: a segment is read backwards in blocks (64 KiB). Lines are
   folded as they come; a record is complete when its `open` line has been seen. Reading stops
   when `limit` matching complete records older than the cursor are in hand **and** the oldest
   `open` line read is more than 60 s older (by ULID time) than the oldest record kept. The margin
   covers requests whose id was made shortly before their `open` line was written by the journal's
   single writer thread; no request waits that long between the two.
4. Without a limit, the whole remaining journal is read, as today.

**Rationale**: SC-004 asks that the newest page not grow with the journal's size. Per-day
segments already bound it across days; reading from the end bounds it within a day too.

**Alternatives rejected**: an index file written next to each segment (more state, and a write on
the request path, which the performance gate covers); keeping whole-segment reads (a 100k-record
day costs about 100 MB of parsing for any page).

**Risk**: the 60 s margin assumes the id and the `open` line are made close together. SC-005's
test includes records whose `open` lines are written out of id order within the margin, and the
fixture checks full-listing equality.

## R7. Paging targets (SC-004)

The brief left the number to the plan (row 10). Criterion bench `records_page` in
`crates/nullrouter-engine/benches/records_page.rs`, on a local machine (CI timings don't compare,
as for other benches):

| Case | Target |
|---|---|
| Newest page of 50, 100k records over 30 daily segments | ≤ 20 ms |
| Newest page of 50, 100k records in one segment | ≤ 20 ms |
| Newest page of 50, 1k records (reference) | the 100k cases are within 2× of this |
| Page of 50 after a cursor 50,000 records back, both layouts | ≤ 1 s |

The baseline goes in `specs/008-read-model/bench-baseline.md`, as for 006.

## R8. The new reads

- **`unified [NAME]`**: the view reuses `resolve`'s member JSON (`member`, `note_json` in
  `crates/nullrouter-cli/src/cmd/resolve.rs`, moved into `views::resolve` and shared). `dropped`
  comes from the load report's skipped unified models. Text and JSON in `contracts/cli.md`, which
  takes 007's `contracts/cli.md` text unchanged.
- **`behaviour show`**: reads `config.toml` only (the `[pipeline]` table, which holds only
  `break_behaviour` today). The JSON has one entry per `[pipeline]` setting, so a setting added
  later joins it. A config that doesn't load is reported with the same message `check` gives, exit
  1, and no value.
- **`records list --before <ID>`**: R6. The `records.list` operator op gains `before` too, so the
  op and the CLI accept the same filter.

## R9. Secrets (FR-007, SC-006)

The views move code that already shows only `…last4` and `env:VAR`; nothing new reads a secret.
The 005 secrets sentinel (`crates/nullrouter-server/tests/secrets.rs`) is extended to run every
view by both routes on a home with planted secret values and scan each answer, including the
`extra` part (R3).

## Brief rows touched

Rows 1–10 (R1–R8), 13 (R9). Nothing here touches the out-of-scope rows 14–21. No decision
contradicts a confirmed row.

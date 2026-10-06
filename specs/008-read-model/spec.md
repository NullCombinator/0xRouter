# Feature Specification: Read Model

**Feature Branch**: `008-read-model`

**Created**: 2026-10-05

**Status**: Draft

**Scope brief**: [specs/briefs/2026-10-05-read-model.md](../briefs/2026-10-05-read-model.md). In
clarify and plan, an answer that contradicts a confirmed row of that brief's ledger means stop and
revisit the brief.

**Input**: User description: "Read model: one shared place that builds what every operator read shows, so the CLI today and the dashboard later show the same facts from the same values. Every CLI read command builds its JSON answer in this shared read model: accounts list, quota (including history), routing, records list and records show, providers, model, plugins list, keys list, check, and resolve. The CLI prints what the read model returns. Each read gives identical values whether it is asked through the operator socket or from inside the running server, because the dashboard will run inside the serve process and every fact it shows must have a CLI twin. Nothing the operator sees today changes: every existing command's text and --json output stays byte for byte the same, and reads keep working with the server stopped exactly as they do now. Three new read-only commands, each with --json: `unified [NAME]` lists every unified model with its kind, ordered members with their upstream ids, limits notes, and dropped models. With no unified models it says so and exits 0; an unknown NAME exits 2 with the message resolve gives. `behaviour show` shows the operator's break behaviour, marked \"(default)\" when it is not set. `records list --before <ID>` pages back through records older than that id. The newest page is read without scanning the whole journal, and an unknown id is an error naming it. A benchmark on a 100,000-record journal measures paging, against a target set in planning. Constraints: read-only views show no more secret text than the CLI shows today (key and token endings only). Plugins stay data, never code. The slice fails if any existing output changes, if any listed read still builds its own value outside the read model, if the socket and in-server answers differ, or if paging is slow on a large journal. Out of scope: `validate` stays outside the read model; it checks a plugin file, not router state → stays as is. Dashboard pages, dashboard token and sign-in, listener, [dashboard] config, style guide → dashboard slice, shaped when the dashboard vision settles. A per-item subject field on check results → dashboard slice. Any command that changes state; whether the dashboard ever writes stays open → later. Latency view (median and 95th percentile, trends) → its own slice after the dashboard. Usage over a chosen period → later, likely with latency. Agents panel and harness tags on keys → later, with the dashboard vision. Estimated cost; whether prices are plugin data or core stays open → later. Scope brief: specs/briefs/2026-10-05-read-model.md"

## User Scenarios & Testing *(mandatory)*

The operator is the person who runs `nullrouter serve` and manages its accounts, keys and
plugins from the CLI. Today each CLI read command assembles its own answer. This slice gives
every read one shared source, so that a later dashboard, running inside the server, can show
exactly the facts the CLI shows. The operator sees no change in any existing command and gains
three new reads.

### User Story 1 — Every read comes from one shared source, and nothing the operator sees changes (Priority: P1)

The operator runs any existing read command (`accounts list`, `quota`, `quota history`,
`routing`, `records list`, `records show`, `providers`, `model`, `plugins list`, `keys list`,
`check`, `resolve`), with or without `--json`, with the server running or stopped. The output is
exactly what it was before this slice. Behind it, each command's answer now comes from one
shared read model, and the same read asked from inside the running server gives the same answer.

**Why this priority**: it is the foundation the dashboard needs (its facts must have CLI twins
and must agree with them by construction), and it is the riskiest part: moving every read must
not change a single byte the operator relies on.

**Independent Test**: on fixture homes that exercise every listed read, capture each command's
text and `--json` output before the change, with the server running and stopped; after the
change the outputs are identical. For each read, the answer obtained the CLI's way and the answer
obtained from inside a running server over the same state are identical.

**Acceptance Scenarios**:

1. **Given** a home with accounts of each kind, unified models, plugins, keys, quota history and
   records, **When** the operator runs each listed read with and without `--json`, **Then** each
   output is byte for byte the output the same command gave before this slice.
2. **Given** the server is stopped, **When** the operator runs each listed read, **Then** it works
   exactly as before: facts from the home's files are shown, and facts that need the running
   server are shown as unavailable in the same words as today.
3. **Given** a running server, **When** a read is asked the CLI's way (home files, plus the
   operator socket for live facts) and from inside the server, over the same loaded state,
   **Then** the two answers are identical.
4. **Given** a read's text output shows a fact the `--json` output does not carry today (for
   example a key's name next to its id), **When** the read model answers, **Then** the answer
   carries that fact too, and the `--json` output is still unchanged.

---

### User Story 2 — Page back through request records (Priority: P2)

The operator's journal holds many requests. They look at the newest page of records, then ask
for the page before it by naming the oldest record they saw, and so on. The newest page appears
without the whole journal being read, so it stays quick however large the journal grows.

**Why this priority**: a dashboard needs paged records, and today `records list` reads the
whole journal before showing anything; on a large journal this is the slowest read.

**Independent Test**: on a journal of 100,000 records, list the newest 50, then pass the last
id shown to `--before` repeatedly; the pages join without gaps or repeats and match the full
listing. The benchmark shows the newest page's time does not grow with the journal's size.

**Acceptance Scenarios**:

1. **Given** a journal of records, **When** the operator runs `records list --limit 50` and then
   `records list --before <last id shown> --limit 50`, **Then** the second page holds the next 50
   older records, with no record repeated or skipped.
2. **Given** filters (provider, account, agent, model, reason, since), **When** they are combined
   with `--before`, **Then** the page is the filtered listing's records older than that id, in the
   same newest-first order.
3. **Given** an id that names no record, **When** the operator passes it to `--before`, **Then**
   the command fails with an error that names the id.
4. **Given** records still in flight on a running server, **When** the operator pages, **Then**
   they appear in their place in the order, named as `records list` names them today.

---

### User Story 3 — See every unified model at once (Priority: P2)

The operator wants to see all their unified models without resolving them one name at a time:
each model's kind, its members in order with the upstream model id each one sends, the limits
notes, and the unified models that were dropped and why.

**Why this priority**: the models view of any future dashboard needs this list, and today only
`resolve <name>` shows a unified model, one at a time.

**Independent Test**: on a home with several unified models (one with members whose limits
differ, one dropped), `unified` lists them all; `unified <name>` shows one; an unknown name and an
empty home behave as below.

**Acceptance Scenarios**:

1. **Given** unified models are loaded, **When** the operator runs `unified`, **Then** each is
   shown with its kind, its members in order with their upstream ids, and its limits notes, and
   each dropped unified model is shown with the reason it was dropped.
2. **Given** a loaded unified model, **When** the operator runs `unified <name>`, **Then** only
   that model is shown, with the same facts `resolve <name>` shows for it.
3. **Given** a name that is not a loaded unified model, **When** the operator runs
   `unified <name>`, **Then** the command exits 2 with the message `resolve` gives for that name.
4. **Given** no unified models are declared, **When** the operator runs `unified`, **Then** it says
   there are none and exits 0.
5. **Given** any of the above, **When** `--json` is added, **Then** the same facts are given as
   JSON.

---

### User Story 4 — See the operator's break behaviour (Priority: P3)

The operator set (or never set) what happens when a stream breaks after output has started. They
want to see the current setting without opening the config file, and to know whether it is their
choice or the default.

**Why this priority**: small, but today the setting can be changed from the CLI and not seen
from it, and a dashboard needs a CLI twin for it.

**Independent Test**: on a fresh home, `behaviour show` shows the default value marked
"(default)"; after `behaviour set-break error_event`, it shows `error_event` without the mark.

**Acceptance Scenarios**:

1. **Given** the operator never set a break behaviour, **When** they run `behaviour show`,
   **Then** it shows the value in effect, marked "(default)".
2. **Given** the operator set it with `behaviour set-break`, **When** they run `behaviour show`,
   **Then** it shows their value, not marked as the default.
3. **Given** either case, **When** `--json` is added, **Then** the value and whether it is the
   default are given as JSON.

---

### Edge Cases

- The home's files changed since the server last loaded them (an edit not yet applied). The CLI
  reads files, so it may show the edit before the server does, as today. The CLI and in-server
  answers are compared over the same loaded state; a pending edit is not a disagreement.
- The server stops while a read is running. The read answers as it would with the server stopped,
  the same way the command behaves today.
- A record id passed to `--before` was pruned. It names no record, so the command fails naming
  the id (User Story 2, scenario 3).
- `--before` without `--limit`: every record older than that id is listed, as `records list` with
  no limit lists every record today.
- A unified model's member belongs to a plugin that was skipped: the model is listed under the
  dropped ones with the reason, as the load report states it.
- The config file has a break behaviour value the server would refuse to load: `behaviour show`
  reports the load error, as other reads of a broken config do today, and shows no value.
- A read whose text output combines a home file with a live answer that may be missing (such as
  `accounts list`): with the server stopped, the live part is shown as unavailable in today's
  words, and the in-server answer is never asked for in that case.

## Requirements *(mandatory)*

### Functional Requirements

**Shared read model**

- **FR-001**: Every listed read MUST get its answer from one shared read model: `accounts list`
  (with and without `--long`), `quota` (current windows), `quota history`, `routing [target]`,
  `records list`, `records show`, `providers`, `model`, `plugins list` (with and without
  `--community`), `keys list`, `check`, and `resolve`. The CLI MUST print what the read model
  returns and MUST NOT build any of these answers itself.
- **FR-002**: Each read's answer MUST carry every fact that command's text output shows, so text
  and `--json` are two renderings of one answer. Where the text shows a fact the `--json` output
  does not carry today, the answer carries it and the `--json` output stays as it is (FR-004).
- **FR-003**: Each read MUST give an identical answer whether it is asked the CLI's way (the
  home's files, plus the operator socket for facts only a running server has) or from inside the
  running server, over the same loaded state.
- **FR-004**: The text and `--json` output of every existing command MUST stay byte for byte the
  same, including error messages and exit codes.
- **FR-005**: Every listed read MUST keep working with the server stopped exactly as it does now:
  the same facts shown, and the same words for what is unavailable without a server.
- **FR-006**: The read model MUST NOT change any state: no file is written and no server state is
  changed by any read.
- **FR-007**: No read MAY show more of a secret than the CLI shows today: provider keys and
  sign-in tokens appear only as their last four characters or as the name of the environment
  variable that holds them, and agent keys only as their last four characters. Plugins MUST
  receive nothing new.

**New reads**

- **FR-008**: `unified` MUST list every loaded unified model with its kind, its members in order
  with each member's provider, model and upstream id, its limits notes, and every dropped unified
  model with the reason it was dropped.
- **FR-009**: `unified <name>` MUST show only that unified model, with the same member facts
  `resolve <name>` shows. An unknown name MUST exit 2 with the message `resolve` gives for it.
- **FR-010**: With no unified models declared, `unified` MUST say so and exit 0.
- **FR-011**: `behaviour show` MUST show the break behaviour in effect and mark it "(default)" when
  the operator has not set it. It MUST need no running server.
- **FR-012**: `records list --before <ID>` MUST list only records older than that id, in the same
  newest-first order, combined with every existing filter and `--limit`.
- **FR-013**: `records list --before` with an id that names no record MUST fail with an error that
  names the id.
- **FR-014**: When a limit is given, reading a page MUST stop once the page is full: the newest
  page MUST NOT require reading the whole journal.
- **FR-015**: Records in flight on a running server MUST appear in pages in their newest-first
  place, named as `records list` names them today.
- **FR-016**: `unified` and `behaviour show` MUST support `--json` and the global `--home`, and
  follow the CLI's exit codes (0 ok, 1 error, 2 not found).
- **FR-017**: Each new read MUST come from the shared read model (FR-001, FR-003).

### Key Entities

- **Read**: one question the operator can ask about the router's state (accounts, quota, routing,
  records, providers, a model, plugins, keys, the load report, a resolution, unified models, the
  behaviour setting), and its answer. The answer is a value from which both the text and the JSON
  output are rendered.
- **Live facts**: the part of an answer that only a running server knows (account states and
  cooldowns, in-flight records, the routing view's current pace and deficits, journal health).
  Without a server they are reported as unavailable.
- **Record page**: a run of records in newest-first order, bounded by an optional `before` id and
  an optional limit, after the listing's filters.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: For every listed read, on fixture homes covering each kind of account, plugin,
  unified model, key, quota history and record (including in-flight and cut-short records), the
  text and `--json` output with the server running and stopped is identical to the output before
  this slice: 0 differing bytes, and every existing CLI test passes unchanged.
- **SC-002**: 100% of the listed reads and the two new list reads give identical answers the CLI's
  way and from inside a running server, on the same fixture homes.
- **SC-003**: No listed read builds its answer outside the read model (checked by review of every
  CLI read command).
- **SC-004**: On a journal of 100,000 records, a benchmark reads the newest page of 50 and a deep
  page. The newest page's time does not grow with the journal's size, and both meet the target
  recorded in the plan.
- **SC-005**: Paging a journal with `--before` from newest to oldest yields every record exactly
  once, in the same order as the full listing, with and without filters.
- **SC-006**: With known secret values planted in a fixture home, no read shows more of any of
  them than the last four characters, by either route.

## Assumptions

- "The CLI's way" for reads that need no running server (`providers`, `model`, `plugins list`,
  `resolve`, `unified`, `behaviour show`, and the file-backed parts of others) means reading the
  home's files, as today. The in-server route reads the server's loaded state. They are compared
  over the same loaded state (Edge Cases).
- The dashboard will run inside the `serve` process (recorded direction, 2026-10-05). This slice
  only makes the in-server route available and tested; nothing uses it outside tests yet.
- Record ids keep their current form; "older than an id" follows the journal's newest-first
  order.
- The text layouts of `unified` and `behaviour show` follow the existing CLI's style; the exact
  wording is settled in the plan's contract.
- Slice 006 (routing decision and persistent request history) is the base: its records, journal,
  routing view and quota history are what the reads show. This branch starts from 006's tip.

## Out of Scope

- `validate`: it checks plugin files the operator names, not router state, and stays outside the
  read model as it is.
- Dashboard pages, the dashboard token and sign-in, its listener, the `[dashboard]` config and the
  style guide: the dashboard slice, shaped again when the dashboard vision settles.
- A per-item subject field on `check` results: the dashboard slice.
- Any command or read that changes state. Whether the dashboard ever writes stays open: later.
- The latency view (median and 95th percentile, trends): its own slice after the dashboard.
- Usage over a chosen period: later, likely with the latency view.
- An agents panel and harness tags on keys: later, with the dashboard vision.
- Estimated cost. Whether prices are plugin data or core stays open: later.

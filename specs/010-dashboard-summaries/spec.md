# Feature Specification: Dashboard summaries and landscape

**Feature Branch**: `010-dashboard-summaries`

**Created**: 2026-10-06

**Status**: Draft

**Scope brief**: [specs/briefs/2026-10-05-dashboard.md](../briefs/2026-10-05-dashboard.md) (slice 2
of two). In clarify, plan and analyze, an answer that contradicts a confirmed row of that brief's
ledger means stop and revisit the brief.

**Builds on** spec 009. By the user's direction of 2026-10-06 it is branched from `009-dashboard`
before 009 ships. This slice fills the slots spec 009 left: the key cards' "requests today" and
the traffic landscape (009 FR-029), the provider window's "last response" (009 FR-030), and
Usage's stat cards, topology graph and period filter (009 FR-035). It replaces 009 FR-039's "this
slice adds no latency summaries" and the harness-tag line of 009's Assumptions. Every other spec
009 requirement holds unchanged.

**Input**: User description: "Dashboard summaries and landscape: fills the slots the dashboard left for them, with real numbers, and the CLI shows the same numbers. Two new read-only CLI views, each with --json: latency over the last 24 hours per agent and per provider (router overhead, median and 95th-percentile time to first token, request counts, and whether the last response resolved or failed), and request and token totals for a chosen period (Today, 24h, 7D, 30D, 60D, All). The Usage page fills its period filter, its five stat cards (requests, input, cached, output, Est. Cost) and its circular topology graph. Est. Cost uses the prices plugins already declare and the operator's account overrides, priced at the time of each request; requests with no price are left out of the total and counted on the card; it is always labelled \"Estimated, not actual billing\". The Endpoint page fills the traffic landscape: agents on the left, the router in the centre, providers on the right, one pipe per agent and per provider, a gauge on each hop, a colour per agent, with gauge numbers and request counts for the last 24 hours, labelled so. Each agent key card fills its \"Requests today\" from the totals for Today. The provider window fills its \"Last response\" from the latency view: resolved or failed, and when. An agent key gains a free-text harness tag: `nullrouter keys issue <name> --harness <text>`, shown in `keys list` and on the agent cards; it is display only, existing keys show none, and the core keeps no list of client names. The dashboard stays look only and without scripts; every number a page shows equals the CLI's for the same moment; a page never shows a secret or a prompt; a fault in the dashboard never slows or breaks client requests. The slice fails if: a page disagrees with the CLI for the same moment; the look visibly departs from the style guide taken from 9router. Out of scope: latency trends over time → the latency slice; extra Usage charts and breakdown tables → later; per-model prices → later; changing anything from the dashboard → later; live auto-refresh → later; client-side adapters → slice 004. Scope brief: specs/briefs/2026-10-05-dashboard.md"

## Clarifications

### Session 2026-10-06

- Q: On the landscape's "router → provider" gauge, should the time to first token cover only that
  provider's own wait, or the client's whole wait for that request? → A: Only the provider's own
  wait: from the start of the attempt that served the request to its first token. Router overhead
  and earlier failed attempts are left out (FR-010, FR-018).
- Q: Should the latency view also give figures per unified model, or stay per agent and per
  provider as the brief confirmed? → A: Per agent and per provider only. Per-unified-model figures
  go to the latency slice; constitution Principle VIII's measuring is met by the records and its
  surfacing by spec 009's Requests table (Out of Scope).
- Q: What should the two new CLI commands be called? → A: `nullrouter usage [--period
  today|24h|7d|30d|60d|all]` for the totals view and `nullrouter latency` for the latency view
  (FR-001, FR-008).
- Q: On Usage's topology graph, should the providers and their counts follow the period filter or
  stay fixed to the last 24 hours, and should it show "connections routed now"? → A: They follow
  the period filter (from `nullrouter usage`); each provider's last response comes from
  `nullrouter latency`, labelled "last 24 h"; no in-flight count (FR-016).
- Q: Should the operator be able to set or change the harness tag on a key that already exists, or
  only when issuing a new key? → A: Both: at issue with `--harness`, and later with
  `nullrouter keys tag <name> <text>` (set or change) and `keys tag <name> --clear` (remove); the
  key itself is unchanged (FR-023a).

## User Scenarios & Testing *(mandatory)*

This slice has one user: the **operator**, who reads 0router's state from the CLI and, since spec
009, from the dashboard on the same machine. Clients (agents) see no change; a key's harness tag
changes nothing about how its requests are handled.

Terms used throughout (spec 009's terms, such as *twin*, *for the same moment*, *slot* and *as of*,
keep their meaning):

- The **latency view** (`nullrouter latency`) is the new CLI read of the last 24 hours of request
  records, per agent and
  per provider. Its twins on the dashboard are the traffic landscape and each provider's "last
  response".
- The **totals view** (`nullrouter usage`) is the new CLI read of request and token totals for one
  period. Its twins on
  the dashboard are Usage's stat cards and topology graph and each key card's "requests today".
- A **period** is one of Today, 24h, 7D, 30D, 60D and All. Today starts at midnight in this
  machine's time zone; 24h, 7D, 30D and 60D are the 24 hours, 7, 30 and 60 days that end at the
  read's time; All is every record the journal holds.
- **Router overhead** is the time from a request's arrival to the start of its first upstream
  attempt: the time 0router itself took before calling a provider.
- **Time to first token** is the record's time to first token, as `records list` shows it: the
  client's whole wait, from arrival to the first token.
- A provider's **own time to first token** is the part of that wait spent on the provider that
  served the request: from the start of its serving attempt to the first token. It leaves out
  router overhead and any earlier failed attempt.
- A **hop** is one pipe of the landscape: an agent into the router, or the router out to a
  provider.
- A **last response** is the result of the newest finished attempt: *resolved* if the provider
  answered, *failed* if it did not, with its time.
- **Est. Cost** is an estimate of what the period's requests cost, from the prices 0router already
  knows. It is never actual billing.
- A **harness tag** is a free-text label the operator gives a key when issuing it, naming the
  client that uses it (for example "claude-code"). It is shown and never acted on.

### User Story 1 — See request and token totals for a period, in the CLI and on Usage (Priority: P1)

The operator asks the CLI for the totals of a period and gets the number of requests, input
tokens, cached tokens, output tokens and Est. Cost, with how many requests the cost left out and
why. On Usage, the period filter (Today, 24h, 7D, 30D, 60D, All) chooses the period, and the five
stat cards show the same numbers. The Est. Cost card always says "Estimated, not actual billing".

**Why this priority**: "How much did my agents use, and roughly what did it cost" is the question
the Usage page is drawn for, and the cards are its first row. The totals view also feeds the key
cards' "requests today".

**Independent Test**: In a test home with records spread over 90 days (some with cached tokens,
some on a priced account, some on an account with an override, some on an account with no price,
some whose usage the provider didn't report, one still in flight), run the totals view for each
period and open Usage with each period chosen, for the same moment. Every card equals the CLI, and
each total equals the sum worked out by hand from the records.

**Acceptance Scenarios**:

1. **Given** records in several periods, **When** the operator runs the totals view for a period,
   **Then** it shows the requests, input, cached and output tokens and Est. Cost of the records
   that arrived in that period, in text and with `--json`.
2. **Given** Usage is opened with no period chosen, **When** it loads, **Then** the filter shows
   Today chosen and the cards show Today's totals.
3. **Given** the operator chooses 7D, **When** the page reloads, **Then** the cards show exactly
   what the totals view shows for 7D at that moment.
4. **Given** a request served by an account whose plugin declares a price that changes by time of
   day, **When** it is priced, **Then** the price in effect at the request's arrival time is used.
5. **Given** an account with a price override, **When** its requests are priced, **Then** the
   override is used instead of the plugin's price.
6. **Given** requests on an account with no price, **When** the totals are shown, **Then** they are
   left out of Est. Cost, and the card and the CLI say how many were left out and why.
7. **Given** requests whose usage the provider did not report, **When** the totals are shown,
   **Then** they count as requests, add no tokens and no cost, and the CLI and the cards say how
   many there were, in the words `records` uses.
8. **Given** any period, **When** Est. Cost is shown, **Then** it is labelled "Estimated, not actual
   billing", on the page and in the CLI.
9. **Given** an empty period, **When** it is shown, **Then** every card shows zero and the page
   says there were no requests in that period.

---

### User Story 2 — See latency over the last 24 hours, per agent and per provider (Priority: P1)

The operator asks the CLI for the latency view and gets, for each agent key and each provider,
the number of requests in the last 24 hours, the latency figures (median and 95th percentile), and
the last response: resolved or failed, and when. An agent's figures are its router overhead and
its time to first token; a provider's figure is its own time to first token.

**Why this priority**: It is the data under the landscape, the provider window's "last response"
and the topology graph. Without it, three of the slots stay empty. It also answers "is 0router
slowing my agents down" from the CLI.

**Independent Test**: In a test home with records over 48 hours (two agents, three providers, a
failed attempt followed by a fallback, a request refused before a key matched, a request with no
time to first token), run the latency view and compare every number with the one worked out by
hand from the records of the last 24 hours.

**Acceptance Scenarios**:

1. **Given** records older and newer than 24 hours, **When** the operator runs the latency view,
   **Then** only records that arrived in the 24 hours ending at the read's time count, and the
   output says "last 24 h".
2. **Given** an agent with requests in the window, **When** the view is read, **Then** that agent's
   row shows its request count and the median and 95th percentile of its router overhead and of
   its time to first token.
3. **Given** a provider that served requests in the window, **When** the view is read, **Then** its
   row shows its request count, the median and 95th percentile of its own time to first token on
   the requests it served, and how many requests each agent sent it.
4. **Given** a request whose first attempt on provider A failed and whose fallback on provider B
   succeeded, **When** the view is read, **Then** A's last response is failed and B's is resolved,
   each with its time, if those are the newest finished attempts on each; and B's own time to
   first token for that request is measured from the start of B's attempt, so the time lost on A
   and the router overhead are not in B's figures.
5. **Given** a request that has no time to first token (it failed, or is still in flight), **When**
   the view is read, **Then** it counts as a request but is left out of the time-to-first-token
   figures.
6. **Given** an agent or provider with no requests in the window, **When** the view is read,
   **Then** its row says so instead of showing zero latency.
7. **Given** a request refused before a key matched, **When** the view is read, **Then** it is in no
   agent's row and no provider's row.

---

### User Story 3 — See the traffic landscape on Endpoint & Key (Priority: P2)

The operator opens Endpoint & Key and sees the traffic landscape where spec 009 drew its slot:
agents on the left, each in its own colour, the router in the centre, providers on the right. One
pipe joins each agent to the router and one joins the router to each provider. A gauge on each
hop shows its own latency: router overhead on agent pipes, the provider's own time to first token
on provider pipes. Each
pipe shows its request count. Every number is for the last 24 hours, and the landscape says so.

**Why this priority**: It is the picture that tells the operator at a glance who is sending
traffic, where it goes, and whether any hop is slow. It depends on the latency view.

**Independent Test**: With the latency view's test home, open Endpoint & Key and compare every
gauge and count with the latency view for the same moment. Check that each agent keeps its colour
across reloads and pages, and that the landscape works with scripts turned off in the browser.

**Acceptance Scenarios**:

1. **Given** two agents and three providers with traffic, **When** the page loads, **Then** it
   shows two agent pipes into the router and three provider pipes out of it, each with its request
   count and gauge, labelled "last 24 h".
2. **Given** an agent pipe, **When** the operator reads its gauge, **Then** it shows the agent's
   router overhead median and 95th percentile as the latency view shows them.
3. **Given** a provider pipe, **When** the operator reads its gauge, **Then** it shows the median and 95th
   percentile of the provider's own time to first token, and the request count from each agent, as the
   latency view shows them, each agent in its colour.
4. **Given** an agent, **When** any page shows it, **Then** it has the same colour, on every load.
5. **Given** no traffic in the last 24 hours, **When** the page loads, **Then** the landscape shows
   the agents and providers with no pipe lit and says there were no requests in the last 24 hours.
6. **Given** a revoked key with traffic in the last 24 hours, **When** the page loads, **Then** it
   appears in the landscape marked revoked, as its key card marks it.

---

### User Story 4 — Fill the key cards, the provider window and the topology graph (Priority: P2)

Each agent key card on Endpoint & Key shows its requests today, from the totals for Today. Each
provider's window on Providers shows its last response: resolved or failed, and when, from the
latency view. Usage's circular topology graph shows the router in the middle and, around it, each
provider that took requests in the chosen period, with its request count for that period and its
last response.

**Why this priority**: These fill the remaining slots spec 009 drew. Each is small and depends on
the two views.

**Independent Test**: With the test homes of Stories 1 and 2, compare each key card's "requests
today" with the totals view for Today, each provider window's "last response" with the latency
view, and the topology graph with the totals view and the latency view for the chosen period, all
for the same moment.

**Acceptance Scenarios**:

1. **Given** a key that sent 12 requests since midnight, **When** Endpoint & Key loads, **Then** its
   card says 12 requests today, as the totals view for Today shows for that key.
2. **Given** a key with no requests today, **When** the page loads, **Then** its card says 0
   requests today.
3. **Given** a provider whose newest finished attempt in the last 24 hours failed, **When** its
   window opens, **Then** "last response" says failed and when, as the latency view shows.
4. **Given** a provider with no finished attempt in the last 24 hours, **When** its window opens,
   **Then** "last response" says there was none in the last 24 hours.
5. **Given** the operator chooses 30D on Usage, **When** the page reloads, **Then** the topology
   graph shows each provider that took requests in those 30 days, with its count, as the totals
   view shows for 30D.

---

### User Story 5 — Tag a key with its harness (Priority: P3)

The operator issues a key with `nullrouter keys issue <name> --harness <text>`, or tags an existing
key with `nullrouter keys tag <name> <text>`. `keys list` shows the tag, in text and with `--json`,
and the key's card on Endpoint & Key shows it as a badge. Keys issued without a tag, and every key
issued before this slice, show none until the operator tags them. Changing or clearing a tag never
changes the key.

**Why this priority**: It makes the key cards and the landscape easier to read, but no number
depends on it.

**Independent Test**: Issue one key with `--harness claude-code`, one with `--harness "my own
tool"`, and one without; then tag the untagged one with `keys tag`, change one tag and clear
another. Check `keys list`, `keys list --json` and the cards after each step, and that each key's
secret still works unchanged. Send requests with each key and check that their records,
placements and responses are the same as for an untagged key.

**Acceptance Scenarios**:

1. **Given** a key issued with `--harness codex`, **When** the operator runs `keys list`, **Then**
   it shows the tag "codex"; **When** Endpoint & Key loads, **Then** that key's card shows it.
2. **Given** a tag that names no known client, **When** the key is issued, **Then** it is accepted
   as written; the core has no list of client names to check it against.
3. **Given** a key issued before this slice, **When** `keys list` runs, **Then** it shows no tag for
   it, and the key keeps working.
4. **Given** a tagged key, **When** its requests are served, **Then** the tag changes nothing about
   how they are handled.
5. **Given** a tag with control characters or longer than the limit, **When** the key is issued,
   **Then** the command refuses it, says why, and issues no key; **When** it is given to
   `keys tag`, **Then** the command refuses it, says why, and the key's tag is unchanged.
6. **Given** a key issued before this slice, **When** the operator runs `keys tag <name> hermes`,
   **Then** `keys list` and its card show "hermes", and the agent using that key keeps working
   with the same secret.
7. **Given** a tagged key, **When** the operator runs `keys tag <name> --clear`, **Then** it shows
   no tag again.
8. **Given** a revoked key, **When** the operator tags it, **Then** the tag is set as for any key;
   the key stays revoked.

---

### Edge Cases

- **A huge journal** (100,000 records or more). The views and the pages that use them stay within
  spec 009's page-load bound, and reading them never slows client requests.
- **Requests in flight at the read's time.** They count as requests in the period they arrived in;
  they add no tokens, cost or latency until they finish.
- **A request that crosses a period boundary** (arrived before midnight, finished after). It
  belongs to the period its arrival falls in.
- **A period while records were not kept.** The totals and latency views carry the warning
  `records list` and `check` show, and so do the pages.
- **A clock change** (daylight saving). Today starts at this machine's local midnight; the rolling
  periods are measured in real elapsed time.
- **An account repriced after its requests.** Est. Cost uses the prices declared now, with the
  time-of-day rule in effect at each request's time; past price changes are not tracked, and the
  Est. Cost note says so.
- **An attempt on an account that no longer exists.** Its requests still count; with no price to
  use, they are left out of Est. Cost and counted as unpriced.
- **Usage counted differently by providers** (input with or without cached tokens). Tokens are
  added as `records` shows them, so cached tokens are never counted twice.
- **More agents than the style guide has colours.** Colours repeat in a fixed order; agents stay
  told apart by name.
- **A very long harness tag or key name.** Shown as spec 009 shows long names: in full, or
  shortened with the full text on the page.
- **Only one request in a window.** Its median and 95th percentile are its own value.

## Requirements *(mandatory)*

### Functional Requirements

**Totals view**

- **FR-001**: The CLI MUST offer a read-only totals view, `nullrouter usage [--period
  today|24h|7d|30d|60d|all]`, for one period (Today when none is given), in text and with
  `--json`.
- **FR-002**: The totals view MUST show, for the records that arrived in the period: the number of
  requests; input, cached and output tokens; Est. Cost; how many requests Est. Cost left out
  because no price applies; and how many requests had no usage reported.
- **FR-003**: The totals view MUST also show, for the period, the number of requests per agent key
  and per provider. These are the twins of the key cards' "requests today" and the topology graph.
- **FR-004**: Tokens MUST be counted as `records` shows them for each request. Cached tokens are
  tokens read from cache, and are never also counted as input.
- **FR-005**: Est. Cost MUST price each request's tokens with the price of the account that used
  them, at the request's arrival time: the account's price override if it has one, otherwise the
  price its provider's plugin declares, choosing the schedule entry in effect at that time. Input,
  cached, cache-write and output tokens are each priced at their own declared price.
- **FR-006**: A request with tokens on an account that has no price (no plugin price and no
  override, or an account that no longer exists) MUST be left out of Est. Cost and counted as
  unpriced.
- **FR-007**: Est. Cost MUST always be labelled "Estimated, not actual billing", on the page and in
  the CLI's text output, and MUST say that prices changed after a request are not tracked.

**Latency view**

- **FR-008**: The CLI MUST offer a read-only latency view, `nullrouter latency`, of the 24 hours
  that end at the read's time, in text and with `--json`, labelled "last 24 h". The window is fixed; it takes no period.
- **FR-009**: For each agent key with requests in the window, the latency view MUST show its request
  count, the median and 95th percentile of its router overhead and of its time to first token, and
  its last response.
- **FR-010**: For each provider with attempts in the window, the latency view MUST show its request
  count, the median and 95th percentile of its own time to first token on the requests it served
  (from the start of the serving attempt to the first token; router overhead and earlier failed
  attempts left out), the request count from each agent, and its last response (resolved or
  failed, and when).
- **FR-011**: A request with no time to first token MUST count as a request and be left out of the
  time-to-first-token figures. A row with no value to summarize MUST say so rather than show zero.
- **FR-012**: A request refused before a key matched MUST NOT appear in any agent or provider row.
- **FR-013**: The median and 95th percentile MUST be computed exactly from every record in the
  window, by one rule the CLI and the dashboard share, so that the same records always give the
  same figures.

**Usage page**

- **FR-014**: Usage MUST fill its period filter with Today, 24h, 7D, 30D, 60D and All, Today chosen
  by default. Choosing a period reloads the page with it; no script runs.
- **FR-015**: Usage MUST fill its five stat cards (requests, input, cached, output, Est. Cost) with
  the totals view's values for the chosen period, including the unpriced and not-reported counts
  and the Est. Cost label and note.
- **FR-016**: Usage MUST fill its circular topology graph: the router in the middle and, around it,
  each provider that took requests in the chosen period, with its request count from the totals
  view and its last response from the latency view, labelled "last 24 h". The graph shows no agent
  nodes, no count of requests in flight now (the mockup's "connections routed now"), and no number
  that neither view shows.

**Endpoint & Key page**

- **FR-017**: Endpoint & Key MUST fill the traffic landscape: agents on the left, the router in the
  centre, providers on the right; one pipe per agent into the router and one per provider out of
  it; a gauge on each hop; each pipe's request count; all from the latency view and labelled
  "last 24 h".
- **FR-018**: An agent's gauge MUST show its router overhead median and 95th percentile; a provider's
  gauge MUST show the median and 95th percentile of its own time to first token (FR-010) and, per
  agent, the requests that agent sent it.
- **FR-019**: Each agent MUST have one colour, taken from the style guide, the same on every page
  and every load. A provider pipe shows the colours of the agents that used it.
- **FR-020**: The landscape MUST show every active key and every provider with an account, even
  with no traffic, and any revoked key or other provider with traffic in the window, marked as its
  card or provider marks it.
- **FR-021**: Each key card MUST fill its "requests today" with that key's count from the totals
  view for Today.

**Providers page**

- **FR-022**: Each provider's window MUST fill its "last response" from the latency view: resolved
  or failed and when, or that there was none in the last 24 hours.

**Harness tag**

- **FR-023**: `nullrouter keys issue <name>` MUST accept `--harness <text>` and store the text with
  the key. The text is free; it MUST be refused, with no key issued, only if it is empty, holds
  control characters, or exceeds a length limit the command states.
- **FR-023a**: `nullrouter keys tag <name> <text>` MUST set or replace the harness tag of an existing
  key, and `nullrouter keys tag <name> --clear` MUST remove it, by the same text rules as FR-023.
  Neither changes the key's secret, id, state or anything else about it; a refused tag leaves the
  old one in place. It works on revoked keys too.
- **FR-024**: `keys list` MUST show each key's harness tag, in text and with `--json`, and each key
  card MUST show it as a badge. Keys without one, including every key issued before this slice
  until it is tagged, show none.
- **FR-025**: The harness tag MUST be display only: it changes nothing about how a request is
  checked, routed, changed, recorded or answered. The core MUST NOT keep a list of client or
  harness names.

**Carried over from spec 009**

- **FR-026**: Every number a page shows MUST equal what the totals view or the latency view shows
  for the same moment (009 FR-019). The pages MUST show everything these views show, mapped as
  follows (this extends 009 FR-021):
  - Usage: the totals view for the chosen period (all but the per-agent counts), and the latency
    view's last response per provider;
  - Endpoint & Key: the latency view, the per-agent counts of the totals view for Today, and the
    harness tags of `keys list`;
  - Providers: the latency view's last response per provider.
- **FR-027**: The dashboard MUST stay look only and run no script (009 FR-004, FR-013). Hover
  details on the landscape and the graph, if any, need no script and show nothing the page doesn't
  already show elsewhere.
- **FR-028**: No page and no new CLI view MUST show a secret or prompt or response content (009
  FR-043, FR-044). A harness tag is not a secret.
- **FR-029**: Reading the two views, from the CLI or a page, MUST NOT fail, slow or delay any client
  request, and the work of one read MUST be bounded (009 FR-016, FR-017).
- **FR-030**: Everything this slice draws (cards, filter, graph, landscape, pipes, gauges, badges,
  agent colours) MUST use only the style guide's values and 9router's component styles (009
  FR-045 to FR-047). Where the style guide lacks a value this slice needs, it is added from
  9router's code, naming the file.
- **FR-031**: The pages' "as of" time MUST be the read time of both views, so that "today", "last 24
  h" and every rolling period end at the time the page names (009 FR-022).

### Key Entities

- **Period**: One of Today, 24h, 7D, 30D, 60D, All; a start and end time, ending at the read's time
  (Today starts at local midnight; All starts at the oldest record).
- **Period totals**: For one period: requests, input, cached and output tokens, Est. Cost, unpriced
  and not-reported counts, and requests per agent key and per provider.
- **Latency summary**: For the last 24 hours: per agent, request count, router overhead and time
  to first token (median and 95th percentile), last response; per provider, request count, its own
  time to first token (median and 95th percentile), last response, and the requests from each
  agent.
- **Last response**: Resolved or failed, the time, and the provider; from the newest finished
  attempt.
- **Price in effect**: The price used for a request's tokens: the account override, or the plugin
  schedule entry for the request's arrival time.
- **Harness tag**: Free text stored with an agent key, set at issue or with `keys tag`, changed or
  cleared without touching the key; shown, never acted on.
- **Agent colour**: A colour from the style guide fixed for each agent key.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: For each period and for the latency view, a test that builds the pages and runs the
  matching view for the same moment finds 0 disagreements, across test homes covering every case
  in User Stories 1 to 4.
- **SC-002**: In a test home whose totals and percentiles are worked out by hand, the totals view
  and the latency view match the hand figures exactly (counts, tokens, percentiles) and to the cent
  (Est. Cost).
- **SC-003**: 100% of requests on accounts with no price are left out of Est. Cost and counted on
  the card; 0 are priced at a made-up value.
- **SC-004**: The totals view for every period, the latency view, and every page that uses them each
  answer in under 1 second on a home with 50 accounts, 20 unified models and 100,000 records.
- **SC-005**: While the views and pages are read continuously, client requests show 0 additional
  failures and no more than 1 ms added latency at the 95th percentile, compared with no reads.
- **SC-006**: Requests sent with a tagged key and with an untagged key produce the same placements,
  upstream bodies and responses in 100% of test cases.
- **SC-007**: 100% of colours, sizes, spacing, radii and shadows used by the new parts of the pages
  are in the style guide, and each traces to a 9router file.
- **SC-008**: Side by side with 9router's usage cards, period filter and topology graph, the new
  parts are judged the same in colour, type, spacing and shape by the user.
- **SC-009**: From Endpoint & Key alone, without running a command, the operator can tell which
  agents sent traffic in the last 24 hours, to which providers, and which hop is the slowest.
- **SC-010**: A scan of every page and every output of the new views in the full test suite finds
  0 secrets and 0 prompt text.

## Assumptions

Items marked *(technical decision)* are Claude's. They may be revised in planning without asking
the user, as long as nothing the user sees changes. The user-visible choices this list first left
open were settled in clarify (see Clarifications).

- Neither `usage` nor `latency` is a command today; the later latency slice extends `latency` with
  trends (names clarified 2026-10-06).
- Today, 24h, 7D, 30D, 60D and All mean what 9router's usage periods mean (`usageRepo.js`): Today
  from local midnight, the others rolling back from now, All without a start.
- Cached tokens are tokens read from cache, as 9router counts them (`cache_read_input_tokens`).
  Cache-write tokens add to Est. Cost at their own price and have no card of their own.
  *(technical decision)*
- Prices are per million tokens in US dollars, as plugins and overrides declare them today. Where a
  declared price has no output or cache price, those tokens are priced by the rule routing already
  uses for that plugin; the plan names it. *(technical decision)*
- Router overhead is the first attempt's start, which a record holds in milliseconds after arrival
  (brief P note; confirmed in plan). *(technical decision)*
- A provider's own time to first token (clarified 2026-10-06) is derived from what a record
  already holds: the record's time to first token minus the start of the attempt that served the
  request. Nothing new is recorded. Its exact derivation for requests whose stream was continued
  or restarted after a break is a plan decision. *(technical decision)*
- A provider's request count is the number of requests with at least one attempt on it in the
  window. An attempt the client cancelled is neither resolved nor failed and does not set the last
  response. An agent's last response is its newest finished request: resolved if it was served,
  failed otherwise. *(technical decision)*
- The 95th percentile uses the nearest-rank rule over every value in the window, with no sampling.
  *(technical decision)*
- The gauges' green, amber and red zones and the agent colour order come from 9router's code
  (its provider topology and status colours) through the style guide. Numbers are always printed
  beside each gauge, so a zone never carries a fact alone. *(technical decision)*
- The harness tag's length limit is set in plan (for example 32 characters) and stated by the
  command. *(technical decision)*
- Reading the views uses the same early-stop and bounded-work discipline as slice 008's records
  page, with a bench on a 100,000-record journal (brief P note). *(technical decision)*
- The views read the journal through the shared read model of slice 008, so a page and the CLI
  can be tested against one snapshot at one "as of" time. *(technical decision)*

## Out of Scope

- Latency trends over time (change charts): the latency slice.
- Latency figures per unified model, and total request time summarized over a window: the latency
  slice. Constitution Principle VIII is met here by the records, which measure both per request,
  and by spec 009's Requests table, which shows them (clarified 2026-10-06).
- Extra Usage charts and breakdown tables, including 9router's "Details" tab: later.
- Per-model prices: later. Prices stay per provider and per account.
- Changing anything from the dashboard, including issuing keys or editing tags: later.
- Live auto-refresh and live state (connections in flight now): later. The operator reloads.
- Client-side adapters, and any behaviour tied to a harness tag: slice 004.
- Tracking price changes over time: Est. Cost uses today's declarations.
- A period chooser on the landscape: it is fixed to the last 24 hours.

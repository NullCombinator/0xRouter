# Feature Specification: Dashboard

**Feature Branch**: `007-dashboard`

**Created**: 2026-10-05

**Status**: Draft

**Scope brief**: [specs/briefs/2026-10-05-dashboard-v1-superseded.md](../briefs/2026-10-05-dashboard-v1-superseded.md). In
clarify and plan, an answer that contradicts a confirmed row of that brief's ledger means stop and
revisit the brief.

**Moved out (2026-10-05)**: the shared view layer (research R1; tasks T004-T012, without T011's
`subject` field) and the new CLI reads `unified`, `behaviour show` and `records list --before`
with the newest-first read and its bench (FR-016a; tasks T038, T040-T042, T044, T046, T049) are
now slice 008, read model, on branch `008-read-model`: see
`specs/briefs/2026-10-05-read-model.md` there. This spec is rewritten when the dashboard is
shaped again, once the vision in `docs/dashboard/mockups` settles; until then, don't implement
from it.

**Input**: User description: "Dashboard: a read-only web view of 0router, so the operator can see accounts, quota, routing, requests, models and keys in a browser. It is written in Rust, server-rendered, and served by the running `nullrouter serve` process on its own port, bound to this machine only. The operator gets a dashboard token from the CLI and the browser asks for it once; no page opens without it. The dashboard is look only: every change (accounts, keys, priorities, behaviour) stays in the CLI. Its look is 9router's: a style guide extracted from 9router's code (colors, type, spacing, component styles such as buttons, cards, badges and tables) is written into this repo and the dashboard's styling is built from it; the layout is new and not 9router's. The dashboard is English only. It has four pages: accounts and quota (accounts, sign-in status, which account needs signing in again, polled or estimated quota with reset times, priority); routing and requests (the routing view with pace, share and deficit, and request records with the reason for each placement, latency and token usage); models and providers (providers and plugins, unified models and their members, model types, the limits notes); keys and behaviour (agent access keys and the operator's behaviour settings). Model tests and Combos appear as entries that say the feature isn't built yet and point to the CLI. Each page shows the state at load time with an \"as of\" time, and the operator reloads to refresh. Latency appears as the records already carry it. A fault in the dashboard never slows or breaks client requests. A page never shows a secret or a prompt, and plugins see nothing they didn't before. The slice fails if: a page disagrees with the CLI for the same moment; the look visibly departs from the style guide taken from 9router. Out of scope: latency summaries and trends over time → the next slice (latency view, CLI and dashboard together); acting from the dashboard (changing accounts, keys, priorities or behaviour) → later; reaching the dashboard from another machine, HTTPS and network binding → later; translations and language switching → later; live auto-refresh → later; model tests and combos themselves → later, with their own slices; fitted quota weights and leak detection → later. Scope brief: specs/briefs/2026-10-05-dashboard-v1-superseded.md"

## Clarifications

### Session 2026-10-05

- Q: The CLI has no list of all unified models and no read view of the behaviour settings, so
  those page facts would have nothing to agree with. Add CLI read views, leave the facts off, or
  check them against the files? → A: Add read-only CLI views for both (FR-016a).
- Q: 9router has light and dark themes with a switch. Which does the dashboard support? → A:
  Light only (FR-034).
- Q: Should each page show everything its matching CLI read commands show, or only the facts the
  spec lists? → A: Everything those commands show, including cooldowns and `check`'s warnings
  and notes; quota poll history stays out (FR-019a, FR-019b).
- Q: Pages run no code in the browser, so they can't learn its time zone. What time zone should
  the dashboard show times in? → A: This machine's local time zone, named on each page (FR-017a).
- Q: When `serve` starts and nothing was set up for the dashboard, should its port be open? → A:
  Yes, on by default; until a token is issued the only page names the CLI command that issues
  one; the operator can turn the dashboard off (FR-003).

## User Scenarios & Testing *(mandatory)*

This slice has one user:

- The **operator** runs 0router on their own machine. Today they read its state from the CLI
  (`accounts list`, `quota`, `routing`, `records`, `providers`, `plugins`, `resolve`, `keys
  list`, `check`). After this slice they can also read it in a browser on the same machine.

Clients (agents) and plugin authors see no change.

Terms used throughout:

- The **dashboard** is the set of web pages the running server shows on its own local port.
- The **dashboard token** is the secret the operator gets from the CLI and enters once in the
  browser. It is not an agent access key and cannot be used to send requests to models.
- The **style guide** is a document in this repo that records 9router's look: its colors, type,
  spacing, radii, shadows, and the styles of its buttons, cards, badges, tables, inputs and
  navigation. It is taken from 9router's code, not drawn from screenshots.
- **For the same moment** means: the page and the CLI read the same server state, with no change
  in between.

### User Story 1 — See accounts and quota at a glance (Priority: P1)

The operator opens the dashboard and sees every provider account: its provider, name, whether it
is enabled, its sign-in status, which accounts need signing in again, its priority, and for each
quota window the quota left, whether that figure is polled or estimated (or the account is
pay-as-you-go), and when the window resets. This replaces running `accounts list --long` and
`quota` and reading two tables side by side.

**Why this priority**: The question the operator asks most is "is everything signed in and how
much quota is left". It is the first page and the one that makes the dashboard worth opening.

**Independent Test**: With a test home holding a signed-in account, an account whose sign-in has
expired, an estimated account and a pay-as-you-go account, open the accounts page and compare
every value with `accounts list --long --json` and `quota --json` taken for the same moment.
They agree, and the expired account is marked as needing sign-in.

**Acceptance Scenarios**:

1. **Given** an account whose sign-in can no longer be refreshed, **When** the operator opens the
   accounts page, **Then** that account is visibly marked as needing sign-in again, and the page
   names the CLI command that signs it in.
2. **Given** an account whose quota is polled, **When** the page loads, **Then** each window
   shows the quota left, "polled", the time of the last poll, and the reset time, matching
   `quota` for the same moment.
3. **Given** an account whose provider reports no quota but whose plugin declares limits,
   **When** the page loads, **Then** its windows are shown as "estimated".
4. **Given** an account with neither, **When** the page loads, **Then** it is shown as
   pay-as-you-go, with no quota bar.
5. **Given** an account still waiting for its first poll, or whose last poll failed, **When** the
   page loads, **Then** it says so, in the same words `quota` uses ("pending first poll",
   "stale").
6. **Given** a disabled account or one with priority 0, **When** the page loads, **Then** its
   state and priority are shown as `accounts list` shows them.

---

### User Story 2 — Sign in to the dashboard once, and only on this machine (Priority: P1)

The operator asks the CLI for a dashboard token, opens the dashboard address in a browser on the
same machine, and enters the token once. That browser stays signed in until the operator replaces
the token. Nobody else can open a page: not another machine, not a browser without the token, and
not another website open in the operator's own browser.

**Why this priority**: The pages show account names, sign-in emails, quota and request history.
Without the token gate, any local process or website could read them. The dashboard cannot ship
without it.

**Independent Test**: Start the server, issue a token, and check that a request for any page
without the token gets only the sign-in page; that the right token opens the pages and they stay
open after a browser restart; that a wrong token is refused; that issuing a new token signs the
old browser out; and that the dashboard port cannot be reached from another machine.

**Acceptance Scenarios**:

1. **Given** no dashboard token has been issued, **When** the operator opens the dashboard,
   **Then** they see a page that names the CLI command that issues one, and nothing else.
2. **Given** a token was issued, **When** the operator enters it, **Then** every page opens, and
   the browser does not ask again until the token is replaced or the operator clears the
   browser's data.
3. **Given** a wrong token, **When** it is entered, **Then** it is refused, no page opens, and
   repeated wrong tries are slowed down.
4. **Given** a browser signed in with token A, **When** the operator issues token B from the
   CLI, **Then** the next page load in that browser asks for the token again.
5. **Given** another machine on the same network, **When** it tries the dashboard port, **Then**
   it cannot connect.
6. **Given** another website open in the operator's browser, **When** it tries to load or embed a
   dashboard page, **Then** it can't read the page or see its content.
7. **Given** the token was printed once by the CLI, **When** the operator looks for it later in
   the home directory, logs, records or any page, **Then** it is not there in readable form.

---

### User Story 3 — See where requests went and why (Priority: P2)

The operator opens the routing and requests page. It shows the routing view the CLI's `routing`
command shows (for each target and account: pace, share, deficit, whether its quota is polled or
estimated) and the newest request records: when, which agent, which target, which account served
it, the reason for each placement, each attempt and its outcome, latency (time to first token and
total time, as recorded), and token usage. The operator can narrow the records the same ways
`records list` can (provider, account, agent, model, reason, since) and open one record to see
everything `records show` shows.

**Why this priority**: Slice 006's promise is that the operator can always tell why a request went
where it did. The CLI already keeps that promise; this page makes it quicker to scan. It comes
after the accounts page because it explains behaviour rather than flagging problems.

**Independent Test**: Replay a set of requests in a test home (warm, cold, overflow, a failed
attempt with fallback). Compare the page's routing view with `routing --json`, its record list
with `records list --json` under the same filters, and one record's detail with `records show
--json`, all for the same moment. They agree.

**Acceptance Scenarios**:

1. **Given** a target with several accounts, **When** the page loads, **Then** each account's
   pace, share, deficit and quota source match `routing` for the same moment.
2. **Given** records exist, **When** the page loads, **Then** it lists the newest first, and each
   row shows the placement reason, the serving account, latency and usage as `records list`
   shows them.
3. **Given** a filter (for example reason `overflow` and one agent), **When** the operator applies
   it, **Then** the page lists exactly the records `records list` lists with the same filters.
4. **Given** a record with a failed attempt followed by a fallback, **When** the operator opens it,
   **Then** every attempt is shown with its outcome, reason and latency, as `records show` shows.
5. **Given** a record whose usage the provider did not report, or a request still in flight,
   **When** it is shown, **Then** the page says so in the words `records` uses, rather than
   showing zero.
6. **Given** a disk-full period while records were not kept, **When** the page loads, **Then** it
   shows the same warning `records list` and `check` show.

---

### User Story 4 — See models, providers and plugins (Priority: P2)

The operator opens the models and providers page. It lists the active providers and what each
offers (its model types), the bundled and installed plugins with their state (active, pending,
declined, skipped), the community plugins and whether each fits this core, every unified model
with its kind and its members in order, and the notes for unified models whose members' limits
differ. Model tests and Combos appear here as entries that say the feature isn't built yet.

**Why this priority**: These facts change only when the operator edits configuration, so they are
looked at less often than accounts or records. They are still needed to read the other pages.

**Independent Test**: With a test home that has a declared unified model whose members differ in
context length, a pending plugin conflict and a skipped plugin, compare the page with
`providers --json`, `plugins list --community --json`, the new unified-model list (FR-016a) and
`check --json`, for the same moment. They agree.

**Acceptance Scenarios**:

1. **Given** a unified model with two members, **When** the page loads, **Then** it shows the
   model's name, kind, and members in their declared order with each member's upstream model, as
   the CLI's unified-model list and `resolve` show them.
2. **Given** a unified model whose members differ in context length, **When** the page loads,
   **Then** it shows the same note `check` and `resolve` print.
3. **Given** a skipped plugin or a dropped unified model, **When** the page loads, **Then** it is
   shown with the reason `check` gives.
4. **Given** the Model tests or Combos entry, **When** the operator opens it, **Then** it says the
   feature isn't built yet, and names the CLI commands that cover the nearest thing today (for
   example `resolve` for a unified model's member order), or says there is none.

---

### User Story 5 — See agent keys and behaviour settings (Priority: P3)

The operator opens the keys and behaviour page. It lists every agent access key as `keys list`
shows it (name, id, the last four characters, created, revoked, break behaviour) and the
operator's behaviour settings (today: the default break behaviour for a stream that breaks after
output).

**Why this priority**: Keys and behaviour change rarely, and the CLI shows them fully. The page
completes the picture but adds the least.

**Independent Test**: With two keys, one revoked and one with its own break behaviour, compare
the page with `keys list --json` and the CLI's new behaviour view (FR-016a), for the same
moment. They agree, and no full key appears anywhere on the page.

**Acceptance Scenarios**:

1. **Given** a revoked key, **When** the page loads, **Then** it is shown as revoked, with the
   time.
2. **Given** a key with no break behaviour of its own, **When** the page loads, **Then** it shows
   "default", and the operator default is shown once on the same page.
3. **Given** any key, **When** the page loads, **Then** only the last four characters appear, as
   in the CLI. The full key does not exist on the server and cannot be shown.

---

### User Story 6 — The dashboard looks like 9router (Priority: P2)

Every page is styled from the style guide. An operator coming from 9router recognises its colors,
type, cards, badges, buttons and tables, even though the pages are arranged differently.

**Why this priority**: The user named "doesn't look like 9router" as one of the two ways this
slice fails. It is P2 because the pages must exist before they can be styled.

**Independent Test**: Check that every color, font, size, spacing, radius and shadow the
dashboard uses is defined in the style guide, and that each component on the pages (card, badge,
button, table, input, navigation) matches the style guide's description of 9router's version.
Compare screenshots of each component with 9router's.

**Acceptance Scenarios**:

1. **Given** the style guide, **When** it is read, **Then** each value in it names the 9router
   file it came from.
2. **Given** the dashboard's styles, **When** they are checked, **Then** no color, size, spacing,
   radius or shadow appears that is not in the style guide.
3. **Given** a status shown on any page (signed in, needs sign-in, polled, estimated,
   pay-as-you-go, revoked, failed), **When** it is shown, **Then** it uses the style guide's badge
   style and the status color 9router uses for the same kind of status.
4. **Given** a page is open, **When** the browser has no internet connection, **Then** it looks
   the same: nothing is fetched from outside this machine.

---

### Edge Cases

- **The dashboard port is in use.** The server still starts and serves clients. It reports that
  the dashboard is unavailable in its log and in `check`.
- **A page is very slow to build** (for example a huge record history). Client requests are not
  slowed. The page shows at most a bounded number of records at a time and lets the operator page
  further back.
- **The dashboard fails while building a page.** That page shows an error. Client requests,
  other pages, and the rest of the server are unaffected.
- **No server is running.** There is no dashboard; it exists only while `nullrouter serve` runs.
- **Server state changes while a page is open.** The page keeps showing the state at its "as of"
  time until the operator reloads.
- **Configuration is reloaded while a page is being built.** The page shows one consistent state:
  all of it from before the reload or all of it from after.
- **An empty home** (no accounts, keys, unified models or records). Each page says what is
  missing and names the CLI command that adds it.
- **A very long name** (account, unified model, agent key). It is shown in full or truncated with
  the full name available on the page, never cut so that two names look the same.
- **A record of a request that was cut short** (server killed mid-request). It is shown as
  `records` shows it ("cut short"), not as in flight.
- **Times.** Every time is shown in this machine's local time zone, and each page names that
  zone. The CLI's UTC times and the page's local times are the same instant.

## Requirements *(mandatory)*

### Functional Requirements

**Serving and access**

- **FR-001**: The running server MUST serve the dashboard on its own port, separate from the port
  clients use.
- **FR-002**: The dashboard port MUST accept connections only from this machine.
- **FR-003**: The dashboard MUST be on by default whenever `nullrouter serve` runs. The operator
  MUST be able to choose the dashboard port, and to turn the dashboard off.
- **FR-003a**: The dashboard MUST be written in Rust and server-rendered: the server builds each
  page, and no application code runs in the browser (brief rows 3 and 4).
- **FR-004**: The CLI MUST issue a dashboard token. It is printed once. Issuing a new one replaces
  the old one, and every browser signed in with the old one must sign in again.
- **FR-005**: The dashboard token MUST be stored the way the operator's other secrets are, and
  MUST NOT be readable from the home directory, logs, records, CLI output (beyond the one time it
  is issued), or any page.
- **FR-006**: No dashboard page and no dashboard data MUST be served without a valid token. The
  only things served without one are the sign-in page and the static style and font files, which
  carry no 0router data.
- **FR-007**: A browser that entered the token once MUST stay signed in until the token is
  replaced or the operator clears the browser's data.
- **FR-008**: Repeated wrong tokens MUST be slowed down.
- **FR-009**: A page or data from the dashboard MUST NOT be readable or usable by another website
  open in the same browser, including by framing it.
- **FR-010**: The dashboard MUST NOT offer any action that changes 0router's state: accounts,
  keys, priorities, routing settings, quota polling, behaviour, plugins, records. Narrowing,
  paging, and opening a detail are views, not changes.
- **FR-011**: The dashboard MUST NOT fetch anything from outside this machine (fonts, icons,
  scripts or styles).

**Isolation from client traffic**

- **FR-012**: A fault in the dashboard (an error, a slow page, a crash in its own work, a port it
  can't open) MUST NOT fail, slow, or delay any client request.
- **FR-013**: The work one page load does MUST be bounded, so that no page load can starve client
  requests of time or memory.
- **FR-014**: Plugins MUST NOT receive anything they did not receive before this slice.

**Agreement with the CLI**

- **FR-015**: Every value a page shows MUST equal what the CLI shows for the same moment, in the
  same words where the CLI uses words (status names, placement reasons, quota sources).
- **FR-016**: Every fact a page shows MUST have a CLI command that shows the same fact.
- **FR-016a**: The CLI MUST gain two read-only views so that FR-016 holds: a list of every
  unified model with its kind, its members in order with their upstream models, and its limits
  notes; and a view of the operator's behaviour settings. Both offer the CLI's usual
  machine-readable output. Neither changes anything.
- **FR-017**: Each page MUST show the time its state was read (its "as of" time). A page MUST NOT
  change after it loads. The operator reloads to see newer state.
- **FR-017a**: Every time on a page MUST be shown in this machine's local time zone, and each
  page MUST name that zone. A page's time and the CLI's UTC time for the same fact MUST be the
  same instant.
- **FR-018**: Each page MUST show one consistent state, never a mix of before and after a reload
  or a change.

**Pages**

- **FR-019**: The dashboard MUST have four pages: accounts and quota; routing and requests;
  models and providers; keys and behaviour. Every page MUST be reachable from every other.
- **FR-019a**: Each page MUST show everything its matching CLI read commands show (the listed
  facts in FR-020 to FR-024 are a minimum):
  - accounts and quota: `accounts list --long`, `quota`;
  - routing and requests: `routing`, `records list`, `records show`;
  - models and providers: `providers`, `model <provider> <model>` for each model of an active
    provider, `plugins list --community`, the unified-model list (FR-016a);
  - keys and behaviour: `keys list`, the behaviour view (FR-016a).
  `quota history` is the exception: it is not on any page.
- **FR-019b**: Every warning, note and error `check` prints MUST appear on the page whose
  subject it concerns, in `check`'s words: plugin conflicts, withheld credentials, skipped
  plugins, dropped unified models and limits notes on models and providers; unmetered quota
  windows, pay-as-you-go accounts with no price, sign-in tokens without an account and sign-in
  accounts without tokens on accounts and quota; records not being kept on routing and requests;
  routing warnings on routing and requests; each file-mode warning on the page of the file's
  subject.
- **FR-020**: The accounts and quota page MUST show, per account: provider, name, enabled or
  disabled, kind (sign-in or key), order, sign-in status and since when, any cooldown per model
  with the time left, whether it needs signing in again, sign-in email and tier where
  `accounts list --long` shows them, priority, and per quota window: quota left, source (polled,
  estimated, pay-as-you-go, pending first poll, stale), last poll time, and reset time.
- **FR-021**: The routing and requests page MUST show the routing view (per target and account:
  pace, share, deficit, quota source) and the request records, newest first, a bounded number at
  a time, with the filters `records list` offers.
- **FR-022**: A request record MUST show what `records show` shows: time, agent, target, resolved
  unified model, each attempt with its account, outcome and reason, the placement reason, latency
  (time to first token and total), and usage.
- **FR-023**: The models and providers page MUST show the active providers and their model types,
  plugins and their state, community plugins and their fit, unified models with kind and ordered
  members, dropped unified models with their reason, and limits notes.
- **FR-024**: The keys and behaviour page MUST show the agent keys as `keys list` shows them and
  the operator's behaviour settings.
- **FR-025**: Model tests and Combos MUST appear as entries that say the feature isn't built yet
  and name the CLI commands that cover the nearest thing today, or say there is none.
- **FR-026**: Latency MUST appear as the records carry it, per request and per attempt. This slice
  adds no latency summaries or trends.
- **FR-027**: Where a page has nothing to show, it MUST say what is missing and name the CLI
  command that adds it.

**Privacy**

- **FR-028**: No page MUST show a secret: provider API keys, sign-in tokens and agent keys beyond
  the last four characters the CLI shows, OAuth client secrets, or the dashboard token.
- **FR-029**: No page MUST show prompt or response content. Records hold none, and the dashboard
  MUST NOT add any.

**Look**

- **FR-030**: The repo MUST contain a style guide taken from 9router's code. It records 9router's
  light color palette (brand, surface, border, text and status colors), type (fonts, sizes, weights),
  spacing, radii, shadows, and the styles of buttons, cards, badges, tables, inputs and
  navigation. Each value names the 9router file it came from.
- **FR-031**: The dashboard's styling MUST be built only from the style guide's values.
- **FR-032**: Status MUST be shown with the style guide's badge style and 9router's color for the
  same kind of status.
- **FR-033**: The page layout MUST be 0router's own: it is arranged around 0router's four pages,
  not 9router's page structure.
- **FR-034**: The dashboard MUST have one theme: 9router's light theme. The style guide records
  only the light palette; there is no dark theme and no theme switch.
- **FR-035**: All dashboard text MUST be English.

### Key Entities

- **Dashboard token**: The secret that opens the dashboard. One at a time; issued by the CLI;
  replacing it signs out every browser.
- **Dashboard session**: A browser's proof that it entered the current token. Valid until the
  token is replaced.
- **Page snapshot**: The server state a page was built from, with its "as of" time. It is the same
  state the CLI would read at that moment.
- **Style guide**: The document of 9router's look: tokens (colors, type, spacing, radii, shadows)
  and component styles, each traced to a 9router source file.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: For each of the four pages, a test that builds the page and runs the matching CLI
  commands for the same moment finds 0 disagreements across a test home covering every status in
  User Stories 1–5.
- **SC-002**: 100% of requests for a page or page data without a valid token get only the sign-in
  page; 0 connections from another machine succeed.
- **SC-003**: While pages are loaded continuously, and while the dashboard is made to fail,
  client requests show 0 additional failures and no more than 1 ms added latency at the 95th
  percentile, compared with the dashboard turned off.
- **SC-004**: A scan of every page, every dashboard response, the logs, and the home directory
  from the full test suite finds 0 secrets and 0 prompt text.
- **SC-005**: 100% of color, size, spacing, radius and shadow values in the dashboard's styling
  are defined in the style guide, and 100% of style guide values trace to a 9router source file.
- **SC-006**: Side by side with 9router in its light theme, each shared component (card, badge, button, table, input,
  navigation) is judged the same in color, type, spacing and shape by the user.
- **SC-007**: Each page loads in under 1 second on a home with 50 accounts, 20 unified models and
  100,000 records.
- **SC-008**: With the internet disconnected, every page renders exactly as it does online.
- **SC-009**: The operator can tell which accounts need signing in again, and how much quota each
  has left, from the first page alone, without running a command.

## Assumptions

Items marked *(technical decision)* are Claude's. They may be revised in planning without asking
the user, as long as nothing the user sees changes.

- The token is entered once per browser and kept by that browser until the token is replaced.
  There is no time-based expiry, because the dashboard is local-only and read-only.
  *(technical decision)*
- One dashboard token at a time; there are no per-user tokens. The operator is the only user.
- The page reads the same server state the CLI reads through the operator socket and the files
  in the home, so "agrees with the CLI" can be tested against one snapshot. *(technical
  decision)*
- SC-003's 1 ms bound and SC-007's 1 second page load are targets Claude set at clarify.
  *(technical decision)*
- The records page shows 50 records at a time, newest first, with paging further back. Its CLI
  equivalent is `records list --limit 50 [--before <id>]`; `records list` itself has no default
  limit. *(technical decision)*
- The last four characters of an agent key, a provider key or a sign-in token are shown, as the
  CLI shows them (an account added from an environment variable shows the variable's name). They
  identify a secret; they are not enough to use it.
- Sign-in email and tier are shown, as `accounts list --long` shows them. They are not secrets.
- Quota history (`quota history`) is not on the accounts page. It was not in the confirmed scope.
- 9router loads its font (Inter) and icon font from Google. The dashboard serves both from the
  binary instead (both licences allow it) or falls back to the style guide's system font stack.
  *(technical decision)*
- The look is 9router's and the layout is new (brief row 2, "so many changes in layout").
  init.md said the dashboard keeps 9router's layout; it was corrected to match on 2026-10-05.
- "Point to the CLI" in the Model tests and Combos entries means naming the commands that cover
  the nearest thing today. Neither feature has a CLI command yet.
- Client traffic and the dashboard share one process. Isolation means the dashboard's work never
  holds anything a client request waits for, and its work per page is bounded. *(technical
  decision)*

## Out of Scope

- A dark theme and a theme switch: light only for now (FR-034); a dark theme may come later.
- Latency summaries and trends over time: the next slice (a latency view in the CLI and the
  dashboard together).
- Acting from the dashboard (changing accounts, keys, priorities, routing settings or behaviour):
  later.
- Reaching the dashboard from another machine, HTTPS, and binding to a network address: later.
- Translations and language switching: later.
- Live auto-refresh: later. The operator reloads.
- Model tests and combos themselves: later, with their own slices.
- Fitted quota weights and leak detection: later.
- Quota poll history on the dashboard: not in this slice.

# Feature Specification: Dashboard

**Feature Branch**: `009-dashboard`

**Created**: 2026-10-05

**Status**: Draft

**Scope brief**: [specs/briefs/2026-10-05-dashboard.md](../briefs/2026-10-05-dashboard.md) (slice 1
of two). In clarify, plan and analyze, an answer that contradicts a confirmed row of that brief's
ledger means stop and revisit the brief.

**Replaces** spec 007 (`specs/007-dashboard`, four-page layout), which is retired. Its open task
T011 (a `subject` on each `check` item) is carried into FR-024. Its plan's dashboard token and
cookie decisions (research R6 to R8, open Low L1) carry over to this slice's plan.

**Input**: User description: "Dashboard: a read-only web view of 0router in a browser, so the operator can see their endpoint, keys, providers, quota, routing and requests on pages that look like the dashboard mockups. It is written in Rust, server-rendered with no scripts, and served by the running `nullrouter serve` process on its own port, bound to this machine only. It is on by default. Until the operator issues a dashboard token with `nullrouter dashboard token`, its only page names that command; the operator can turn the dashboard off, and `nullrouter dashboard status` shows its state. The browser asks for the token once; no other page opens without it. The dashboard is look only: every change (accounts, keys, priorities, behaviour, plugins) stays in the CLI. Its look is 9router's: a style guide extracted from 9router's code (colors, type, spacing, component styles) is written into this repo and the styling is built from it, with one light theme, English only, the constant background grid, icons and bright colors. Its sidebar and page names are 9router's (Endpoint & Key, Providers, Combo, Usage, Quota Tracker, and under System: Proxy Pools, Console Log, Settings) under the title \"0Router Proxy\"; this replaces spec 007's four-page layout. Every fact a page shows has a CLI command that shows the same fact, a page shows everything its matching CLI reads show, and every `check` warning, note and error appears on the page it concerns. Each page shows an \"as of\" time in this machine's time zone, named, and the operator reloads to refresh. Endpoint & Key shows the endpoint URL and the agent keys as cards (as `keys list` shows them), and a Client adapters side panel that says client-side adapters aren't built yet. Providers shows provider cards, a detail window per provider with its accounts and all its models under a filter that lists every model kind the plugins declare, and a Provider plugins side panel; the buttons \"Add … Compatible\" and \"Test All\" and the plugin enable, disable and hide switches are disabled and say they aren't built yet or name the CLI command. A plugin may ship a PNG logo, at most 64 KiB and 256 px; the core checks it, and a bad logo is ignored so the plugin still loads with its text icon, and `check` says why. Quota Tracker shows every account with its sign-in status, quota with reset times (polled or estimated), priority, and its pace, share and deficit. Usage shows the Requests table, newest first, 50 at a time, with the reason for each placement, latency and result; a row opens a window with that request's attempts, stay-warm decision and changes. The traffic landscape, the usage stat cards and topology graph, and the period filter appear as slots that say they arrive with the next dashboard slice. Settings shows the operator's behaviour settings, where the data lives, and the dashboard's own status. A round housekeeping button opens a panel that lists the current notices from `check` and the accounts; its chat box is disabled and says it isn't built yet. Combo, Console Log and Proxy Pools are entries that say the feature isn't built yet and name what exists instead. This slice adds `nullrouter dashboard token`, `nullrouter dashboard status` and a subject on each `check` item so notices reach their page. A page never shows a secret or a prompt, plugins see nothing they didn't before, and a fault in the dashboard never slows or breaks client requests. The slice fails if: a page disagrees with the CLI for the same moment; the look visibly departs from the style guide taken from 9router. Visual reference only (where it disagrees with this text, this text wins): docs/dashboard/mockups. Out of scope: changing anything from the dashboard, including plugin switches, adapter review and the housekeeping chat → later; the traffic landscape and gauges, usage totals per period, Est. Cost, the topology graph, the period filter, and the harness tag on keys → the next dashboard slice (summaries and landscape); latency trends over time → the latency slice; extra Usage charts and breakdown tables → later; live auto-refresh → later; side panels on Usage, Quota Tracker and Settings → later; reaching the dashboard from another machine, HTTPS and network binding → later; translations → later; client-side adapters → slice 004; model tests and combos themselves → later, with their own slices; fitted quota weights and leak detection → later; 9router's Token Saver, CLI Tools, Translator, Skills, Media pages and Basic Chat → not planned. Scope brief: specs/briefs/2026-10-05-dashboard.md"

## User Scenarios & Testing *(mandatory)*

This slice has two users:

- The **operator** runs 0router on their own machine. Today they read its state from the CLI
  (`accounts list`, `quota`, `routing`, `records`, `providers`, `plugins`, `unified`, `keys list`,
  `behaviour show`, `check`). After this slice they can also read it in a browser on the same
  machine.
- A **plugin author** can ship a logo with a provider plugin.

Clients (agents) see no change.

Terms used throughout:

- The **dashboard** is the set of web pages the running server shows on its own local port.
- The **dashboard token** is the secret the operator gets from the CLI and enters once in the
  browser. It is not an agent access key and cannot be used to send requests to models.
- The **style guide** is a document in this repo that records 9router's look: its colors, type,
  spacing, radii, shadows, the background grid, its icon set, and the styles of its sidebar,
  buttons, cards, badges, tables, inputs, windows and panels. It is taken from 9router's code, not
  drawn from screenshots.
- The **mockups** are the pages in `docs/dashboard/mockups`. They are a visual reference: where
  they disagree with this spec, this spec wins. Their sample content (names, counts, the
  "hermes · always on" adapter, third-party adapter rows, the "Pruned 214 request records" notice,
  the "Decisions" kind with no data, gauge numbers) is not shipped.
- A **twin** of a fact is the CLI command that shows the same fact.
- **For the same moment** means: the page and the CLI read the same server state, with no change
  in between.
- A **notice** is a warning, note or error that `check` prints, or an account condition the
  operator must act on (needs signing in again, cooling down), in the words the CLI uses.
- A **slot** is a marked place on a page where a later slice's content will go. It says what will
  appear there and that it arrives with the next dashboard slice.

### User Story 1 — Open the dashboard once, and only on this machine (Priority: P1)

The operator starts `nullrouter serve` and opens the dashboard address in a browser on the same
machine. Until a token exists, the only page says to run `nullrouter dashboard token`. The
operator runs it, enters the printed token once, and from then on that browser opens every page
without asking, until the operator issues a new token. Nobody else can open a page: not another
machine, not a browser without the token, and not another website open in the operator's own
browser. `nullrouter dashboard status` tells the operator whether the dashboard is on, where it
listens, and whether a token has been issued.

**Why this priority**: Every other page depends on it, and the pages show account names, sign-in
emails, quota and request history. Without the token gate, any local process or website could
read them.

**Independent Test**: Start the server with a fresh home, check that every address gives only the
"run `nullrouter dashboard token`" page; issue a token; check that a request without it gets only
the sign-in page, that the right token opens the pages and they stay open after a browser
restart, that a wrong token is refused, that issuing a new token signs the old browser out, that
turning the dashboard off closes its port, and that the port cannot be reached from another
machine. `dashboard status` matches each state.

**Acceptance Scenarios**:

1. **Given** no dashboard token has been issued, **When** the operator opens any dashboard
   address, **Then** they see one page that names `nullrouter dashboard token`, and nothing else.
2. **Given** a token was issued, **When** the operator enters it, **Then** every page opens, and
   the browser does not ask again until the token is replaced or the operator clears the
   browser's data.
3. **Given** a wrong token, **When** it is entered, **Then** it is refused, no page opens, and
   repeated wrong tries are slowed down.
4. **Given** a browser signed in with token A, **When** the operator issues token B, **Then** the
   next page load in that browser asks for the token again.
5. **Given** the operator turned the dashboard off, **When** `serve` runs, **Then** the dashboard
   port is not open, client traffic is served as before, and `dashboard status` says it is off.
6. **Given** another machine on the same network, **When** it tries the dashboard port, **Then**
   it cannot connect.
7. **Given** another website open in the operator's browser, **When** it tries to load or embed a
   dashboard page, **Then** it can't read the page or see its content.
8. **Given** the token was printed once by the CLI, **When** the operator looks for it later in
   the home directory, logs, records, `dashboard status` or any page, **Then** it is not there in
   readable form.

---

### User Story 2 — See every account's sign-in, quota and routing state (Priority: P1)

The operator opens Quota Tracker and sees one card per provider account: provider, name, priority,
sign-in status, and for each quota window the quota left, whether that figure is polled or
estimated (or the account is pay-as-you-go), and when the window resets. Each card also shows the
account's routing state: its pace, share and deficit for each target it serves. An account that
needs signing in again says so and names the CLI command that signs it in.

**Why this priority**: The question the operator asks most is "is everything signed in, how much
quota is left, and where is traffic going". It is the page that makes the dashboard worth
opening.

**Independent Test**: With a test home holding a signed-in polled account, an account whose
sign-in has expired, an estimated account, a pay-as-you-go account, an account in a cooldown and
a target with two accounts, open Quota Tracker and compare every value with
`accounts list --long --json`, `quota --json` and `routing --json` for the same moment. They
agree.

**Acceptance Scenarios**:

1. **Given** an account whose sign-in can no longer be refreshed, **When** the page loads,
   **Then** that card is marked as needing sign-in again and names the CLI command that signs it
   in.
2. **Given** an account whose quota is polled, **When** the page loads, **Then** each window shows
   the quota left, "polled", the time of the last poll and the reset time, as `quota` shows them.
3. **Given** an account whose provider reports no quota but whose plugin declares limits, **When**
   the page loads, **Then** its windows are shown as estimated.
4. **Given** an account with neither, **When** the page loads, **Then** it is shown as
   pay-as-you-go, with no quota bar.
5. **Given** an account still waiting for its first poll, or whose last poll failed, **When** the
   page loads, **Then** it says so in the words `quota` uses ("pending first poll", "stale").
6. **Given** a target served by several accounts, **When** the page loads, **Then** each account's
   pace, share, deficit and priority for that target match `routing` for the same moment, and the
   amortization window `routing` shows is on the page.
7. **Given** the operator narrows the page to one provider or one account, **When** the page
   reloads, **Then** it shows exactly the accounts `quota <provider> [<name>]` shows.

---

### User Story 3 — See where each request went and why (Priority: P2)

The operator opens Usage and sees the Requests table: the newest 50 request records, each with its
time, agent, model, the account it was placed on, the reason for the placement, time to first
token, total time and result. They can page further back 50 at a time. Clicking a row opens a
window with everything `records show` shows for that request: each attempt with its account,
outcome and reason, the stay-warm decision, and the changes made to the request. The places for
the stat cards, the topology graph and the period filter are drawn as slots.

**Why this priority**: Slice 006's promise is that the operator can always tell why a request went
where it did. The CLI already keeps that promise; this page makes it quicker to scan.

**Independent Test**: Replay a set of requests in a test home (warm, cold, overflow, a failed
attempt with fallback, a request with recorded changes). Compare the table with
`records list --limit 50 --json` and the next page with `records list --limit 50 --before <id>
--json`, and one row's window with `records show <id> --json`, all for the same moment. They
agree.

**Acceptance Scenarios**:

1. **Given** records exist, **When** the page loads, **Then** it lists the newest 50 first, and
   each row shows what `records list` shows for that record, including the placement reason,
   latency and result.
2. **Given** more than 50 records, **When** the operator asks for the next page, **Then** it lists
   exactly what `records list --limit 50 --before <id>` lists.
3. **Given** a record with a failed attempt followed by a fallback, **When** the operator opens it,
   **Then** every attempt is shown with its outcome, reason and latency, as `records show` shows.
4. **Given** a record with a stay-warm decision and recorded changes, **When** the operator opens
   it, **Then** both are shown as `records show` shows them.
5. **Given** a record whose usage the provider did not report, a request still in flight, or one
   cut short, **When** it is shown, **Then** the page says so in the words `records` uses, rather
   than showing zero.
6. **Given** a period while records were not kept, **When** the page loads, **Then** it shows the
   warning `records list` and `check` show.
7. **Given** the page loads, **When** the operator looks at the stat cards, topology graph and
   period filter, **Then** each is a slot that says it arrives with the next dashboard slice, and
   shows no number.

---

### User Story 4 — See providers, their accounts, models and plugins (Priority: P2)

The operator opens Providers and sees a card per provider, grouped as the mockup groups them, each
with its logo (or text icon) and how many accounts it has and their state. Clicking a card opens
a window with that provider's accounts and all its models, under a filter that lists every model
kind the plugins declare. A side panel lists the provider plugins: bundled, installed from the
community, and how many community plugins are available, each with its state. Buttons and switches
that would change something are drawn disabled and say what to do instead.

**Why this priority**: These facts change only when the operator edits configuration, so they are
looked at less often than quota or records. They are still needed to read the other pages.

**Independent Test**: With a test home that has providers with and without accounts, a plugin with
a valid logo, a plugin with an oversized logo, a pending plugin conflict and a skipped plugin,
compare the page with `providers --json`, `model <provider> <model> --json` for each model,
`accounts list --long --json`, `plugins list --community --json` and `check --json` for the same
moment. They agree, the valid logo is shown, and the oversized one is replaced by the text icon
with `check`'s note on the page.

**Acceptance Scenarios**:

1. **Given** a provider with two accounts, **When** the operator opens its window, **Then** both
   accounts are listed with what `accounts list --long` shows for them, and every model the
   provider declares is listed with what `model` shows.
2. **Given** the plugins declare text, embedding and speech models, **When** the window's kind
   filter is opened, **Then** it lists exactly the kinds the plugins declare, each with its count,
   and choosing one lists exactly the provider's models of that kind.
3. **Given** a skipped plugin or a pending conflict, **When** the page loads, **Then** it appears
   in the side panel with the state and reason `plugins list` and `check` give.
4. **Given** the "Add … Compatible" or "Test All" buttons, or a plugin's enable, disable or hide
   switch, **When** the operator clicks it, **Then** nothing changes, and it says it isn't built
   yet or names the CLI command that does it.

---

### User Story 5 — See the endpoint, agent keys and settings (Priority: P3)

The operator opens Endpoint & Key and sees the URL agents should point at and one card per agent
key, as `keys list` shows them. A Client adapters side panel says client-side adapters aren't
built yet. The place for the traffic landscape is a slot. Settings shows the operator's behaviour
settings, where the data lives, and the dashboard's own status.

**Why this priority**: These facts change rarely and the CLI shows them fully. The pages complete
the picture but add the least.

**Independent Test**: With two keys, one revoked and one with its own break behaviour, compare
Endpoint & Key with `keys list --json` and the CLI read that shows the endpoint URL, and Settings
with `behaviour show --json`, `check --json` (home) and `dashboard status --json`, for the same
moment. They agree, and no full key or token appears anywhere on either page.

**Acceptance Scenarios**:

1. **Given** a revoked key, **When** the page loads, **Then** its card shows it as revoked, with
   the time.
2. **Given** a key with no break behaviour of its own, **When** the page loads, **Then** its card
   shows "default", and the operator default is on Settings.
3. **Given** any key, **When** the page loads, **Then** only the last four characters appear, as in
   the CLI.
4. **Given** the "Add Agent" control the mockup draws, **When** the operator clicks it, **Then** it
   names `nullrouter keys issue <name>` and changes nothing.
5. **Given** Settings loads, **When** the operator reads the dashboard section, **Then** it shows
   what `dashboard status` shows: on, its address, and when the token was issued.

---

### User Story 6 — See every notice in one place, and on its own page (Priority: P2)

On every page, a round housekeeping button opens a panel that lists the current notices: every
warning, note and error `check` prints and every account that needs action. Each notice also
appears on the page it concerns. The panel's chat box is drawn disabled and says it isn't built
yet. Combo, Console Log and Proxy Pools are entries in the sidebar; each says the feature isn't
built yet and names what exists instead.

**Why this priority**: Notices are how the operator learns something is wrong without running
`check`. They depend on the pages existing.

**Independent Test**: With a test home that triggers at least one notice of every kind `check`
reports, open the panel and compare it with `check --json` and `accounts list --json` for the same
moment: every notice is listed, in `check`'s words. Then open each page and check that each notice
appears on the page its subject names.

**Acceptance Scenarios**:

1. **Given** `check` prints a warning, **When** the operator opens the housekeeping panel on any
   page, **Then** the warning is listed in `check`'s words.
2. **Given** a notice whose subject is a page, **When** the operator opens that page, **Then** the
   notice is shown there.
3. **Given** no notices, **When** the operator opens the panel, **Then** it says there are none.
4. **Given** the panel's chat box, **When** the operator tries to type, **Then** it is disabled and
   says it isn't built yet.
5. **Given** the Combo entry, **When** the operator opens it, **Then** it says combos aren't built
   yet and names `[[unified_model]]` in `config.toml` and `nullrouter unified` as what exists
   instead. Console Log names the Usage page; Proxy Pools says there is no proxy pool feature.

---

### User Story 7 — The dashboard looks like 9router (Priority: P2)

Every page is styled from the style guide. An operator coming from 9router recognises its sidebar,
colors, type, background grid, icons, cards, badges, buttons, windows and tables, and the page
names in the sidebar are 9router's.

**Why this priority**: The user named "visibly departs from the style guide taken from 9router" as
one of the two ways this slice fails. It is P2 because the pages must exist before they can be
styled.

**Independent Test**: Check that every color, font, size, spacing, radius and shadow the dashboard
uses is defined in the style guide, and that each component on the pages matches the style guide's
description of 9router's version. Compare screenshots of each component with 9router's and with
the mockups.

**Acceptance Scenarios**:

1. **Given** the style guide, **When** it is read, **Then** each value in it names the 9router file
   it came from.
2. **Given** the dashboard's styles, **When** they are checked, **Then** no color, size, spacing,
   radius or shadow appears that is not in the style guide.
3. **Given** a status shown on any page (signed in, needs sign-in, cooling down, polled,
   estimated, pay-as-you-go, revoked, served, failed, fallback), **When** it is shown, **Then** it
   uses the style guide's badge style and the status color 9router uses for the same kind of
   status.
4. **Given** a page is open, **When** the browser has no internet connection, **Then** it looks the
   same: nothing is fetched from outside this machine.

---

### User Story 8 — A plugin ships its logo (Priority: P3)

A plugin author adds a PNG logo to a provider plugin. The core checks it when it loads plugins. A
good logo is shown on the provider's card and window; a bad one is ignored, the plugin loads with
its text icon, and `check` says why.

**Why this priority**: Logos make the Providers page readable at a glance, but every page works
without them.

**Independent Test**: Load plugins whose logos are a valid PNG, a PNG over 64 KiB, a PNG over 256
px on a side, a file that isn't a PNG, and a missing file. Only the first is shown; each other
plugin loads and its text icon is shown, and `check` names the plugin and the reason.

**Acceptance Scenarios**:

1. **Given** a plugin with a valid PNG logo of at most 64 KiB and 256 × 256 px, **When** Providers
   loads, **Then** the logo is shown on the card and in the window.
2. **Given** a plugin whose logo breaks a limit or is not a PNG, **When** plugins load, **Then**
   the plugin loads and serves requests as before, the page shows its text icon, and `check` names
   the plugin and the limit it broke.
3. **Given** any logo, **When** it is served, **Then** the browser treats it only as an image.

---

### Edge Cases

- **The dashboard port is in use.** The server still starts and serves clients. It reports that
  the dashboard is unavailable in its log and in `dashboard status`.
- **A page is slow to build** (for example a huge record history). Client requests are not
  slowed. The Requests table reads 50 records at a time.
- **The dashboard fails while building a page.** That page shows an error. Client requests, other
  pages, and the rest of the server are unaffected.
- **No server is running.** There is no dashboard; it exists only while `nullrouter serve` runs.
  `dashboard status` says no server is running and still shows the configured state and whether a
  token has been issued.
- **Server state changes while a page is open.** The page keeps showing the state at its "as of"
  time until the operator reloads.
- **Configuration is reloaded while a page is being built.** The page shows one consistent state:
  all of it from before the reload or all of it from after.
- **An empty home** (no accounts, keys, plugins installed from the community, or records). Each
  page says what is missing and names the CLI command that adds it.
- **A very long name** (account, model, agent key). It is shown in full or truncated with the full
  name available on the page, never cut so that two names look the same.
- **A model kind no plugin declares** (for example "decision" today). It is not in the kind
  filter; it appears once a plugin declares it.
- **A fact the mockup draws that no CLI read shows** (for example "last used" or "requests today"
  on a key card, "last response resolved" on a provider). It is not shown. Facts that a later
  slice adds a CLI read for (the latency and totals of slice 2) are shown in slots instead.
- **Times.** Every time is shown in this machine's local time zone, and each page names that zone.
  The CLI's UTC times and the page's local times are the same instant.

## Requirements *(mandatory)*

### Functional Requirements

**Serving and access**

- **FR-001**: The running server MUST serve the dashboard on its own port, separate from the port
  clients use. The operator MUST be able to choose that port.
- **FR-002**: The dashboard port MUST accept connections only from this machine.
- **FR-003**: The dashboard MUST be on by default whenever `nullrouter serve` runs. The operator
  MUST be able to turn it off; then its port is not opened.
- **FR-004**: The dashboard MUST be written in Rust and server-rendered: the server builds each
  page, and no script runs in the browser.
- **FR-005**: `nullrouter dashboard token` MUST issue a dashboard token and print it once. Issuing
  a new one replaces the old one, and every browser signed in with the old one must sign in again.
- **FR-006**: The dashboard token MUST be stored the way the operator's other secrets are, and
  MUST NOT be readable from the home directory, logs, records, CLI output (beyond the one time it
  is issued), or any page.
- **FR-007**: Until a token is issued, every dashboard address MUST show only a page that names
  `nullrouter dashboard token`.
- **FR-008**: Once a token is issued, no dashboard page and no dashboard data MUST be served
  without it. The only things served without it are the sign-in page and the static style, font,
  and icon files, which carry no 0router data.
- **FR-009**: A browser that entered the token once MUST stay signed in until the token is
  replaced or the operator clears the browser's data.
- **FR-010**: Repeated wrong tokens MUST be slowed down.
- **FR-011**: A page or data from the dashboard MUST NOT be readable or usable by another website
  open in the same browser, including by framing it.
- **FR-012**: `nullrouter dashboard status` MUST show whether the dashboard is on or off, the
  address it listens on, whether the running server is serving it (or why not: no server, port
  unavailable), and whether a token has been issued and when. It MUST NOT show the token. It
  offers the CLI's usual machine-readable output.
- **FR-013**: The dashboard MUST NOT offer any action that changes 0router's state: accounts,
  keys, priorities, routing settings, quota polling, behaviour, plugins, records. Narrowing,
  paging, opening a window and signing in with the token are views, not changes.
- **FR-014**: Where the mockup draws a control that would change state ("Add … Compatible", "Test
  All", plugin enable, disable and hide switches, plugin install and uninstall, "Add Agent", the
  housekeeping chat box), the page MUST show it disabled, with a hint that names the CLI command
  that makes the change, or says it isn't built yet when there is none.
- **FR-015**: The dashboard MUST NOT fetch anything from outside this machine (fonts, icons,
  scripts or styles).

**Isolation from client traffic**

- **FR-016**: A fault in the dashboard (an error, a slow page, a crash in its own work, a port it
  can't open) MUST NOT fail, slow, or delay any client request.
- **FR-017**: The work one page load does MUST be bounded, so that no page load can starve client
  requests of time or memory.
- **FR-018**: Plugins MUST NOT receive anything they did not receive before this slice. A plugin's
  logo is read and checked by the core; the plugin itself does nothing.

**Agreement with the CLI**

- **FR-019**: Every value a page shows MUST equal what the CLI shows for the same moment, in the
  same words where the CLI uses words (status names, placement reasons, quota sources, `check`
  messages).
- **FR-020**: Every fact a page shows MUST have a twin. A fact with no twin MUST NOT be shown,
  even where the mockup draws it. The endpoint URL MUST be shown by a CLI read; if none shows it
  today, this slice adds it to an existing read.
- **FR-021**: Each page MUST show everything its matching CLI reads show:
  - Endpoint & Key: `keys list`, and the read that shows the endpoint URL (FR-020);
  - Providers: `providers`, `model <provider> <model>` for each model of an active provider,
    `accounts list --long` (in each provider's window), `plugins list --community`;
  - Quota Tracker: `accounts list --long`, `quota`, `routing`;
  - Usage: `records list --limit 50 [--before <id>]`, and `records show` in a row's window;
  - Settings: `behaviour show`, the home `check` reports, `dashboard status`;
  - Combo: the notices `check` reports about unified models (FR-024).
  `quota history` is the exception: it is not on any page.
- **FR-022**: Each page MUST show the time its state was read (its "as of" time). A page MUST NOT
  change after it loads. The operator reloads to see newer state.
- **FR-023**: Every time on a page MUST be shown in this machine's local time zone, and each page
  MUST name that zone. A page's time and the CLI's UTC time for the same fact MUST be the same
  instant.
- **FR-024**: Every warning, note and error `check` reports MUST carry a subject that names the
  page it concerns, shown in `check`'s machine-readable output. Each MUST appear on that page in
  `check`'s words:
  - Providers: plugin conflicts (pending, declined), withheld credentials, skipped plugins, logo
    problems (FR-041);
  - Combo: dropped unified models and limits notes;
  - Quota Tracker: unmetered quota windows, sign-in errors, sign-in tokens without an account,
    sign-in accounts without tokens, routing warnings;
  - Usage: records not being kept;
  - each file-mode warning on the page of its file's subject (accounts and sign-in files on Quota
    Tracker, the keys file on Endpoint & Key, the configuration and dashboard files on Settings).
  A notice whose subject is none of these MUST appear on Settings.
- **FR-025**: Each page MUST show one consistent state, never a mix of before and after a reload
  or a change.

**Layout and navigation**

- **FR-026**: Every page MUST have the sidebar, titled "0Router Proxy" with the version
  `nullrouter --version` shows, listing Endpoint & Key, Providers, Combo, Usage and Quota Tracker,
  then under "System": Proxy Pools, Console Log and Settings. Every page MUST be reachable from
  every other. This replaces spec 007's four-page layout.
- **FR-027**: Side panels MUST appear only on Endpoint & Key (Client adapters) and Providers
  (Provider plugins).
- **FR-028**: Where a page has nothing to show, it MUST say what is missing and name the CLI
  command that adds it.

**Pages**

- **FR-029**: Endpoint & Key MUST show the endpoint URL agents point at, and one card per agent key
  with what `keys list` shows (name, id, last four characters, created, revoked, break behaviour).
  Its Client adapters panel MUST say client-side adapters aren't built yet and show no adapter
  rows. The traffic landscape MUST be a slot.
- **FR-030**: Providers MUST show a card per provider, grouped as the mockup groups them, with its
  logo or text icon and the number and state of its accounts. A card MUST open a window with the
  provider's accounts and all its models.
- **FR-031**: The window's model filter MUST list every model kind the plugins declare, with its
  count for that provider, and no other kind.
- **FR-032**: Providers' side panel MUST list the bundled plugins, the plugins installed from the
  community, and the number of community plugins available, with each plugin's state as
  `plugins list` shows it. Its switches MUST be disabled (FR-014).
- **FR-033**: Quota Tracker MUST show one card per account with what `accounts list --long` shows
  (provider, name, enabled or disabled, kind, order, sign-in status and since when, cooldowns
  with time left, whether it needs signing in again, sign-in email and tier, priority), each quota
  window with what `quota` shows (quota left, source: polled, estimated, pay-as-you-go, pending
  first poll or stale; last poll time; reset time), and the account's pace, share and deficit for
  each target as `routing` shows them, with the amortization window. It MUST offer narrowing to
  one provider or one account, matching `quota <provider> [<name>]`.
- **FR-034**: Usage MUST show the Requests table: the newest 50 records first, with paging further
  back 50 at a time; each row with what `records list` shows (time, agent, model, the account it
  was placed on, placement reason, time to first token, total time, result). A row MUST open a
  window with everything `records show` shows, including each attempt, the stay-warm decision and
  the recorded changes.
- **FR-035**: Usage's stat cards, topology graph and period filter MUST be slots. A slot shows no
  number.
- **FR-036**: Settings MUST show the operator's behaviour settings as `behaviour show` shows them,
  where the data lives as `check` shows it, and the dashboard's status as `dashboard status`
  shows it.
- **FR-037**: Combo, Console Log and Proxy Pools MUST each say the feature isn't built yet and name
  what exists instead: Combo names `[[unified_model]]` in `config.toml` and `nullrouter unified`;
  Console Log names the Usage page; Proxy Pools says 0router has no proxy pool feature.
- **FR-038**: Every page MUST have the round housekeeping button. It MUST open a panel listing
  every current notice: every warning, note and error `check` reports, and every account that
  needs signing in again or is cooling down, in the CLI's words. The panel's chat box MUST be
  disabled and say it isn't built yet.
- **FR-039**: Latency MUST appear as the records carry it, per request and per attempt. This slice
  adds no latency summaries or trends.

**Plugin logos**

- **FR-040**: A provider plugin MAY declare a logo: a PNG file of at most 64 KiB and at most 256 px
  wide and 256 px high. Bundled and community plugins may carry one the same way.
- **FR-041**: The core MUST check each declared logo when it loads plugins. A logo that is
  missing, not a PNG, or over a limit MUST be ignored; the plugin MUST still load and serve as
  before, the dashboard shows its text icon, and `check` MUST report the plugin and the reason.
- **FR-042**: A logo MUST be served so that the browser can treat it only as an image.

**Privacy**

- **FR-043**: No page MUST show a secret: provider API keys, sign-in tokens and agent keys beyond
  the last four characters the CLI shows, OAuth client secrets, or the dashboard token.
- **FR-044**: No page MUST show prompt or response content. Records hold none, and the dashboard
  MUST NOT add any.

**Look**

- **FR-045**: The repo MUST contain a style guide taken from 9router's code. It records 9router's
  light color palette (brand, surface, border, text and status colors), type (fonts, sizes,
  weights), spacing, radii, shadows, the constant background grid, the icon set, and the styles
  of the sidebar (translucent, 288 px), buttons, cards, badges, tables, inputs, windows and side
  panels. Each value names the 9router file it came from.
- **FR-046**: The dashboard's styling MUST be built only from the style guide's values.
- **FR-047**: Status MUST be shown with the style guide's badge style and 9router's color for the
  same kind of status.
- **FR-048**: The dashboard MUST have one theme: 9router's light theme. There is no dark theme and
  no theme switch.
- **FR-049**: All dashboard text MUST be English.

### Key Entities

- **Dashboard token**: The secret that opens the dashboard. One at a time; issued by the CLI;
  replacing it signs out every browser.
- **Dashboard session**: A browser's proof that it entered the current token. Valid until the
  token is replaced.
- **Dashboard status**: On or off, the address, whether it is being served, and when the token
  was issued. Shown by `dashboard status` and on Settings.
- **Page snapshot**: The server state a page was built from, with its "as of" time. It is the same
  state the CLI would read at that moment.
- **Notice**: A `check` warning, note or error, or an account condition that needs action, with
  its subject (the page it concerns) and its CLI words.
- **Plugin logo**: A PNG a plugin declares, checked by the core at load; shown if it passes,
  ignored with a `check` note if not.
- **Slot**: A marked place for a later slice's content; it names what will appear there.
- **Style guide**: The document of 9router's look: tokens (colors, type, spacing, radii, shadows,
  grid) and component styles, each traced to a 9router source file.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: For each page, a test that builds the page and runs its matching CLI reads (FR-021)
  for the same moment finds 0 disagreements, across a test home covering every status in User
  Stories 2 to 6.
- **SC-002**: For each page, 0 facts are shown that have no twin.
- **SC-003**: 100% of `check` notices in a test home that triggers every kind appear in the
  housekeeping panel and on the page their subject names.
- **SC-004**: 100% of requests for a page or page data without a valid token get only the sign-in
  page (or, before a token is issued, only the page naming `nullrouter dashboard token`); 0
  connections from another machine succeed.
- **SC-005**: While pages are loaded continuously, and while the dashboard is made to fail,
  client requests show 0 additional failures and no more than 1 ms added latency at the 95th
  percentile, compared with the dashboard turned off.
- **SC-006**: A scan of every page, every dashboard response, the logs, and the home directory
  from the full test suite finds 0 secrets and 0 prompt text.
- **SC-007**: 100% of color, size, spacing, radius and shadow values in the dashboard's styling
  are defined in the style guide, and 100% of style guide values trace to a 9router source file.
- **SC-008**: Side by side with 9router in its light theme, the sidebar and each shared component
  (card, badge, button, table, input, window, panel) are judged the same in color, type, spacing
  and shape by the user.
- **SC-009**: Each page loads in under 1 second on a home with 50 accounts, 20 unified models and
  100,000 records.
- **SC-010**: With the internet disconnected, every page renders exactly as it does online.
- **SC-011**: Of plugins with a logo that is missing, not a PNG, over 64 KiB or over 256 px, 100%
  load and serve, show their text icon, and have a `check` note naming the reason.
- **SC-012**: From Quota Tracker alone, without running a command, the operator can tell which
  accounts need signing in again and how much quota each has left.

## Assumptions

Items marked *(technical decision)* are Claude's. They may be revised in planning without asking
the user, as long as nothing the user sees changes.

- The token is entered once per browser and kept until the token is replaced. There is no
  time-based expiry, because the dashboard is local-only and read-only. *(technical decision)*
- One dashboard token at a time; there are no per-user tokens. The operator is the only user.
- The page reads the same server state the CLI reads, through the shared read model of slice 008,
  so "agrees with the CLI" can be tested against one snapshot. *(technical decision)*
- Narrowing controls the mockup draws (provider and plugin search, the kind filter, Quota
  Tracker's provider and account filters, Requests paging) are views. Each shows a subset of what
  its page's CLI reads show and adds no fact. Quota Tracker's "Expiring first" is an order, not a
  fact. Without scripts, they work by reloading the page with the choice in its address.
  *(technical decision)*
- Windows (a provider's detail, a record's detail, the housekeeping panel) open without scripts,
  each with its own address, so a reload keeps it open. *(technical decision)*
- The mockup's copy button beside the endpoint URL needs a script; the URL is shown as text the
  operator can select instead. *(technical decision)*
- The mockup's "Recent Requests" list on Usage is a records read (`records list`) and is shown
  where the mockup draws it, beside the topology graph slot. *(technical decision)*
- Dropped unified models and limits notes go on the Combo entry, because it is the page that names
  unified models; slice 1 has no other page that lists them. *(technical decision)*
- Key cards show what `keys list` shows. The mockup's "last used", "requests today" and harness
  badge have no twin in this slice; the harness tag arrives in slice 2.
- Account notices in the housekeeping panel are the account conditions the CLI already shows
  (needs signing in again, cooling down); the panel invents no new kind.
- SC-005's 1 ms bound and SC-009's 1 second page load carry over from spec 007. *(technical
  decision)*
- The last four characters of an agent key, a provider key or a sign-in token are shown, as the
  CLI shows them (an account added from an environment variable shows the variable's name). They
  identify a secret; they are not enough to use it.
- Sign-in email and tier are shown, as `accounts list --long` shows them. They are not secrets.
- `docs/dashboard/style-guide.md`, written for spec 007, is the draft baseline for FR-045. Its
  "Not taken: page structure, navigation entries" line predates FR-026 and is revised: the sidebar
  and its entries are now 9router's. Its references to spec 007's FR numbers move to this spec's.
  `docs/dashboard/9router-inventory.md` is background. *(technical decision)*
- 9router loads its font (Inter) and icon font from Google. The dashboard serves both from the
  binary instead (both licences allow it), or falls back to the style guide's system font stack.
  *(technical decision)*
- The provider logos in the mockup come from 9router; where bundled and community plugins carry
  them, each must pass FR-040, and any over a limit is shrunk before it ships. *(technical
  decision)*
- Client traffic and the dashboard share one process. Isolation means the dashboard's work never
  holds anything a client request waits for, and its work per page is bounded. *(technical
  decision)*

## Out of Scope

- Changing anything from the dashboard, including plugin switches, plugin install, adapter review
  and the housekeeping chat: later.
- The traffic landscape and its gauges, usage totals per period, Est. Cost, the topology graph,
  the period filter, and the harness tag on keys: the next dashboard slice (summaries and
  landscape).
- Latency trends over time: the latency slice.
- Extra Usage charts and breakdown tables: later.
- Live auto-refresh: later. The operator reloads.
- Side panels on Usage, Quota Tracker and Settings: later.
- Reaching the dashboard from another machine, HTTPS, and binding to a network address: later.
- A dark theme and a theme switch; translations and language switching: later.
- Client-side adapters: slice 004.
- Model tests and combos themselves: later, with their own slices.
- Fitted quota weights and leak detection: later.
- Quota poll history on the dashboard.
- 9router's Token Saver, CLI Tools, Translator, Skills, Media pages and Basic Chat: not planned.

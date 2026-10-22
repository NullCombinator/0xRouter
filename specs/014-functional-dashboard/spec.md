# Feature Specification: Functional Dashboard

**Feature Branch**: `014-functional-dashboard`

**Created**: 2026-10-22

**Status**: Draft

**Scope brief**: [specs/briefs/2026-10-22-functional-dashboard.md](../briefs/2026-10-22-functional-dashboard.md).
In clarify, plan and analyze, an answer that contradicts a confirmed row of that brief's ledger
means stop and revisit the brief.

## Clarifications

### Session 2026-10-22

- Q: Should the four history-deleting housekeeping commands also be in the browser? → A: No, they
  stay CLI-only for now (FR-001).
- Q: What should the custom-provider form do with a private or local address (such as a model
  server on this machine)? → A: Refuse it with the plugin check's text and point to a new Settings
  switch for private endpoints (with a CLI twin), which applies to every plugin (FR-021).

**Input**: User description: "Functional dashboard: the operator runs 0router entirely from the
browser. Every change the CLI can make, the dashboard's pages can make too, in four areas: accounts
and keys; routing settings; plugins and adapters; combos and tests. A change made in the browser
runs exactly what the matching CLI command does. Anything that can't be undone first opens a
confirm window. Console Log lists every change made in the browser. Long tasks say they started and
show their result on reload. Pages stay server-rendered Rust with no JavaScript; one token covers
reading and writing; forms are protected against cross-site posting; this machine only; an API key
is never shown back. Out of scope: the housekeeping chat box."

## Context

Slices 009 and 010 built the dashboard as a look-only window: every page shows the operator's real
state, and every control that would change something is drawn disabled with a hint naming the CLI
command. This slice makes those controls work. It adds no new routing behaviour: every change the
browser makes is one the CLI already makes (or gains in this slice, for the few changes that have no
CLI command yet), so the CLI and the dashboard stay two faces of one set of operations.

- The **operator** runs 0router on their own machine and opens the dashboard in a browser on that
  machine, after entering the dashboard token once.
- A **change** is any operation that alters the operator's state: the home's files, the running
  server's state, or a started test, sign-in or build.
- A **CLI twin** is the `nullrouter` command that makes the same change.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Accounts and client keys from the browser (Priority: P1)

The operator adds a provider account by pasting its API key into a form, or signs an OAuth account
in through the provider's own page, sets each account's order, turns accounts on and off or makes
one exclusive, and removes an account. On Endpoint & Key they issue a client key (seeing it exactly
once), set its harness tag, break behaviour and adapter, and revoke a key.

**Why this priority**: accounts and keys are the first thing a new operator sets up and the change
they make most often; without them nothing else on the dashboard is usable without the CLI.

**Independent Test**: on a fresh home with only a dashboard token, add an API-key account, issue a
client key, send one request with that key through the router, and revoke the key, all from the
browser; the CLI's `accounts list` and `keys list` show the same results.

**Acceptance Scenarios**:

1. **Given** the Providers page, **When** the operator opens a provider, enters an account name and
   pastes an API key and submits, **Then** the account appears with its state, the key is never
   shown again on any page, and `nullrouter accounts list` shows the same account.
2. **Given** an OAuth provider, **When** the operator chooses to sign an account in, **Then** the
   browser goes to the provider's sign-in page and, after the operator approves there, comes back
   to the dashboard with the account signed in; a sign-in that is refused or abandoned leaves the
   account as it was and the page says why.
3. **Given** a provider whose sign-in carries a terms warning in the CLI, **When** the operator
   starts that sign-in, **Then** the page shows the same warning and asks for the same acceptance
   before going on.
4. **Given** an account, **When** the operator chooses Remove, **Then** a confirm window names the
   account and what is lost (its stored credentials; requests stop using it), and nothing changes
   until the operator confirms.
5. **Given** Endpoint & Key, **When** the operator issues a key with a name and an optional harness
   tag, **Then** the full key is shown once on the next page with a warning that it won't be shown
   again; reloading or coming back never shows it.
6. **Given** a key, **When** the operator revokes it, **Then** a confirm window says that clients
   using that key stop working at once, and after confirming the key is listed as revoked.
7. **Given** an account name already used for that provider, **When** the operator adds it again,
   **Then** the account's key is replaced and the page says so, as the CLI does. **Given** an add
   the CLI refuses (such as an invalid name), **Then** the page shows the CLI's refusal text next to
   the field and the form keeps the other entered fields (never the pasted secret).

---

### User Story 2 - Routing settings from the browser (Priority: P1)

On Settings, Quota Tracker and the provider and account windows, the operator changes the default
break behaviour, an account's or a plugin's meter overrides, the amortization window, connection
timeouts and retries, quota poll intervals, proxies (add, assign, clear, remove, mark fixed), and
acknowledges alerts.

**Why this priority**: routing settings are the reason operators open the CLI after setup; with US1
they make day-to-day running possible without a terminal.

**Independent Test**: change the default break behaviour, one account's reserve, and a provider's
timeout from the browser; `nullrouter behaviour show`, `routing` and `connection show` print the new
values, and the next request uses them.

**Acceptance Scenarios**:

1. **Given** Settings, **When** the operator picks another default break behaviour and saves,
   **Then** the page shows the new value and `nullrouter behaviour show` agrees.
2. **Given** an account, **When** the operator sets an override the CLI would refuse (a value out of
   range or of the wrong kind), **Then** nothing is saved and the page shows the CLI's error text
   next to the field.
3. **Given** a running server, **When** a saved change can't be applied (the server refuses the
   reload), **Then** the page says the change is saved but not applied and shows the server's reason,
   as the CLI does.
4. **Given** the Proxy Pools page, **When** the operator adds a proxy with a password, **Then** the
   password field is write-only like an API key, and the proxy is listed without it.
5. **Given** a proxy in use, **When** the operator tries to remove it, **Then** the page shows the
   CLI's refusal naming what uses it.

---

### User Story 3 - Unified models, combos and tests from the browser (Priority: P2)

On Combo, the operator declares a unified model (its name and member provider models), declares a
combo (its name and ordered members), edits or removes either, and runs a model test, a combo test
or Test All, seeing each verdict when it finishes.

**Why this priority**: unified models and combos are the routing targets clients ask for; tests are
how the operator trusts them. They come after accounts and settings because they need accounts to
exist.

**Independent Test**: declare a unified model with two members and a combo over it from the browser,
run the combo's test, reload until the verdict shows; `nullrouter unified`, `combos` and `verdicts`
agree.

**Acceptance Scenarios**:

1. **Given** Combo, **When** the operator declares a unified model whose member names a model no
   plugin declares, **Then** nothing is saved and the page shows the same error the CLI twin gives.
2. **Given** a test that bills more than one call, **When** the operator starts it, **Then** the page
   first states how many billed calls it will make (the CLI's prompt) and starts only after the
   operator confirms.
3. **Given** a started test, **When** the page returns, **Then** it says the test started and when;
   reloading shows progress, and the verdict once it ends; leaving the page does not stop the test.
4. **Given** a model with a verdict, **When** the operator marks or clears it, **Then** the verdict
   changes as `nullrouter verdicts mark|clear` would.
5. **Given** a unified model that a combo uses, **When** the operator removes it, **Then** the page
   shows the CLI twin's refusal naming the combo.

---

### User Story 4 - Plugins, custom providers and adapters from the browser (Priority: P2)

In the Providers side panel and its settings window, the operator installs a community plugin,
enables, disables or hides a provider, uninstalls a plugin, and creates a custom OpenAI- or
Anthropic-compatible provider with "Add … Compatible". In the Endpoint & Key side panel and its
settings window, the operator installs a harness adapter (from a local package or the catalogue),
approves or rejects its review, retries a review or build, clears a suspect adapter, removes an
adapter, acknowledges adapter alerts and sets the review settings.

**Why this priority**: these change what 0router can talk to; they are less frequent than accounts
and settings but they are the last CLI-only operations.

**Independent Test**: create a custom OpenAI-compatible provider from the browser, add an account to
it, send a request through it; then install the catalogue's claude-code adapter, approve its review
and bind a key to it; `nullrouter plugins list`, `providers` and `adapters list` agree.

**Acceptance Scenarios**:

1. **Given** "Add OpenAI Compatible", **When** the operator enters a provider id, a base URL and its
   model ids and submits, **Then** a provider plugin is written and checked as any plugin is, and the
   provider appears on the page at once; an invalid entry (bad URL, id taken, a URL the plugin checks
   refuse) is refused with the checker's text and nothing is written.
2. **Given** a provider, **When** the operator hides it, **Then** it leaves the Providers grid and
   model lists as the CLI twin's hide would, and the side panel still lists it so it can be shown
   again; disabling stops routing to it.
3. **Given** an installed plugin, **When** the operator uninstalls it, **Then** a confirm window names
   what is lost (the plugin, its fit state, and the accounts that stop working), and nothing changes
   until confirmed.
4. **Given** an adapter in review, **When** the operator approves it, **Then** its state changes as
   `adapters approve` would; a review or build that is running shows "started, reload to see".
5. **Given** a suspect adapter, **When** the operator clears it, **Then** a confirm window states what
   clearing means (it serves again) before it does.

---

### User Story 5 - Console Log of browser changes (Priority: P3)

Console Log lists every change made in the browser, newest first: when, what (the operation and its
subject, such as "account removed: anthropic/work"), and whether it succeeded or what error it gave.
A CLI command prints the same list.

**Why this priority**: it gives the operator an account of what was done from the browser and
replaces Console Log's empty page; it isn't needed for any other story to work.

**Independent Test**: make three browser changes, one of them refused; Console Log and its CLI twin
list all three in order with their results, and no entry contains a secret.

**Acceptance Scenarios**:

1. **Given** a change that pasted a secret, **When** it is logged, **Then** the entry names the
   operation and subject but never the secret, and never a prompt or response.
2. **Given** a refused change, **When** it is logged, **Then** the entry carries the refusal text.

---

### Edge Cases

- **Two tabs, stale form**: the operator submits a form built from state that has changed since
  (another tab or the CLI changed it). The change is checked against the current state, exactly as
  the CLI checks it; when the target no longer exists the page says so.
- **Reload after submit**: reloading the page a change returned to never repeats the change.
- **Cross-site post**: a form posted from another site, or without the page's check value, changes
  nothing and is answered with an error page.
- **Token reissued**: after `dashboard token` replaces the token, open sessions can no longer change
  anything and are asked for the new token.
- **Server stopped**: the dashboard is served by the server, so when the server stops there is no
  page; a change sent as it stops either completes or fails with the CLI's text, never half-applied
  beyond what the CLI would leave.
- **A change that needs the running server** (quota poll now, tests): the CLI refuses these when no
  server runs, but the dashboard runs only inside the server, so from the browser they always have
  one.
- **Long task still running** when the operator starts another on the same target: the page shows the
  CLI's answer (refused or queued, as the CLI does).
- **OAuth callback after the dashboard token expired or with a wrong state value**: the sign-in is
  refused and nothing is stored.
- **Very busy dashboard**: change requests share the dashboard's existing limit on concurrent work
  and never slow client requests.

## Requirements *(mandatory)*

### Functional Requirements

**Changes and their CLI twins**

- **FR-001**: Every change a `nullrouter` command can make to the operator's state MUST be possible
  from the dashboard page that shows that state, except `serve` itself, `dashboard token` (which
  must stay a CLI act so a lost browser session can't lock the operator out) and the destructive
  housekeeping commands `records prune`, `records forget`, `quota prune` and `quota forget`, which
  stay CLI-only so no click in the browser can erase request or quota history (user, 2026-10-22).
- **FR-002**: The changes in scope MUST include, by area:
  - accounts and keys: add (API key), OAuth sign-in, priority, enable/disable, exclusive, remove;
    key issue, revoke, harness tag, break behaviour, adapter binding;
  - routing settings: default break behaviour, account and plugin meter overrides, amortization
    window, connection settings, quota poll interval, poll now, alert acknowledgement, proxies (add,
    remove, use, clear, fixed);
  - plugins and adapters: plugin install, uninstall, enable, disable, hide, show; custom compatible
    provider; adapter install (local package and catalogue), review, build, approve, reject, clear,
    rebuild, remove, review settings, adapter alert acknowledgement;
  - combos and tests: declare, edit and remove unified models and combos; run model tests, combo
    tests and Test All; mark and clear verdicts; test settings;
  - the private-endpoints setting (FR-021).
- **FR-003**: A change made in the browser MUST run the same operation as its CLI twin: the same
  checks, the same refusal text (shown on the page next to what it concerns), and the same effect on
  the files and the running server, applied at once.
- **FR-004**: Changes that have no CLI command today MUST gain one in this slice, so every browser
  change keeps a CLI twin: declaring, editing and removing unified models and combos; enabling,
  disabling, hiding and showing a provider; creating a custom compatible provider; turning private
  endpoints on and off (FR-021); and listing the browser change log (FR-016).
- **FR-005**: A change MUST never be repeated by reloading or going back to the page it returned;
  after a change the browser lands on the page that shows the changed state.

**Secrets**

- **FR-006**: An API key, a proxy password or a pasted sign-in code MUST be entered only in a
  write-only field, never echoed back, never kept in a form that is redisplayed after an error, and
  never written to any page, log or record.
- **FR-007**: A newly issued client key MUST be shown exactly once, on the page the issue returns,
  with a warning that it won't be shown again; the dashboard MUST not be able to show it afterwards.
- **FR-008**: OAuth sign-in MUST use the provider's own sign-in page in the operator's browser and
  return to the dashboard; any terms warning and acceptance the CLI requires for that provider MUST
  be shown and required the same way.

**Confirmation and long tasks**

- **FR-009**: An irreversible change MUST first open a confirm window that names the subject and what
  is lost, and MUST change nothing until the operator confirms. Irreversible changes are at least:
  removing an account, revoking a key, uninstalling a plugin, removing a proxy, removing an adapter,
  removing a unified model or combo, and clearing a suspect adapter. Where the CLI asks for
  confirmation (billed test calls, adapter clear and remove), the page MUST ask the same question.
- **FR-010**: A long task (sign-in wait, model or combo test, Test All, adapter review, build,
  rebuild, catalogue install, proxy fixed, quota poll) MUST return at once with a page saying it
  started and when; reloading MUST show its progress and its result once done; leaving the page MUST
  not stop it. The page MAY reload itself every few seconds while a task it shows is running.

**Access and safety**

- **FR-011**: The one dashboard token MUST cover reading and changing; there is no second token.
- **FR-012**: Every change MUST be accepted only from a form the dashboard itself served to a
  signed-in session, so a page on another site can't make the browser post a change (cross-site
  request forgery), and only from this machine, as reading is.
- **FR-013**: The pages MUST stay server-rendered with no script; every change is a form submission
  that reloads the page. The pages' security policy MUST keep forbidding scripts and MUST only allow
  forms to post to the dashboard itself.
- **FR-014**: No page, window, error or log entry MUST show a secret, a prompt or response content
  (009 FR-043, FR-044).
- **FR-015**: A fault while making a change (a refused change, a failed apply, a panic in a page
  build) MUST never slow or break client requests (009 FR-012), and the change limits MUST share the
  dashboard's existing limits on concurrent work.

**Console Log**

- **FR-016**: Console Log MUST list every change made in the browser, newest first: time (in this
  machine's time zone, named), operation, subject, and result (done, saved but not applied, or the
  refusal text); with a CLI twin printing the same list. Entries MUST never contain a secret.
  Changes made from the CLI are not listed.
- **FR-017**: The log MUST be kept across restarts and MUST be bounded so it can't grow without
  limit; older entries beyond the bound are dropped.

**Look and parity**

- **FR-018**: Every control that 009 and 010 drew disabled with a CLI hint for an operation in FR-002
  MUST become a working control in the same place; 9router's look, sidebar and page names stay
  (009 FR-001 to FR-005). Forms and windows follow 9router's form and modal styles from the style
  guide.
- **FR-019**: Every fact a page shows MUST keep its CLI twin (009 FR-009); new facts this slice adds
  (a task's progress, the change log) have theirs.
- **FR-020**: The housekeeping chat box MUST stay disabled with its "not built yet" hint.
- **FR-021**: A custom compatible provider whose address is private or local MUST be refused with
  the plugin check's text while private endpoints are off, and the refusal MUST point to the setting.
  Settings MUST show and change the private-endpoints setting (it applies to every plugin, as today),
  with a CLI twin; turning it on MUST first open a confirm window saying that plugins may then reach
  this machine and the local network.

### Key Entities

- **Change**: one operation the operator asks for from the browser: its kind, its subject (account,
  key, provider, adapter, unified model, combo, proxy, setting), its inputs (secrets never kept),
  and its result.
- **Change log entry**: the record of a change on Console Log: time, operation, subject, result.
- **Task**: a long-running change (test, sign-in, build, review, poll): its subject, start time,
  progress and result.
- **Custom compatible provider**: a provider plugin the operator creates from a form: id, display
  name, API style (OpenAI-compatible or Anthropic-compatible), base URL, and its model ids.
- **Unified model / combo declaration**: the operator's declaration of a routing target, now
  editable from the browser and the CLI.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: On a fresh home with only a dashboard token, the operator sets up a working router (one
  account, one client key, one unified model, one successful request through it) using only the
  browser, in under 10 minutes, without opening a terminal.
- **SC-002**: For each change in FR-002, a test makes the change once from the browser and once with
  the CLI twin on identical homes, and the resulting homes and server state are identical (100% of
  the listed changes).
- **SC-003**: For each refusal the CLI twin can give for a change in FR-002 that the tests exercise,
  the page shows the same text.
- **SC-004**: No served page, form, log entry or error contains a secret entered through the browser:
  a test pastes marker secrets into every write-only field and finds none of them in any response
  body or file other than the CLI's own secret store.
- **SC-005**: A cross-site post, a post without the form's check value, and a reload of a change's
  result each change nothing (100% of the tested forms).
- **SC-006**: While the browser makes changes as fast as it can, client requests' router overhead
  stays within the 013 baseline's noise (p95 no more than 10% above it).
- **SC-007**: Every irreversible change in FR-009 shows a confirm window first and is undone by
  nothing if the operator cancels (100%).
- **SC-008**: The pages still contain no script and the security policy still forbids scripts (the
  009 style-guide tests keep passing).

## Assumptions

- The dashboard keeps its existing address, sign-in and limits (009); changes are made by the same
  signed-in session that reads.
- A change from the browser is made by the server process that serves the dashboard, so changes that
  need a running server always have one.
- Hiding a provider and disabling it are different: hidden providers leave the grid and model lists
  (as 9router's "show on page" switch), disabled providers are not routed to. Both are reversible.
- The change log's bound is a default the plan sets (for example the newest 1,000 entries), not a
  user choice in this slice.
- A custom compatible provider supports the text section only (chat in the chosen API style, model
  list); other model types come from full plugins.
- Changes made from the CLI are not added to Console Log; the CLI's own output is its record.

## Out of Scope

- The housekeeping chat box (→ later).
- Deleting request or quota history from the browser: `records prune|forget` and
  `quota prune|forget` stay CLI-only (→ later, if wanted).
- A second, read-only token or per-user access (one operator, one token).
- Script-based interactivity: live updates without reload, drag-and-drop ordering, inline editing.
- Latency trends and latency per unified model (→ latency slice 2).
- Dashboard access from other machines.

# Feature Specification: Account Sign-In

**Feature Branch**: `005-account-sign-in`

**Created**: 2026-10-02

**Status**: Draft

**Scope brief**: [specs/briefs/2026-10-02-account-sign-in.md](../briefs/2026-10-02-account-sign-in.md). In
clarify and plan, an answer that contradicts a confirmed row of that brief's ledger means stop and
revisit the brief.

**Input**: User description: "Account sign-in: subscription accounts become ordinary 0router accounts. The operator signs a subscription account in from the CLI for three chosen providers: anthropic (Claude Pro/Max), xai (Grok account) and grok-cli (Grok Build). Sign-in can be completed on a machine with no browser, such as an SSH session. Before an anthropic subscription sign-in completes, the CLI warns once that Anthropic's terms limit subscription use outside its own apps and that the operator carries that risk. A signed-in account then serves requests like an API-key account, with the same retry, fallback and request records, and a provider can have several signed-in accounts. xai serves text, image and video; grok-cli serves text, plus any other model type its live model list shows. 0router keeps tokens fresh on its own, so no client request fails because a token expired. When an account can no longer be refreshed, it is taken out of service, other accounts and providers carry on, and the operator is told which account needs signing in again: in the accounts list, in the request record and in the informational error. Quota is visible. For every chosen-provider account whose provider reports quota, signed-in or API-key (opencode-go and opencode-zen included), 0router polls on a steady slow schedule, also while idle, adjustable per account. The CLI shows the remaining quota, the window reset time and when it was last polled; an account whose provider reports no quota shows \"quota not reported\". Each poll is kept locally, with a tally of the input, output, cache-read and cache-write tokens 0router sent through that account since the previous poll, so the routing slice learns from real data from its first day. The core is generic and the specifics are data: the core provides the sign-in flows (browser, device code, token refresh) and quota polling, and each provider's plugin declares its sign-in endpoints, scopes, quota endpoint, and the client identity its endpoints expect. In this slice, only the chosen providers may use them. The slice fails if: a standard client breaks on 0router; a single 429, 5xx or timeout reaches the client when another account or provider could serve; a token expiry makes a client request fail; an account needs signing in again and the operator isn't told which one; a shown quota doesn't match what the provider reports or doesn't say when it was polled; sign-in can't be completed without a browser; usage numbers are missing or wrong; a secret appears in a log, record, error, or anything a plugin sees. Tests use more than one client harness. Constraints: plugins are data, never code, and never hold secrets; the core does all sending and injects secrets only at execution; signed-in tokens are stored and protected like API-key secrets (refused if readable by others, bound to their provider's hosts, never shown in full); every model type stays first-class. Out of scope: quota-aware routing (an account at 0% is still tried until retry and fallback move on), between-poll quota estimates, fitted quota weights, leak detection, and persistent request history → slice 006; sign-in for community plugins and for other providers (Codex, Gemini CLI, Copilot, Kiro, Cursor and others) → later; grok-cli bulk import → later; sign-in from a web page or dashboard → later. Scope brief: specs/briefs/2026-10-02-account-sign-in.md"

## Clarifications

### Session 2026-10-02

- Q: Without a disguise, what does a Claude Pro/Max sign-in in 005 send? 9router's subscription
  path disguises traffic as Claude Code to evade detection: it renames tools, injects decoy tools
  and a fabricated billing header, and invents device and account ids. → A: The sign-in stays in
  scope, with no disguise. Requests carry only the plugin's declared headers. Nothing in the body
  changes: no fake ids, decoy tools, tool renaming or injected system text. If Anthropic refuses
  these requests, the account is marked "refused by provider" with a clear message, and xai and
  grok-cli still ship complete.
- Q: When is the Anthropic terms warning shown? → A: At every anthropic subscription sign-in,
  including a repeat sign-in of the same account. Acceptance is required each time; nothing is
  stored.
- Q: May a plugin's declared client identity copy the official client's identity (for example
  `User-Agent: claude-cli/…`, `X-App: cli`, Stainless SDK headers), or only send what the
  endpoint needs to work? → A: It may copy the official client's identity in headers (brief row
  16). The body stays unchanged (Q1).
- Q: How long are quota poll history and token tallies kept? → A: Until the operator prunes them
  or removes the account's history. Nothing is dropped automatically.
- Q: grok-cli's official client sends per-request and per-account header values (session id,
  request id, turn count, account email and user id, a machine-id hash). May the core fill such
  headers? → A: Yes, from a closed set of true values the core owns: session id, fresh request
  id, turn count, upstream model, the signed-in account's own email and user id, and a random
  id generated once per installation (which replaces the machine-id hash). Nothing else.
- Q: May a plugin force request parameters that aren't prompt content (grok-cli's `store`,
  `reasoning.summary`, `include`, and `reasoning.effort` for effort-suffixed model ids)? → A:
  The request goes upstream as the client sent it, as far as possible, with minimum friction.
  A plugin may force a parameter from the closed set `store`, `reasoning.summary`,
  `reasoning.effort`, `include` only where the provider fails without it (shown by a live
  check), or where the client asked for it through the model id (effort-suffixed ids). Each
  forced parameter is recorded. Prompt content, tools and conversation items are never changed.

## User Scenarios & Testing *(mandatory)*

This slice has three kinds of user:

- The **operator** runs 0router. They sign subscription accounts in, add API-key accounts, issue
  agent keys, and read accounts, quota and request records from the CLI.
- The **client** (agent) is any standard harness or SDK, as in slice 003. It must not be able to
  tell whether an API-key account or a signed-in account served it.
- The **plugin author** declares a provider as data. In this slice, only the chosen providers'
  plugins may declare sign-in and quota.

### User Story 1 — Sign in a subscription account and serve from it (Priority: P1)

The operator runs one CLI command to sign in a subscription account on anthropic (Claude
Pro/Max), xai (Grok account) or grok-cli (Grok Build). The command works on a machine with no
browser: it shows a link or code the operator opens on any other device, and it finishes there.
From then on, the account serves client requests exactly as an API-key account does: same
access keys, same unified models, same retry, fallback, informational errors and records. A
provider can hold several signed-in accounts, alongside API-key accounts.

**Why this priority**: Subscription accounts are the reason for the slice. Slice 006 amortizes
work across subscription windows, and there are no windows without them.

**Independent Test**: Over an SSH session with no browser, sign in one account on each of the
three providers. From at least two client harnesses in different API styles, send streamed and
non-streamed requests to each provider. Every request succeeds and is recorded against the
signed-in account.

**Acceptance Scenarios**:

1. **Given** a machine with no browser, **When** the operator starts a grok-cli sign-in, **Then**
   the CLI shows a verification link and code, waits while the operator approves on another
   device, and reports the account as signed in.
2. **Given** a machine with no browser, **When** the operator starts an xai or anthropic
   sign-in, **Then** the CLI shows a link to open on another device, and the operator completes
   sign-in by pasting back what the provider shows after approval (the redirect address or the
   code).
3. **Given** a machine with a browser, **When** the operator starts a sign-in, **Then** the CLI
   can open the browser and complete without the operator pasting anything.
4. **Given** an anthropic subscription sign-in, **When** it is about to complete, **Then** the
   CLI shows a warning that Anthropic's terms limit subscription use outside Anthropic's own
   apps and that the operator carries that risk, and completes only after the operator accepts.
5. **Given** a signed-in anthropic account, **When** Claude Code and an OpenAI SDK client each
   send a request for an anthropic model, **Then** both get a valid answer in their own style,
   and the usage they see matches what anthropic reported.
6. **Given** two signed-in grok-cli accounts and one transient failure on the first, **When** a
   client sends a request, **Then** it is retried and falls back across the accounts exactly as
   slice 003 does for API-key accounts, and the client sees only the answer.
7. **Given** a signed-in xai account, **When** clients request text, an image and a video,
   **Then** each succeeds through the same pipeline and is recorded with its model type.
8. **Given** a signed-in grok-cli account, **When** its live model list includes a model type
   other than text, **Then** that model is listed to clients and served like any other model of
   that type.
9. **Given** a signed-in account, **When** the operator lists accounts, **Then** it shows the
   provider, account name, that it is signed in, its state, and its secret only in shortened
   form.
10. **Given** a sign-in the operator abandons, or one the provider refuses, **When** the CLI
    stops, **Then** no account is added, and the CLI says why.

---

### User Story 2 — Tokens stay fresh without the client noticing (Priority: P1)

Signed-in access tokens expire, some within the hour. 0router refreshes them on its own, ahead
of expiry and while idle, so a client request never fails because a token expired. If a token
is rejected anyway, 0router refreshes it and retries without the client seeing the rejection.

**Why this priority**: The user named "a token expiry makes a client request fail" as a
failure of this slice. Without silent refresh, a signed-in account is unusable for long
sessions.

**Independent Test**: With a fake provider issuing short-lived tokens, run a steady stream of
requests from two client harnesses across many token lifetimes, including idle gaps longer than
a token lifetime. No request fails, and the records show no expiry-caused failure.

**Acceptance Scenarios**:

1. **Given** a token close to expiry, **When** its refresh margin is reached, **Then** 0router
   refreshes it before any request needs it, whether or not traffic is flowing.
2. **Given** a provider that rejects a token as expired before its stated expiry, **When** a
   request gets that rejection, **Then** 0router refreshes the token and retries the request on
   the same account, and the client sees only the answer.
3. **Given** many concurrent requests on one account at expiry, **When** they all need a fresh
   token, **Then** the account is refreshed once, and every request uses the new token.
4. **Given** a refresh that fails transiently (timeout, 5xx), **When** requests arrive, **Then**
   they fall back to other accounts or providers as for any transient failure, and 0router
   keeps retrying the refresh. The account is not taken out of service for a transient failure.
5. **Given** 0router restarts, **When** it comes back, **Then** signed-in accounts keep serving
   without signing in again.

---

### User Story 3 — An account that can't be refreshed is named, and service carries on (Priority: P1)

When a provider refuses to refresh an account for good (revoked, password changed, sign-in
expired), 0router takes that account out of service. Other accounts and providers keep serving.
The operator is told exactly which account needs signing in again, in three places: the accounts
list, the request record, and the informational error when nothing else could serve.

**Why this priority**: The user named "an account needs signing in again and the operator isn't
told which one" as a failure of this slice.

**Independent Test**: With two accounts on one provider, make the fake provider refuse the first
account's refresh for good. Requests from two client harnesses keep succeeding on the second
account. The accounts list, the records of affected requests, and (with no second account) the
informational error each name the first account and say it needs signing in again.

**Acceptance Scenarios**:

1. **Given** a refresh refused for good, **When** the operator lists accounts, **Then** that
   account shows as "needs sign-in" with the provider, the account name, when it happened, and
   the provider's reason.
2. **Given** an account out of service, **When** a request would have used it, **Then** the
   request goes to the next account or provider, and its record shows the skipped account by
   name with "needs sign-in" as the reason.
3. **Given** the only account of a provider is out of service and no other provider can serve,
   **When** a client sends a request, **Then** the informational error names the account and
   says it needs signing in again, with the sign-in command to run.
4. **Given** an account marked "needs sign-in", **When** the operator signs the same account in
   again, **Then** it returns to service on the next request without a restart, under the same
   name.

---

### User Story 4 — Quota is visible from the CLI (Priority: P2)

For every chosen-provider account whose provider reports quota, signed-in or API-key, 0router
polls the provider on a steady slow schedule, also while idle. The CLI shows each account's
remaining quota for each window the provider reports, when that window resets, and when it was
last polled. An account whose provider reports no quota shows "quota not reported". The
operator can change the polling schedule per account.

**Why this priority**: The operator needs to see what a subscription has left. Routing on quota
waits for slice 006, so this story doesn't change which account serves.

**Independent Test**: With a fake provider reporting known quota values, poll every chosen
provider type. The CLI shows exactly the reported values, reset times and poll times. Changing
the provider's values shows up after the next poll, and an account without quota reporting
shows "quota not reported".

**Acceptance Scenarios**:

1. **Given** a signed-in anthropic account, **When** the operator shows quota, **Then** each
   window the provider reports (for example the 5-hour and the weekly window) appears with its
   remaining amount, its reset time, and the last poll time.
2. **Given** signed-in grok-cli accounts and API-key accounts on opencode-go and opencode-zen,
   **When** the operator shows quota, **Then** each appears with the values its provider
   reported.
3. **Given** an account whose provider reports no quota (for example xai, if research finds no
   quota report), **When** the operator shows quota, **Then** it reads "quota not reported".
4. **Given** no traffic for a day, **When** the operator shows quota, **Then** the last poll time
   is no older than the account's polling interval (plus one failed-poll retry), because polls
   continue while idle.
5. **Given** the operator sets a different polling interval for one account, **When** time
   passes, **Then** only that account polls on the new schedule.
6. **Given** a poll fails, **When** the operator shows quota, **Then** the last good values are
   still shown with their own poll time, and the failure is shown with its time and reason.
7. **Given** an account at 0% remaining, **When** a request arrives, **Then** it is still tried
   in the usual order, and slice 003's retry and fallback handle whatever the provider answers.

---

### User Story 5 — Each poll is kept with a tally of 0router's own traffic (Priority: P2)

Every poll is kept locally: the provider's reported values, the poll time, and a tally of the
input, output, cache-read and cache-write tokens 0router sent through that account since the
previous poll. Slice 006 fits quota weights from this history, so it has real data from its
first day.

**Why this priority**: Without the history, 006 starts blind. The data is cheap to keep now and
can't be recovered later.

**Independent Test**: Send a known set of requests through an account between two polls. The
kept poll record shows a tally equal to the sum of the provider-reported usage of those
requests, and the history survives a restart.

**Acceptance Scenarios**:

1. **Given** requests served by one account between two polls, **When** the second poll is
   kept, **Then** its tally equals the sum of input, output, cache-read and cache-write tokens
   the provider reported for those requests.
2. **Given** a request whose provider reported no usage, **When** it is tallied, **Then** the
   tally counts it as a request with unreported usage instead of adding zero tokens.
3. **Given** a restart, **When** 0router comes back, **Then** the poll history and the current
   interval's tally are still there.
4. **Given** a request retried or fallen back across accounts, **When** it is tallied, **Then**
   each account's tally holds only the tokens of the attempts sent through that account.

---

### User Story 6 — Sign-in and quota are generic core, specifics are plugin data (Priority: P3)

The core provides the sign-in flows (browser with paste-back, device code, token refresh) and
quota polling. Each chosen provider's plugin declares, as data: its sign-in endpoints, client
id, scopes, quota endpoint and how to read it, and the client identity its endpoints expect
(for example user agent and client headers). No plugin holds a secret. In this slice, only the
chosen providers' plugins may use these declarations.

**Why this priority**: Opening sign-in to community plugins later should be a switch, not a
rewrite. The user also wants no provider-specific code path where data can do.

**Independent Test**: Validate the three sign-in plugins and the opencode plugins through the
validation gate. A community plugin declaring sign-in or quota is still refused with "not
supported by this core". A plugin that tries to hold a client secret or token is refused.

**Acceptance Scenarios**:

1. **Given** the chosen plugins, **When** 0router loads them, **Then** their sign-in and quota
   declarations pass the validation gate.
2. **Given** a community plugin that declares sign-in, **When** the operator installs it,
   **Then** it is refused with a "not supported by this core" message that names sign-in.
3. **Given** a plugin that places a secret (client secret, token) in its file, **When** it is
   validated, **Then** it is refused.
4. **Given** a plugin declaring a sign-in or quota endpoint on a host outside its provider's
   hosts, **When** it is validated, **Then** it is refused, and no token is ever sent there.

---

### Edge Cases

- **Two sign-ins with the same name**: signing in under an existing account name replaces that
  account's tokens only when it is the same provider. The CLI says it replaced them. A different
  provider with the same name is a separate account.
- **Same subscription signed in twice under two names**: both accounts serve. 0router doesn't
  detect that they share one subscription; their quota readings will match.
- **Sign-in times out**: a device code or link that expires before approval ends the sign-in
  with a clear message. No account is added.
- **Provider rotates the refresh token on every refresh**: the new refresh token is stored
  before the old one is discarded, so a crash mid-refresh never loses the account.
- **Refresh during a stream**: a refresh never interrupts a stream already running on the old
  token.
- **Clock skew**: refresh timing tolerates the local clock differing from the provider's by a
  few minutes.
- **Quota reported in different units**: each window is shown in the provider's own unit
  (percent, credits, requests). 0router doesn't convert between units.
- **Quota poll rejected for an expired token**: the token is refreshed and the poll retried,
  like a request.
- **Quota poll spends quota**: a poll must not cost the account any of the quota it measures.
  A provider whose only quota report costs quota is treated as "quota not reported".
- **Account removed**: removing an account stops its polls. Its poll history is kept until the
  operator removes it too.
- **Token store readable by others**: 0router refuses to start, as for API-key secrets, and
  names the file.
- **Anthropic refuses undisguised subscription traffic**: the account is marked "refused by
  provider" (FR-004b), other accounts and providers serve, and xai and grok-cli are unaffected.
  0router never retries with a disguise.
- **A signed-in token echoed in an upstream error**: it is redacted before it reaches the
  record, log or client.

## Requirements *(mandatory)*

### Functional Requirements

**Sign-in**

- **FR-001**: The operator MUST be able to sign in a subscription account from the CLI on
  anthropic (Claude Pro/Max), xai (Grok account) and grok-cli (Grok Build), naming the account.
- **FR-002**: Every sign-in MUST be completable on a machine with no browser: by a device code
  where the provider offers one, otherwise by a link opened on another device and the redirect
  address or code pasted back into the CLI.
- **FR-003**: Where a browser is available, the CLI MUST be able to complete sign-in through
  it without manual pasting.
- **FR-004**: Before an anthropic subscription sign-in completes, the CLI MUST warn that
  Anthropic's terms limit subscription use outside Anthropic's own apps and that the operator
  carries that risk, and MUST complete only after the operator accepts. The warning MUST appear
  at every anthropic subscription sign-in, including a repeat sign-in of the same account.
  Declining adds or changes no account.
- **FR-004a**: Requests through a signed-in account MUST carry the plugin's declared client
  identity headers on top of what an API-key request sends. Each value is either a fixed string
  declared in the plugin, which may match the provider's official client, or one of a closed
  set of values the core fills with true information: the agent's session id, a fresh request
  id, the conversation's turn count, the upstream model id, the signed-in account's own email
  and user id, and a random id generated once per installation. No header value may be a hash
  of the request or an id made to resemble another machine or account. 0router MUST NOT change the
  request body beyond the forced parameters of FR-004c: no tool renaming, decoy tools, injected
  system text, conversation-item changes, or invented device, account or user ids.
- **FR-004b**: When a provider refuses requests from a signed-in account because of how they are
  sent (not because of quota, expiry or a transient failure), the account MUST be marked
  "refused by provider" in the accounts list with the provider's reason. Requests fall back as
  for any non-serving account, and the informational error names the account and the reason.
- **FR-004c**: The request body MUST go upstream as the client sent it, as far as possible. A
  plugin MAY declare forced parameters from the closed set `store`, `reasoning.summary`,
  `reasoning.effort`, `include` (appended), per endpoint or per model, only where the provider
  fails without them or the client asked for them through the model id. Each forced parameter
  MUST be noted in the request record. This extends slice 003's FR-038 for these parameters
  only.
- **FR-005**: A provider MUST be able to hold several signed-in accounts and API-key accounts
  at once. Signing in applies to the next request without a restart.
- **FR-006**: An abandoned, expired or refused sign-in MUST add no account and MUST say why.

**Serving**

- **FR-007**: A signed-in account MUST serve requests exactly as an API-key account does: the
  same client styles, access keys, unified models, retry, fallback, stay-warm, informational
  errors and records as slice 003. A client MUST NOT be able to tell which kind served it.
- **FR-008**: xai MUST serve text, image and video. grok-cli MUST serve text, plus any other
  model type its live model list shows. Every model type stays first-class (slice 003 FR-011).
- **FR-009**: xai and grok-cli MUST become chosen (bundled) providers, alongside slice 003's
  chosen five.

**Token freshness**

- **FR-010**: 0router MUST refresh signed-in tokens ahead of expiry, whether or not traffic is
  flowing.
- **FR-011**: When a provider rejects a token as expired or invalid, 0router MUST refresh it
  and retry the request on the same account before the client sees anything.
- **FR-012**: Concurrent refreshes of one account MUST be merged into one.
- **FR-013**: A refresh that fails transiently MUST be retried, and MUST NOT take the account
  out of service. Meanwhile, requests use the usual retry and fallback.
- **FR-014**: Signed-in accounts MUST survive a restart without signing in again.

**Accounts that need signing in again**

- **FR-015**: When a provider refuses a refresh for good, 0router MUST take the account out of
  service and keep serving from other accounts and providers.
- **FR-016**: The operator MUST be told which account needs signing in again: in the accounts
  list (provider, account name, time, provider's reason), in the record of every request that
  skipped it, and in the informational error when no other account or provider could serve.
- **FR-017**: Signing the same account in again MUST return it to service on the next request,
  without a restart.

**Quota**

- **FR-018**: For every chosen-provider account whose provider reports quota, signed-in or
  API-key (opencode-go and opencode-zen included), 0router MUST poll the provider on a steady
  slow schedule, also while idle.
- **FR-019**: The operator MUST be able to change the polling interval per account from the CLI.
- **FR-020**: The CLI MUST show, per account and per reported window: the remaining quota as
  the provider reported it, the window reset time, and the last poll time. An account whose
  provider reports no quota MUST show "quota not reported".
- **FR-021**: A failed poll MUST keep the last good values visible with their own poll time,
  and MUST show the failure with its time and reason.
- **FR-022**: Quota MUST NOT change which account serves a request in this slice.

**Poll history and traffic tally**

- **FR-023**: Each poll MUST be kept locally with its time, the provider's reported values per
  window, and a tally of the input, output, cache-read and cache-write tokens 0router sent
  through that account since the previous poll, plus the request count and the count of
  requests whose usage was not reported.
- **FR-024**: Tallies MUST equal the sum of the provider-reported usage of the attempts sent
  through that account in the interval.
- **FR-025**: Poll history and the current interval's tally MUST survive a restart. They MUST be
  kept until the operator prunes them (for example older than a date) or removes an account's
  history from the CLI. Nothing is dropped automatically.
- **FR-026**: The kept data MUST use the units and token categories of the approved quota meter
  design, so slice 006 can fit from it without migration.

**Generic core, plugin data**

- **FR-027**: The core MUST provide the sign-in flows (browser with paste-back, device code,
  token refresh) and quota polling as generic capabilities.
- **FR-028**: Each provider plugin MUST be able to declare, as data: sign-in endpoints, client
  id, scopes, the flows it supports, its quota endpoint and how to read the reported windows,
  and the client identity its endpoints expect.
- **FR-029**: In this slice, only chosen providers' plugins may use sign-in and quota
  declarations. A community plugin declaring them MUST still be refused with a "not supported by
  this core" message.
- **FR-030**: Sign-in, refresh, quota and client-identity declarations MUST pass the validation
  gate. Their endpoints MUST be on the provider's declared hosts.

**Secrets**

- **FR-031**: Signed-in tokens MUST be stored and protected like API-key secrets: refused if
  readable by others, sent only to their provider's hosts, and never shown in full.
- **FR-032**: OAuth client secrets MUST stay in the core credentials table, never in a plugin.
- **FR-033**: No access token, refresh token, OAuth client secret, sign-in code or API key may
  appear in any log, record, error, CLI output, or data a plugin can see.
- **FR-034**: The core MUST do all sending, for requests, refreshes and polls, and inject
  secrets only at execution.

### Key Entities

- **Provider account** (extended from slice 003): an API-key account or a signed-in account.
  Has a provider, a name, a state (active, refreshing, needs sign-in, refused by provider,
  disabled), and a polling interval. "Refreshing" means the token has expired and a refresh is
  being retried.
- **Signed-in credentials**: the tokens of one signed-in account, with their expiry. Held only
  by the core, stored like API-key secrets.
- **Sign-in declaration**: a provider plugin's data for sign-in: endpoints, client id, scopes,
  supported flows, client identity.
- **Quota declaration**: a provider plugin's data for its quota report: endpoint and how to read
  each window.
- **Quota window**: one limit the provider reports for an account (for example 5-hour, weekly,
  credits): remaining amount, unit, and reset time.
- **Quota poll**: one reading of an account's quota windows at a time, or its failure, plus the
  traffic tally since the previous poll.
- **Traffic tally**: per account and poll interval, the input, output, cache-read and
  cache-write tokens 0router sent, with request counts.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Over an SSH session with no browser, the operator completes sign-in on each of the
  three providers, each in under 3 minutes of operator time.
- **SC-002**: At least two different client harnesses, in different API styles, complete
  streamed and non-streamed requests through a signed-in account on each of the three providers
  with zero client-side errors.
- **SC-003**: In token-expiry tests spanning at least 20 token lifetimes, with traffic and with
  idle gaps, 0 client requests fail because of an expired token.
- **SC-004**: In failure-injection tests on signed-in accounts, 0% of injected 429, 5xx, timeout
  and connection failures reach the client when another account or provider could serve.
- **SC-005**: In 100% of refresh-refused tests, the account is named as needing sign-in in the
  accounts list, in the record of each affected request, and in the informational error when
  nothing else could serve.
- **SC-006**: For every polled account in the test matrix, the shown quota equals what the
  provider reported at the shown poll time, and every shown value carries its poll time.
- **SC-007**: After 24 idle hours, every polled account's last poll is no older than its polling
  interval plus one retry.
- **SC-008**: Every kept tally equals the sum of provider-reported input, output, cache-read and
  cache-write tokens of that interval's attempts on that account, with zero missing intervals.
- **SC-009**: A secret scan across all logs, records, errors, CLI output, plugin-visible data and
  sign-in output from the full test suite finds zero occurrences of any token, client secret,
  sign-in code or API key.
- **SC-010**: Every xai model type (text, image, video) and every grok-cli model type in its live
  list completes at least one request through a signed-in account.

## Assumptions

Items marked *(technical decision)* are technical decisions Claude made. Per the user's
direction, technical choices are Claude's to make. They are not user requirements, and they may
be revised in planning without asking the user, as long as nothing the user sees changes.

- The anthropic subscription is a signed-in account kind on the existing anthropic provider,
  not a separate provider. 9router's separate `claude` provider stays in the community set.
  *(technical decision)*
- Sign-in flows, refresh timing and refresh-failure classification follow 9router's behaviour
  for these providers (Constitution VI), except the deliberate deviations listed in research
  R19, each asserted in `tests/parity/deviations.toml`. Merging concurrent refreshes follows
  9router's dedup.
- A provider's client identity is limited to headers: fixed declared values, which may match
  the official client's (Clarifications Q3), and the core-filled values of Clarifications Q5.
  Hashes of the request and ids made to resemble another machine or account are not
  declarable. 9router's Claude subscription disguise (`claudeCloaking.js`: tool renaming, decoy
  tools, fabricated billing header, invented device and account ids) is not ported, in any
  form (Clarifications 2026-10-02).
- grok-cli needs provider-specific behaviour that 9router keeps in a custom executor. Where data
  can't express it, it becomes core built-in behaviour (Constitution I), never plugin code.
- The default polling interval is slow (on the order of minutes) and the same for every
  provider, unless a provider's reporting forces a different floor. *(technical decision)*
- Signed-in tokens are stored in `$NULLROUTER_HOME` like API-key secrets, without encryption at
  rest (brief row 18).
- Whether xai reports quota is a research question. Without a report, xai accounts show "quota
  not reported" (brief row 14).
- Slice 003's FR-034 ("exactly the chosen five") becomes seven with xai and grok-cli.

## Out of Scope

- Quota-aware routing (an account at 0% is still tried until retry and fallback move on),
  between-poll quota estimates, fitted quota weights, leak detection, and persistent request
  history: slice 006.
- Sign-in for community plugins and for other providers (Codex, Gemini CLI, Copilot, Kiro,
  Cursor and others): later.
- grok-cli bulk import: later.
- Sign-in from a web page or dashboard: later.

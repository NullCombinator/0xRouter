# Feature Specification: Routing Decision and Persistent Request History

**Feature Branch**: `006-routing-decision`

**Created**: 2026-10-03

**Status**: Draft

**Scope brief**: [specs/briefs/2026-10-03-routing-decision.md](../briefs/2026-10-03-routing-decision.md). In
clarify and plan, an answer that contradicts a confirmed row of that brief's ledger means stop and
revisit the brief.

**Input**: User description: "Routing decision and persistent request history: a target backed by several accounts spends subscription quota before it expires, keeps warm caches, and remembers every decision. Several agents can share one target backed by several accounts (subscriptions and pay-as-you-go keys, on one provider or across providers). The account, not the provider, is the unit being balanced, and direct provider/model targets with several accounts are spread the same way as unified models. Objectives, in order: waste as few tokens as possible, then spread load across accounts. Decisions use only the present state, never forecasts. Warm first: 0router keeps, per agent, fingerprints (hashes, never content) of prompt prefixes and which account holds each one cached. A request whose prefix that agent has warm on an account stays there, the shared system prompt and tools included. A warm request moves only when its account can't serve it (a rate limit or the reserve floor), or to leave a pay-as-you-go account once a subscription can serve. Each provider's plugin declares how long its prompt cache lives, and the operator can override it per account; past that idle time the prefix is cold. Cold work (no warm prefix on any account, or idle past the cache lifetime) is placed by pace over a configurable amortization window. A quota window's pace is the share of its quota left divided by the share of its time left. An account's weight is its sustainable rate times its capped pace times the operator's priority, and shares follow the weights. Each account keeps a deficit of work owed against its share, cold work goes to the largest deficit, and deficits reset at each amortization-window boundary. Short windows such as per-minute limits only admit or refuse. Windows that count requests are compared with token windows using the size of the request being placed, so large requests favour request-limited accounts. Between polls, 0router subtracts its own counted traffic from the last polled quota, and each poll corrects the estimate. An account whose provider reports no quota is paced from 0router's own count against limits its plugin declares and is shown as \"estimated\"; an account with neither is pay-as-you-go. Pay-as-you-go accounts are overflow only, used when no subscription can serve, and chosen by priority divided by the price in effect now, from a price schedule the plugin declares. The operator sets a priority per account from the CLI (0 means never for cold work), and a CLI routing view shows each account's pace, share, deficit, and whether its quota is polled or estimated. Every request record (attempts, latency, usage and the reason for each placement) is kept on disk and survives restarts and crashes, as do warm fingerprints and deficits; nothing is dropped until the operator prunes. When the operator creates or updates a unified model whose members' limits (such as context size) differ, the CLI notes it. The slice fails if: a warm session is moved for balancing rather than capacity; a subscription window resets with quota left while cold work went to pay-as-you-go or another account; the operator can't tell from the CLI or the record why a request went where it did; a restart or crash loses records, warm state or deficits; a standard client breaks on 0router; a single 429, 5xx or timeout reaches the client when another account or provider could serve; usage numbers are missing or wrong; a secret appears in a log, record, error or anything a plugin sees. Evidence: a simulated-clock test replays a week of traffic over mock accounts with real-shaped windows and checks that cold work met each window's target share, no warm session moved for balancing, pay-as-you-go served only overflow, and a restart mid-run lost nothing; an opt-in live check on the operator's real accounts shows the routing view matching the polls. Tests use more than one client harness. Constraints: plugins are data, never code, and never see secrets; plugins declare and the operator overrides; the client's request is forwarded as received; there is no runtime check that a request fits each member's limits, so a rare misfit fails and ordinary retry and fallback move on. Out of scope: fitted quota weights, leak detection and outside-use detection → slice 007; automatic responsiveness (0router lowering a slow account's share by itself) → later; a dashboard → later. Scope brief: specs/briefs/2026-10-03-routing-decision.md"

## Clarifications

### Session 2026-10-03

- Q: If the machine itself loses power or the operating system crashes (not just 0router), how
  much of the most recent request history and routing state may be lost? → A: A 0router crash
  loses nothing. A power loss or OS crash loses at most about the last second of records and
  routing state.
- Q: When the disk is full and 0router can't save records or routing state, should it keep
  serving clients or stop serving? → A: Keep serving. Records and state changes made while the
  disk is full aren't kept, and 0router warns loudly in the log, routing view and `check` until
  space returns.
- Q: Once a prompt prefix's cache has expired on the provider, should 0router delete that
  prefix's fingerprint automatically, or keep it until the operator prunes it? → A: Expired
  fingerprints are deleted automatically. Records are kept until pruned, and live fingerprints
  and deficits survive restarts and crashes.
- Q: By default, how long should an amortization window be? → A: 5 hours, the length of the
  common short subscription window. The operator can change the default and override it per
  target.

## User Scenarios & Testing *(mandatory)*

This slice has three kinds of user:

- The **operator** runs 0router. They hold several accounts (subscriptions and pay-as-you-go
  keys), declare unified models, set per-account priorities and overrides, and read the routing
  view and request records from the CLI.
- The **agent** (client) is any standard harness or SDK, as in slice 003, identified by its
  access key. Several agents may share one target. An agent must not be able to tell that a
  routing decision happened, beyond which account's cache it benefits from.
- The **plugin author** declares a provider as data: its prompt-cache lifetime, reserve floor,
  quota window sizes and limits, and price schedule.

Terms used throughout:

- A **target** is what a client names: a unified model, or a direct provider/model with one or
  more accounts behind it.
- A request is **warm** on an account when that agent sent a request with the same prompt prefix
  through that account within the cache lifetime. Otherwise it is **cold**.
- An account is a **subscription** account when its quota is known, either polled from the
  provider or estimated from plugin-declared limits. Otherwise it is **pay-as-you-go**.

### User Story 1 — Warm requests stay where their cache is (Priority: P1)

An agent working on a long task sends requests that share a growing prompt prefix. 0router
remembers, for that agent, which account holds each prefix in its cache and keeps sending the
agent there, so the provider serves the prefix from cache instead of re-reading it. A new
session or a compacted conversation that reuses the agent's system prompt and tools is also
warm, because that shared prefix is cached. The agent is moved only when its account can't
serve it: it hit a rate limit, or one of its quota windows reached the reserve floor. The one
other move is off a pay-as-you-go account once a subscription can serve.

**Why this priority**: Wasting as few tokens as possible is the first objective. A router that
moves a warm session to balance load pays to re-read the whole prefix on the new account. This
is the cache-aware routing and per-agent isolation of Constitution II.

**Independent Test**: With two subscription accounts on a mock provider that reports cache
reads, run two agents from two different client harnesses through one unified model. Each
agent's follow-up requests stay on the account that served its first request, and the mock
reports cache reads on them. Then exhaust one account's rate limit: only the agent on that
account moves, and its record names the rate limit.

**Acceptance Scenarios**:

1. **Given** an agent whose last request on account A was within A's cache lifetime, **When**
   it sends the next turn of the conversation, **Then** the request goes to A, whatever the
   current shares and deficits say, and the record gives "warm on A" as the reason.
2. **Given** an agent that finished one session on A and starts a new one with the same system
   prompt and tools within A's cache lifetime, **When** the first request of the new session
   arrives, **Then** it goes to A as warm.
3. **Given** two agents with identical system prompts, **When** agent 1 is warm on A and agent 2
   has never sent a request, **Then** agent 2's first request is cold. Agents never share warm
   state.
4. **Given** an agent warm on A, **When** A is rate-limited or one of A's windows is at its
   reserve floor, **Then** the request moves, and the record gives the capacity reason and where
   it went.
5. **Given** an agent warm on pay-as-you-go account P, **When** a subscription account can serve
   the request, **Then** the request moves to the subscription, and the record says it left
   pay-as-you-go.
6. **Given** an agent warm on A whose last request on A was longer ago than A's cache lifetime,
   **When** it sends a request, **Then** the request is cold and is placed by pace (User
   Story 2).
7. **Given** an agent whose request prefix is warm on A for its system prompt only, and on B for
   the whole conversation so far, **When** the request arrives, **Then** it goes to B, the
   account with the longest warm prefix.

---

### User Story 2 — Cold work spends subscription quota before it expires (Priority: P1)

Cold work is placed so that every subscription window is spent at its own pace before it
resets. 0router compares each account's quota left with the time left in its window, and sends
cold work where quota would otherwise expire unused. Over the amortization window, each account
receives a share of cold work in proportion to its weight. Short windows such as per-minute
limits only decide whether an account can take a request now. Large requests favour accounts
whose windows count requests rather than tokens.

**Why this priority**: Spreading load is the second objective, and quota that expires unused is
the waste a subscription holder feels. This is the windowed amortization of Constitution II.

**Independent Test**: In the simulated-clock replay (User Story 6), mock accounts with 5-hour,
weekly and request-counted windows receive a week of cold work. Each amortization window, the
cold work each account received matches its target share within tolerance, and no window resets
with quota above its floor while cold work went to pay-as-you-go or to an account whose turn it
was not.

**Acceptance Scenarios**:

1. **Given** two subscription accounts with equal priority, where A has 80% of its window left
   with 20% of the time left and B has 50% left with 50% of the time left, **When** cold work
   arrives, **Then** A receives the larger share until the paces even out.
2. **Given** an account with several windows (for example 5-hour and weekly), **When** its
   weight is computed, **Then** the tightest window rules: the lowest pace and the lowest
   sustainable rate among its windows.
3. **Given** an account whose per-minute window can't take the request now, **When** cold work
   is placed, **Then** that account is skipped for this request, and its share goes to the
   others without building up a deficit.
4. **Given** a request-counted account and a token-counted account with equal pace, **When** a
   large cold request and a small cold request are placed, **Then** the large one favours the
   request-counted account and the small one favours the token-counted account.
5. **Given** the operator sets an account's priority to 0, **When** cold work is placed, **Then**
   that account receives none. Work already warm on it stays (User Story 1).
6. **Given** the end of an amortization window, **When** the next window starts, **Then** every
   account's deficit starts again from zero.
7. **Given** two accounts with equal weight and equal deficit, **When** cold work is placed,
   **Then** the tie is broken the same way every time: higher share first, then the operator's
   account order.

---

### User Story 3 — Pay-as-you-go is overflow only (Priority: P1)

Pay-as-you-go keys cost money per token; subscriptions are already paid. 0router sends work to a
pay-as-you-go account only when no subscription account can serve it: every subscription is
rate-limited, at its reserve floor, or has priority 0 for cold work. Among pay-as-you-go
accounts, the choice follows priority divided by the price in effect now, from a price schedule
the plugin declares, so a provider's off-peak price is used when it applies.

**Why this priority**: Overflow to paid keys while subscription quota expires is a failure the
user named.

**Independent Test**: With two subscriptions and two pay-as-you-go accounts whose plugins declare
different peak and off-peak prices, replay traffic heavier than the subscriptions can take. The
pay-as-you-go accounts serve only requests made while no subscription could serve, and their
split follows priority ÷ current price, switching when the declared schedule changes price.

**Acceptance Scenarios**:

1. **Given** at least one subscription that can serve, **When** cold work arrives, **Then** no
   pay-as-you-go account receives it.
2. **Given** every subscription rate-limited or at its floor, **When** cold work arrives,
   **Then** it goes to the pay-as-you-go account with the largest priority ÷ current price, and
   the record names the overflow and why each subscription couldn't serve.
3. **Given** a plugin whose price schedule has an off-peak period, **When** the clock crosses
   into it, **Then** the next overflow decision uses the off-peak price.
4. **Given** the operator overrides an account's price, **When** overflow is placed, **Then** the
   override is used instead of the plugin's schedule.
5. **Given** every subscription that can't serve is held back only by its reserve floor, and no
   pay-as-you-go account can serve, **When** cold work arrives, **Then** it goes to a floor-held
   subscription as a last resort rather than failing the client, and the record says
   "last resort".

---

### User Story 4 — The operator sees and steers every decision (Priority: P1)

The operator sets a priority for each account from the CLI, along with per-account overrides
(cache lifetime, reserve floor, price, declared limits), and the next decision uses them
without a restart. A routing view shows, per target and account: pace, share, deficit, priority,
the quota windows with their remaining amount and reset time, and whether the quota is "polled"
or "estimated". Every request record says why each attempt went where it did, with the values
the decision used, so the operator can explain any placement from the CLI alone.

**Why this priority**: "The operator can't tell why a request went where it did" is a failure
the user named. Responsiveness is manual in this slice: the operator lowers a slow account's
priority by hand.

**Independent Test**: Run mixed warm, cold and overflow traffic. For a sample of records, the
operator reads the record and the routing view and names the reason for each attempt's account.
Recomputing each cold decision from the values in its record gives the same account.

**Acceptance Scenarios**:

1. **Given** the operator sets account A's priority to 2, **When** the next cold request is
   placed, **Then** A's weight reflects the new priority, without a restart.
2. **Given** a running server, **When** the operator opens the routing view, **Then** each
   account shows pace, share, deficit, priority, each window's remaining amount and reset time,
   and "polled" (with the last poll time) or "estimated".
3. **Given** a request whose first attempt failed and whose second attempt succeeded, **When**
   the operator shows its record, **Then** each attempt shows its account, its placement reason
   (warm, cold by deficit, moved for capacity, left pay-as-you-go, overflow, last resort, retry or
   fallback),
   the candidate accounts with the values compared, its latency and its usage.
4. **Given** a warm request that moved, **When** the operator shows its record, **Then** it names
   the account it was warm on and the capacity reason that account couldn't serve.

---

### User Story 5 — History and routing state survive restarts and crashes (Priority: P1)

Every request record (attempts, latency, usage and placement reasons) is kept on disk, as are
each agent's warm fingerprints and each account's deficit. They survive a clean restart and a
crash. No record is dropped until the operator prunes it. A fingerprint is deleted once its cache
lifetime has passed, because it can no longer make a request warm.

**Why this priority**: "A restart or crash loses records, warm state or deficits" is a failure
the user named. Lost warm state after a restart re-reads every agent's prefix; lost deficits
undo a window's balancing.

**Independent Test**: Kill the server without warning in the middle of the simulated week and
start it again. Every record of a request that finished before the kill is present, warm
agents stay on their accounts, and deficits continue from where they were.

**Acceptance Scenarios**:

1. **Given** records of finished requests, **When** 0router is killed and restarted, **Then**
   every one of them is still listed, unchanged.
2. **Given** a request in flight when 0router is killed, **When** it restarts, **Then** the
   request's record shows it as interrupted, with whatever was known up to the kill.
3. **Given** an agent warm on A before a restart, **When** its next request arrives after the
   restart and within A's cache lifetime, **Then** it goes to A as warm.
4. **Given** deficits in the middle of an amortization window, **When** 0router restarts,
   **Then** the routing view shows the same deficits as before, and placement continues from
   them.
5. **Given** no server running, **When** the operator lists or shows records, **Then** the CLI
   reads them from disk.
6. **Given** the operator prunes records older than a date, **When** pruning completes, **Then**
   only those records are removed, and the CLI says how many.

---

### User Story 6 — A simulated week proves the routing (Priority: P1)

A simulated-clock test replays a week of traffic over mock accounts whose windows are shaped
like real ones (5-hour and weekly subscription windows, per-minute limits, request-counted
windows, pay-as-you-go with a price schedule). Several agents send warm and cold traffic. The
test checks that cold work met each window's target share, that no warm session moved for
balancing, that pay-as-you-go served only overflow, and that a restart mid-run lost nothing. An
opt-in live check on the operator's real accounts shows the routing view matching the polls.

**Why this priority**: The routing rules only matter over windows that last hours and days. A
week can't be tested in real time, and the live check proves the view reflects what the
providers report.

**Independent Test**: The replay runs in minutes from a fixed seed and gives the same result
every time.

**Acceptance Scenarios**:

1. **Given** the simulated week, **When** it completes, **Then** every check above passes, and
   the report lists each window's target share, actual share and remaining quota at reset.
2. **Given** a restart injected mid-run between requests, **When** the run completes, **Then**
   every placement after the restart is the same as in an uninterrupted run.
3. **Given** the operator's real accounts, **When** they run the opt-in live check, **Then** the
   routing view's remaining quota for each polled account equals the provider's latest report,
   less the traffic 0router counted since that poll.

---

### User Story 7 — Quota is estimated between polls and without a quota report (Priority: P2)

Between polls, 0router subtracts the traffic it sent from the last polled quota, so decisions
don't run on stale numbers. Each poll replaces the estimate. A provider that reports no quota is
paced from 0router's own count against limits its plugin declares, and the routing view shows it
as "estimated". An account with neither a report nor declared limits is pay-as-you-go.

**Why this priority**: Polls are minutes apart, and a busy agent can spend a large part of a
window between two of them. Providers without a quota report would otherwise fall out of pacing
entirely.

**Independent Test**: A mock provider reports quota on demand. Between polls, the routing view's
remaining amount equals the last report less 0router's counted traffic. At the next poll, it
equals the new report. An account on a provider with declared limits and no report shows
"estimated" and is paced; one with neither is used as pay-as-you-go.

**Acceptance Scenarios**:

1. **Given** a polled account, **When** 0router sends traffic through it between polls,
   **Then** its remaining quota drops by that traffic, weighted in the window's own unit.
2. **Given** an estimate that differs from the next poll, **When** the poll arrives, **Then** the
   poll's value replaces the estimate.
3. **Given** a window whose reset time passes between polls, **When** the next decision is
   made, **Then** the window counts as reset, until the next poll confirms or corrects it.
4. **Given** a provider with no quota report and plugin-declared limits, **When** the operator
   opens the routing view, **Then** the account shows "estimated" and its pace from 0router's own
   count.
5. **Given** a provider with neither a quota report nor declared limits, **When** cold work is
   placed, **Then** the account is treated as pay-as-you-go.

---

### User Story 8 — Direct targets and mixed-limit unified models (Priority: P3)

A client that names a direct provider/model with several accounts behind it gets the same
routing as a unified model: warm first, then pace. When the operator creates or updates a
unified model whose members declare different limits (such as context size or maximum output),
the CLI notes the difference when the configuration is checked or reloaded. 0router doesn't
check at request time whether a request fits each member; a rare misfit fails, and ordinary
retry and fallback move on.

**Why this priority**: Direct targets are common in harness configs. The note is a one-time
warning that saves the operator from a surprise, at no runtime cost.

**Independent Test**: A direct target with three accounts on one provider shows warm and cold
placement identical to a unified model of the same accounts. A unified model whose members
declare 200k and 128k context produces a note in the check and reload output, and the
configuration still loads.

**Acceptance Scenarios**:

1. **Given** `provider/model` with three accounts, **When** agents send warm and cold work,
   **Then** placement follows the same rules as for a unified model, and the routing view shows
   the direct target.
2. **Given** a unified model whose members declare different context sizes, **When** the
   operator checks or reloads the configuration, **Then** the output names the unified model, the
   limit, and each member's value, and the model loads.
3. **Given** a request too large for the member it was placed on, **When** that member rejects
   it, **Then** retry and fallback move to another member, as for any non-transient refusal in
   slice 003.

---

### Edge Cases

- **Warm on several accounts**: the account with the longest warm prefix wins. Equal lengths
  fall to the most recently used account.
- **Warm account becomes unusable** (removed, disabled, needs sign-in, refused by provider):
  its warm prefixes no longer count, and the request is placed as if warm elsewhere or cold. The
  record says why.
- **Warm account with priority 0**: warm work stays there. Priority affects cold work only.
- **Every account blocked, no pay-as-you-go**: a subscription held back only by its reserve
  floor serves as a last resort. When nothing can serve, the client gets slice 003's
  informational error, which lists each account and why it couldn't serve (including priority 0,
  reserve floor and per-minute limits).
- **Poll failing for a long time**: the estimate keeps running from 0router's own count, the
  routing view shows the age of the last good poll and the failure, and the account keeps its
  "polled" label with a "stale" mark.
- **Quota spent outside 0router**: the estimate is too high until the next poll corrects it.
  Detecting outside use is slice 007.
- **Account added, re-enabled or reprioritised mid-window**: its deficit starts at zero, and
  shares are recomputed for the next decision.
- **Amortization window boundary during a request**: the request is debited in the window where
  it was placed.
- **Concurrent cold requests**: two requests arriving together don't both go to the same account
  just because they saw the same largest deficit. Each placement accounts for the others already
  placed.
- **System clock jumps**: window timing tolerates a jump without a burst of placements to one
  account.
- **Crash during a write**: a half-written record or state entry never corrupts the store. At
  most the interrupted entry is dropped, and the request it belonged to is marked interrupted.
- **Disk full**: 0router keeps serving clients, routing from the state it holds in memory. It
  warns in the log, the routing view and `check` that records and routing state are not being
  kept, with the count of unkept requests, until space returns. Saving resumes on its own.
- **Request without a cacheable prefix** (for example an embedding or image request): it is
  placed as cold work. Every model type uses the same placement rules (slice 003 FR-011).
- **Request-counted window with a tiny request**: the request size used to compare windows is
  never zero.
- **Prompt content in fingerprints**: only hashes are stored. A prefix can't be recovered from
  the store.

## Requirements *(mandatory)*

### Functional Requirements

**Scope of the decision**

- **FR-001**: Routing MUST balance accounts, not providers. Every account behind a target, on one
  provider or across providers, subscription or pay-as-you-go, is a candidate.
- **FR-002**: Direct provider/model targets with several accounts MUST be routed by the same
  rules as unified models.
- **FR-003**: Decisions MUST pursue, in order: wasting as few tokens as possible, then spreading
  load across accounts.
- **FR-004**: Decisions MUST use only present-state quantities: quota left, window size, time
  left, request size, declared price, priority and cache state. No forecast of future demand may
  influence a decision.
- **FR-005**: Every model type MUST use the same routing rules.

**Warm placement**

- **FR-006**: 0router MUST keep, per agent, fingerprints of the prompt prefixes it sent and which
  account holds each one cached, with the time it was last used. Fingerprints MUST be hashes; no
  prompt content may be stored for routing.
- **FR-007**: Warm state MUST be isolated per agent. One agent's fingerprints MUST NOT make
  another agent's request warm.
- **FR-008**: A request with a prefix that agent has warm on an account MUST go to the account
  with the longest warm prefix, the shared system prompt and tools included.
- **FR-009**: A warm request MUST move only when its account can't serve it (a rate limit, a
  window at or below its reserve floor, or the account being out of service), or to leave a
  pay-as-you-go account once a subscription can serve. Shares and deficits MUST NOT move warm
  work.
- **FR-010**: Each provider plugin MUST be able to declare its prompt-cache lifetime. The
  operator MUST be able to override it per account. A prefix idle longer than its account's
  cache lifetime MUST count as cold.
- **FR-011**: Computing fingerprints MUST NOT change the request. 0router MUST NOT add, move or
  remove cache markers or any other content (slice 005 FR-004c stays the only exception).

**Cold placement**

- **FR-012**: Cold work (no warm prefix on any account, or every warm prefix idle past its cache
  lifetime) MUST be placed by pace over an amortization window. The window MUST default to 5
  hours. The operator MUST be able to change the default and override it per target.
- **FR-013**: For each quota window, pace MUST be the share of its quota left divided by the
  share of its time left, and sustainable rate MUST be the quota left divided by the time left.
  For an account with several windows, the lowest pace and the lowest sustainable rate rule.
- **FR-014**: An account's weight MUST be its sustainable rate, times its pace capped at an upper
  bound, times the operator's priority. Weight MUST be 0 when a window is at or below its reserve
  floor, a short window refuses the request, or the priority is 0. Shares MUST be the weights
  divided by their sum, recomputed whenever state changes.
- **FR-015**: Each account MUST keep a deficit of work owed against its share. When an account
  serves work of size T, every eligible account's deficit rises by its share of T and the serving
  account's falls by T. Cold work MUST go to the eligible account with the largest deficit. An
  account with share 0 MUST NOT build up a deficit. Deficits MUST stay within a bound, and MUST
  reset at each amortization-window boundary.
- **FR-016**: Ties MUST be broken deterministically: higher share first, then the operator's
  account order.
- **FR-017**: Short windows (such as per-minute request or token limits) MUST only admit or
  refuse a request. They MUST NOT change pace or shares.
- **FR-018**: Windows that count requests MUST be compared with token windows by converting
  their sustainable rate into tokens using the size of the request being placed.
- **FR-019**: A window reported only as a percentage MUST take its size from the plugin's
  declaration, which the operator can override per account.
- **FR-020**: Each plugin MUST be able to declare a reserve floor per window. The operator MUST
  be able to override it per account.

**Quota between polls, and estimated accounts**

- **FR-021**: Between polls, an account's remaining quota MUST be the last polled value less the
  traffic 0router sent through it since that poll, weighted in the window's own unit. Each poll
  MUST replace the estimate.
- **FR-022**: When a window's reset time passes between polls, it MUST count as reset until the
  next poll confirms or corrects it.
- **FR-023**: An account whose provider reports no quota MUST be paced from 0router's own count
  against limits its plugin declares (window length and size), which the operator can override,
  and MUST be shown as "estimated".
- **FR-024**: An account with neither a quota report nor declared limits MUST be treated as
  pay-as-you-go.

**Pay-as-you-go overflow**

- **FR-025**: A pay-as-you-go account MUST receive cold work only when no subscription account
  can serve it.
- **FR-025a**: When no subscription can serve above its reserve floor and no pay-as-you-go
  account can serve, a subscription account held back only by its reserve floor MUST serve as a
  last resort rather than fail the client.
- **FR-026**: Among pay-as-you-go accounts, overflow MUST go by priority divided by the price in
  effect now. Each plugin MUST be able to declare a price schedule (prices that change by time
  of day or day of week). The operator MUST be able to override the price per account.

**Operator control and visibility**

- **FR-027**: The operator MUST be able to set a priority per account from the CLI. Priority 0
  means the account never receives cold work. The default is 1.
- **FR-028**: Changes to priority, overrides and the amortization window MUST apply to the next
  decision without a restart, through the existing reload path.
- **FR-029**: A CLI routing view MUST show, per target and per account: pace, share, deficit,
  priority, the cache lifetime in effect, each window's remaining amount, unit, reset time and
  reserve floor, and whether quota is "polled" (with the last poll time, and "stale" when polls
  are failing) or "estimated". Pay-as-you-go accounts MUST show their current price.
- **FR-030**: Each attempt in a request record MUST carry its placement reason (warm, cold by
  deficit, moved for capacity, left pay-as-you-go, overflow, last resort, retry, fallback) and the
  values the decision compared: for cold and overflow decisions, each candidate's pace, share,
  deficit, weight or price; for warm decisions, the warm account, prefix length and idle time; for moves,
  the capacity reason.
- **FR-031**: When the configuration is checked or reloaded, the CLI MUST note each unified model
  whose members declare different limits (such as context size or maximum output), naming the
  limit and each member's value. The note MUST NOT stop the model from loading.
- **FR-032**: 0router MUST NOT check at request time whether a request fits each member's limits.
  A member that refuses a request for its size is handled by ordinary retry and fallback.

**Persistence**

- **FR-033**: Every request record (attempts, latency including time to first token and total
  duration, usage and placement reasons) MUST be kept on disk.
- **FR-034**: Records, warm fingerprints and deficits MUST survive a clean restart and a 0router
  crash. No record of a request that finished before a 0router crash may be lost. A power loss or
  operating-system crash MAY lose at most about the last second of records and routing state. A
  request in flight at a crash MUST appear as interrupted after restart.
- **FR-035**: No record MUST be dropped until the operator prunes it. The operator MUST be able to
  prune records older than a date, and an account's or agent's history, from the CLI. A warm
  fingerprint MUST be deleted automatically once its account's cache lifetime has passed since
  its last use. Deficits are replaced at each amortization-window boundary.
- **FR-036**: The operator MUST be able to list and show records from the CLI whether or not the
  server is running.
- **FR-037**: A crash during a write MUST NOT corrupt stored records or routing state.
- **FR-037a**: When records or routing state can't be saved (for example, a full disk), 0router
  MUST keep serving clients and MUST warn in the log, the routing view and `check`, with the
  number of requests not kept, until saving works again. Saving MUST resume without a restart.

**Standing requirements**

- **FR-038**: Every standard client that works on 0router today MUST keep working. The client's
  request MUST be forwarded as received (Constitution IV).
- **FR-039**: A single 429, 5xx, timeout or connection failure MUST NOT reach the client when
  another account or provider could serve. Each retry and fallback attempt is placed by these
  rules, excluding accounts already tried. An account with priority 0 can't serve cold work, so
  it is never a fallback: when only priority-0 accounts remain, the client gets the error, which
  names them with the reason "priority 0".
- **FR-040**: Usage numbers in records and in the client's response MUST match what the provider
  reported.
- **FR-041**: No secret may appear in any log, record, error, CLI output, routing view or data a
  plugin can see.
- **FR-042**: The new plugin declarations (cache lifetime, reserve floor, window sizes and limits,
  price schedule) MUST be data, MUST pass the validation gate, and MUST NOT give a plugin access
  to secrets, the network or the filesystem.

### Key Entities

- **Target**: a unified model, or a direct provider/model, that a client names. Has its candidate
  accounts and an amortization window.
- **Account** (extended from slice 005): adds a priority, overrides (cache lifetime, reserve
  floor, window sizes and limits, price), and a kind derived from its quota source: polled,
  estimated, or pay-as-you-go.
- **Quota window**: one limit on an account, polled or declared: length, size and unit (tokens,
  requests, percent, credits), remaining amount (polled or estimated), reset time, reserve
  floor. Short windows only admit or refuse.
- **Warm fingerprint**: per agent, a hash of a prompt prefix, the account holding it cached, its
  length, and when it was last used.
- **Deficit**: per target and account, the work (in tokens) owed against its share in the
  current amortization window.
- **Placement decision**: per attempt, the reason and the values compared.
- **Request record** (extended from slice 003): kept on disk, with attempts, latency, usage,
  placement decisions, and an interrupted mark when a crash cut it short.
- **Routing declarations** (plugin data): prompt-cache lifetime, reserve floors, window sizes and
  limits, price schedule.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: In the simulated week, for every account and amortization window, the cold work the
  account received differs from its target by at most 5% of all the cold work placed in that
  window, or by the window's largest cold request if that is more. The target is the sum, over the window's cold placements, of the account's share at
  that moment times the work placed. Work is counted in tokens (input, cache read, cache write
  and output, unweighted), the same for every account.
- **SC-002**: In the simulated week and in harness tests, 0 warm requests are moved for any
  reason other than capacity or leaving pay-as-you-go, and every move's record names its reason.
- **SC-003**: In the simulated week, pay-as-you-go accounts receive 0 requests at moments when a
  subscription could serve.
- **SC-004**: In the simulated week, no subscription window of an account with priority above 0
  resets with quota above its reserve floor while cold work in that window went to
  pay-as-you-go, or to another account while this account held the largest deficit and could
  serve. When cold demand exceeds the subscriptions'
  combined capacity, every subscription window resets within 5% of its capacity of its reserve
  floor.
- **SC-005**: After a kill without warning at a random point in the simulated week, 100% of
  records of requests that finished before the kill are present, every agent's warm state and
  every deficit match their pre-kill values, and, for a restart between requests, every later
  placement equals an uninterrupted run's. In a simulated power loss, at most the last second of
  records and routing state is missing, and nothing older.
- **SC-006**: For 100% of cold and overflow attempts in the simulated week, recomputing the
  decision from the values in its record selects the same account.
- **SC-007**: In the opt-in live check, the routing view's remaining quota for every polled
  account equals the provider's latest report less 0router's counted traffic since that poll, and
  equals the report exactly right after a poll.
- **SC-008**: Against a mock provider with no outside use, the between-poll estimate equals the
  provider's own figure at every poll.
- **SC-009**: At least two client harnesses in different API styles complete warm, cold and
  overflow scenarios with zero client-side errors, and the mock provider reports cache reads on
  every warm request.
- **SC-010**: In failure-injection tests, 0% of injected 429, 5xx, timeout and connection
  failures reach the client when another account or provider could serve.
- **SC-011**: Every recorded usage figure equals the provider-reported usage of its attempt.
- **SC-012**: A scan of all logs, records, routing views, CLI output, stored routing state and
  plugin-visible data from the full test suite finds zero secrets and zero prompt text.
- **SC-013**: The routing decision and persistent recording together add no more than 5 ms at the
  95th percentile to a request, measured against slice 005 on the same machine.

## Assumptions

Items marked *(technical decision)* are technical decisions Claude made. Per the user's
direction, technical choices are Claude's to make. They are not user requirements, and they may
be revised in planning without asking the user, as long as nothing the user sees changes.

- An agent is identified by its access key, as in slice 003.
- When several accounts hold a warm prefix, the longest one wins, because it saves the most
  tokens (objective 1). *(technical decision)*
- Which prefixes become warm follows the serving provider's declared cache mode: explicit cache
  markers, automatic prefix caching, or none (plan research R3). *(technical decision)*
- Deficits are kept per target and account, because shares are computed among a target's
  candidates. Quota windows belong to the account and are shared by every target it serves.
  *(technical decision)*
- The pace cap (for example 10), the deficit bound, and the default reserve floor are technical
  values chosen in planning. *(technical decision)*
- For percentage windows, the declared size uses the token categories and weights of slice 005's
  quota meter. Fitting those weights from history is slice 007. *(technical decision)*
- The routing declarations are pure data, so any plugin may declare them, unlike slice 005's
  quota endpoints, which only chosen providers may use. *(technical decision)*
- The existing per-account order becomes the final tie-break. Unified-model member order no
  longer decides placement. *(technical decision)*
- Deficits count work in plain tokens (input, cache read, cache write and output, unweighted),
  so accounts on different providers are compared in one unit. Each window's own meter weights
  apply only to that window's remaining quota. *(technical decision, made at analyze)*
- Three choices settled at analyze on 2026-10-03, when the user delegated the fixes to Claude.
  The user can overrule any of them:
  - A subscription held back only by its reserve floor serves as a last resort, after
    pay-as-you-go, rather than failing the client (FR-025a).
  - Priority 0 outranks FR-039: a priority-0 account is never a fallback for cold work.
  - SC-001's tolerance is 5% of the window's total cold work, not 5% of each account's target,
    because a relative tolerance on a small share is mostly noise. It is never tighter than the
    window's largest cold request: a request is placed whole, so a window of a few requests can't
    be split closer than one of them.
- The per-agent warm map of slice 003 (last-serving account per agent and target, in memory) is
  replaced by persistent prefix fingerprints.
- 9router's account selection (fill-first and round-robin with a sticky limit) and combo
  fallback inform only tie-breaking and equal-state behaviour. There is no parity target for the
  placement rule itself.

## Out of Scope

- Fitted quota weights, leak detection and outside-use detection: slice 007.
- Automatic responsiveness (0router lowering a slow account's share by itself): later. In this
  slice, the operator adjusts priority by hand.
- A dashboard: later. The routing view is CLI only.
- A runtime check that a request fits each member's limits: never planned (brief row 26).

# Feature Specification: Quota Fit, Outside Use and Leak Detection

**Feature Branch**: `012-quota-fit`

**Created**: 2026-10-07

**Status**: Draft

**Scope brief**: [specs/briefs/2026-10-07-quota-fit.md](../briefs/2026-10-07-quota-fit.md).
In clarify and plan, an answer that contradicts a confirmed row of that brief's ledger means
stop and revisit the brief.

**Input**: User description: "Quota fit, outside use and leak detection: 0router learns each account's real quota costs from the provider's polls and corrects a plugin's wrong numbers by itself, never mistakes usage it didn't cause for a wrong plugin, and can tell the operator when an account declared as used only through 0router loses quota 0router didn't spend. Polls are the source of truth: a plugin's wrong numbers must not fool the router for long, though a one-off misplaced session is tolerated. The fit covers every number a plugin's quota meter declares that polls can reveal: each window's capacity, the four token weights (input, output, cache read, cache write) and the per-model multipliers. Token weights and model multipliers are learned across all accounts of one plugin; capacity is learned per account; an account whose own polls disagree significantly with the pooled value is learned on its own, and the CLI says so. A mismatch between polls and 0router's own counted traffic is classified by its pattern against the fitted model: one that grows with 0router's tokens and points consistently one way is a wrong number and is corrected; one that doesn't track 0router's traffic, including quota that drops while 0router sent nothing, is outside use, is left out of the fit, and is never blamed on the plugin. Every estimate carries a 95% range, a significance test decides when to act, and the polls' whole-percent rounding is the noise the test accounts for. Until a correction is significant, routing behaves exactly as it does today; once it is, routing uses the fitted number. The operator can override every fitted number, token weights and model multipliers included, and the override always wins: operator override, then significant fit, then the plugin's declaration. The plugin's file is never rewritten. If recent polls disagree significantly and consistently with a fitted value, 0router concludes the provider changed its rules, falls back to the declared or overridden number, relearns from that point and says so; a fit also restarts when the plugin's declared meter changes, and fits and their history survive restarts. Outside-use detection runs for every polled account and lists the intervals it found, without alerting. Leak alerts are opt-in per account: the operator declares from the CLI that an account is used only through 0router, and only such an account can raise an alert, only for unexplained use beyond the rounding noise; the alert states facts (when and how much), never claims a key leaked, and takes no action on the account. Per account and window, the CLI routing view shows the plugin's declared value, the fitted range, which number routing uses now and why, and each number's state: not enough evidence yet (with progress), fitted, split off, or relearning since a given time; a number the traffic can't separate stays at the plugin's value and the view says why. Leak alerts and detected rule changes also go to the serve log. Between polls, the estimate stays the last poll minus 0router's own counted traffic, with no forecast of outside use. The slice fails if: a plugin whose numbers are right gets corrected; outside use pulls a fitted number away from the truth; a leak alert fires on an account not declared as used only through 0router, or within rounding noise; routing places cold work worse than today while the fit is still learning. Evidence: a simulated week at 10-minute, whole-percent polls with a mock provider whose real weights are 3× its plugin's (corrected), one whose plugin is right (never corrected), injected outside use (moves no fitted number), and idle-time drops plus rounding-sized noise (alerts only on the drops, only on opted-in accounts); capacity and the input and output weights must reach significance within the week; an opt-in live check on the operator's real accounts reports how far the fit got, with no time target. Constraints: constitution Principle II as amended before this slice (plugins declare, polls correct once the evidence is significant, the operator overrides both); plugins are data and never see secrets; decisions use the present state, never forecasts; fitted values and outside-use intervals never leave the operator's machine. Out of scope: the fit, outside use and alerts on the dashboard → later; fitting accounts whose provider reports no quota, and learning limits from rate-limit refusals → later; pausing or disabling an account on a leak signal → not planned; forecasting outside use between polls → later, if the live check shows it matters; Jev integration → later; sending fitted values anywhere → not planned; exporting a fitted meter as plugin TOML for authors → later. Scope brief: specs/briefs/2026-10-07-quota-fit.md"

## Clarifications

### Session 2026-10-07

- Q: On a percent window, the polls can't tell "every token costs 3× more" apart from "the window
  is 3× smaller", so which number should 0router hold fixed as the yardstick so the others can be
  learned? → A: The input weight. On percent windows it stays at its declared or overridden value
  and is never fitted; capacity is learned per account, and the other three token weights and the
  model multipliers are learned relative to input. Counted and balance windows report absolute
  units and need no yardstick. Brief row 22 amended to match.
- Q: How strict should the test be that decides a correction is real, given that 0router re-checks
  it at every poll for weeks? → A: Ranges stay 95%, but routing acts only when a test that stays
  valid under checking at every poll rejects the declared value, with at most a 0.1% chance over
  a number's whole life of falsely correcting it. SC-002 keeps "0 in 100 seeded weeks".
- Q: On an account you've declared as used only through 0router, should a leak alert also fire
  when outside use happens while 0router is sending traffic, or only during idle time and as a
  steady rate? → A: Also during traffic, when the excess beyond what the fit can explain passes
  FR-010's test. Idle drops still alert as soon as they exceed rounding noise.
- Q: Do you accept the seven items the spec chose on its own (SC-001 10%, SC-006 1 day, SC-002 100
  weeks, SC-004 90%, alert acknowledgement, per-record meter sources, refusal on unpolled
  accounts)? → A: Yes, all seven as written.
- Q: Should token-weight and model-multiplier overrides be set per account, per plugin (covering
  all its accounts), or both? → A: Both. A plugin-level override covers all the plugin's accounts;
  an account-level override wins over it. Capacity overrides stay per account.

## User Scenarios & Testing *(mandatory)*

This slice has two kinds of user:

- The **operator** runs 0router. They hold subscription accounts whose providers report quota
  when polled (slice 005), read the routing view (slice 006), set overrides, and decide which
  accounts they use only through 0router.
- The **plugin author** declares a provider's quota meter as data: per window, its capacity,
  token weights and model multipliers (slice 006). Nothing changes for them: their file is read
  as before and never written.

Clients see no new surface. What changes for them is where their cold work lands once a
correction is significant.

Terms used throughout:

- A **meter number** is one number a plugin's quota meter declares for one window: the window's
  capacity, one of its four token weights (input, output, cache read, cache write), or one of its
  per-model multipliers.
- A **poll interval** is the time between two consecutive good polls of one account. For each
  interval, slice 006 already keeps 0router's own counted traffic by model and token kind, and
  the two polls give how much of each window the provider says was used.
- **Explained use** is the share of a window that 0router's own traffic in an interval should
  have used, costed by the meter numbers in effect. **Unexplained use** is what the polls show
  beyond that.
- The **fit** is 0router's estimate of each meter number from poll intervals, with a 95% range.
- The **yardstick** is the input weight of a percent window. A percent poll only shows the cost of
  the traffic divided by the capacity, so scaling every weight and the capacity by the same factor
  leaves every poll unchanged. The yardstick holds the input weight at its declared or overridden
  value, so the other numbers can be learned relative to it. A plugin whose weights are all off by
  the same factor therefore shows up as a capacity correction, with the same placements.
- A correction is **significant** when the fit's evidence rejects the declared value under the
  significance test of FR-010. Before that, the number is **learning**. A capacity the provider
  reports exactly (a counted window's `limit`) goes through the same test, with one unit as its
  rounding.
- **Outside use** is unexplained use that doesn't track 0router's traffic: use while 0router sent
  nothing, or use beyond what any fitted meter could explain from 0router's traffic.
- A **break** is a change in a provider's rules, seen as recent polls that disagree significantly
  and consistently with a fitted number.
- An **exclusive-use account** is one the operator has declared used only through 0router. Only
  such an account can raise a **leak alert**. The operator sees it as a **usage alert**: its name
  and text state what was used, never a cause (FR-025).
- **Rounding noise** is what a provider's reporting resolution can produce with no real use: a
  reading can move by at most one resolution step (one percentage point on a percent window, one
  unit on a counted window) from rounding alone.

### User Story 1 — A plugin's wrong numbers get corrected, and right ones are left alone (Priority: P1)

An account's plugin says its 5-hour window holds 9 million weighted tokens and that output
counts 5× input. In reality the provider charges output 15×. Each poll shows the window draining
faster than 0router's count predicts, in proportion to the output 0router sent. 0router fits the
real weight from the polls, and once the evidence is significant, routing costs that account's
requests with the fitted weight. Meanwhile, an account whose plugin is right is never
"corrected", however long it runs.

Token weights and model multipliers are the provider's pricing rules, so they are learned across
all accounts of one plugin. Capacity depends on the plan, so it is learned per account. An
account whose own polls disagree significantly with the pooled weights is split off and learned
on its own, and the routing view says so.

**Why this priority**: This is the slice. Polls are the source of truth (Constitution II as
amended): a plugin's wrong numbers must not fool the router for long. A correction that fires on
a right plugin is the failure the user named first.

**Independent Test**: On a simulated clock, run a week of traffic at 10-minute, whole-percent
polls against two mock providers: one whose real output and cache weights are 3× its plugin's
relative to input, and whose real capacity differs from its plugin's, and one whose plugin is
exactly right. The first is corrected, with ranges that contain the true values; the
second is never corrected.

**Acceptance Scenarios**:

1. **Given** a plugin whose real output weight is 3× the declared one, **When** a simulated week
   of mixed traffic runs, **Then** the output weight becomes significant within the week, its
   95% range contains the true value, and routing uses the fitted value from then on.
2. **Given** a plugin whose numbers are all right, **When** the same week runs, **Then** no
   number ever becomes significant, and every placement equals the placement 0router makes
   without this slice.
3. **Given** a window whose capacity the plugin understates by half, **When** the week runs,
   **Then** that account's capacity becomes significant within the week and routing paces the
   account against the fitted capacity.
4. **Given** three accounts of one plugin with the same real weights and different plan
   capacities, **When** traffic runs on all three, **Then** the token weights are learned once
   from all three accounts' polls, and each account's capacity is learned on its own.
5. **Given** one of those accounts is actually charged differently (for example a promotion),
   **When** its polls disagree significantly with the pooled weights, **Then** it is split off
   and learned on its own, the routing view marks it "split off" with the reason, and the other
   accounts' pooled fit no longer uses its polls.
6. **Given** a number that has reached significance, **When** a request is placed, **Then** its
   record names which number was used and that it came from the fit.

---

### User Story 2 — Outside use never counts against the plugin (Priority: P1)

The operator also uses their Claude subscription in claude.ai, and a teammate shares one account.
The polls show quota leaving that 0router didn't spend. 0router recognises that use as outside
use: it doesn't track 0router's traffic. It leaves it out of the fit, so no meter number moves
because of it, and lists the intervals where it found it. This happens on every polled account,
whether or not the operator opted in to anything, and raises no alert.

**Why this priority**: Without it, User Story 1 fails in practice: outside use looks to a naive
fit like a plugin that under-counts. The user named "outside use pulls a fitted number off the
truth" as a failure.

**Independent Test**: Run the 3×-off and the right-plugin mocks of User Story 1 again with
outside use injected (bursts while 0router is idle, a steady background rate, and bursts during
0router's traffic). The fitted numbers end where they ended without outside use, and the
injected intervals beyond rounding noise appear in the outside-use list.

**Acceptance Scenarios**:

1. **Given** a window that drops 6% during an interval in which 0router sent nothing, **When**
   the next poll arrives, **Then** that interval is listed as outside use (time and amount) and
   no meter number changes because of it.
2. **Given** a steady outside-use rate on a right plugin, **When** a week runs, **Then** no
   number becomes significant, and the routing view reports a steady unexplained rate for the
   account.
3. **Given** outside use during an interval in which 0router also sent traffic, **When** the use
   exceeds what any meter within the fit's range could explain, **Then** the excess is listed as
   outside use and left out of the fit.
4. **Given** an account that is not an exclusive-use account, **When** outside use is found,
   **Then** it is listed and nothing else happens: no alert, no log line beyond the routine poll
   record.
5. **Given** a drop of one resolution step during an idle interval, **When** the next poll
   arrives, **Then** it is treated as rounding noise and not listed.

---

### User Story 3 — The operator sees which number routing uses, and why (Priority: P2)

The operator opens the routing view. For each account and window, next to the plugin's declared
value, they see the fitted range, which number routing uses right now (override, fit or
declaration) and why, and the state of each number: learning (with its progress), fitted, split
off, or relearning since a given time. A number the traffic can't separate (for example cache
writes, when the operator's traffic never varies them independently) stays at the plugin's value
and the view says why. The outside-use intervals are listed per account.

**Why this priority**: The outcome the user confirmed is that the operator can see the
correction with its range. Without the view, a correction is invisible and a learning fit looks
like nothing happening.

**Independent Test**: From one routing-view call during the simulated week, the operator can
tell for every meter number of every polled account which value is in use, where it came from,
and how far its fit has progressed.

**Acceptance Scenarios**:

1. **Given** a number still learning, **When** the operator reads the routing view, **Then** it
   shows the declared value as in use, "learning", the number of poll intervals used and the
   current width of the range (for example "312 intervals, output weight ±40%").
2. **Given** a fitted number, **When** the operator reads the view, **Then** it shows the
   declared value, the fitted 95% range, the fitted value as in use and "fitted since <time>".
3. **Given** a number the traffic can't separate from another, **When** the operator reads the
   view, **Then** it shows the declared value as in use and names the number it can't be told
   apart from.
4. **Given** the machine-readable form of the view, **When** it is read, **Then** it carries the
   same facts per number: declared, override, fitted range, in use, source, state, progress.
5. **Given** an account with no quota reports ("estimated" in slice 006), **When** the operator
   reads the view, **Then** it shows the account as before, with "not fitted: provider reports no
   quota".

---

### User Story 4 — The operator's override wins over everything (Priority: P2)

The operator knows their plan's real numbers, or distrusts a fit. They override any meter number
for an account from the CLI, token weights and model multipliers included, as they can already
override a window's capacity, length and reserve. Their override is used whatever the fit says.
The fit keeps learning underneath, so the view still shows what the polls say.

**Why this priority**: Plugins declare, polls correct, the operator overrides both
(Constitution II as amended). Today the operator can't override token weights or model
multipliers at all.

**Independent Test**: Override the output weight on the 3×-off mock before its fit becomes
significant and after. Routing uses the override in both cases; the view shows the override in
use and the fitted range beside it.

**Acceptance Scenarios**:

1. **Given** a fitted output weight, **When** the operator overrides it, **Then** the next
   placement uses the override, and the view shows "override" as the source with the fitted
   range still visible.
2. **Given** an override, **When** the operator removes it, **Then** routing uses the significant
   fit if there is one, otherwise the plugin's declaration.
3. **Given** an override of a model multiplier for one model pattern, **When** a request for a
   matching model is placed, **Then** the override applies to it, and to no other model.
4. **Given** an override value that breaks the rules the plugin's own value must follow, **When**
   the operator sets it, **Then** it is refused with the same wording the plugin check uses, as
   for today's overrides.
5. **Given** a plugin-level override of the output weight and an account-level override of the
   same weight on one of the plugin's accounts, **When** requests are placed, **Then** that
   account uses its own override and every other account of the plugin uses the plugin-level
   one.

---

### User Story 5 — When the provider changes its rules, 0router notices and relearns (Priority: P2)

The provider halves its weekly limit on a Tuesday. The fit has weeks of evidence for the old
capacity. Recent polls now disagree with it significantly and in one direction. 0router concludes
the rules changed, stops using the fitted number, falls back to the override or the declaration,
relearns from the break, and says "provider rules changed around Tue 14:00, relearning" in the
routing view and the server log.

**Why this priority**: Without it, a confident old fit keeps routing on a wrong number for weeks,
which is the user's first failure (a wrong number in use) by a different route.

**Independent Test**: In the simulated week, halve one account's real capacity on day 4. The
break is reported, routing stops using the old fitted capacity, and the fit starts again from the
break.

**Acceptance Scenarios**:

1. **Given** a significant fitted capacity, **When** the provider's real capacity halves, **Then**
   the break is reported within one simulated day, and routing uses the override or declaration
   from then until a new fit is significant.
2. **Given** a break, **When** the fit relearns, **Then** only poll intervals after the break are
   evidence for the new fit.
3. **Given** a plugin update that changes the declared meter of a window, **When** it loads,
   **Then** every fitted number of that window restarts from nothing, and the view says
   "restarted: plugin meter changed".
4. **Given** a burst of outside use, **When** polls disagree with the fit, **Then** it is not a
   break: a break needs disagreement that tracks 0router's traffic, as for any correction.
5. **Given** a restart or crash of the server, **When** it comes back, **Then** every fit, its
   state, its history and the outside-use list are as they were.

---

### User Story 6 — Opt-in leak alerts on accounts used only through 0router (Priority: P3)

The operator declares from the CLI that their Claude Max account is used only through 0router.
One night, while 0router sent nothing, the account's weekly window drops 4%. 0router raises a
leak alert: "anthropic/max: 4% of weekly used 02:10–02:30 with no traffic from 0router". It
appears in `nullrouter check`, the routing view and the server log. 0router does nothing else to
the account. Whether it is a leaked key or a forgotten browser tab is the operator's call.

**Why this priority**: It is a thin layer over outside-use detection (User Story 2), and it only
serves operators who opt in.

**Independent Test**: In the simulated week, inject idle-time drops and rounding-sized noise on
two accounts, one declared exclusive-use and one not. Alerts fire only for the drops beyond
rounding noise, and only on the declared account.

**Acceptance Scenarios**:

1. **Given** an exclusive-use account, **When** a poll shows unexplained use beyond rounding
   noise in an interval where 0router sent nothing, **Then** a leak alert names the account, the
   window, the interval and the amount, in plain facts.
2. **Given** an exclusive-use account with a steady unexplained rate that is significantly above
   zero, **When** it is established, **Then** one leak alert reports the rate and since when.
3. **Given** an account not declared exclusive-use, **When** the same use happens, **Then** it is
   listed as outside use (User Story 2) and no alert is raised.
4. **Given** an exclusive-use account, **When** an idle interval shows a one-step drop, **Then**
   no alert is raised (rounding noise).
5. **Given** a leak alert, **When** it is raised, **Then** the account's state, priority and
   routing don't change, and the alert's wording never says the key leaked.
6. **Given** a leak alert the operator has read, **When** they acknowledge it from the CLI,
   **Then** it leaves the alert list and stays in the account's outside-use history.
7. **Given** the operator withdraws the exclusive-use declaration, **When** new unexplained use
   occurs, **Then** no alert is raised.
8. **Given** an exclusive-use account, **When** a burst of outside use lands in an interval where
   0router also sent traffic and its excess beyond what the fit can explain passes FR-010's test,
   **Then** a leak alert names the account, the window, the interval and the excess.

### Edge Cases

- **Poll failures**: intervals bridge over failed polls; a longer interval is still evidence,
  with a wider range. A stale account (slice 006) keeps its fit and gains no evidence until a good
  poll.
- **Window reset inside an interval**: an interval that spans a window's reset is not evidence
  for that window, and is not outside use.
- **Usage the provider didn't report**: an interval in which 0router sent attempts whose usage
  the provider didn't report is not evidence for that window's token weights, and is not outside
  use. The view counts how many intervals were set aside for this.
- **A request that spans a poll**: its traffic counts in the interval where slice 006 counts it;
  the resulting skew is treated as noise.
- **Balance windows** (credits with no reset): their token weights and multipliers can be fitted;
  their capacity is not a fitted number.
- **Request-counted windows**: their capacity can be fitted; they have no token weights.
- **Counted and balance windows** report absolute units, so they need no yardstick: their input
  weight is fitted like any other number.
- **A plugin whose weights are all off by the same factor** on a percent window: the polls can't
  show it as a weight error. It is fitted as a capacity correction, which gives the same
  placements.
- **Pay-as-you-go accounts** and accounts with no quota reports: nothing is fitted (out of scope),
  and pooled weights are not applied to them.
- **A plugin with one account**: pooling has nothing to pool; the account's weights are fitted
  from its own polls.
- **All accounts of a plugin split off**: each is fitted on its own; the view says no pooled fit
  remains.
- **A plugin removed**: its fits are dropped with it. Reinstalling it starts from nothing.
- **An account removed or re-added**: its capacity fit and outside-use list go with it; the
  pooled weights keep the evidence its polls already gave. Its poll history is kept (slice 005),
  but a re-added account's capacity fit starts from nothing and its old outside-use list is not
  shown.
- **Pruned history**: pruning poll history from inside a fit's evidence keeps what those
  intervals proved, in summary form; no fitted number moves because of a prune.
- **Outside use with a daily rhythm** (claude.ai during office hours, while 0router is also
  busy): outside use is allowed its own rate per part of the day, learned from the intervals in
  which 0router sent nothing. A part of the day in which 0router is never idle leaves that rate
  and the meter numbers unseparable, so those numbers stay at the declared value (FR-005), and
  the view names the part of the day as the reason. Outside use that rises and falls with
  0router's own traffic within the same part of the day can't be told apart from a wrong number
  by any poll; the split-off test (FR-003) keeps it out of the pooled weights.
- **A capacity the plugin doesn't declare** ("assumed from peers" in slice 006): the fit learns
  it like any other capacity, starting from the assumed value as the declaration.
- **Outside use large enough to empty a window**: the window is shown as exhausted from the poll,
  as today; the interval is outside use.
- **Exclusive-use declared on an account that isn't polled**: refused, with the reason: there are
  no polls to detect unexplained use from.
- **Disk full**: fit updates made while state can't be saved stay in memory, and the view warns,
  as slice 006 does for routing state.

## Requirements *(mandatory)*

### Functional Requirements

**The fit**

- **FR-001**: 0router MUST estimate, from poll intervals, every meter number of every window of
  every polled subscription account: capacity, the four token weights and the per-model
  multipliers, where the window's kind has them (Edge Cases). On a percent window the input
  weight is the yardstick: it MUST NOT be fitted, MUST stay at its declared or overridden value,
  and the other numbers MUST be fitted relative to it. The view MUST show it as "yardstick".
- **FR-002**: Token weights and model multipliers MUST be fitted per plugin and window name,
  pooled over all polled accounts of that plugin. Capacity MUST be fitted per account and window.
- **FR-003**: When an account's own intervals disagree significantly with the pooled token
  weights or multipliers, that account MUST be split off: its numbers are fitted from its own
  intervals only, and its intervals stop counting towards the pooled fit. The split MUST be shown
  with its reason.
- **FR-004**: Each fitted number MUST carry a 95% range. Whole-percent (or whole-unit) rounding
  of the provider's reports MUST be part of the noise the range accounts for.
- **FR-005**: A number that the operator's traffic can't separate from another (their effects on
  the polls can't be told apart) MUST NOT be fitted; it MUST stay at its declared or overridden
  value, and the view MUST name the number it can't be separated from.
- **FR-006**: The plugin's file MUST NOT be written. Fits MUST live in 0router's own state.

**Classifying a mismatch**

- **FR-007**: For each interval, 0router MUST compare the use the polls report with the use
  0router's own traffic explains under the fitted meter. A mismatch that grows with 0router's
  traffic and points consistently one way MUST be treated as evidence about meter numbers.
- **FR-008**: Use that doesn't track 0router's traffic MUST be classified as outside use and left
  out of the fit: use in an interval where 0router sent nothing, use beyond what any meter
  within the fit's ranges could explain from 0router's traffic in that interval, and a steady
  rate per part of the day (Edge Cases: outside use with a daily rhythm).
- **FR-009**: Use within rounding noise (at most one resolution step per window per interval)
  MUST NOT be classified as outside use.

**Significance and routing**

- **FR-010**: A fitted number MUST become significant only when a significance test rejects the
  value currently declared or overridden. The test MUST stay valid although it is re-checked at
  every poll, MUST account for rounding noise and for the number of meter numbers tested at once,
  and MUST give each number at most a 0.1% chance, over its whole life, of becoming significant
  when the declared value is right. Until then the number is learning. The 95% range of FR-004 is
  what the view shows; it is not this test. Split-offs (FR-003), breaks (FR-015) and a leak
  alert's steady rate (FR-024) use the same test.
- **FR-011**: Until a number is significant, routing MUST use exactly the value it uses today
  (override, else declaration), and every placement MUST equal the placement 0router makes
  without this slice.
- **FR-012**: Routing MUST take each meter number from, in order: the operator's account
  override; else the operator's plugin override (token weights and multipliers only); else a
  significant fit; else the plugin's declaration (Constitution II as amended).
- **FR-013**: Every request record MUST state, for the windows its placement used, whether each
  meter number in effect came from an override, a fit or the declaration.
- **FR-014**: Between polls, the estimate of what is left MUST stay the last poll less 0router's
  own counted traffic, costed with the numbers in effect. It MUST NOT subtract any expected
  outside use.

**Breaks and restarts**

- **FR-015**: When recent intervals disagree significantly and consistently with a significant
  fitted number, in a way that tracks 0router's traffic or the window's capacity, 0router MUST
  record a break: routing falls back to the override or declaration for that number, and the fit
  restarts using only intervals after the break.
- **FR-016**: A break MUST be reported in the routing view ("provider rules changed around
  <time>, relearning") and in the server log.
- **FR-017**: When a plugin's declared meter for a window changes, every fitted number of that
  window MUST restart from nothing, and the view MUST say why.
- **FR-018**: Fits, their states, the intervals they rest on, the outside-use list, breaks,
  exclusive-use declarations and leak alerts MUST survive restarts and crashes, as slice 006's
  routing state does.

**Overrides**

- **FR-019**: The operator MUST be able to override, per account and window, every meter number:
  capacity (as today), each token weight and each model multiplier, and remove each override,
  from the CLI. Token weights and model multipliers MUST also be overridable per plugin and
  window, applying to every account of that plugin; an account override wins over a plugin
  override. Capacity has no plugin-level override. Overrides MUST be checked by the same rules as
  the plugin's values.
- **FR-020**: An override MUST NOT stop the fit: the fit keeps learning, and the view keeps
  showing its range beside the override.

**Outside use**

- **FR-021**: Outside-use detection MUST run for every polled account, without any opt-in, and
  MUST list per account each interval of outside use (start, end, window, amount) and any steady
  unexplained rate it has established.
- **FR-022**: Outside use MUST NOT raise an alert, a notice or a log line on an account that is
  not an exclusive-use account.

**Leak alerts**

- **FR-023**: The operator MUST be able to declare, and withdraw, from the CLI that a polled
  account is used only through 0router. Declaring it for an account that isn't polled MUST be
  refused with the reason.
- **FR-024**: Only an exclusive-use account MUST raise a leak alert, and only for (a) outside use
  beyond rounding noise in an interval where 0router sent nothing, (b) outside use in an interval
  where 0router sent traffic, when the excess beyond what the fit can explain passes FR-010's
  test, or (c) a steady unexplained rate significantly above zero.
- **FR-025**: A leak alert MUST state facts: account, window, interval or rate, and amount. It
  MUST NOT say the key leaked.
- **FR-026**: A leak alert MUST NOT change the account's state, priority, routing or sign-in.
- **FR-027**: Leak alerts MUST appear in `nullrouter check`, in the routing view and in the server
  log. The operator MUST be able to acknowledge one; an acknowledged alert leaves the alert list
  and stays in the outside-use history.

**The routing view**

- **FR-028**: For each polled account and window, the routing view MUST show per meter number: the
  declared value, any override, the fitted 95% range, the value in use, its source, and its state
  (learning with progress, fitted since, split off, relearning since, not separable, restarted,
  yardstick).
  The machine-readable form MUST carry the same facts.
- **FR-029**: For an account with no quota reports, the view MUST say it is not fitted and why.

**Boundaries**

- **FR-030**: Fitted values, intervals, outside-use lists and leak alerts MUST NOT be sent
  anywhere. They MUST NOT appear in any request to a provider, nor be given to any plugin.
- **FR-031**: Plugins MUST stay data. No plugin may declare, read or change a fit, an
  exclusive-use declaration or an alert. Secrets MUST NOT appear in any fit, list, alert, view or
  log line.
- **FR-032**: The dashboard MUST NOT change in this slice.

**Evidence**

- **FR-033**: A simulated-clock test MUST run a week of traffic at 10-minute, whole-percent polls
  against: a mock provider whose real output and cache weights are 3× its plugin's relative to
  input, and whose real capacity differs from its plugin's; one whose plugin is right; injected outside use (idle bursts, a steady rate, bursts during traffic); idle-time
  drops, busy-time bursts and rounding-sized noise on an exclusive-use and a non-exclusive
  account; and a capacity
  that halves mid-week.
- **FR-034**: An opt-in live check MUST report, for the operator's real polled accounts, the state
  and progress of every meter number, with no time target.

### Key Entities

- **Meter number**: one declared number of one window of one plugin's quota meter: capacity, a
  token weight or a model multiplier.
- **Poll interval**: two consecutive good polls of one account, the use they report per window,
  and 0router's counted traffic between them (from slice 006).
- **Fit**: the estimate of one meter number, with its scope (pooled per plugin, or per account
  when split off or for capacity), 95% range, state, the intervals it rests on and since when.
- **Break**: a detected change in a provider's rules for one meter number: when, and what it
  replaced.
- **Outside-use entry**: one interval (or a steady rate) of unexplained use on one account and
  window: start, end, amount.
- **Exclusive-use declaration**: the operator's statement that one polled account is used only
  through 0router, with the time it was made.
- **Leak alert**: an outside-use entry on an exclusive-use account that meets FR-024, with its
  acknowledged state.
- **Meter override**: the operator's value for one meter number, either of one account (extends
  slice 006's account overrides) or, for token weights and model multipliers, of one plugin
  (applies to all its accounts; an account override wins).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: In the simulated week, for the 3×-off mock, capacity and the output weight become
  significant within 7 simulated days, and each final fitted value is within 10% of the true
  value (the output weight measured relative to the input weight, the yardstick).
- **SC-002**: Over a seeded suite of at least 100 simulated weeks with a right plugin, with and
  without outside use, 0 meter numbers become significant, and 100% of placements equal those
  0router makes without this slice.
- **SC-003**: Across that suite, the 95% ranges contain the true value in at least 93% of at least
  1,000 checks for every number that is reported (a calibrated 95% range lands below 95% about
  half the time by chance; 93% is the sampling tolerance).
- **SC-004**: With outside use injected, every final fitted value of the 3×-off mock lies within
  the 95% range of the same run without outside use, and at least 90% of injected outside-use
  intervals beyond rounding noise are listed.
- **SC-005**: Until the first number becomes significant, 100% of placements in the 3×-off run
  equal those 0router makes without this slice.
- **SC-006**: When a real capacity halves mid-week, the break is reported within 1 simulated day,
  and 0 placements after the report use the old fitted capacity.
- **SC-007**: 0 leak alerts on accounts not declared exclusive-use; 0 leak alerts from
  rounding-sized noise; 100% of injected idle-time drops beyond rounding noise on the
  exclusive-use account raise an alert at the first poll that shows them; 0 leak alerts on busy
  intervals of the exclusive-use account without injected outside use, and an alert for every
  injected busy-time burst whose excess passes FR-010's test.
- **SC-008**: Across a restart and a crash, 100% of fits, states, breaks, outside-use entries,
  declarations and alerts are as they were. After a plugin meter change, 100% of that window's
  numbers have restarted and no other number changed.
- **SC-009**: From one routing-view call, the operator can tell for every meter number of every
  polled account which value is in use, its source, its state and its progress.
- **SC-010**: 0 provider requests, plugin inputs, records or log lines contain a secret, and 0
  provider requests or plugin inputs contain a fitted value, interval or alert.
- **SC-011**: Choosing each meter number's source adds no measurable regression to the placement
  of a request in the routing benchmarks.
- **SC-012**: The opt-in live check reports a state and progress for 100% of the meter numbers of
  the operator's polled accounts.

## Assumptions

- The rate at which a correction is accepted is set by FR-010's test: valid under checking at
  every poll, at most a 0.1% lifetime false-correction chance per number, corrected for testing
  many numbers at once. The exact test and the evidence it needs are for the plan. The strict
  test slows the correction of small errors; brief row 2 tolerates that. The
  user's failure condition ("a right plugin gets corrected") is measured as SC-002 (no number
  becomes significant on a right plugin across the seeded suite); a 95% range, by its nature,
  misses the truth in about 5% of checks, which SC-003 measures and does not count as a failure.
- "Within 10% of the true value" (SC-001), "within 1 simulated day" (SC-006), the 100-week suite
  (SC-002) and "90% of injected intervals" (SC-004) were chosen in this spec, not by the brief, as
  were three behaviours: acknowledging a leak alert (FR-027), the request record naming each
  meter number's source (FR-013), and refusing an exclusive-use declaration on an account that
  isn't polled (FR-023). The user confirmed all seven in clarify (2026-10-07).
- A one-off session placed badly before a correction is significant is tolerated (brief row 2).
- Rounding noise is one resolution step per window per interval: one percentage point on a
  percent window, one unit on a counted window.
- Fits work on whatever polls slice 005 already makes (every 10 minutes by default); this slice
  adds no polls and changes no poll interval.
- A leak alert's "steady rate significantly above zero" uses the same test as FR-010.
- Acknowledging an alert is a CLI action on 0router's own state; it changes nothing at the
  provider.
- Fits apply at the next decision after they become significant; no reload is needed.
- Depends on: quota polling and poll history (005), the per-interval traffic tally, quota meters,
  account overrides, the routing view, request records and durability (006), and records (008).
- Out of scope: the fit, outside use and alerts on the dashboard → later; fitting accounts whose
  provider reports no quota, and learning limits from rate-limit refusals → later; pausing or
  disabling an account on a leak signal → not planned; forecasting outside use between polls →
  later, if the live check shows it matters; Jev integration → later; sending fitted values
  anywhere → not planned; exporting a fitted meter as plugin TOML for authors → later.

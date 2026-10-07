# Feature Specification: Model Tests and Combos

**Feature Branch**: `011-model-tests-combos`

**Created**: 2026-10-07

**Status**: Draft

**Scope brief**: [specs/briefs/2026-10-07-model-tests-combos.md](../briefs/2026-10-07-model-tests-combos.md).
In clarify and plan, an answer that contradicts a confirmed row of that brief's ledger means
stop and revisit the brief.

**Input**: User description: "Model tests and combos: the operator can prove which models work, routing stops sending to a model only when a provider has definitively rejected it, and the operator can define combos that clients ask for by name. A test is one real, minimal call through 0router to a model on one account, for every model type (text, embedding, speech, image and video), billed like any call; it gives PASS, BROKEN or UNKNOWN with the reason. The CLI tests one model, a unified model, a combo, or everything, and only when the operator asks. BROKEN comes only from a definitive rejection: 0router's own list (the model doesn't exist, isn't available to this account, or doesn't support the request type) plus rejection signals a provider plugin declares as data; rate limits, server errors and timeouts give UNKNOWN. A verdict belongs to one account and one model: BROKEN takes that model away from that account only, and a rejected key or expired sign-in still marks the account, not its models. Untested models route normally. UNKNOWN is retested automatically, by default after about 1 minute, 5 minutes, 30 minutes and then every 6 hours until it settles; BROKEN is not retested automatically unless the operator turns that on. The operator sets the retest schedule, the test timeouts (by default 30 seconds for text, embedding and speech, 5 minutes for image and video) and the other test limits. The operator can clear a verdict or mark a model BROKEN for an account by hand, and the CLI shows it as set by the operator. Verdicts survive restarts and return to untested when the account's secret or sign-in or its plugin changes. Test calls are recorded like any request, marked as tests and without the prompt, and count against quota and pacing. A combo is an ordered fallback chain of unified models or other combos, defined by the operator in config.toml; plugins cannot declare combos. A combo is checked at load (its name may not clash with a unified model, and it may not contain itself), appears in clients' model lists next to unified models, and moves to its next member only after the current member has used up its own retries and fallbacks, never once the answer has started. A combo test makes one real call through the combo as a client would and shows the combo's verdict, which member answered, and the verdict of each member it tried, walking nested combos the same way. Plugins stay data and never see secrets; the core makes every call. Out of scope: load-balancing groups across unified models and 9router's fusion → not planned; operator-written test cases for a combo → later; combos and verdicts on the dashboard → later; scheduled test runs and testing models when they first appear → not planned. Scope brief: specs/briefs/2026-10-07-model-tests-combos.md"

## Clarifications

### Session 2026-10-07

- Q: Which pairs does "test everything" cover? → A: Only the pairs routing can reach: each
  member model of each loaded unified model, on every account that serves it, once each. Not
  every model a provider declares, and no combo tests; the operator names combos to test them.
- Q: When every account behind a target is BROKEN for it, do clients still see it in their model
  lists? → A: Yes. It stays listed, and a request for it fails at once with no upstream call and
  an error listing each pair's verdict and reason.
- Q: When a combo test's attempt lands on a pair, does it update that pair's verdict? → A: Only
  PASS and BROKEN do. A non-definitive failure is shown in the combo test's output, and the pair
  keeps its verdict, so no retests start for a pair the operator didn't test.
- Q: Do automatic retests wait while an account is rate-limited or at its reserve floor? → A: Yes.
  They wait, as cold work does, and run once the account can serve again. Tests the operator
  asks for still run at once.

## User Scenarios & Testing *(mandatory)*

This slice has three kinds of user:

- The **operator** runs 0router. They hold accounts, declare unified models and combos in
  `config.toml`, run tests from the CLI, and read verdicts and request records.
- The **agent** (client) is any standard harness or SDK, as in slice 003. It names a target and
  never sees a test, but its requests no longer reach a model an account has definitively
  rejected, and it can name a combo.
- The **plugin author** declares a provider as data, and may add the rejection signals that
  provider uses to say "this model is not for you".

Terms used throughout:

- A **pair** is one account and one model of that account's provider. Verdicts belong to pairs.
- A **verdict** is PASS, BROKEN or UNKNOWN, with a reason, a time and a source: a test, an
  automatic retest, or the operator. A pair with no verdict is **untested**.
- A **definitive rejection** is a provider answer that says the model itself can't be used on
  this account: the model doesn't exist, isn't available to this account, or doesn't support
  the request type, or a rejection signal the provider's plugin declares. Everything else that
  isn't a success (rate limits, server errors, timeouts, broken connections, malformed answers)
  is not definitive.
- A **combo** is an operator-declared, ordered fallback chain whose members are unified models
  or other combos.

### User Story 1 — The operator proves which models work (Priority: P1)

The operator asks 0router to test a model on one account, on every account that has it, a
unified model (each member on each account that serves it), or everything. For each pair, 0router
makes one real, minimal call of that model's type (text, embedding, speech, image or video)
through the same path a client request takes, and reports PASS, BROKEN or UNKNOWN with the
reason and how long the call took. Each call is billed like any call, so tests run only when the
operator asks.

**Why this priority**: Without a test, the operator learns that a model is gone or not on their
plan only when a client's request fails. This is Constitution VII: a verdict comes only from a
real, minimal call.

**Independent Test**: Against mock providers that answer success, "model not found", "not
available on your plan", "unsupported request type", a plugin-declared rejection, 429, 500 and
a hang, test one pair of each model type. Each verdict matches the answer: PASS for success,
BROKEN for the four rejections, UNKNOWN for the rest, each with the provider's reason.

**Acceptance Scenarios**:

1. **Given** a text model that answers normally, **When** the operator tests it on one account,
   **Then** the result is PASS, with the duration and time to first output.
2. **Given** a provider that answers "model not found" for that model, **When** it is tested,
   **Then** the result is BROKEN, and the reason quotes the provider's rejection.
3. **Given** a provider that answers 429, 5xx, or doesn't answer within the type's timeout,
   **When** the model is tested, **Then** the result is UNKNOWN with that reason, never BROKEN.
4. **Given** a provider whose plugin declares a rejection signal (for example an error code
   meaning "model retired"), **When** a test receives that signal, **Then** the result is
   BROKEN and the reason names the signal and the plugin that declared it.
5. **Given** an account whose key is rejected or whose sign-in has expired, **When** one of its
   models is tested, **Then** the account is marked as slice 005 marks it, the model's verdict
   does not change, and the output says the pair was not tested because of the account.
6. **Given** an embedding, speech-to-text, text-to-speech, image and video model, **When** each
   is tested, **Then** each gets a real call of its own type, and PASS requires a usable result
   of that type (vectors, text, audio, an image, a video).
7. **Given** a unified model with two members, each on two accounts, **When** the operator tests
   the unified model, **Then** four pairs are tested and the output groups them by member.
8. **Given** a provider that answers success with an empty or malformed result, **When** it is
   tested, **Then** the result is UNKNOWN with "malformed answer", not PASS.

---

### User Story 2 — Routing stops only at a definitive rejection (Priority: P1)

A BROKEN verdict takes that model away from that account, and from nothing else. Client
requests for that model, whether directly, through a unified model or through a combo, skip
that account without spending an attempt on it. Other accounts of the same provider keep
serving the model. Untested, PASS and UNKNOWN pairs route exactly as before.

**Why this priority**: This is how tests pay off: a model a provider has refused stops costing
an attempt and latency on every request. A false BROKEN removes working capacity, so only a
definitive rejection may cause it.

**Independent Test**: Mark one pair BROKEN by a test against a mock rejection. Send client
requests for that model through a direct target, a unified model and a combo. None reaches the
BROKEN account for that model, the other account serves them, and each record names the skipped
pair and its verdict.

**Acceptance Scenarios**:

1. **Given** model M is BROKEN on account A and untested on account B of the same provider,
   **When** a client asks for M, **Then** the request goes to B, and the record says A was
   skipped because M is BROKEN there.
2. **Given** M is BROKEN on A, **When** a client asks for another model N on A, **Then** A serves
   N as before.
3. **Given** M is UNKNOWN on A, **When** a client asks for M, **Then** A is a candidate as usual.
4. **Given** a target whose every pair is BROKEN, **When** a client asks for it, **Then** no
   upstream call is made, and the client gets the informational error of slice 003, listing
   each pair's verdict and reason.
5. **Given** a client request that receives a definitive rejection from an untested pair,
   **When** it fails over to another account, **Then** the pair's verdict does not change; only
   a test sets a verdict.

---

### User Story 3 — UNKNOWN settles by itself, on the operator's terms (Priority: P1)

An UNKNOWN pair is retested automatically until it settles to PASS or BROKEN: by default about
1 minute, 5 minutes and 30 minutes after the UNKNOWN, then every 6 hours. A BROKEN pair is not
retested automatically unless the operator turns that on. The operator sets the retest
schedule, the per-type test timeouts (by default 30 seconds for text, embedding and speech,
5 minutes for image and video) and the other test limits, and the next test uses them.

**Why this priority**: UNKNOWN means "we couldn't tell"; leaving it unresolved means the operator
never learns whether a model works, and calling it BROKEN would violate Constitution VII.

**Independent Test**: On a simulated clock, a mock provider returns 503 three times and then
success. The pair is retested at about 1, 5 and 30 minutes, becomes PASS on the fourth call,
and is not retested again.

**Acceptance Scenarios**:

1. **Given** a pair that just became UNKNOWN, **When** no one intervenes, **Then** it is retested
   about 1 minute, 5 minutes and 30 minutes later, then every 6 hours, until a retest gives PASS
   or BROKEN.
2. **Given** a pair that is BROKEN, **When** the operator hasn't turned on BROKEN retests,
   **Then** it is never retested automatically.
3. **Given** the operator turned on BROKEN retests, **When** the retest interval passes, **Then**
   the BROKEN pair is retested, and a PASS returns it to routing.
4. **Given** the operator changes the retest schedule or a timeout, **When** the next test or
   retest runs, **Then** it uses the new value, without a restart.
5. **Given** the server was stopped while retests were due, **When** it starts again, **Then**
   overdue retests run soon after start, spread out rather than all at once.
6. **Given** a pair is UNKNOWN on an account that can't currently serve (disabled, needs sign-in,
   refreshing, rate-limited, or at its reserve floor), **When** its retest falls due, **Then** the
   retest waits until the account can serve again, and the verdict list shows why it waits.

---

### User Story 4 — The operator keeps the last word on verdicts (Priority: P2)

The operator can clear any verdict (the pair returns to untested and to routing) or mark a pair
BROKEN by hand, for example for a model they know their plan doesn't include. The CLI lists
verdicts with their reason, time, source and next retest, and shows operator-set verdicts as
set by the operator. Verdicts survive restarts and crashes. A pair returns to untested when its
account's secret or sign-in changes, or when its provider's plugin changes, since the old answer
may no longer hold.

**Why this priority**: Tests can be wrong about the world after it changes; the operator needs
to override them and trust that stale verdicts don't linger.

**Independent Test**: Mark a pair BROKEN by hand, restart the server, and check it is still
BROKEN and shown as operator-set. Replace the account's key and check the pair is untested.
Update the provider's plugin and check every pair of that provider is untested.

**Acceptance Scenarios**:

1. **Given** a BROKEN pair, **When** the operator clears it, **Then** it is untested and routes
   on the next request.
2. **Given** an untested pair, **When** the operator marks it BROKEN with an optional note,
   **Then** routing skips it, and the verdict list shows "set by the operator" with the note.
3. **Given** an operator-set BROKEN with BROKEN retests turned on, **When** the retest interval
   passes, **Then** the pair is not retested; only the operator or a test they request changes it.
4. **Given** verdicts of every kind, **When** the server restarts or crashes, **Then** every
   verdict, its source and its retest step are as before.
5. **Given** an account with verdicts, **When** its secret is replaced or it signs in again,
   **Then** its pairs return to untested. A routine token refresh does not count as a change.
6. **Given** a provider with verdicts on several accounts, **When** its plugin is installed,
   updated, replaced or removed, **Then** every pair of that provider returns to untested.
7. **Given** an account that is removed, **When** the verdict list is shown, **Then** its pairs
   are gone.

---

### User Story 5 — Clients ask for a combo by name (Priority: P1)

The operator declares combos in `config.toml`: a name and an ordered list of members, each a
unified model or another combo. Clients see combos in their model lists next to unified models
and ask for one by name. A request to a combo goes to its first member that can serve. It moves
to the next member only after the current member has used up its own retries and fallbacks,
and never once the answer has started reaching the client.

**Why this priority**: Combos are how the operator says "if this whole model family is down, use
that one instead", a policy over unified models (Constitution III). Without them, a unified
model that can't serve fails the client even when another model would do.

**Independent Test**: Declare combo `coder` = [`sonnet`, `fallback-chain`], where
`fallback-chain` = [`gpt`, `glm`]. With every `sonnet` and `gpt` account failing transiently,
a request for `coder` is answered by `glm`. The record shows each member tried and why each one
couldn't serve.

**Acceptance Scenarios**:

1. **Given** a combo whose first member can serve, **When** a client asks for the combo,
   **Then** the first member answers, and the record names the combo and the member.
2. **Given** a first member whose every account and provider fails transiently, **When** a
   client asks for the combo, **Then** the request moves to the second member only after the
   first member's own retries and fallbacks are used up.
3. **Given** a stream from a member that breaks after the client received output, **When** the
   member's own continuation or restart rules (slice 003) are used up, **Then** the combo does
   not move to another member, and the client gets the error slice 003 would give.
4. **Given** an error that slice 003 returns to the client without fallback (the request itself
   is invalid), **When** a combo member receives it, **Then** the combo returns it and tries no
   further member.
5. **Given** a member whose every pair is BROKEN, **When** a client asks for the combo, **Then**
   that member is skipped without an attempt, and the record says why.
6. **Given** nested combos where the same unified model appears twice, **When** it has already
   failed in this request, **Then** it is not tried again.
7. **Given** a client in each of the four API styles, **When** it lists models, **Then** each
   loaded combo appears next to the unified models, with the model type of its members.

---

### User Story 6 — The operator tests a combo as a client would (Priority: P2)

The operator tests a combo. 0router makes one real, minimal call through the combo as a client
would, and shows the combo's verdict, which member answered, and the verdict of each member it
tried, walking nested combos the same way.

**Why this priority**: A combo is a routing policy, so testing its members alone doesn't prove
the chain works (Constitution III: a combo is testable on its own).

**Independent Test**: For a three-level nested combo whose first two leaf members are BROKEN and
UNKNOWN and whose third answers, the combo test reports PASS, names the third member as the one
that answered, and shows the first as skipped (BROKEN) and the second as UNKNOWN with its
reason, indented under their combos.

**Acceptance Scenarios**:

1. **Given** a combo whose first member answers, **When** the operator tests it, **Then** the
   result is PASS, answered by that member, and no other member is called.
2. **Given** a combo where no member answers and at least one failed for a reason that isn't
   definitive, **When** it is tested, **Then** the result is UNKNOWN.
3. **Given** a combo where every member is BROKEN or definitively rejected, **When** it is
   tested, **Then** the result is BROKEN.
4. **Given** a combo test whose attempt reached a pair, **When** that attempt gives PASS or a
   definitive rejection, **Then** the pair's verdict becomes PASS or BROKEN. **When** it fails
   for any other reason, **Then** the output shows UNKNOWN for that attempt, and the pair keeps
   its verdict and starts no retests.

---

### Edge Cases

- **A combo with a name clash**: a combo named like a unified model, or like another combo, is
  a load error that names both declarations.
- **A combo that contains itself**, directly or through other combos, is a load error that
  prints the cycle.
- **A combo that names an unknown member** is a load error. A combo whose member was dropped
  because a plugin was skipped is dropped and reported, as unified models are today.
- **A combo whose members have conflicting model types** (for example text and embedding) is a
  load error.
- **An empty combo** is a load error.
- **A plugin that declares a combo** fails plugin validation.
- **A plugin that declares a rate limit, server error or timeout as a rejection signal** fails
  plugin validation: those can never give BROKEN.
- **An authentication failure in a test** (rejected key, expired sign-in) marks the account, not
  the model, as in User Story 1 scenario 5.
- **A test while no server is running**: the CLI says a running server is needed and makes no
  call.
- **A test of a disabled account**, or one that needs sign-in, makes no call and says why.
- **A test of a passthrough provider's model** that the plugin doesn't list: the operator names
  the model, and it is tested like any other.
- **A model the plugin removes**: its verdicts are dropped with the plugin change (User Story 4).
- **Two tests of the same pair at once** (the operator and a retest): the later result wins, and
  both calls are recorded.
- **A test the operator interrupts**: calls already sent finish and are recorded; calls not yet
  sent are not made, and their pairs keep their verdicts.
- **A client disconnect during a combo**: pending attempts are cancelled, as in slice 003.
- **Disk full**: verdicts made while records can't be saved stay in memory and the CLI warns, as
  slice 006 does for routing state.

## Requirements *(mandatory)*

### Functional Requirements

**Tests**

- **FR-001**: The operator MUST be able to test, from the CLI: one pair; one model on every
  account that has it; a unified model (each member on each account that serves it); a combo
  (User Story 6); or everything (FR-004).
- **FR-002**: A test of a pair MUST be one real call through the same execution path as a client
  request, sent to that account, with the smallest request 0router can make for that model's
  type: text, embedding, text-to-speech, speech-to-text, image or video. Untyped models are
  tested as text.
- **FR-003**: Tests MUST run only when the operator asks, apart from automatic retests (FR-012).
  0router MUST NOT test models on its own when they first appear or on a timetable.
- **FR-004**: "Test everything" MUST test every pair that a loaded unified model or combo can
  route to: each member model of each unified model, on every account that serves it, once each
  even when several unified models or combos share it. It MUST NOT test models that no unified
  model or combo uses, and MUST NOT run combo tests; the operator names those separately.
- **FR-005**: Before a test that will make more than one call, the CLI MUST say how many calls
  it will make, by model type, and ask the operator to confirm. A flag MUST skip the prompt.
- **FR-006**: Each test result MUST show the pair, its verdict, the reason, the duration and,
  for streamed types, the time to first output.

**Verdicts**

- **FR-007**: A test MUST give PASS when the provider returns a usable result of the model's
  type, BROKEN only on a definitive rejection, and UNKNOWN for everything else: rate limits,
  server errors, timeouts, broken connections, and empty or malformed results.
- **FR-008**: 0router MUST ship its own list of definitive rejections: the model doesn't exist,
  isn't available to this account, or doesn't support the request type. A provider plugin MAY
  add rejection signals as data. Plugin validation MUST refuse a declared signal that is a rate
  limit, a server error or a timeout.
- **FR-009**: An authentication failure during a test (rejected key, expired sign-in) MUST mark
  the account as slice 005 does and MUST NOT change the verdict of the model.
- **FR-010**: A verdict MUST belong to one pair. Only a test, a retest or the operator MAY set
  it. Client traffic MUST NOT set or change a verdict.
- **FR-011**: Routing MUST skip a BROKEN pair for every target (direct, unified model, combo)
  without making an attempt, and MUST name the skipped pair and its verdict in the request
  record. Untested, PASS and UNKNOWN pairs MUST route as they do without this slice. When every
  pair of a target is BROKEN, the client MUST get slice 003's informational error listing the
  verdicts, with no upstream call.

**Retests and limits**

- **FR-012**: An UNKNOWN pair MUST be retested automatically, by default about 1 minute,
  5 minutes and 30 minutes after it became UNKNOWN, then every 6 hours, until it gives PASS or
  BROKEN. A new UNKNOWN from an operator-requested test restarts the schedule.
- **FR-013**: A BROKEN pair from a test MUST NOT be retested automatically unless the operator
  turns BROKEN retests on, with an interval they set (by default once a day). An operator-set
  BROKEN MUST never be retested automatically.
- **FR-014**: An automatic retest MUST wait while its account can't serve (disabled, needs
  sign-in, refreshing) or couldn't take cold work (rate-limited, or a quota window at its reserve
  floor), and MUST run once the account can serve again; the verdict list shows why it waits.
  Tests the operator asks for are not held back by the reserve floor or priority. After a restart,
  overdue retests MUST run soon after start, spread out.
- **FR-015**: The operator MUST be able to set the retest schedule, BROKEN retests on or off and
  their interval, a test timeout per model type (by default 30 seconds for text, embedding and
  speech, 5 minutes for image and video) and how many test calls run at once (by default 4).
  Changes MUST apply to the next test without a restart.

**Operator control and persistence**

- **FR-016**: The operator MUST be able to clear a verdict (the pair becomes untested) and mark a
  pair BROKEN by hand with an optional note, from the CLI.
- **FR-017**: The CLI MUST list verdicts, filterable by provider, account, model and verdict,
  showing for each the verdict, reason, time, source (test, retest, operator) and, for UNKNOWN,
  the next retest. Operator-set verdicts MUST read "set by the operator".
- **FR-018**: Verdicts, their sources and retest steps MUST survive restarts and crashes with
  the same durability as slice 006's routing state.
- **FR-019**: A pair MUST return to untested when its account's secret is replaced, when the
  account signs in again (not on a routine token refresh), or when its provider's plugin is
  installed, updated, replaced or removed. Removing an account MUST delete its verdicts.

**Records, quota, secrets**

- **FR-020**: Every test call MUST be recorded like a client request (attempts, latency, usage,
  reason), marked as a test, naming the test run, and without the prompt or the generated
  output.
- **FR-021**: Test calls MUST count against the account's quota and pacing as client traffic
  does. They MUST NOT belong to any agent or create warm-cache state for one.
- **FR-022**: Plugins MUST stay data: a plugin MUST NOT see secrets, run a test, or declare a
  combo. The core makes every test call and injects secrets only at execution.

**Combos**

- **FR-023**: The operator MUST be able to declare combos in `config.toml`: a name and an
  ordered, non-empty list of members, each a loaded unified model or another combo.
- **FR-024**: At load, 0router MUST reject: a combo whose name clashes with a unified model or
  another combo; a combo that contains itself directly or through other combos (printing the
  cycle); an unknown member; members whose model types conflict. These are `config.toml`
  errors, reported and handled as unified-model errors are today. A combo that needs a unified
  model dropped because of a skipped plugin MUST be dropped and reported.
- **FR-025**: Combos MUST appear in every client style's model list next to unified models, and
  in the CLI wherever unified models are listed and resolved. Verdicts MUST NOT change any
  model list: a target whose every pair is BROKEN stays listed (FR-011 gives its error).
- **FR-026**: A request to a combo MUST try members in order. It MUST move to the next member
  only after the current member has used up its own retries and fallbacks, and MUST NOT move
  once output has reached the client. An error that slice 003 returns without fallback MUST be
  returned without trying further members. A member whose every pair is BROKEN MUST be skipped
  without an attempt. A unified model already tried in this request MUST NOT be tried again.
- **FR-027**: A combo request's record MUST name the combo, the member that answered, and each
  member tried with why it couldn't serve, nested combos included.

**Combo tests**

- **FR-028**: A combo test MUST make one real, minimal call through the combo as a client
  request would, and report the combo's verdict (PASS if a member answered; BROKEN if every
  member was BROKEN or definitively rejected; UNKNOWN otherwise), which member answered, and
  each member tried with its verdict and reason, walking nested combos the same way.
- **FR-029**: An attempt a combo test makes MUST set the verdict of the pair it reached to PASS
  or BROKEN when it gives that result. A non-definitive failure MUST be shown as UNKNOWN in the
  combo test's output only: the pair keeps its verdict, and no retest starts.

### Key Entities

- **Pair**: one account and one model of its provider. The unit a verdict belongs to.
- **Verdict**: PASS, BROKEN or UNKNOWN for a pair; with reason, time, source (test, retest,
  operator), operator note, and for UNKNOWN the retest step and next retest time.
- **Rejection signal**: something a provider answer contains that means the model can't be used
  on this account. 0router's own list plus plugin-declared signals.
- **Test run**: one operator request (a pair, a model, a unified model, a combo, everything), its
  calls and its results, each call recorded as a test.
- **Test settings**: retest schedule, BROKEN retest switch and interval, per-type timeouts, test
  calls at once. Set by the operator, with 0router's defaults.
- **Combo**: operator-declared name and ordered members (unified models or combos), with the
  model type its members share.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Across a mock suite of every core rejection, a plugin-declared rejection, 429,
  5xx, timeout, broken connection and malformed success, for all six model types, 100% of
  BROKEN verdicts come from definitive rejections and 0 from anything else.
- **SC-002**: After a pair becomes BROKEN, 0 client requests for that model reach that account,
  through any target, and other accounts of the same provider serve 100% of those requests that
  they can.
- **SC-003**: On a simulated clock, an UNKNOWN pair is retested within 10% of each scheduled
  time over 24 hours, and never again once it settles. A BROKEN pair is retested 0 times unless
  the operator turned BROKEN retests on.
- **SC-004**: The operator gets the verdict and reason for one pair with a single CLI command,
  within that type's timeout plus 5 seconds.
- **SC-005**: Across a restart and a crash, 100% of verdicts keep their verdict, source and next
  retest. After a secret change, a new sign-in or a plugin change, 100% of the affected pairs
  are untested, and no other pair changed.
- **SC-006**: With every account of a combo's earlier members failing transiently, 0 client
  requests fail while a later member can serve, and 0 requests switch members after output
  started.
- **SC-007**: For a combo nested three levels deep, the combo test names the member that
  answered and every member tried, and the operator can tell from the output alone why each
  earlier member didn't answer.
- **SC-008**: 0 test records contain a prompt, a generated output or a secret, and 100% of test
  calls appear in the account's quota tally.
- **SC-009**: Skipping BROKEN pairs and resolving combos adds no measurable regression to the
  placement of a request in the routing benchmarks.
- **SC-010**: An opt-in live check tests one real model of each type the operator holds an
  account for, and every result is PASS or an UNKNOWN whose reason the operator recognises.

## Assumptions

- "Speech" covers both text-to-speech and speech-to-text. A speech-to-text test sends a short
  silent clip that 0router supplies, as 9router does.
- A test needs a running server, as `quota poll` does; it goes through the server so that it is
  recorded, counted and paced like client traffic.
- A test goes to the named account regardless of its routing priority or reserve floor, because
  the operator asked for it; it is not sent to an account that can't serve.
- The confirmation before multi-call tests (FR-005), BROKEN retests defaulting to once a day,
  4 test calls at once, and the spread-out restart catch-up are defaults chosen in this spec,
  not by the brief; all are operator-settable.
- Test settings are global, with per-type timeouts; per-provider or per-account overrides are
  not part of this slice.
- Combo members are unified models or combos only, as the brief states; a direct
  `provider/model` target is not a combo member (a one-member unified model serves that need).
- Combos, like unified models, apply at the next start or at the next reload a mutating command
  triggers; this slice adds no reload command.
- The verdict of a combo is shown in the test output and in the test's records; it is not stored
  apart from the pair verdicts, and it doesn't steer routing.
- Depends on: unified models (002), the request pipeline's retry, fallback, classification and
  informational errors (003), account states (005), routing, quota tally and durability (006),
  and records (006, 008).
- Out of scope: load-balancing groups across unified models and fusion → not planned;
  operator-written test cases for a combo → later; combos and verdicts on the dashboard → later
  (the dashboard's Combo page and disabled "Test All" stay as they are); scheduled test runs and
  testing models when they first appear → not planned.

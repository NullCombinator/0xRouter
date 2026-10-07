# Scope brief: model tests and combos

Shaped 2026-10-07 with /shape-spec in the main session (branch `009-dashboard`). Dashboard slice 2
is specified separately as 010 (`.worktrees/010`); slice 004 runs in `.worktrees/004`. This slice
takes the next free number.

## Core map (2026-10-07)

| Capability | Status | Evidence |
|---|---|---|
| Unified models | shipped | 002, `nullrouter unified` |
| Request execution, retry, fallback inside one unified model | shipped | 003 |
| Non-text routes (embeddings, image, speech, video) | shipped | 003 `crates/nullrouter-server/src/media.rs` |
| Account sign-in | shipped | 005 |
| Routing decision (cache-aware, per-agent, amortization) | shipped | 006 |
| Records and read model | shipped | 008 |
| Dashboard | partial | 009 built (user-gated tasks open); 010 specified |
| Client-side adapters | absent, in progress | 004 in a parallel session |
| Latency trends | absent | "the latency slice", not shaped |
| Model tests | absent | no code; 009 shows "Test All" disabled |
| Combos, nested combos, combo tests | absent | 003 brief: combos → later; 009 Combo page "not built yet" |

## Playback (confirmed)

You can test any model, unified model or combo from the CLI, or test everything. Each test is one
real, minimal, billed call, for every model type including image and video. It gives PASS, BROKEN
or UNKNOWN with the reason. BROKEN takes that model away from that account only, and only when the
provider definitively rejects it. Everything else is UNKNOWN, which retests itself on a schedule
you control, as you control the timeouts. You can clear or mark a verdict by hand. Verdicts survive
restarts and reset when the account or plugin changes. Tests are recorded and count toward quota.
You can also define combos in `config.toml`: ordered fallback chains of unified models or other
combos, which clients ask for by name. A combo moves to its next member only when the current one
can't serve. A combo test shows whether it answers and which member did. All of this is CLI only.

## Ledger

| # | Tag | Claim | Source |
|---|---|---|---|
| 1 | U | Shape model tests and testable combos next | "approved" (2026-10-07) |
| 2 | U | Test models, unified models and combos from the CLI; PASS/BROKEN/UNKNOWN with the reason; verdicts steer routing; combos route requests | Q outcome: "Test, and verdicts steer routing" |
| 3 | C✓ | Combos are fallback chains only, nestable; no load-balancing groups, no fusion | Q combo kinds |
| 4 | U | Every model type is tested, always, image and video included (also in test-all and retests) | Q test cost: "All types, always" |
| 5 | C✓ | Verdicts are per account and model; a rejected key or expired sign-in marks the account (005), not its models | Q verdict scope |
| 6 | U | The operator controls retries, timeouts "and so on"; 0router ships defaults | "User must have control over retry trial, default timeouts and so on." |
| 7 | C✓ | Default retests of UNKNOWN: about 1 min, 5 min, 30 min, then every 6 h until settled; BROKEN not retested automatically unless the operator turns it on | playback "Yes, defaults too" |
| 8 | C✓ | Default test timeout: 30 s for text, embedding and speech; 5 min for image and video | playback "Yes, defaults too" |
| 9 | C✓ | Tests run only when the operator asks; only UNKNOWN retests run by themselves; untested models route normally | Q triggers |
| 10 | C✓ | A combo test makes one real call through the combo as a client would; reports the combo's verdict, which member answered, and each member it tried; nested combos walked the same way | Q combo test |
| 11 | C✓ | BROKEN only on definitive rejection: a core list (model not found, not available to this account, request type not supported) plus rejection signals a plugin declares as data; rate limits, server errors and timeouts are UNKNOWN | Q BROKEN signal |
| 12 | C✓ | The operator can clear a verdict (model returns, untested) or mark a model BROKEN for an account; shown as set by the operator | Q override |
| 13 | C✓ | CLI only; combos and verdicts on the dashboard → later | Q dashboard |
| 14 | C✓ | Combos are defined by the operator in config.toml, not by plugins; checked at load (no clash with a unified model name, no combo containing itself); listed to clients next to unified models | Q definition |
| 15 | C✓ | A combo moves to its next member only after the current member is exhausted (its own retries and fallbacks), never mid-stream | premortem 1 |
| 16 | C✓ | Test calls are recorded like requests, marked as tests, no prompt kept, and count against quota and pacing | premortem 2 |
| 17 | C✓ | Verdicts survive restarts and reset to untested when the account's secret or sign-in or its plugin changes | premortem 3 |
| 18 | C✓ | Operator-written test cases per combo → later | Q combo test |
| 19 | K | A verdict comes only from a real minimal call; BROKEN only on definitive rejection; UNKNOWN retests and is never promoted to BROKEN without a rejection | constitution VII |
| 20 | K | Combos are policies over unified models; nested combos first-class; a combo is testable on its own | constitution III, init.md |
| 21 | K | Plugins are data; the core makes every call and holds every secret | constitution I |

## P notes for research.md

- 9router's model test (`src/app/api/models/test/ping.js`) pings through its own endpoint per kind
  (llm, embedding, image, tts, stt with a silent 250 ms WAV), 15 s timeout, ok/failed only: no
  UNKNOWN and no retest. Its minimal request bodies per kind are a useful oracle for "smallest
  call".
- 9router's combos (`open-sse/services/combo.js`): fallback, round-robin with a sticky limit, and
  fusion. Only the fallback order is relevant; round-robin and fusion are out (row 3).
- A combo member that is BROKEN on every account counts as unable to serve, so the combo moves on
  without spending an attempt on it.

## Final command

```
/speckit-specify Model tests and combos: the operator can prove which models work, routing stops sending to a model only when a provider has definitively rejected it, and the operator can define combos that clients ask for by name. A test is one real, minimal call through 0router to a model on one account, for every model type (text, embedding, speech, image and video), billed like any call; it gives PASS, BROKEN or UNKNOWN with the reason. The CLI tests one model, a unified model, a combo, or everything, and only when the operator asks. BROKEN comes only from a definitive rejection: 0router's own list (the model doesn't exist, isn't available to this account, or doesn't support the request type) plus rejection signals a provider plugin declares as data; rate limits, server errors and timeouts give UNKNOWN. A verdict belongs to one account and one model: BROKEN takes that model away from that account only, and a rejected key or expired sign-in still marks the account, not its models. Untested models route normally. UNKNOWN is retested automatically, by default after about 1 minute, 5 minutes, 30 minutes and then every 6 hours until it settles; BROKEN is not retested automatically unless the operator turns that on. The operator sets the retest schedule, the test timeouts (by default 30 seconds for text, embedding and speech, 5 minutes for image and video) and the other test limits. The operator can clear a verdict or mark a model BROKEN for an account by hand, and the CLI shows it as set by the operator. Verdicts survive restarts and return to untested when the account's secret or sign-in or its plugin changes. Test calls are recorded like any request, marked as tests and without the prompt, and count against quota and pacing. A combo is an ordered fallback chain of unified models or other combos, defined by the operator in config.toml; plugins cannot declare combos. A combo is checked at load (its name may not clash with a unified model, and it may not contain itself), appears in clients' model lists next to unified models, and moves to its next member only after the current member has used up its own retries and fallbacks, never once the answer has started. A combo test makes one real call through the combo as a client would and shows the combo's verdict, which member answered, and the verdict of each member it tried, walking nested combos the same way. Plugins stay data and never see secrets; the core makes every call. Out of scope: load-balancing groups across unified models and 9router's fusion → not planned; operator-written test cases for a combo → later; combos and verdicts on the dashboard → later; scheduled test runs and testing models when they first appear → not planned. Scope brief: specs/briefs/2026-10-07-model-tests-combos.md
```

## Trace

| Sentence of the command | Rows |
|---|---|
| Purpose: prove which models work; routing stops only on definitive rejection; combos by name | 1, 2 |
| A test is one real minimal call, every model type, billed; PASS/BROKEN/UNKNOWN with reason | 2, 4, 19 |
| CLI tests one model, a unified model, a combo or everything, only when asked | 2, 9 |
| BROKEN only from definitive rejection: core list plus plugin-declared signals; 429/5xx/timeouts UNKNOWN | 11, 19, 21 |
| Verdict per account and model; auth failure marks the account | 5 |
| Untested models route normally | 9 |
| UNKNOWN retest defaults; BROKEN not retested unless turned on | 7, 19 |
| Operator sets schedule, timeouts (defaults) and other limits | 6, 8 |
| Clear or mark by hand, shown as operator-set | 12 |
| Verdicts persist, reset on account or plugin change | 17 |
| Tests recorded, marked, no prompt, count against quota | 16 |
| Combo = ordered fallback chain of unified models or combos, in config.toml, not plugins | 3, 14, 20 |
| Load checks; listed to clients; moves on only after a member is exhausted, never mid-stream | 14, 15 |
| Combo test: one real call, combo verdict, member that answered, each tried member, nested | 10, 20 |
| Plugins stay data; core makes every call | 21 |
| Out of scope: balancing and fusion | 3 |
| Out of scope: own test cases | 18 |
| Out of scope: dashboard | 13 |
| Out of scope: scheduled runs and first-appearance tests | 9 |

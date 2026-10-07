# Data Model: Model Tests and Combos

Research: [research.md](research.md).

## Pair

`(provider, account, model)`. `provider` is the canonical id, `account` the account name,
`model` the upstream model id (`plan::Candidate::upstream_id`), so a unified-model member and a
direct target naming the same model share one pair. A no-auth provider uses account `-`.

## Verdict

| Field | Type | Notes |
|---|---|---|
| `state` | `pass` \| `broken` \| `unknown` | untested = no entry |
| `reason` | string | the provider's status and message (redacted, ≤ 500 chars), `timeout after 30 s`, `malformed answer: …`, or the operator's note |
| `rejection` | `model_not_found` \| `model_not_available` \| `type_not_supported` \| `plugin:<reason>` | `broken` from a test only |
| `source` | `test` \| `retest` \| `combo_test` \| `operator` | |
| `at` | RFC 3339 | when the verdict was set |
| `record` | `rq_…` | the test call's record; none for `operator` |
| `step` | integer | `unknown` only: the retest step reached (0 = first) |
| `next` | RFC 3339 | `unknown`, and `broken` from a test with BROKEN retests on |
| `basis` | `{secret?, signed_in_at?, plugin}` | R7 |

State transitions:

```text
untested ──test PASS──▶ pass ──test BROKEN──▶ broken
   │  ▲                  ▲ │                    │ ▲
   │  └──clear (op)──────┼─┴────────────────────┘ │
   │                     │                        │
   └──test UNKNOWN──▶ unknown ──retest PASS/BROKEN┘
                       │ ▲
                       └─┘ retest UNKNOWN: step+1 (last step repeats)
any ──mark (op)──▶ broken (source operator; never retested)
any ──basis change──▶ untested
```

A combo test sets only `pass` or `broken` (clarify Q3). A retest that is `waiting` keeps its
verdict; `waiting` is computed for display, never stored.

### On disk: `routing/verdicts.jsonl` (mode 0600)

One line per change, newest last; last line per pair wins:

```json
{"provider":"anthropic","account":"max","model":"claude-opus-4-1","state":"broken","rejection":"model_not_available","reason":"403: model claude-opus-4-1 is not available on your plan","source":"test","at":"2026-10-07T09:12:03Z","record":"rq_01J…","basis":{"signed_in_at":"2026-10-03T14:02:00Z","plugin":"sha256:…"}}
{"provider":"anthropic","account":"max","model":"claude-opus-4-1","cleared":true,"why":"account changed","at":"2026-10-08T10:00:00Z"}
```

## Rejection rule (plugin)

`[[rejections]]`: `status` (u16 or list, each 400–499, not 408 or 429), `body_contains`
(optional), `reason` (`model_not_found` | `model_not_available` | `type_not_supported`).

## Test settings (`config.toml` `[tests]`)

| Field | Default | Rule |
|---|---|---|
| `retest` | `["1m","5m","30m","6h"]` | 1–10 steps, each ≥ 30 s, non-decreasing; the last repeats |
| `broken_retest` | `"off"` | `"off"` or an interval ≥ 1 h |
| `concurrency` | 4 | 1–32 |
| `timeout.text`, `.embedding`, `.tts`, `.stt` | `"30s"` | 5 s–30 min |
| `timeout.image`, `.video` | `"5m"` | 5 s–30 min |

## Combo (`config.toml` `[[combo]]`)

| Field | Type | Rule |
|---|---|---|
| `name` | string | non-empty, no `/`, unique across unified models and combos |
| `members` | list of names | non-empty; each a unified model or combo; no cycle; kinds agree |

Loaded form: `Combo { name, kind: Option<ModelKind>, members: Vec<String>, flat: Vec<usize> }`,
where `flat` is the depth-first, de-duplicated list of unified-model indices (R13).

## Test run

`{ id: "tr_…", asked: Target, pairs: Vec<Pair>, started, results: Vec<TestResult> }`, in memory
only for the life of the `test.run` call. Each call's record carries the run id.

`TestResult`: `{ pair, state, reason, rejection?, ms, ttft_ms?, record, skipped? }`. `skipped`
holds why no call was made (account disabled, needs sign-in, no server).

`ComboResult`: `{ combo, state, answered_by?, tried: [ { member, kind: "unified"|"combo",
state, reason, attempts: [TestResult], tried?: [...] } ] }`.

## Record additions

`RequestRecord.test: Option<{ run, source }>`, `RequestRecord.combo: Option<String>`,
`Attempt.member: Option<String>` (`a › b › unified`), `ErrorClass::Broken` for skipped BROKEN
pairs.

# Contract: `config.toml` and plugin additions

## `config.toml`

```toml
schema = 1

[[combo]]
name = "coder"                         # non-empty, no "/", unique among unified models and combos
members = ["sonnet", "fallback-chain"] # unified models or combos, tried in order

[[combo]]
name = "fallback-chain"
members = ["gpt", "glm"]

[tests]
retest = ["1m", "5m", "30m", "6h"]     # after an UNKNOWN; the last step repeats until it settles
broken_retest = "off"                  # or an interval ≥ 1h
concurrency = 4                        # test calls at once, retests included (1–32)

[tests.timeout]
text = "30s"
embedding = "30s"
tts = "30s"
stt = "30s"
image = "5m"
video = "5m"
```

Load errors, reported as `config.toml:L:C path: rule` and handled like unified-model errors
(fatal at start, a rejected reload keeps the previous state):

| Path | Rule |
|---|---|
| `combo[i].name` | `name "x" is already a unified model` / `… already a combo (combo[j])` / `must not be empty or contain "/"` |
| `combo[i].members` | `must not be empty` |
| `combo[i].members[k]` | `unknown unified model or combo "x"` |
| `combo[i]` | `contains itself: coder → fallback-chain → coder` |
| `combo[i].members` | `members disagree on kind: sonnet is llm, embed is embedding` |
| `tests.retest[k]` | `at least 30s` / `steps must not get shorter` / `1 to 10 steps` |
| `tests.broken_retest` | `"off" or at least 1h` |
| `tests.concurrency` | `1 to 32` |
| `tests.timeout.<type>` | `5s to 30m` / `unknown type "x"` |

A combo that needs a unified model dropped because a plugin was skipped is dropped, and `check`
prints `dropped combo coder: needs unified model sonnet (dropped)`.

## Provider plugin (schema 2)

```toml
[[rejections]]
status = [400, 404]                # required; each 400–499, never 408 or 429
body_contains = "model_retired"    # optional, case-sensitive
reason = "model_not_found"         # model_not_found | model_not_available | type_not_supported
```

Validation errors (the plugin is refused, as for other gate failures):

| Path | Rule |
|---|---|
| `rejections[i].status` | `408 and 429 are never a rejection` / `only 400–499 can be a rejection` |
| `rejections[i].reason` | `unknown reason "x"` |

A plugin has no `combo` table; the existing unknown-key check refuses one with
`unknown field "combo"`.

`docs/plugins.md` gains a `[[rejections]]` section; `docs/operator-config.md` gains "Combos",
"Model tests" and the `[tests]` reference.

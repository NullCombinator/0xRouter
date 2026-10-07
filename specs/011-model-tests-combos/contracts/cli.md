# CLI contract: tests, verdicts, combos

Every command takes `--json` (prints only the JSON value) and follows the existing exit codes:
0 ok, 1 error, 2 not found. Commands that call provider models need a running server; without
one they print `no running server; start it with nullrouter serve` and exit 1, having made no
call.

## `nullrouter test`

```text
nullrouter test <provider>/<model> [--account NAME]   # one model: one account, or every account that has it
nullrouter test <unified>                              # each member on each account that serves it
nullrouter test <combo>                                # one call through the combo (R14)
nullrouter test --all                                  # every pair a unified model or combo can reach (FR-004)
        [--yes]                                        # skip the confirmation
```

Before more than one call:

```text
12 billed calls: 8 text, 2 embedding, 1 image, 1 video. Continue? [y/N]
```

Output, one line per pair as it finishes, then a summary:

```text
PASS     anthropic/max        claude-sonnet-4-5          1.8 s (first output 0.9 s)
BROKEN   anthropic/max        claude-opus-4-1            model not available: 403 model claude-opus-4-1 is not available on your plan
UNKNOWN  openrouter/main      anthropic/claude-sonnet-4.5  503 upstream overloaded; retest in 1 min
SKIPPED  xai/main             grok-4                     account needs sign-in: run nullrouter accounts signin xai main
4 pairs: 1 pass, 1 broken, 1 unknown, 1 skipped
```

Combo output:

```text
combo coder: PASS, answered by glm
  sonnet           BROKEN   skipped: BROKEN on every account
  fallback-chain   PASS
    gpt            UNKNOWN  openrouter/main 503 upstream overloaded (not saved)
    glm            PASS     opencode-go/main 2.1 s
```

Exit 0 when every pair ran (whatever the verdicts), 1 when the run failed or was interrupted.
Ctrl-C stops calls not yet sent; finished results stay saved.

## `nullrouter verdicts`

```text
nullrouter verdicts [--provider P] [--account N] [--model M] [--state pass|broken|unknown]
```

```text
provider    account  model              verdict  since             source    reason / next
anthropic   max      claude-opus-4-1    BROKEN   2026-10-07 09:12  test      model not available: 403 …
openrouter  main     anthropic/claude…  UNKNOWN  2026-10-07 09:13  retest    503 …; next retest 09:43 (step 3)
xai         main     grok-4-imagine     UNKNOWN  2026-10-07 08:00  test      timeout after 5 min; waiting: needs sign-in
anthropic   api      claude-haiku-4-5   BROKEN   2026-10-06 18:00  operator  set by the operator: not on our plan
```

```text
nullrouter verdicts clear <provider> <account> <model>        # → untested; `applied` / error if none
nullrouter verdicts mark <provider> <account> <model> [--note TEXT]   # → BROKEN, source operator
```

Both write through the server (`verdicts.set` op); with no server running, they append to
`routing/verdicts.jsonl` directly and print `saved; applies at next start`.

### Settings

```text
nullrouter verdicts settings                                  # show the [tests] values and defaults
nullrouter verdicts settings retest 1m,5m,30m,6h              # or `default`
nullrouter verdicts settings broken-retest 24h                # or `off`
nullrouter verdicts settings timeout image 10m                # type: text|embedding|tts|stt|image|video
nullrouter verdicts settings concurrency 2
```

Each writes `config.toml` atomically and asks the server to reload (`applied` / `saved; applies
at next start`), as `routing window` does. Invalid values print the rule and exit 1.

## `nullrouter combos`

```text
nullrouter combos            # every combo with its members, then `dropped combo …` lines
nullrouter combos NAME       # one; exit 2 if not loaded
```

```text
combo coder (llm):
  0. sonnet          unified
  1. fallback-chain  combo
     0. gpt          unified
     1. glm          unified
```

`resolve NAME` accepts a combo name and prints the same tree; `--json` prints the same value as
`combos NAME --json`. With none declared: `no combos; declare one with [[combo]] in config.toml`.

## Changes to existing commands

- `records list` gains `--test` / `--no-test`; test records show a `test` tag and their run id.
  `records get` shows `test`, `combo` and each attempt's `member`.
- `check` reports combo load errors and dropped combos, and warns `verdicts not being kept` when
  the journal can't write (disk full).
- `unified NAME` shows each member's verdict per account in a `verdicts` column.

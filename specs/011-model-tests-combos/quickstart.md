# Quickstart: validating model tests and combos

Automated checks run in GitHub Actions on push (no local cargo). The scenarios below are what
the suites prove and what the operator can repeat by hand against a mock or real home.

## Automated suites (CI)

| Suite | Proves | Spec |
|---|---|---|
| `nullrouter-engine/tests/verdict_judge.rs` | each core rejection, a plugin rule, 408/429/5xx/timeout/break/malformed success → the right verdict, for all six types | SC-001, US1 |
| `nullrouter-engine/tests/verdict_routing.rs` | a BROKEN pair is skipped for direct, unified and combo targets; other accounts serve; all-BROKEN fails with no upstream call | SC-002, US2 |
| `nullrouter-engine/tests/verdict_retest.rs` (simulated clock) | 503 ×3 then 200 → retests at ~1/5/30 min, then PASS, then none; BROKEN never retested unless on; waits while rate-limited or at the floor | SC-003, US3 |
| `nullrouter-engine/tests/verdict_store.rs` | restart and kill -9 keep every verdict; secret change, new sign-in, plugin change reset only the affected pairs; refresh doesn't | SC-005, US4 |
| `nullrouter-registry/tests/combos.rs` | clash, cycle, unknown member, kind conflict, empty, dropped-by-skipped-plugin; a plugin `combo` key refused; a `[[rejections]]` with 429 refused | FR-024, edge cases |
| `nullrouter-engine/tests/combo_walk.rs` | moves on only after a member is exhausted; never after output; request errors don't move on; a repeated unified model isn't retried | SC-006, US5 |
| `nullrouter-server/tests/combo_test.rs` | 3-level combo test output and which pair verdicts change | SC-007, US6 |
| `nullrouter-server/tests/secrets.rs` (extended) | no secret, prompt or output in test records, socket answers or `verdicts.jsonl` | SC-008 |
| `nullrouter-server/tests/models.rs` (extended) | combos listed in all four styles; an all-BROKEN target still listed | FR-025 |
| `nullrouter-cli/tests/read_golden.rs` (extended) | `verdicts`, `combos`, `test` output byte for byte | FR-017 |
| `nullrouter-engine/benches/engine.rs` | plan with 1000 verdicts and a 3-level combo within 5 % of baseline | SC-009 |

## Manual scenario (mock home)

1. Start from the 009 fixture home with two anthropic accounts and an openrouter account.
2. Add to `config.toml`:

   ```toml
   [[unified_model]]
   name = "sonnet"
   members = [{ provider = "anthropic", model = "claude-sonnet-4-5" }]

   [[combo]]
   name = "coder"
   members = ["sonnet", "gpt"]
   ```

3. `nullrouter serve`, then `nullrouter combos coder`: the tree prints. A client listing models
   sees `coder` next to `sonnet`.
4. `nullrouter test --all`: the prompt shows the call count; answer `y`. One line per pair.
5. With the mock returning `404 {"error":{"message":"model claude-sonnet-4-5 does not exist"}}` for
   account `max`: `nullrouter test anthropic/claude-sonnet-4-5 --account max` → BROKEN.
   `nullrouter records list --target sonnet` after a client request shows `max` skipped as BROKEN.
6. With the mock returning 503: the pair is UNKNOWN, and `nullrouter verdicts` shows the next
   retest about 1 minute later.
7. `nullrouter verdicts mark anthropic api claude-sonnet-4-5 --note "not on plan"`, restart the
   server, and `nullrouter verdicts` still shows it as set by the operator.
8. For an account added with `--env VAR`, restart the server with a different value in `VAR`: its
   pairs are untested again, and the other accounts' verdicts are unchanged.
9. `nullrouter test coder`: the combo output nests members and names the one that answered.

## Live check (opt-in, `NR_LIVE=1`)

`nullrouter test --all --yes` on the operator's real home, one model of each type they hold an
account for. Every result is PASS or an UNKNOWN whose reason the operator recognises (SC-010).
It bills real calls, image and video included; run it only when asked.

# Slice 004: operator runs

What is left of slice 004 after the code tasks: what you run, with which accounts, about how
long it takes, and what counts as a pass. Times are estimates for this machine (4 threads,
16 GB), not measurements.

**Order** (your plan, 2026-10-19; changed 2026-10-20: 012 merges first):

1. Now, on the branch as it is: T092 (benchmarks), and T097 (quickstart).
2. When you say 012 is merged, I rebase 004 onto `main`, resolve, run the engine and CLI
   tests, push, and read CI.
3. On the rebased branch, just before the merge: publishing the claude-code archive (below),
   then T098, the final gate and the merge.
4. After the merge: T031 and T084, in your whole-system live run.

Run everything in a normal terminal, not inside `claude-0router`. The live runs need
`~/.0router`, the network, and the `hermes` and `claude` binaries, and the sandbox denies all
three.

## Common setup

```sh
cd ~/Desktop/0router/.worktrees/004
git pull                                   # the branch as pushed (6b2defb or later)
export CARGO_HOME=$HOME/Desktop/0router/.cargo-home
export CARGO_TARGET_DIR=/mnt/storage/0router/target/004
export NULLROUTER_HOME=$HOME/.0router      # your real home: accounts, keys, builder; T097 alone uses a throwaway one
cargo build -p nullrouter-cli              # ~2 min after the earlier builds
NR=$CARGO_TARGET_DIR/debug/nullrouter
B=$CARGO_TARGET_DIR/release/nullrouter-builder   # the builder you built for the fixtures
```

Pause the 012 session's cargo runs while you do T092; the others don't mind sharing.

---

## T031: hermes live run, fills the echoed-reasoning reject table (after the merge)

Part of your whole-system live run. It doesn't gate the merge.

**Needs**
- hermes installed and on `PATH` (`hermes --version`). See T028 at the end.
- Signed-in accounts in `~/.0router` for the six chosen text providers: anthropic,
  openrouter, opencode-zen, opencode-go, xai, grok-cli. xai and grok-cli are sign-in
  accounts (slice 005): `$NR accounts signin …` if `$NR check` shows them without tokens.
- A key bound to the hermes adapter. Use `--adapter`. `--harness` only tags the key and runs
  nothing; the doc comment at the top of `harness_hermes.rs` says `--harness` and is wrong.

**Run**

```sh
$NR serve &                                  # 127.0.0.1:20129, home ~/.0router
$NR check                                    # no errors; the six providers have accounts
$NR keys issue hermes-live --adapter hermes  # prints the key once
NR_LIVE=1 NR_KEY=0r-… cargo test -p nullrouter-server --test harness_hermes -- --ignored --nocapture
```

`NR_MODELS=xai/grok-4,grok-cli/grok-4.5` limits a rerun to some providers.

**Takes** about 15 to 25 minutes: six providers, four turns each (plain, tool, `--continue`
replay, image). Each turn has a 300 s timeout.

**Pass**
- The test ends `test result: ok`: every turn of every provider exits 0 and prints something,
  and the tool turn prints the tool's sentinel (SC-001).
- A provider flagged as rejecting echoed reasoning (a 400 or 422 naming `reasoning_content`,
  `reasoning` or `reasoning_details`) is not a final failure. Send me the output. I add that
  provider to `REJECTS_ECHOED_REASONING` with the run date, and you rerun it until it passes.
- Send me the output either way: I record the results in `tests/harness/README.md`.

---

## T084: Claude Code through the full pipeline (after the merge)

Part of your whole-system live run. It doesn't gate the merge. After the merge, with the
archive published, you can also install from the catalogue (`$NR catalogue install
claude-code`) instead of the local source; both take the same steps.

**Needs**
- `claude` (Claude Code) on `PATH`.
- The same six providers' accounts as T031.
- The builder in `~/.0router/builder`. Your `setup` ran there, and `serve` passes its home to
  the builder. `serve` looks for the binary on `PATH` unless `config.toml` names it. Add one
  line under `[adapters]` in `~/.0router/config.toml`; if the file has no `[adapters]` table,
  add the table header as well:

  ```toml
  [adapters]
  builder = "/mnt/storage/0router/target/004/release/nullrouter-builder"
  ```

  `nullrouter adapters install` reads the line when it runs, so `serve` needs no restart for
  it. `serve` reads it only at startup, to rebuild after a kit upgrade.

  Afterwards, delete the `builder = …` line, and the `[adapters]` header too if you added it.
  Without the line, a later install stops at `queued` with `builder_not_installed`. The
  approved claude-code version keeps serving, because its module is already in the store.

- A review model: one model your accounts can serve, for example
  `anthropic/claude-sonnet-4-20250514`.

**Run**

```sh
$NR serve &                                            # if not already running
$NR adapters review-settings --model anthropic/claude-sonnet-4-20250514 --budget 60000
$NR adapters install adapters/community/claude-code    # → queued → building → in_review
$NR adapters show claude-code                          # note the version id: v0.1.0-xxxxxxxx
$NR adapters review claude-code <version> --retry      # → reported
$NR adapters show claude-code <version>                # read the report and its risk level
$NR adapters approve claude-code <version>
$NR keys issue cc-laptop --adapter claude-code         # prints the key once
NR_LIVE=1 NR_KEY=0r-… cargo test -p nullrouter-server --test harness_claude_code -- --ignored --nocapture
```

Before the merge, install from the local source: the catalogue index is read from `main`,
where `catalogue/` doesn't exist yet.

**Takes** about 20 to 30 minutes:
- install and build, 1 to 3 minutes;
- the review, 1 to 3 minutes, on one model call;
- six providers times two sessions (Bash tool, web search), each with a 180 s limit.

**Pass**
- The test ends `test result: ok`. On every provider, both sessions complete, and every record
  the key made shows the adapter ran (SC-008).
- Records of targets other than Anthropic show `server_tool_use` and `web_search_tool_result`
  removed with `target_cannot_carry_block`. On Anthropic, foreign thinking blocks show
  `foreign_block`. To see them: `$NR keys list` for the key's id, then
  `$NR records list --agent <key id>` and `$NR records show <record id>`.
- Send me the review's risk level and the per-provider pass/fail: they go into
  `tests/harness/README.md`.

---

## T092: benchmarks (before the rebase)

Run it on the branch as it is now. The pre-004 baseline (`f7518cd`) is valid against this
branch: the only difference is 004. After the rebase, the branch also holds 012, and the
comparison would mix the two.

No slice-003 baseline is left on this machine: it lived in the old `target/`, from before
builds moved to `/mnt/storage`. The baseline comes from `f7518cd`, main's head and the commit
004 merged last, so it is everything except 004. Both sides use the same `CARGO_TARGET_DIR`,
because Criterion keeps its baselines there.

**Run** (the machine otherwise idle, the 012 session paused)

```sh
export CARGO_HOME=$HOME/Desktop/0router/.cargo-home
export CARGO_TARGET_DIR=/mnt/storage/0router/target/bench
git -C ~/Desktop/0router worktree add /tmp/nr-pre-004 f7518cd
cd /tmp/nr-pre-004
cargo bench -p nullrouter-engine --bench engine -- --save-baseline pre-004
cargo bench -p nullrouter-server --bench server -- --save-baseline pre-004
cargo bench -p nullrouter-wire   --bench wire   -- --save-baseline pre-004

cd ~/Desktop/0router/.worktrees/004
cargo bench -p nullrouter-engine --bench engine -- --baseline pre-004 | tee /tmp/bench-engine.txt
cargo bench -p nullrouter-server --bench server -- --baseline pre-004 | tee /tmp/bench-server.txt
cargo bench -p nullrouter-wire   --bench wire   -- --baseline pre-004 | tee /tmp/bench-wire.txt
cargo bench -p nullrouter-adapters --bench adapters -- --save-baseline slice-004 | tee /tmp/bench-adapters.txt
cargo bench -p nullrouter-sandbox  --bench sandbox  -- --save-baseline slice-004 | tee /tmp/bench-sandbox.txt
git -C ~/Desktop/0router worktree remove /tmp/nr-pre-004
```

The adapters bench reports Criterion's mean, not a p95. This prints each bench's p95 over its
per-iteration sample times:

```sh
for f in $CARGO_TARGET_DIR/criterion/adapters/*/new/sample.json \
         $CARGO_TARGET_DIR/criterion/sandbox/*/new/sample.json; do
  python3 -c 'import json,sys; s=json.load(open(sys.argv[1])); t=sorted(x/i for x,i in zip(s["times"],s["iters"])); print(f"{t[int(0.95*(len(t)-1))]/1e6:9.3f} ms p95  {sys.argv[1]}")' "$f"
done
```

**Takes** about 45 to 75 minutes, most of it the two release builds and Criterion's runs.

**Pass**
- engine, server and wire against `pre-004`: each bench says `No change in performance
  detected` or `Change within noise threshold`. Any `Performance has regressed` on the
  no-adapter path blocks the merge (FR-030).
- SC-010: request side p95 ≤ 5 ms on the 1 MB request, adding `guard_request` at 1 MB to
  `sandbox/call_claude_code`. Event side: `guard_event` p95 ≤ 1 ms.
- Send me the `/tmp/bench-*.txt` files and the p95 lines. I write `bench-baseline.md` in
  slice 003's format. If a target is missed, I report the numbers to you before any
  optimising.

---

## T097: the non-live quickstart sections

Follow `quickstart.md`'s prerequisites. They set a throwaway `NULLROUTER_HOME`, which
replaces the common setup's `~/.0router` for this task only, and run the builder `setup` for
it (network, about 3 minutes). Before § 3, put the builder on `PATH` as the quickstart says.
Stop any `serve` you already have running first: both listen on 127.0.0.1:20129.

| § | Run | Pass |
|---|---|---|
| 0 | `cargo test --workspace` (~25 to 40 min locally) | all green |
| 1 | `PATH=/usr/bin:/bin $(command -v cargo) test -p nullrouter-server`, then `nr serve &` and `nr adapters list` | green; `serve` logs no builder or review-model warning; the list shows only hermes |
| 2 | `nr keys issue hermes-desktop --adapter hermes`, then `cargo test -p nullrouter-adapters --test hermes` | key printed once; tests green |
| 3 | `nr adapters install crates/nullrouter-adapters/tests/fixtures/noop`, then `nr adapters show noop` | reaches `in_review`, then quarantined with `no_review_model`; nothing serves |
| 4 | `cargo test -p nullrouter-adapters --test gate`, then `nr adapters install crates/nullrouter-adapters/tests/gate/invalid/multi_reason; echo $?` | green; exit 3; one line per reason, as in `gate/invalid/multi_reason.expected` |
| 5 | `cargo test -p nullrouter-server --test hostile` and `cargo test -p nullrouter-adapters --test guard` | green |
| 6 | `cargo test -p nullrouter-adapters --test tamper` | green |
| 7 | `cargo test -p nullrouter-server --test adapter_lifecycle` | green |
| 8 | `cargo test -p nullrouter-adapters --test kit_upgrade` | green |
| 9 | `cargo test -p nullrouter-adapters --test catalogue` | green |

Section 3's review and approve steps call a live model; T084 covers them, with claude-code
in place of noop. Section 10 is T084 and section 11 is T092.

The `cargo test -p nullrouter-server` runs in § 0, 1 and 5 include `records_no_content`, whose
three Claude Code cases take about 2 GB and 40 s each in a debug build. Without the 2.5 GB
`capped` wrapper that's fine.

**Takes** about 1 hour, most of it § 0 and § 1.

**Pass**: every row passes. Tell me any row that doesn't, or any step that doesn't match the
text: it goes in the commit message (T097), and I fix the quickstart. Four gaps found while
writing this runbook are already fixed in it: the old `~/Desktop/0router` and `./target`
paths, § 1's stripped `PATH` losing cargo, § 4's path to `multi_reason.expected`, and the
builder `setup` each new home needs.

---

## Publishing the claude-code archive (after the rebase, just before the merge)

The catalogue entry in `catalogue/index.toml` names
`https://github.com/NullCombinator/0xRouter/releases/download/adapter-claude-code-v0.1.0/claude-code-0.1.0.tar.gz`.
Publish it after the rebase is pushed and CI is green, just before the merge, so the release
tag points at the rebased head (the commit that merges) and, once the index is on `main`, it
never points at a missing file. You run it: it publishes under the repo's name. The rebase
doesn't touch the adapter's source, so the sha256 stays the same.

```sh
cd ~/Desktop/0router/.worktrees/004
git status -sb | head -1                    # must read: ## 004-client-side-adapters...origin/004-client-side-adapters (no ahead/behind)
tools/package-adapter.sh adapters/community/claude-code /tmp/claude-code-0.1.0.tar.gz
sha256sum /tmp/claude-code-0.1.0.tar.gz   # must be 7a5527de96bbf921e885d68a8a73120eff4a955bcdb1fa018263a6a437e4ddab
gh release create adapter-claude-code-v0.1.0 /tmp/claude-code-0.1.0.tar.gz \
  --repo NullCombinator/0xRouter --target "$(git rev-parse HEAD)" \
  --title "claude-code adapter 0.1.0" \
  --notes "Source: adapters/community/claude-code. sha256 7a5527de96bbf921e885d68a8a73120eff4a955bcdb1fa018263a6a437e4ddab"
```

**Takes** a minute.

**Pass**: this prints the same sha256.

```sh
curl -sL https://github.com/NullCombinator/0xRouter/releases/download/adapter-claude-code-v0.1.0/claude-code-0.1.0.tar.gz | sha256sum
```

Any later change to the adapter's source needs a new version: a new archive, a new release
and a new `[[entry.version]]` in the index.

---

## T098: final gate

**Needs**: T092 and T097 done before the rebase; the rebase pushed with CI green; the archive
published from the rebased head; and your OK. T031 and T084 come after the merge.

**Run**: I push and read CI through the github MCP.

**Pass**
- CI green: `cargo test --workspace` and `cargo clippy --workspace --all-targets -D warnings`.
- `unsafe` only in `crates/nullrouter-adapter-kit/src/abi.rs`. The task's literal
  `grep -rn "unsafe" crates/ --include='*.rs'` also matches the word in comments, strings and
  the gate's refusal fixtures, 12 files today. The check that means it:

  ```sh
  grep -rnE '\bunsafe[[:space:]]*(\{|fn\b|impl\b|extern\b|trait\b)|#!?\[unsafe\(' crates/ --include='*.rs' \
    | grep -v '^crates/nullrouter-adapter-kit/src/abi.rs'
  ```

  It passes when it lists only the gate's message strings (`gate.rs`) and the two refusal
  fixtures, adapter sources that must be refused:
  `adapters/tests/gate/invalid/unsafe_code` and `builder/tests/fixtures/unsafe_block`. That
  holds today.
- The published archive's sha256 matches the index.
- Then I ask you about the PR.

**Takes** a CI run, about 30 minutes.

---

## T028: hermes harness runner (after the merge, with T031)

Its code is written: `tests/harness/hermes/run.sh` and
`crates/nullrouter-server/tests/harness_hermes.rs`. The task is open because it has never run.
What it needs:
- hermes installed outside the session, on `PATH`;
- T031's accounts and key;
- the run itself, which is T031's command.

When T031 passes, T028 is done with it. Not covered: whether hermes streams. Its `streaming`
option is display-only, so the script can't force either API mode.

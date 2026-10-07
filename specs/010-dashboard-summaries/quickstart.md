# Quickstart: validating the summaries and the landscape

These scenarios prove the slice end to end. Contracts: [cli](contracts/cli.md),
[dashboard](contracts/dashboard.md). Shapes: [data-model](data-model.md).

Project rule: tests and clippy run in GitHub Actions, not on this machine. The `cargo test` lines
name the suites CI runs; run the binary steps by hand on a machine where building is allowed.
Benches stay local, by the user's decision.

## Prerequisites

```bash
export CARGO_HOME=$PWD/.cargo-home
export NULLROUTER_HOME=$(mktemp -d)        # a throwaway home; never your real ~/.0router
cargo build -p nullrouter-cli -j 2
alias nr=target/debug/nullrouter
```

A home with records: the summaries fixture (`crates/nullrouter-engine/tests/fixtures/summaries/`,
added by this slice) is a home whose totals, costs and percentiles are worked out by hand in
`expected.toml`. Copy it to `$NULLROUTER_HOME`, or send real traffic through `nr serve` with two
keys.

## 1. Totals for a period (US1)

```bash
nr usage                          # Today, local midnight → now
nr usage --period 7d
nr --json usage --period all | jq '{requests, tokens, cost}'
nr usage --period week            # exit 1, names the six periods
```

Expect, against the fixture's `expected.toml`:
- requests, input (uncached), cached and output equal the hand sums for each period;
- Est. Cost equals the hand figure to the cent, labelled "Estimated, not actual billing", with the
  note about price changes;
- the record on an account with no price, and the one with output tokens but no output price,
  are counted under "not priced" with their reasons;
- the record with no usage counts as a request and as "not reported";
- a record priced by a time-of-day schedule uses the rate in effect at its arrival.

## 2. Latency over the last 24 hours (US2)

```bash
nr latency
nr --json latency | jq '.providers[] | {id, own_ttft, last}'
```

Expect:
- only records from the last 24 hours count;
- for the fixture's fallback record (A failed, B served), A's last response is failed, B's
  resolved; B's own TTFT for that record is measured from B's attempt start, so it is smaller
  than the record's TTFT by A's time and the router overhead;
- the request refused before a key matched is in no row;
- a row with no first token prints `none`, never `0 ms`.

## 3. Tag a key (US5)

```bash
nr keys issue tagged --harness claude-code
nr keys issue plain
nr keys tag plain codex
nr keys tag tagged --clear
nr keys tag plain "$(printf 'bad\ttag')"    # exit 1, the tag stays "codex"
nr keys list                                # HARNESS column: -, codex
```

The `plain` key's secret printed at issue still authenticates after `keys tag`.

## 4. Pages agree with the CLI (SC-001)

```bash
cargo test -p nullrouter-dashboard -j 2 --test agreement --test twins
```

By hand, with `nr serve` running and signed in (spec 009 quickstart, steps 1 and 2):

| Page | Compare with |
|---|---|
| `/` landscape and key cards | `nr latency`, `nr usage --period today` (per-agent counts), `nr keys list` |
| `/providers?provider=<id>` "Last response" | `nr latency` provider row |
| `/usage?period=<p>` cards and graph | `nr usage --period <p>`, `nr latency` (last response) |

Read the CLI within the same second as the page's "as of", or compare with the agreement test,
which passes the page's `as_of` as the views' `at`.

## 5. The look (SC-007, SC-008)

```bash
cargo test -p nullrouter-dashboard --test style_guide      # local; new tokens carry 9router sources
```

Side by side (the user's judgement): 9router's Usage page (cards, period filter, provider
topology) in light mode, the mockups in `docs/dashboard/mockups` (`usage.html`, `endpoint.html`),
and this dashboard. With scripts turned off in the browser, the landscape, gauges and graph look
the same.

## 6. Speed and isolation (SC-004, SC-005)

```bash
cargo bench -p nullrouter-engine --bench usage_totals_100k     # local; warm < 50 ms, cold < 1 s
cargo bench -p nullrouter-engine --bench latency_24h_100k      # local; < 1 s
cargo bench -p nullrouter-dashboard --bench pages              # local; each page < 1 s
cargo bench -p nullrouter-engine --bench engine                # no regression from price::entry_at
cargo test -p nullrouter-dashboard --release --features fault -- --ignored isolation   # local
```

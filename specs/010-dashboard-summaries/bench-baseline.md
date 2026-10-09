# 010 summaries bench baseline (T048, SC-004, SC-005)

Benches stay local (CLAUDE.md): cloud timings don't compare with local ones. The first local run
fills these tables, with the machine named. Records are about 1 KB each (a request's three
lines), five agents in rotation.

Machine: _not recorded yet_

## `cargo bench -p nullrouter-engine --bench usage_totals_100k`

100,000 records over 30 daily segments. Warm: the same window again with the day cache filled.
Cold: a fresh copy of the home for every run. Targets: warm under 50 ms, cold under 1 s.

| Period | Warm median | Cold median | Warm < 50 ms | Cold < 1 s |
|---|---|---|---|---|
| today | | | | |
| 30d | | | | |
| all | | | | |

## `cargo bench -p nullrouter-engine --bench latency_24h_100k`

Every run cold. Target: under 1 s.

| Shape | Median | Under 1 s |
|---|---|---|
| 100,000 records over 30 days | | |
| 100,000 records in one day | | |

## `cargo bench -p nullrouter-dashboard --bench pages`

Spec 009's table (`specs/009-dashboard/bench-baseline.md`) plus the two pages this slice
changes. Target: every page under 1 s.

| Page | Median | Under 1 s |
|---|---|---|
| /endpoint | | |
| /usage | | |
| /usage?period=all | | |

## `cargo bench -p nullrouter-engine --bench engine`

No regression from `price::entry_at`. Run it before and after on the same machine.

| Bench | Before | After |
|---|---|---|
| (each `engine` bench) | | |

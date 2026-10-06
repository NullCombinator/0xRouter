# Bench baseline: records paging (slice 008)

`cargo bench -p nullrouter-engine --bench records_page` (Criterion, local; CI timings don't
compare). Medians, run with `nice` and `-j 2` on a 4-core machine under load (load average 7), so
expect noise of a few tens of percent. Targets are research R7.

| Case | Median | Target | |
|---|---|---|---|
| `newest_50/1k` (reference) | 3.19 ms | | |
| `newest_50/100k_30_days` | 3.47 ms | ≤ 20 ms, ≤ 2× the 1k case | met |
| `newest_50/100k_one_day` | 4.30 ms | ≤ 20 ms, ≤ 2× the 1k case | met |
| `after_50000_back/100k_30_days` | 15.4 ms | ≤ 1 s | met |
| `after_50000_back/100k_one_day` | 199 ms | ≤ 1 s | met |

## How it got here

- First run, 64 KiB blocks and no index: one-day newest page 8.5 ms (2.4× the 1k case, missed);
  one-day page 50,000 back 3.28 s (missed).
- A cursor page reads from an in-memory index of the segment (`journal/index.rs`): one-day deep
  page 155 to 199 ms.
- The line scan in `read_tail` reads only `t` and `id`, and blocks are 16 KiB: one-day newest page
  4.3 ms. At 100k records a day, the 60 s open-line margin spans about 70 records, so a smaller
  block over-reads less.

## Not covered by the bench

The deep-page figures are warm: Criterion's warm-up builds the index. The first deep page of a
segment in a process indexes the whole segment, about the old full scan (about 3 s for 100k
records of 1 KB). The `nullrouter` CLI is one process per command, so it pays that every time;
only a long-running server keeps the index.

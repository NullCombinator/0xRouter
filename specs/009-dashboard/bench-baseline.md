# 009 pages bench baseline (T072, SC-009)

`cargo bench -p nullrouter-dashboard --bench pages` loads each page through the real dashboard
listener, signed in, on a home with 50 accounts, 20 unified models and 100,000 request records
over 30 days (about 1 KB each). Target: every page under 1 s.

Baseline: **not recorded yet.** Local cargo is off for this slice and cloud timings don't compare
with local ones (CLAUDE.md), so the first local run fills this table, with the machine named.

| Page | Median | Under 1 s |
|---|---|---|
| /endpoint | | |
| /providers | | |
| /combo | | |
| /usage | | |
| /quota | | |
| /proxy-pools | | |
| /console-log | | |
| /settings | | |
| /providers/anthropic | | |
| /usage/records/&lt;newest&gt; | | |
| /quota?notices | | |

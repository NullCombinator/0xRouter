# Quickstart: validating the read model (slice 008)

Run from the `008-read-model` worktree. Build commands need `export CARGO_HOME=$PWD/.cargo-home`;
keep to `-j 2`, one crate at a time.

## 1. Nothing the operator sees changed (SC-001)

```bash
cargo test -p nullrouter-cli -j 2 --test read_golden
cargo test -p nullrouter-cli -j 2
```

Expected: every golden matches (stdout, stderr, exit code) for every listed read, text and
`--json`, server stopped and running; every existing CLI test passes unchanged. `git log` shows
the goldens committed before the code moved (research R4).

## 2. Both routes give the same answer (SC-002, SC-006)

```bash
cargo test -p nullrouter-server -j 2 --test views_routes
cargo test -p nullrouter-server -j 2 --test secrets
```

Expected: every view equal by both routes on every fixture home; no planted secret appears beyond
its last four characters in any `json` or `extra`.

## 3. The new reads

On a scratch home (`export NULLROUTER_HOME=$(mktemp -d)`), following `contracts/cli.md`:

```bash
nullrouter unified                      # "no unified models; …", exit 0
nullrouter behaviour show               # break_behaviour  restart  (default)
nullrouter behaviour set-break error_event
nullrouter behaviour show               # break_behaviour  error_event
```

Then declare two unified models in `config.toml` (one with members whose `context_length`
differs) and check `unified`, `unified <name>`, `unified nope` (exit 2, `not found: …`) and their
`--json` against `resolve <name> --json`.

## 4. Paging (SC-004, SC-005)

```bash
cargo test -p nullrouter-engine -j 2 --test records_page
cargo bench -p nullrouter-engine --bench records_page
```

Expected: paging with `--before` from newest to oldest yields every record once, in the full
listing's order, with and without filters, including out-of-order `open` lines within the margin.
The bench meets the targets in research R7; record the numbers in `bench-baseline.md`.

By hand, on a home with records:

```bash
nullrouter records list --limit 5
nullrouter records list --limit 5 --before <last id above>
nullrouter records list --before rq_01JZZZZZZZZZZZZZZZZZZZZZZZ   # no record …, exit 1
```

## 5. Review gate (SC-003)

Check that no CLI read command in `crates/nullrouter-cli/src/cmd/` builds an answer itself: each
listed read calls a view and renders it. `validate` is the only command left as it was.

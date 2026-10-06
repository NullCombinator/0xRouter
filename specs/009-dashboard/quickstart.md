# Quickstart: validating the dashboard

These scenarios prove the slice end to end. Contracts:
[dashboard-http](contracts/dashboard-http.md), [cli](contracts/cli.md),
[plugin-logo](contracts/plugin-logo.md), [style-guide](contracts/style-guide.md).

## Prerequisites

```bash
export CARGO_HOME=$PWD/.cargo-home
export NULLROUTER_HOME=$(mktemp -d)        # a throwaway home; never your real ~/.0router
cargo build -p nullrouter-cli -j 2
alias nr=target/debug/nullrouter
```

## 1. On by default, nothing without a token (US1)

```bash
nr serve &                                              # clients 20129, dashboard 20130
nr dashboard status                                     # on, listening; token: none
curl -si http://127.0.0.1:20130/quota | head -3         # 303 → /signin
curl -s  http://127.0.0.1:20130/signin | grep -o 'nullrouter dashboard token'
```

## 2. Sign in once; replacing the token signs out (US1)

```bash
TOKEN=$(nr --json dashboard token | jq -r .token)
curl -si -c jar -d "token=$TOKEN" http://127.0.0.1:20130/signin | grep -i '^set-cookie'
curl -s  -b jar http://127.0.0.1:20130/quota | grep -o 'as of [^<]*'
curl -si -d "token=nrd_wrong" http://127.0.0.1:20130/signin | head -1          # 401, after a delay
curl -si -X DELETE -b jar http://127.0.0.1:20130/quota | head -1               # 405
curl -si -H 'Host: evil.example:20130' -b jar http://127.0.0.1:20130/quota | head -1   # 421
nr dashboard token >/dev/null
curl -si -b jar http://127.0.0.1:20130/quota | head -1                         # 303 → /signin
```

In a browser: open `http://127.0.0.1:20130`, enter the token, restart the browser: it doesn't
ask again. Set `[dashboard] enabled = false`, restart `serve`: the port is closed, clients are
served, `nr dashboard status` says off.

## 3. Pages agree with the CLI; every fact has a twin (SC-001, SC-002)

```bash
cargo test -p nullrouter-dashboard -j 2 --test agreement --test twins
```

By hand, against the fixture home or your own:

| Page | Commands |
|---|---|
| Endpoint & Key | `nr check` (endpoint), `nr keys list` |
| Providers | `nr providers`, `nr accounts list --long`, `nr plugins list --community`, `nr model <p> <m>` |
| Quota Tracker | `nr accounts list --long`, `nr quota`, `nr routing` |
| Usage | `nr records list --limit 50`, `nr records list --limit 50 --before <id>`, `nr records show <id>` |
| Settings | `nr behaviour show`, `nr check` (home), `nr dashboard status` |

Each `<time>` holds the CLI's UTC value in `datetime`; the page shows local time and names the
zone.

## 4. Every notice reaches its page and the panel (SC-003)

```bash
cargo test -p nullrouter-dashboard -j 2 --test notices
nr --json check | jq '.notices[] | [.subject, .text]'
```

Open any page with `?notices`: every line is listed. Open each subject's page: its lines are
there.

## 5. Look only (FR-013, FR-014)

Every control that would change something is disabled and names the CLI command or says it isn't
built yet. `cargo test -p nullrouter-dashboard --test access -- no_write_routes`.

## 6. Logos (US8, SC-011)

```bash
nr --json check | jq '.notices[] | select(.text | test("logo ignored"))'   # none on a clean home
cargo test -p nullrouter-registry -j 2 logo
cargo test -p nullrouter-dashboard -j 2 --test logos
```

Put a user plugin with an oversized logo in `$NULLROUTER_HOME/plugins/`: it loads and serves,
Providers shows its text icon, and `check` prints `note: logo ignored: <id>: …`. After updating
`ref/9router`, `node tools/gen-bundled/generate.mjs` rewrites the logos and `plugins/LOGOS.md`.

## 7. Faults never reach clients (SC-005)

```bash
cargo test -p nullrouter-dashboard --release --test isolation -- --ignored
```

p95 with pages loading and failing is within 1 ms of the dashboard-off run, with 0 extra
failures. Start a second `serve` with port 20130 taken: clients are served, `nr dashboard status`
says not listening, `nr check` warns.

## 8. Looks like 9router (SC-007, SC-008)

```bash
cargo test -p nullrouter-dashboard --test style_guide    # source check needs ref/9router
```

Side by side (the user's judgement): 9router's dashboard in light mode, the mockups in
`docs/dashboard/mockups`, and this dashboard. Compare the sidebar, card, badge, button, table,
input, window and panel for color, type, spacing and shape.

## 9. No secrets, no prompts, offline, speed (SC-006, SC-010, SC-009)

```bash
cargo test -p nullrouter-dashboard -j 2 --test secrets --test offline
cargo bench -p nullrouter-dashboard --bench pages        # local; 100k-record home, each page < 1 s
cargo bench -p nullrouter-engine --bench keys_last_used_100k
```

# Quickstart: validating the dashboard

These scenarios prove the slice end to end. Contracts: [dashboard-http](contracts/dashboard-http.md),
[cli](contracts/cli.md), [style-guide](contracts/style-guide.md).

## Prerequisites

```bash
export CARGO_HOME=$PWD/.cargo-home
export NULLROUTER_HOME=$(mktemp -d)        # a throwaway home; never your real ~/.0router
cargo build -p nullrouter-cli
alias nr=target/debug/nullrouter
```

## 1. On by default, nothing without a token (US2)

```bash
nr serve &                                  # client port 20129, dashboard 20130
curl -si http://127.0.0.1:20130/accounts | head -3        # 303 → /signin
curl -s  http://127.0.0.1:20130/signin | grep -o 'nullrouter dashboard token'
```

Expected: no page except the one naming `nullrouter dashboard token`.

## 2. Issue a token and sign in once (US2)

```bash
TOKEN=$(nr dashboard token --json | jq -r .token)
curl -si -c jar -d "token=$TOKEN" http://127.0.0.1:20130/signin | grep -i '^set-cookie'
curl -s -b jar http://127.0.0.1:20130/accounts | grep -o 'as of [^<]*'
curl -si -d "token=nrd_wrong" http://127.0.0.1:20130/signin | head -1   # 401, after a delay
nr dashboard token >/dev/null               # replace
curl -si -b jar http://127.0.0.1:20130/accounts | head -1               # 303 → /signin
curl -si -H 'Host: evil.example:20130' -b jar http://127.0.0.1:20130/accounts | head -1   # 421
```

In a browser: open `http://127.0.0.1:20130`, enter the token, close and reopen the browser. It
doesn't ask again.

## 3. Pages agree with the CLI (SC-001)

Load the agreement fixture home, which has every status from User Stories 1–5:

```bash
cargo test -p nullrouter-dashboard --test cli_agreement
```

By hand, compare a page with its commands:

```bash
nr accounts list --long; nr quota          # vs /accounts
nr routing; nr records list --limit 50     # vs /routing and /records
nr providers; nr plugins list --community; nr unified   # vs /models
nr keys list; nr behaviour show            # vs /keys
nr check                                   # each warning appears on its page
```

Times on pages are in this machine's zone. Hover is not needed: the `datetime` attribute holds
the CLI's UTC value.

## 4. Look-only (FR-010)

On every page there are no buttons or forms that change anything. Only filters, paging, links,
and the sign-in form exist.

```bash
cargo test -p nullrouter-dashboard --test access -- no_write_routes
```

## 5. Faults never reach clients (SC-003)

```bash
cargo test -p nullrouter-dashboard --test isolation --release -- --ignored
```

Expected: p95 with pages loading and failing is within 1 ms of the dashboard-off run, with 0
extra failures. Also start a second `serve` with port 20130 taken: clients are still served,
and `nr check` prints `warning: dashboard not listening`.

## 6. No secrets, no prompts, offline (SC-004, SC-008)

```bash
cargo test -p nullrouter-dashboard --test secrets
cargo test -p nullrouter-dashboard --test offline     # any outbound fetch fails the test
```

In a browser with networking off, every page looks the same as online.

## 7. Looks like 9router (SC-005, SC-006)

```bash
cargo test -p nullrouter-dashboard --test style_guide  # needs ref/9router for the source check
```

Side by side (the user's judgment): run 9router's dashboard in light mode and open both. Compare
card, badge, button, table, input and navigation for color, type, spacing and shape. The layout
is expected to differ.

## 8. Speed (SC-007)

```bash
cargo bench -p nullrouter-dashboard --bench pages   # local only; 100k-record home
```

Expected: each page builds in under 1 s.

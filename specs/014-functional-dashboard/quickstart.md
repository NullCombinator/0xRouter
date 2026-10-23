# Quickstart: validating the functional dashboard (014)

## Prerequisites

- A build of this branch (`cargo build -p nullrouter-cli`).
- A home on a Unix filesystem with a short path (the operator socket's SUN_LEN), for example
  `export NULLROUTER_HOME=/tmp/nr-014`.
- A browser on the same machine.

## 1. Automated checks (CI)

```bash
export CARGO_HOME=$PWD/.cargo-home
cargo test -p nullrouter-server ops::              # every operation's unit tests
cargo test -p nullrouter-cli                       # unchanged CLI tests + new commands
cargo test -p nullrouter-cli --test dashboard_parity   # SC-002, SC-003
cargo test -p nullrouter-dashboard                 # forms, CSRF (SC-005), secret sweep (SC-004), confirm (SC-007), no script (SC-008)
```

Expected: all green. `dashboard_parity` prints one line per FR-002 change, `browser == cli`.

## 2. Fresh setup from the browser only (SC-001)

1. `nullrouter serve &` then `nullrouter dashboard token`. Open `http://127.0.0.1:20130` and enter
   the token.
2. Providers → a provider (or "Add OpenAI Compatible" pointing at a mock or real endpoint) → add an
   account by pasting its key. The key is not shown again anywhere.
3. Endpoint & Key → Issue key. Copy the key from the one-time box. Reload: the box is gone.
4. Combo → declare a unified model with that provider's model.
5. Send one request with the key to the unified model (e.g. with `curl`).
6. Console Log lists the four changes. `nullrouter dashboard log` prints the same.

Expected: done in under 10 minutes with no terminal after step 1, except the request in step 5.

## 3. Guards

- Revoke the key: a confirm window names what is lost. Cancel, and nothing changes. Confirm, and the
  key is revoked.
- Post a change from a page on another origin (any local HTML file with a form to `/do/keys/revoke`):
  403, nothing changes, and the log records nothing.
- Reload the page a change returned to: nothing repeats.

## 4. Long task

Combo → Test a combo with two members. A combo test is one request through the combo, so it bills
one call and starts at once. Test All with two accounts plans two billed calls: its confirm window
says "2 billed calls: 2 text. Continue?". Either way the page says "started at …" and refreshes
itself until the verdict shows, and `nullrouter verdicts` agrees.

## 5. Sign-in (user, real account)

Providers → an OAuth provider → Sign in. The browser goes to the provider's page and comes back to
the dashboard with the account signed in. For a code-page provider, paste the code into the
write-only field.

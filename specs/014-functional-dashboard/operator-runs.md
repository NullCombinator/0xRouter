# Operator runs: 014-functional-dashboard (T049)

**Date:** 2026-10-22, at cc1d272. **How:** quickstart.md §§ 1–4 on a scratch home (`/tmp/nr-014`,
server `127.0.0.1:20229`, dashboard `127.0.0.1:20230`), the debug build of this commit, and a local
OpenAI-style mock on `127.0.0.1:20231`. A Python script played the browser: one cookie jar, each
post carried the `_t` read from the page it came from and the dashboard's `Origin`, and it
followed each 303. § 5 (real OAuth sign-in) joins the user's whole-system live run.

## § 1 Automated checks

The same commit, run locally with `CARGO_TARGET_DIR` on the HDD:

| Crate | Result |
|---|---|
| registry | 13 test binaries ok, 0 failed |
| server | 47 ok, 0 failed |
| dashboard | 20 ok, 0 failed |
| cli (includes `dashboard_parity`) | 23 ok, 0 failed |
| engine (at 55b5502) | 61 ok, 1 failed: `adapter_store` hit its sandbox deadline under load; it passed 3 of 3 when rerun on its own. That code is from slice 004 and this slice doesn't touch it. |

`cargo clippy --workspace --all-targets -- -D warnings` is clean.

## § 2 Fresh setup from the browser only (SC-001)

| Step | Result |
|---|---|
| 1. Sign in with the token | 303 to `/`. The cookie is a session id, not the token. |
| 2. Private endpoints on (the mock is on loopback) | Unconfirmed: 303 to the confirm window. Confirmed: applied. |
| 2. Add OpenAI Compatible `mockco` → mock, model `m1` | "mockco: added (1 model), applied" |
| 2. Add account by pasting a marker key | "mockco/main: applied". The marker appears on no page and in no log entry. |
| 3. Issue key | Shown once in the one-time box. A reload doesn't show it again. |
| 4. Declare unified model `pong` = `mockco/m1` | "pong: declared (1 member), applied" |
| 5. A client request to `pong` with the new key | 200, the mock's answer. The mock got the account's key. |
| 6. Console Log vs `nullrouter dashboard log --json` | 6 entries each, the same changes |

Scripted, § 2 took 5.7 s. The "under 10 minutes by hand" target is for the user's live run.

## § 3 Guards

| Check | Result |
|---|---|
| Revoke without confirming | 303 to `/endpoint?window=revoke&key=qs`. The home is byte-identical (log file excluded). |
| A post from another origin (`Origin: http://evil.example`), even with a valid `_t` and `confirmed=on` | 403. The home is byte-identical and the change log doesn't grow. |
| Confirmed revoke | "qs: applied". `nullrouter keys list` shows it revoked. |
| Reload the page a change returned to, twice | Nothing repeats; the log count is unchanged. |
| Sign out | 303 to `/signin`. The old cookie then gets `/signin?next=%2F`. |

In the first run the home differed in § 3 because the request record from § 2.5 was still being
written when the "before" snapshot was taken. The rerun waited a second, and the home stayed
byte-identical.

## § 4 Long task

| Check | Result |
|---|---|
| Test combo `duo` (members `pong`, `pong2`) | One billed call, so no confirm window. Task started; the page refreshes; done within 2 s. `nullrouter verdicts`: `duo PASS, answered by pong`. |
| Test All with two accounts on `mockco` | Unconfirmed: refused with "2 billed calls: 2 text. Continue?", the same text the window shows. Confirmed: started, done within 2 s. Both pairs PASS. |
| Upstream calls | 1 per combo test and 2 for Test All, as planned |

quickstart.md § 4 said a two-member combo test asks for confirmation. It doesn't: a combo test
is one request. Corrected.

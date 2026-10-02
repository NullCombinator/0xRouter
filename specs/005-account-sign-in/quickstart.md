# Quickstart: validating slice 005

Formats: [contracts/operator-cli.md](contracts/operator-cli.md),
[contracts/signin-quota-schema.md](contracts/signin-quota-schema.md). States:
[data-model.md](data-model.md).

```bash
export CARGO_HOME=$PWD/.cargo-home
alias nr='cargo run -q -p nullrouter-cli --'
```

## 1. Automated suite (mock identity provider and mock providers)

```bash
nice cargo test -p nullrouter-registry -j 2     # gate, fit, schema sections, extractor
nice cargo test -p nullrouter-engine -j 2       # flows, refresh, states, quota, tally
nice cargo test -p nullrouter-server -j 2       # end to end with signed-in mock accounts
nice cargo test -p nullrouter-cli -j 2          # signin over stdin, quota, list
```

Expected: all pass. Notable tests and what they prove:

| Test | Proves |
|---|---|
| `signin_device_code`, `signin_pkce_paste`, `signin_headless` | sign-in completes with no display and no listener on the operator's device (SC-001) |
| `terms_warning_every_time` | anthropic sign-in asks every time; "n" writes nothing (FR-004) |
| `expiry_soak` | 2 s tokens over 20+ lifetimes, with idle gaps, zero client failures (SC-003) |
| `refresh_dedup`, `refresh_rotation_crash` | one refresh per account; newest refresh token always on disk |
| `needs_signin_named`, `refused_named` | account named in list, records and informational error (SC-005) |
| `fallback_signin_accounts` | 429/5xx/timeout never reach the client while another account serves (SC-004) |
| `quota_extract_*`, `quota_idle_polls`, `quota_failed_poll` | shown values equal reported values with poll time (SC-006, SC-007) |
| `tally_matches_usage` | per-model tally equals summed reported usage, across restarts (SC-008) |
| `secrets_sentinel` | zero tokens, codes or verifiers anywhere visible (SC-009) |

## 2. Harnesses against signed-in mock accounts

```bash
NR_HARNESS=1 nice cargo test -p nullrouter-server --test harness -j 2
```

Python and Node SDKs and Claude Code send streamed and non-streamed requests through mock
anthropic, xai and grok-cli sign-in accounts. Expected: zero client errors (SC-002).

## 3. Manual sign-in over SSH (real accounts)

On a machine with no browser (`unset DISPLAY WAYLAND_DISPLAY`):

```bash
nr accounts signin grok-cli work     # open the shown link on your phone, approve
nr accounts signin xai main          # open the link elsewhere, paste the final address back
nr accounts signin anthropic max     # read the warning, answer y, paste the code
nr accounts list --long
```

Expected: each finishes in under 3 minutes, `accounts list` shows the three as `signin`,
`active`, with shortened secrets.

## 4. Serving and quota with real accounts

```bash
nr serve &
nr keys issue laptop                 # copy the key into NR_KEY
curl -s localhost:20129/v1/chat/completions -H "authorization: Bearer $NR_KEY" \
  -d '{"model":"grok-cli/grok-build","messages":[{"role":"user","content":"hi"}],"stream":true}'
ANTHROPIC_BASE_URL=http://127.0.0.1:20129 ANTHROPIC_API_KEY=$NR_KEY claude -p "say ok"
nr quota                             # windows, reset times, last poll per account
nr quota history grok-cli work --limit 3
```

Expected: answers in each style; `quota` matches the provider's own usage page at the shown poll
time; history entries carry tallies matching `nr records list` for the same period. xai shows
"quota not reported" unless live check L2 found headers.

## 5. Live checks (opt-in)

```bash
NR_LIVE=1 nice cargo test -p nullrouter-engine --test live -- signin -j 2
```

Runs research L1–L5 with the operator's accounts and prints what each found. Never in CI.

## 6. Out-of-service accounts by hand

Revoke 0router's access on the provider's account page, then send a request:

```bash
nr accounts list                     # → needs sign-in since … (invalid_grant) + command
nr records list --provider xai       # → skipped: needs sign-in: run nullrouter accounts signin xai main
nr accounts signin xai main          # back to active on the next request, no restart
```

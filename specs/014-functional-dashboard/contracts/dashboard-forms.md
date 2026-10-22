# Contract: dashboard forms (014)

## Every form

- Method: POST to `/do/<area>/<op>`, with the body `application/x-www-form-urlencoded`.
- Required fields: `_t` (the form token, R3) and `_back` (the page path to return to; it must be a
  dashboard page path, otherwise `/endpoint`).
- Checks, in this order, before any change:
  1. Host (009 R7) → 421.
  2. A valid session cookie → `303 /signin`.
  3. `Origin` equals the dashboard origin; when `Origin` is absent, `Referer` must → else 403 page "This change didn't come from the dashboard."
  4. `_t` matches → else 403 page with the same text.
- Answer:
  - success → `303 <_back>?done=<flash>`;
  - refusal → `303 <_back>?window=<form window>&done=<flash>`, so the form reopens with the error
    next to its field and the non-secret fields kept;
  - started task → `303 <_back>?task=<id>`.
- Every handler writes one change-log line (R7) before answering.
- The page that receives a `done` flash shows its text in a notice at the top. A `once` flash shows
  the value in a box with the warning "This is the only time it is shown."

## Write-only fields

`secret` (API key), `password` (proxy), `code` (sign-in paste) are `<input type="password"
autocomplete="off">`. Handlers move them into `SecretString` at once. They are never echoed, logged,
kept in a flash, or put in a redirect.

## Routes

| Op id | Route | Fields | Page / window | CLI twin |
|---|---|---|---|---|
| accounts.add | /do/accounts/add | provider, name, secret, order? | Providers → provider window | accounts add |
| accounts.signin | /do/accounts/signin | provider, name, accept_terms? | provider window → task | accounts signin |
| accounts.signin_paste | /do/accounts/signin-paste | task, code | task panel | accounts signin --paste |
| accounts.priority | /do/accounts/priority | provider, name, priority | account row | accounts priority |
| accounts.enable / disable | /do/accounts/enable, /disable | provider, name | account row | accounts enable/disable |
| accounts.exclusive | /do/accounts/exclusive | provider, name, on | account row | accounts exclusive |
| accounts.remove | /do/accounts/remove | provider, name | confirm window | accounts remove |
| keys.issue | /do/keys/issue | name, harness?, break?, adapter? | Endpoint & Key | keys issue |
| keys.revoke | /do/keys/revoke | key | confirm window | keys revoke |
| keys.tag | /do/keys/tag | key, text (empty clears) | key card | keys tag |
| keys.set_break | /do/keys/set-break | key, behaviour | key card | keys set-break |
| keys.set_adapter | /do/keys/set-adapter | key, harness (empty clears) | key card | keys set-adapter |
| behaviour.set_break | /do/behaviour/set-break | behaviour | Settings | behaviour set-break |
| routing.set / unset | /do/routing/set, /unset | provider, name, key, value | account window | routing set/unset |
| routing.set_plugin / unset_plugin | /do/routing/set-plugin, /unset-plugin | provider, key, value | provider window | routing set-plugin/unset-plugin |
| routing.window | /do/routing/window | target?, duration | Settings | routing window |
| connection.set / unset | /do/connection/set, /unset | provider, model?, key, value | provider window | connection set/unset |
| proxy.add | /do/proxy/add | name, url, username?, password? | Proxy Pools | proxy add |
| proxy.remove | /do/proxy/remove | name | confirm window | proxy remove |
| proxy.use / clear | /do/proxy/use, /clear | proxy, scope (all/provider/account), target | Proxy Pools | proxy use/clear |
| proxy.fixed | /do/proxy/fixed | name | Proxy Pools → task | proxy fixed |
| quota.interval | /do/quota/interval | provider, name, every | Quota Tracker | quota interval |
| quota.poll | /do/quota/poll | provider, name | Quota Tracker → task | quota poll |
| quota.ack | /do/quota/ack | id (or all), provider?, account? | Quota Tracker | quota ack |
| plugins.install / uninstall | /do/plugins/install, /uninstall | id | side panel; uninstall via confirm | plugins install/uninstall |
| plugins.private_endpoints | /do/plugins/private-endpoints | on | Settings; on via confirm | plugins private-endpoints |
| providers.enable / disable / hide / show | /do/providers/<op> | id | provider settings window | providers <op> |
| providers.add_compatible | /do/providers/add-compatible | id, name?, style, base_url, models (one per line) | "Add … Compatible" window | providers add-compatible |
| adapters.install | /do/adapters/install | path | adapter settings window | adapters install |
| catalogue.install | /do/catalogue/install | harness, version? | adapter settings window → task | catalogue install |
| adapters.review / build / rebuild | /do/adapters/<op> | harness, version?, retry? | side panel → task | adapters <op> |
| adapters.approve / reject | /do/adapters/approve, /reject | harness, version, note? | side panel | adapters approve/reject |
| adapters.clear | /do/adapters/clear | harness, version | confirm window | adapters clear --yes |
| adapters.remove | /do/adapters/remove | harness, version?, force? | confirm window | adapters remove --yes |
| adapters.review_settings | /do/adapters/review-settings | model, budget, reserve_output?, clear? | adapter settings window | adapters review-settings |
| alerts.ack | /do/alerts/ack | id (or all) | Endpoint & Key | alerts ack |
| unified.add / set / remove | /do/unified/<op> | name, members (one per line) | Combo; remove via confirm | unified <op> |
| combos.add / set / remove | /do/combos/<op> | name, members (one per line) | Combo; remove via confirm | combos <op> |
| test.run | /do/test/run | target or all, account?, confirmed? | Combo / provider window → confirm when billed > 1 → task | test --yes |
| verdicts.mark / clear | /do/verdicts/mark, /clear | provider, account, model, note? | provider window | verdicts mark/clear |
| verdicts.settings | /do/verdicts/settings | setting, type?, value | Settings | verdicts settings |

Out of scope (CLI only): `records prune|forget`, `quota prune|forget`, `dashboard token`, `serve`.

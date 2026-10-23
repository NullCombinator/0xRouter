# Security review: 014-functional-dashboard (T048)

**Scope:** the forms layer (`forms.rs`, `changes/*.rs`, `lib.rs`, `headers.rs`), flash, write-only fields, the change log, the shared ops, the custom compatible provider, the sign-in hand-back (`set_return`), and engine tasks. I also re-checked L1 (cookie scope). The review is static against the working tree of `014-functional-dashboard`. Every finding below was read in the code.

## Summary

The CSRF design holds against browsers:
- Every `/do/<area>/<op>` POST is checked in this order: Host (421), method (405), session cookie, then `Origin` (or `Referer` when `Origin` is absent).
- If `Origin` and `Referer` are both missing, the post is refused.
- `_t` is checked in constant time (`forms.rs:156-171`).
- The page sends a CSP with no `script-src`, plus `form-action 'self'`, `frame-ancestors 'none'`, `X-Frame-Options: DENY`, CORP same-origin and `no-store` on every response.

Other parts are sound:
- **`_back`:** it can't be used as an open redirect, and it can't split a header.
- **Write-only fields:** they are never echoed back, and a new key is shown once from a session-bound flash that is read once.
- **Change log:** the file is 0600, bounded, and serde-escaped.
- **Sign-in hand-back:** its return URL can't be controlled by an attacker.
- **Tasks:** they are bounded as specified.
- **Custom provider:** its TOML is built safely through `toml::Value`, and a private address is gated.

The weak point is the **cookie**:
- Its value is the long-lived dashboard token (`signin.rs:108`, `access.rs:101-104`, 400-day `Max-Age`).
- `_t` is derived only from that value (`forms.rs:131-133`).
- So anyone who receives the cookie gets the same write authority as the CLI. They send their own `Host` and `Origin`, sign in with the token, read `_t` from any page, and post.
- Research R10 says "a cookie sent to another local server is still useless for posting here without the page's token". That is false for any non-browser client.

There are also five Lows (a TOML injection, a URL echo into the log and flash, unescaped log output in the terminal, unlocked `config.toml` writes, and `adapters.install` taking any server-readable path) and four Info items.

## Findings

| ID | Severity | Location | Issue | Fix |
|---|---|---|---|---|
| M1 (was L1) | **Medium** | `crates/nullrouter-dashboard/src/access.rs:24,101-104`; `pages/signin.rs:108`; `forms.rs:131-133`; `specs/014-functional-dashboard/research.md:161-167` | **What happens:** browsers scope cookies by host, not by port, so the cookie goes to every server the browser visits on `127.0.0.1`. The cookie value *is* the dashboard token, kept for 400 days. Its holder signs in from any client (sign-in allows a missing `Origin`), GETs a page to read `_t`, and posts with its own `Host` and `Origin`. The `Origin` check and `_t` only stop a browser on a foreign origin. **Why it matters now:** before 014 this gave read access. Now it gives every op, for example `keys.issue` (the new key is shown to the holder, who can then spend the operator's accounts through `127.0.0.1:20129`), `plugins.private_endpoints`, `providers.add_compatible`, `proxy.use` for all providers, `adapters.approve`, `accounts.remove` and `keys.revoke`. **Why the old rationale fails:** it said "a process running as your own user can already read `~/.0router`". That doesn't cover the processes that can get and use the cookie: another local OS user's server on 127.0.0.1 (loopback is shared between users), host-network containers, and same-user processes in a sandbox (Landlock, Flatpak, snap) that may listen and connect on loopback but can't read `~/.0router`. **What remains safe:** a remote recipient, such as a server behind `ssh -L` or `kubectl port-forward`, gets the token but can't reach the loopback dashboard. Through the operator's browser it is stopped by `Origin` and `_t`. | (1) Stop using the token as the cookie. At sign-in, create a random 256-bit session id and keep only its digest on the server, in memory or in a 0600 file. Give it an absolute lifetime (for example 7 days) and an idle lifetime (for example 12 h). Bind `_t` and flashes to that id, and add a sign-out. A leaked cookie then never exposes the token, and it expires. (2) Give the dashboard its own cookie host. One option is a dedicated name such as `http://nullrouter.localhost:<port>`: add it to `HostRule` and print it from `dashboard token`. A host-only cookie set there isn't sent to `127.0.0.1` or `localhost`. Check which target browsers resolve `*.localhost` before making it the default. On Linux, `127.0.0.2` is a fallback. (3) Optionally, ask for the token again before the highest-impact ops (`keys.issue`, `plugins.private_endpoints`, `adapters.approve`, `proxy.use` with scope `all`). Correct R10 in `research.md`. |
| L1 | Low | `crates/nullrouter-server/src/ops/routing.rs:476` (`edit_key`), reached from `routing.rs:453` and `crates/nullrouter-dashboard/src/changes/routing.rs:42` | **TOML injection in `routing.window`.** The target from the form is written as `format!("\"{key}\"")` without escaping. A target with `"` and a newline adds arbitrary keys and tables to `config.toml`, for example `<valid target>" = "1h"\n[some.table]\nk = v\n#`. Only the loader check (`config_text::check`) stands in the way. That bypasses each op's own validation and the confirm windows. Only an authenticated poster can do this, but with M1 that can be another local principal. | Write the key with `config_text::quoted()`. Refuse control characters (`\n`, `\r`) in the target before editing, or build the edit with `toml_edit`. Add a test that a target containing a quote and a newline is refused or stays data. |
| L2 | Low | `crates/nullrouter-registry/src/compatible.rs:80,82,85`; `crates/nullrouter-server/src/ops/plugins.rs:143`; `crates/nullrouter-dashboard/src/changes/mod.rs:220-221,279-280,302-305` | **A credential-bearing URL is echoed into the flash, the refilled form and `dashboard-log.jsonl`.** The `base_url` refusals put the whole URL in the error text, for example `"base_url {base_url} must have no credentials, query or fragment"` and the private-address refusal. A pasted `https://u:p@host/v1` or `https://host/v1?api_key=…` therefore lands in the change log's `text` field (on disk), in the refusal flash, and back in the `base_url` field. The same refill happens for the proxy `url` field (`pages/proxy_pools.rs:82`); there the error text doesn't carry the URL. This contradicts R7's claim that no secret can reach the log. | Strip userinfo and the query from any URL before it goes into an error message, or don't quote the URL at all. Drop a kept `url` or `base_url` value that has userinfo or a query. Add both cases to `tests/secret_sweep.rs`. |
| L3 | Low | `crates/nullrouter-cli/src/cmd/dashboard.rs:69`; `crates/nullrouter-dashboard/src/changes/accounts.rs:12` (and the other handlers' subjects) | **The change log prints raw input to the terminal.** Subjects are raw form input and are logged even when the op is refused. For example, `accounts.add` with a name containing ESC sequences is refused, but `subject = "{provider}/{name}"` is written verbatim. `nullrouter dashboard log` then prints it unfiltered, so a poster can plant terminal escape sequences. JSON escaping protects the file, not the terminal. | Filter control characters out of the subject and text at write time (the existing `printable()` helper from the sign-in L1 fix does this), or at print time. Test it with a name containing `\x1b`. |
| L4 | Low | `crates/nullrouter-server/src/ops/config_text.rs:131-134`; `ops/proxy.rs:205-208`; `ops/model_tests.rs:475-478`; `ops/plugins.rs:124-131,161-164` | **`config.toml` writes are not locked, and they share one temp name.** Each write reads, edits, then writes the fixed `config.toml.tmp` and renames it, with no lock. R8 says file-writing ops "take the same file locks the CLI takes", but I found no lock on these paths. The dashboard guard allows two posts at once, and the CLI can run alongside. Two writers can lose an update, or one can rename a temp file the other has just truncated, leaving a partial `config.toml` that the next start refuses. The temp file is created with the process umask, so a `config.toml` the operator set to 0600 becomes 0644 after a dashboard edit. `add_compatible` checks `taken.exists()` and renames later, so two posts with the same id overwrite each other. | Take the writers' lock around read, edit and write. Write through a unique temp file (`create_new`, mode 0600, `sync_all`) in the same directory, then rename. For a plugin, use `create_new` on the final file or rename with no overwrite. |
| L5 | Low | `crates/nullrouter-dashboard/src/changes/adapters.rs:23-25` | `adapters.install` takes a filesystem path verbatim from the form, and the server opens it as the operator. The path is also logged as the subject. With M1 this lets another local principal make the server read any file the operator can read. I did not check whether the install pipeline's error messages echo file content. | Accept packages only from a staging directory under the home (or by upload), or refuse anything that isn't a regular file with an archive extension. Confirm that install errors never quote file bytes. |
| I1 | Info | `crates/nullrouter-dashboard/src/forms.rs:25,180-182` | Write-only fields are a deny-list of names (`secret`, `password`, `code`). Nothing ties `components::secret_field(name, …)` to that list, so a future write-only field under another name would be kept in the flash for 5 minutes and echoed into a `value=` attribute. | Make `secret_field` take a closed enum whose names make up `WRITE_ONLY`, or add a test that every `secret_field` call uses a listed name. |
| I2 | Info | `crates/nullrouter-engine/src/tasks.rs:28-35,162-165` | `TaskId::random` ignores a `getrandom` failure and returns `tk_aaaaaaaaaaaaaaaa`. The second `start` after such a failure would loop forever in the `while … any(id)` re-roll while holding the table lock, which would deadlock tasks and the dashboard. `forms.rs:121` panics on the same failure instead. | Return an error (or `Busy`) when `getrandom` fails, and cap the re-roll loop. |
| I3 | Info | `crates/nullrouter-engine/src/tasks.rs:191-209`; `crates/nullrouter-dashboard/src/lib.rs:249-264` | Tasks and the paste channel are not bound to a session. Any signed-in browser can view any task id and paste into any task waiting for a code. Today every browser holds the same token, so there is one principal and this is by design. A paste is accepted only by a running task in `Paste` state, ids have 80 random bits, and there are at most 200 kept and 4 running. If M1 brings in per-browser sessions, record each task's session and check it in `paste` and in the panel. The paste bridge turns the code back into a plain `String` (`ops/accounts.rs:363`); it isn't zeroised. | Bind tasks to the session when sessions exist. |
| I4 | Info | `crates/nullrouter-dashboard/src/changes/mod.rs:196-200` | Up to 256 KiB of body is buffered before the session check, so an unauthenticated local client can make the server buffer that much per request. The guard is not involved at that point. | Check the cookie before reading the body, or lower the limit for posts without a session. |

### Verified and sound

**Cross-site posts**
- `check_post` refuses an unknown session (303 `/signin`), a foreign or `null` `Origin`, and a `Referer` whose host isn't the dashboard's. When `Origin` is present, `Referer` is not consulted. A post with neither header is refused.
- `_t` is compared in constant time.
- The HMAC matches RFC 4231 test cases 2 and 6.
- The token key is 32 bytes from the OS for each process.

**Redirects (`_back` and `_window`)**
- `forms::back` goes through `sanitize_next` (no `//`, backslash, space, control or non-ASCII character).
- It only accepts a known page path, and a query without `#`. Anything else becomes `/endpoint`.
- `_window` and the flash ids are percent-encoded by `join`.
- `see_other` falls back to `/` on a bad header value.
- So there is no open redirect and no header injection.

**Test-only routes:** `/debug/form-token` and `debug.noop` exist only under `cfg(test)` or `feature = "testkit"`. The workspace uses resolver 3 and the CLI doesn't enable that feature, so release builds don't carry them.

**Secrets**
- `secret_field` never has a `value` attribute.
- Kept fields drop `secret`, `password` and `code`.
- The `Once` flash holds a `SecretString`, is bound to its session, is read once, expires after 5 minutes, and is shown under `Cache-Control: no-store`.
- `keys.issue` puts only the id and name in `Done.text`.
- `AccountError` messages don't echo the secret.
- The client API strips `cookie` before sending upstream (`crates/nullrouter-server/tests/passthrough.rs:128`).

**Change log**
- The file is created 0600.
- Rotation goes through a 0600 `create_new` temp file and a rename.
- Lines are serde-escaped.
- Write errors are only logged as a warning.

**Sign-in hand-back**
- The `return_to` base is `http://<Host>` only when `HostRule::allows` that Host (`changes/mod.rs:224-229`).
- The path and query are rebuilt as `/providers?window=provider&id=…&task=…` (`ops/accounts.rs:317-326`).
- `Loopback::set_return` refuses control and whitespace characters.
- The 303 is sent only when `verdict` accepts the callback.
- An attacker can't choose the target.

**Custom provider**
- `check_id` runs before the plugin path is built, so there is no path traversal.
- Strings are quoted through `toml::Value`, so a name or model can't inject TOML.
- Base URLs without http(s), or with userinfo, a query or a fragment, are refused.
- A private address needs `allow_private_endpoints`.
- The plugin then passes `check_user_plugin`.
- At send time, `CheckedResolver` refuses private resolved addresses unless private endpoints are allowed, so DNS rebinding is covered.

**Other `config.toml` edits**
- `connection` quotes model names with `quoted()`, and a raw newline fails the loader check.
- The `proxy.use` value is a validated proxy name.
- `[tests]` values are parsed before they are written.
- Provider ids are checked against the registry before going into a header.

**Tasks:** at most 4 run at once, at most 200 are kept, and only ended tasks are evicted.

## L1 decision

**The acceptance from 2026-10-06 no longer holds as written.** It was given for a read-only dashboard, and its rationale only considered same-user processes that can read `~/.0router`. Since 014:
- The cookie value is the long-lived dashboard token (400 days).
- Holding it is enough to make every change the CLI can make, including issuing a client key that is shown to the holder.
- `_t` and the `Origin` check stop browsers on other origins, but not a client that sends its own headers.

Some principals receive the cookie and can reach the loopback dashboard without being able to read `~/.0router`:
- other local OS users' servers on 127.0.0.1;
- host-network containers;
- same-user sandboxed processes.

For them the stakes rose from reading to full control. Re-rate it **Medium (M1)** and fix it under T048's "fix every High and Medium":
- (1) a session id that isn't the token, with a short lifetime;
- (2) a dedicated cookie host offered by default (`nullrouter.localhost`, with `127.0.0.2` as the Linux fallback);
- also correct R10 in `research.md`.

The risk could be accepted again only once (1) has landed. It would then be Low: a leaked cookie expires and never reveals the token, and the dedicated host removes the leak path for operators who use it.

### Proposed "What the cookie reaches" paragraph (`docs/operator-config.md`, after fixes 1 and 2)

> ### What the cookie reaches
>
> Browsers scope a cookie by host name, not by port. A cookie set by `127.0.0.1:20130` is also sent to every other web server you open on `127.0.0.1` in the same browser, such as a development server, a container's published port or another user's server. Since the dashboard can make changes, that cookie is worth protecting. It is a sign-in session, not your dashboard token. It ends after 12 hours unused and 7 days at most, and **Sign out** or `nullrouter dashboard token` ends every session at once. Whoever holds a live session can do anything the dashboard can do, including issuing client keys, until it ends. The form token and the origin check stop other web pages from posting through your browser, but they don't stop a program that has the cookie and can connect to this machine's loopback.
>
> To keep the cookie away from other local servers, open the dashboard on its own host name: `http://nullrouter.localhost:20130` (printed by `nullrouter dashboard token`). Your browser sends that cookie only to `nullrouter.localhost`. Where your browser doesn't resolve `*.localhost`, on Linux set `listen = "127.0.0.2:20130"` in `[dashboard]` and open that address, since every `127.0.0.0/8` address is loopback there. If other people or untrusted programs share this machine, use one of these, sign out when you're done, and reissue the token if you think the cookie was seen.

If the fixes are deferred, the current paragraph's last sentence ("that acceptance needs to be checked against them") should be replaced with something like: "This was accepted for the read-only dashboard; with changes enabled it is an open Medium finding (security-review.md M1). Until it is fixed, the cookie is your dashboard token: any program that receives it and can connect to this machine's loopback can make every change the dashboard can. Open the dashboard on `127.0.0.2` (Linux) when other servers run on `127.0.0.1`."

## Resolution

- M1, I2, I3 and the confirm gate: fixed in 55b5502 (random session id kept as a digest, idle 12 h / absolute 7 days, sign-out, a reissued token ends sessions; task ids refuse a failed random source; tasks bound to their session).
- L1–L5: fixed in the next commit (quoted keys and no control characters in `routing.window`; URLs redacted in errors and dropped from kept fields when they hold userinfo or a query; control characters stripped from change-log entries; `config.toml` writes under one lock through a unique 0600 temp file, plugin files made with `create_new`; the dashboard installs only regular `.tar.gz`/`.tgz` files).
- Open: the own cookie host (`nullrouter.localhost`) and re-asking for the token before high-impact ops (M1 fixes 2 and 3), and I1 and I4. None is above Low once M1 fix 1 holds. The CLI still accepts a directory for `adapters install`; only the browser refuses one.

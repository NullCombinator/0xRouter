# Security review: dashboard (T073)

Reviewed 2026-10-08 by the `security-auditor` agent, read-only, against spec.md,
contracts/dashboard-http.md and research R8. Scope: `crates/nullrouter-dashboard` and its wiring
into `nullrouter serve` and `nullrouter dashboard`. Areas from the task: token storage, cookie,
Host/Origin/CSP, the 405 rule, logo serving, bounded work; also secrets in pages and logs,
escaping, and the bind address.

## Findings and resolutions

| # | Severity | Finding | Resolution |
|---|---|---|---|
| H1 | High | `Referrer-Policy: no-referrer` on every response makes a browser's sign-in form post send `Origin: null` (Fetch, "append a request Origin header"), which `origin_ok` refuses. No browser could sign in; the tests use reqwest and couldn't see it. | Fixed: the policy is `same-origin`, so same-origin posts carry the real `Origin` and other sites still get no referrer. `null` stays refused, since accepting it would admit sandboxed cross-site frames. Contract and research R8 amended. |
| M1 | Medium | A record id that no record can have (`/usage/records/r`, `/usage?before=t`) made `records::get` fold every segment, or `cursor_day` parse every line. A timed-out build freed its permit while its blocking read went on, so reads piled up past two. A page on another loopback port is same-site and could fire these GETs with the cookie. | Fixed in two parts. (1) `Req::impossible_record`: a window or `before` that isn't `rq_` plus a ULID is "no record <id>", the CLI's message, without reading the journal. (2) The guard tells a timed-out page at once but keeps the build's permit until the build ends, with an abort at 6 × the limit as a backstop. Deferred: making `records::get` read only the id's day and the days either side. That is engine code in `journal/records.rs`, which slices 004, 011 and 013 also change; do it after they merge. A valid-format id that doesn't exist still reads every segment, but the guard now bounds that to two at a time. |
| L1 | Low | `dashboard token` saved the digest, then returned on a failed reload before printing the token. The running server kept the old token, the new one was never shown, and nobody could sign in after the next good reload. | Fixed: the token is always printed. On a failed reload the status says the previous token still works on the running server until a reload succeeds, and the exit code is 1. Contract updated. |
| L2 | Low | `dashboard token` said "Open {url}" when the server was running but the dashboard hadn't bound. Another local user could hold the port and collect the token. | Fixed: in that state the output names the bind error and says not to open the address. Covered in `status_and_check_when_the_port_is_taken`. |
| L3 | Low | Only the first `nr_dashboard` cookie was checked. A server on another loopback port could set one with a longer path, which browsers send first, and lock pages out. | Fixed: a request is signed in if any `nr_dashboard` value matches. |
| L4 | Low | No connection cap or request/body timeout on the dashboard listener. The sign-in body read has no time limit, and wrong-token sleeps aren't capped in number. A local process could exhaust file descriptors. | Deferred: the client listener has the same gap (`serve.rs`). Fix both listeners together in one change, with a concurrency limit, header and body timeouts, and a cap on sleeping sign-ins. |
| I1 | Info | `HEAD` gets 405. | Intended: the contract says any method but `GET`. The task's "non-GET/HEAD" wording was loose. |
| I2 | Info | The cookie has no `Secure` attribute. | Accepted: plain-HTTP loopback. `__Host-` would need `Secure`, which browsers don't treat consistently on `127.0.0.1`. |
| I3 | Info | `listen = "localhost:…"` passes the loopback check by name and is resolved at bind time. | Fixed: after binding, an address that isn't loopback is reported as not listening, and the dashboard doesn't serve. |
| I4 | Info | Requests hyper rejects before axum (bad request line or headers) get no security headers. | Accepted: they carry no page. |
| I5 | Info | `tokens.rs` `shown_token` has no length guard, so a token under 8 characters shows mostly in `…last4`. | Accepted: real tokens are long, and the CLI does the same. |

## Verified sound

- **Token issue and storage:** 32 random bytes, `nrd_` plus base64url. Only the SHA-256 digest is
  stored, written 0600 and atomically with fsync. A group- or world-readable file, a symlink, or a
  malformed digest is refused on read.
- **Comparison:** the presented token is hashed, then the digests are compared in constant time
  (`subtle`). There is no session table; every request is checked against the snapshot's digest,
  so a new token signs every old cookie out.
- **Cookie:** `Path=/; HttpOnly; SameSite=Strict; Max-Age=34560000`, set only after a right token.
  The wrong-token delay is 1 s, doubling to 30 s.
- **Host:** an exact match on the loopback names with the bound port, or `421`, before routing,
  on every path.
- **CSP and headers:** no script source, `frame-ancestors 'none'`, `X-Frame-Options: DENY`,
  `nosniff`, `Cross-Origin-Resource-Policy: same-origin`, `no-store`. They are applied after the
  handler, so errors, assets and logos carry them too.
- **The 405 rule:** only `GET`, plus `POST` on exactly `/signin`. Nothing reachable by `GET`
  writes state: the views' live operations are a fixed read-only list.
- **`next` redirect:** must be a local path; backslash, control, space and non-ASCII characters
  are refused.
- **Logos:**
  - Accepted only as bare `.png` names: at most 64 KiB, with a PNG signature and an IHDR chunk,
    each side at most 256 px, regular files only.
  - Served from memory with a fixed `image/png` type, and only with the cookie.
  - No SVG.
- **Escaping:** all markup goes through maud with no `PreEscaped`, and URLs built from data are
  percent-encoded.
- **Secrets:** account secrets, sign-in tokens and keys appear only as `…last4` or `env:NAME`. The
  dashboard view shows only when its token was issued, and logs go through `RedactWriter`.
- **Bounded work (with M1's fixes):** at most two builds; a third waits 5 s, then gets `503`. Each
  build has 10 s and runs in its own task. Records are capped at 50 per page and the sign-in body
  at 8 KiB.
- **Bind address:** the default is `127.0.0.1:20130`. A `listen` that isn't loopback, including
  `::ffff:`-mapped addresses, is a config load error. A bind failure doesn't stop `serve`.

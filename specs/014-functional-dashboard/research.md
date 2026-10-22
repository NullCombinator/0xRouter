# Research: Functional Dashboard (014)

Decisions for the plan. Each one names what was chosen, why, and what else was weighed.

## R1. One set of operations, called by the CLI and the dashboard

**Decision**: Every change moves into a new module, `nullrouter_server::ops`, as one function per
operation: `ops::keys::issue(&Ctx, IssueKey) -> Result<Done, OpError>`, and so on. `Ctx` carries
the home and, when there is one, the running engine. `OpError` carries the CLI's exact text and its
exit code. The CLI's `cmd/*.rs` keep their clap parsing and printing and call `ops`. The dashboard's
form handlers call the same functions in-process. Where the CLI applies a change through the
operator socket (`reload`, `test.run`, `quota.poll` …), `ops` takes the in-process path when it runs
inside the server and the socket path otherwise, so both paths reach the same engine call.

**Rationale**: FR-003 and SC-002/SC-003 ask that a browser change *is* the CLI change. The
dependency order is cli → dashboard → server → engine (007 R2), so the dashboard can't call the
CLI's code. Moving it down is how 008 made the CLI and the dashboard agree on reads (`views`, 007
R1): agreement becomes structural, not a matter of keeping two copies in step.

**Alternatives**: the dashboard shells out to the `nullrouter` binary (rejected: a process per
click, argument quoting, and secrets on a command line or pipe); the dashboard sends operator-socket
ops only (rejected: most CLI changes are file edits plus `reload`, which the socket doesn't
offer as ops; adding ~60 socket ops would be a second API to keep in step).

## R2. Forms, POST-redirect-GET, and no script

**Decision**: each change is an HTML `<form method="post">` to `/do/<area>/<op>`. Its handler checks
the session and the form token (R3), calls `ops`, writes the change log (R7), and answers `303 See
Other` to the page that shows the result (FR-005). A result to show once, such as a new key, an
error text, or the fields to redisplay without secrets, travels in a one-time *flash* held
server-side under a random id carried in the redirect's query (`?done=<id>`). The flash is consumed
on first read and expires after 5 minutes. Windows (confirm, forms) are the existing `?window=` pages
(009). The CSP stays `default-src 'none'`, with no `script-src` and `form-action 'self'` (already
present in `headers.rs`).

**Rationale**: the user's constraint is server-rendered Rust with no JavaScript (brief row 3).
POST-redirect-GET makes a reload harmless (FR-005). A server-side flash keeps a new key out of URLs,
history and logs (FR-007).

**Alternatives**: rendering the result directly in the POST response (rejected: a reload re-posts);
putting the flash in a cookie (rejected: a key in a cookie is a secret at rest in the browser).

## R3. Cross-site request forgery

**Decision**: a per-session form token. At sign-in (009 R6), the dashboard derives a form token as
HMAC-SHA256 (built on the workspace's `sha2`) of a per-process random key over the session cookie's
value. Every form carries it in a
hidden field. A change is accepted only when:
- the method is POST;
- the cookie is valid;
- the field matches (constant-time compare);
- `Origin` is present and equals the dashboard's own origin, or, when `Origin` is absent, `Referer`
  does.

The Host check (009 R7) and `SameSite=Strict` stay.

**Rationale**: FR-012. Three independent guards: SameSite, the origin check and the token. The token
is stateless and dies with the session when `dashboard token` reissues the token (edge case "Token
reissued").

**Alternatives**: a double-submit cookie (rejected: weaker than an HMAC tied to the session, and a
second cookie); origin check only (rejected: some browsers omit `Origin` on same-origin POSTs).

## R4. Long tasks

**Decision**: a `Tasks` table in the engine: id, kind, subject, started, progress lines, and result.
It is in memory, keeps the newest 200 tasks, and is shown on the page of the task's subject and on
Console Log. `ops` functions for long changes (test, sign-in wait, review, build, rebuild, catalogue
install, proxy fixed, quota poll) spawn on the server's runtime and return the task id at once
(FR-010). The CLI keeps waiting as today by following the same task. A page that shows a running task
adds `<meta http-equiv="refresh" content="3">`, which needs no script.

**Rationale**: no script means no live updates; reload or meta refresh is the confirmed behaviour
(brief row 6). One table serves the browser and, later, other clients.

**Alternatives**: holding the form until the task ends (rejected by the user, brief row 6).

## R5. Sign-in in the browser

**Decision**: the dashboard starts the engine's existing sign-in flows (`nullrouter_engine::signin`)
as a task (R4), with the same flows and messages as the CLI:
- **Loopback redirect** (the provider redirects to a fixed `127.0.0.1` port, 005 R3): the engine's
  listener receives the code. Its success page then sends the browser to the dashboard with a
  `303 See Other` to `/providers?window=account&...`, instead of saying "return to the terminal".
- **Code page** (the provider shows a code): the dashboard shows a write-only field to paste the code
  or address, posted as a form.
- **Device flow**: the dashboard shows the code and link, and the task polls.

The terms warning (005 Q2) is a confirm window the operator must accept. The sign-in link opens in
the same tab, so the browser returns to the dashboard.

**Rationale**: FR-008, and the 005 flows stay the only flows. The provider-registered redirect
addresses can't point at the dashboard's port, so the loopback listener hands the browser back.

**Alternatives**: a new redirect on the dashboard port (rejected: providers register fixed redirect
URIs).

## R6. CLI twins for the changes the CLI lacks

**Decision**: new commands, each in `ops` first:

| Change | New command |
|---|---|
| Unified models | `nullrouter unified add <name> <provider>/<model>...`, `unified set <name> <members>...`, `unified remove <name>` |
| Combos | `nullrouter combos add <name> <member>...`, `combos set <name> <member>...`, `combos remove <name>` |
| Provider visibility | `nullrouter providers enable|disable|hide|show <id>` |
| Custom provider | `nullrouter providers add-compatible <id> --style openai|anthropic --base-url URL --model ID... [--name TEXT]` |
| Private endpoints | `nullrouter plugins private-endpoints on|off` |
| Change log | `nullrouter dashboard log [--limit N]` |

Unified models and combos are written to `config.toml`, where they are declared today, in their
existing form. A provider's on/off and hidden state goes in the existing `[provider.<id>]` table of
`config.toml` (`enabled`, `hidden`). A disabled provider is skipped by routing like a disabled
account; a hidden one is left out of model lists and the Providers grid. A custom provider is
written as `plugins/<id>.toml`: a schema-2 plugin with one text section in the chosen style. It
passes the same gate as any user plugin (`load::check_user_plugin`) before it is written.

**Rationale**: FR-004, FR-021. Reusing the existing declarations and the plugin gate keeps one source
of truth and one safety check (constitution I).

**Alternatives**: a separate file for dashboard-made declarations (rejected: two places to look, and
the CLI's reads would need to merge them).

## R7. The change log

**Decision**: `<home>/dashboard-log.jsonl`, mode 0600, one JSON line per browser change: `at`
(RFC 3339), `op` (stable id, e.g. `accounts.remove`), `subject` (e.g. `anthropic/work`), and
`result` (`done`, `saved_not_applied`, or `refused`, with `text`). It is written by the form handler
after `ops` returns. The file keeps the newest 1,000 lines and is rewritten when it reaches 1,200.
Field values come from typed subjects; there is no free-form input echo, so a secret can't reach the
log (FR-006, FR-016, SC-004).

**Rationale**: FR-016/FR-017. A file in the home survives restarts. JSON lines match the journal's
habits.

**Alternatives**: putting the entries in the request journal (rejected: different retention, and the
journal is about client traffic).

## R8. Concurrency and isolation from client traffic

**Decision**: form handlers run under the dashboard's existing guard (009 R8: at most 2 builds at
once, 10 s timeout), and file-writing `ops` take the same file locks the CLI takes. Long tasks run
outside the guard on the server runtime, at most 4 at once; a fifth is refused with "busy, try again"
as the CLI's server answer would be. A dashboard change never holds a lock that a request path takes,
because requests read the engine snapshot and `reload` swaps it atomically (002).

**Rationale**: FR-015, SC-006.

## R9. Confirm windows

**Decision**: an irreversible button links to `?window=confirm&op=<op>&subject=<id>`. That window
names the subject and what is lost, using text from `ops::describe_loss(op, subject)` (for example,
"clients using key work-laptop stop working at once"). It holds the real form with Confirm, and a
Cancel link that closes the window. The CLI's own `[y/N]` prompts (billed tests, adapter clear and
remove) reuse the same question text from `ops`.

**Rationale**: FR-009, SC-007, with no script.

## R10. What the cookie scope note (007 L1) means now

**Decision**: the session cookie's name gains the port (`nrd_session_<port>`). The quickstart
recommends a distinct loopback address for the dashboard when other local servers run. The form
token (R3) is bound to the cookie value, so a cookie sent to another local server is still useless
for posting here without the page's token.

**Rationale**: write access raises the stakes of 007's open Low L1. The token closes the posting
path; the per-port name stops two dashboards on different ports from clobbering each other.

# Security review: slice 006 (T097)

Run 2026-10-05 by the main session over `1a7e6ae^..HEAD`, not the literal `main...006-routing-decision`.
Local `main` already holds `1a7e6ae` ("feat(006): routing decision, journal, and operator view (in
progress)"), which carries the journal, the routing state, the salt, `forget` and the new socket ops;
`main...006-routing-decision` is only the last 11 commits and would have left all of that unreviewed.
No `security-auditor` subagent ran: the Agent tool was not available in this session. Read in full:
`journal/writer.rs`, `journal/records.rs` (lines 1–100, 205–490), `journal/state.rs`, `routing/router.rs`
(the salt), `routing/fingerprint.rs` (the hashing, lines 60–210), `server/src/operator.rs`, the CLI's
`records forget` and `prune`, and every place `attempt.rs` builds a failure reason. Read in part, or
not read: `routing/meter.rs`, `price.rs`, `pace.rs`, `view.rs`, the registry's `[routing]` schema and
gate, `quota/history.rs` and `quota/tally.rs`. The scale and format follow
[slice 005's review](../005-account-sign-in/security-review.md); its open items are not repeated.

## High

None.

## Medium (fixed, see Resolution)

| # | Finding | Fix |
|---|---|---|
| M1 | A client chooses two strings that reach the journal and the operator's terminal unchecked: the `target` (the request's `model`, up to the 32 MiB body cap) and the session (a header or `prompt_cache_key`, capped at 256 characters but not filtered). `walk` stores `target` before the plan can reject it (`attempt.rs:598-602`), so an unknown model is enough. `open_line` writes both to `records/*.jsonl` (`journal/records.rs:64-80`), and `records list`/`show` print them with no filtering (`cmd/records.rs` `line`, `show`). Any holder of an agent key, a weaker party than the operator, can plant escape sequences in the operator's terminal (the class slice 005 closed as L1) and write megabytes into every request's `open` line. The same holds for provider text in an attempt's `reason`. | Keep a client's names plain and short where the record is set: drop control characters and cap at 256 characters (`records::plain`, applied to `target` in `walk` and to the session in `serve.rs`). Scrub every string in a record before the text views print it. `--json` output keeps the text; JSON escapes control characters. |
| M2 | `records forget --account` without a server leaves the account's ledger entries. The contract (`operator-cli.md:93`) says forget drops "that account's fingerprints and ledger entries"; a running server does (`route::drop_account`), but the offline path (`state::forget_account`) rewrites only `routing/warm.jsonl`. `accounts remove` takes the same offline path (`cmd/accounts.rs:238`), so a removed account's name and deficits stay in `routing/ledger.jsonl`. Both callers also discarded the result (`let _ =`): if the rewrite failed, `forget` still printed "forgot N records" and the fingerprints stayed. | `forget_account` also rewrites `ledger.jsonl` (the account's deficits go, and a line left with none). `records forget` exits 1 and says the routing state could not be rewritten; `accounts remove` prints a note. |

## Low (open)

| # | Finding |
|---|---|
| L1 | Files the journal opens follow symlinks, and rewrites use predictable temporary names. `records.lock` is opened with `create` and no link check (`writer.rs:486`, `journal/records.rs:389`); an existing day file is reopened for append without one (`writer.rs:499`); the temporary file is `<name>.jsonl.tmp`, opened with `create` rather than `create_new`, so its mode and target are whatever is already there (`writer.rs:342-343`, `journal/records.rs:478-479`). The salt does this right (`refuse_symlink`, `create_new`). Needs write access to the 0700 home, so the same user only; slice 005's L5 and slice 003's L4 are the same class. A crash during a rewrite leaves `*.jsonl.tmp` holding the lines that were kept, never the forgotten ones. |
| L2 | `make_private_dir` returns at once when the directory exists (`writer.rs:323-325`), so a `records/` or `routing/` created earlier with a wider mode stays wide. The files inside are 0600, so contents stay private; names (days) and sizes do not. I did not check whether `nullrouter check` reports these two directories' modes. |
| L3 | `forget --account` matches a request only by its attempts and `served_by` (`Filter::matches`). The decision table of every routed request lists every account the placement considered, with its pace, share, deficit and remaining quota, so requests served by other accounts keep naming the forgotten one. The contract says "that account's records", and this reading meets it, but an operator forgetting an account for privacy may expect more. Either document it in `operator-config.md` or scrub the rows; scrubbing means remapping `decision.order`, which indexes into `candidates`. |
| L4 | `forget` races with work in flight. After the server drops an agent's fingerprints, one of its requests that finishes later learns a new fingerprint and appends lines for a record whose `open` was removed. When the disk is full, lines held in memory (`HOLD_LINES`) are written later and bring back forgotten records. Narrow windows; `forget` on a quiet server is exact. |
| L5 | The operator socket reads a line of any length (`operator.rs` `connection`), and `records.list` with no `limit` returns the whole journal in one answer. The socket is 0600 in a 0700 directory, so only the same user can do this. |
| L6 | The journal applies no redaction of its own: `attempt.rs` redacts each reason where it is made. I checked every site that makes one (lines 473, 860, 1069, 1093, 1272, 1360, 1417, 1475, 1510): each is redacted, static text, or a serde error position without content. A new reason site that forgets `redactor.redact` would write a secret to disk. A final redact pass in `journal::records::lines_for` would make the rule hold by construction. |

## Holding

- **File modes.** Journal and routing files are created 0600 (`create_new` with `mode(0o600)`), `records/` and `routing/` 0700 when created, `records.lock` 0600, the salt 0600 with `create_new`. The operator socket's directory is 0700 before the bind and the socket 0600 after it.
- **Temp-then-rename.** `replace_file` and `rewrite` write a temporary file, `fdatasync` it, rename it into place and `fsync` the directory; a crash leaves the old file or the new one. The writer thread runs a compaction in order with the appends and under `records.lock`, and skips it while the disk refuses writes. A CLI `prune` or `forget` takes the same lock for its whole run and gives up with exit 1 after 10 s.
- **Salt.** 32 bytes from the OS random source, written once with `create_new`, fsynced, symlink-refused. A file of the wrong length is refused, not replaced. `Debug` prints `Salt(..)`. It is not in a record, a log line or the operator socket.
- **Fingerprints hold no content.** Each boundary is SHA-256 over the salt, tagged and length-prefixed fields, chained to the previous boundary, truncated to 128 bits. A `warm` line carries the agent's key id (the key's id, not the secret: `serve.rs:109, 152` use `key.id`), the hash, provider, account, model, token count and times. Nothing from a prompt is stored.
- **Record lines.** An `open` line carries id, time, style, op, type, target, unified model, agent id and session; `close` carries outcome, served-by, timings, usage and break handling. A test (`no_secret_or_prompt_field_exists_in_the_lines`) walks every key of the lines.
- **Operator socket ops.** `records.list`, `records.get`, `routing.view` and `routing.health` return records and routing numbers, never a token or key. `records.get` compares the id as a string and builds no path from it. `records.forget` needs exactly one of `account` and `agent`, and `account` must be `provider/name`.

## Resolution (2026-10-05)

| # | Status | Test |
|---|---|---|
| M1 | Fixed. `records::plain` (no control characters, at most 256 characters) is applied to the recorded `target` and the recorded session; the CLI's text views scrub every string of a record. | `records::tests::a_clients_names_are_kept_short_and_without_control_characters`, `fallback::a_clients_target_is_recorded_short_and_without_control_characters`, `cli records::a_clients_escape_sequences_never_reach_the_terminal` |
| M2 | Fixed. `forget_account` rewrites `ledger.jsonl` as well; `records forget` fails with exit 1 and names the problem when the routing state can't be rewritten, and `accounts remove` prints a note. | `state::tests::forgetting_an_account_also_drops_its_ledger_entries`, `cli records::forget_says_so_when_the_routing_state_could_not_be_rewritten` |
| L1–L6 | Open. L1 and L2 are the same fix as slice 005's L5 (refuse links, `create_new` for temporaries, tighten or report a directory's mode); L6 is a one-line defence in depth. None is needed to merge. | — |

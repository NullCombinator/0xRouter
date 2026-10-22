# Data Model: Functional Dashboard (014)

## Operation (`nullrouter_server::ops`)

One function per change. All take a `Ctx`, typed inputs, and return `Result<Done, OpError>`.

| Field | Meaning |
|---|---|
| `Ctx.home` | The operator home. |
| `Ctx.engine` | The running engine, when called inside the server (dashboard); `None` from the CLI. |
| `Done.status` | `Applied`, `SavedNotApplied { reason }` (the server refused the reload), or `Started { task }`. |
| `Done.text` | The line the CLI prints on success, e.g. `anthropic/work: added (order 0)`. |
| `Done.once` | A value to show exactly once (a new client key). Never logged. |
| `OpError.text` | The CLI's refusal text, verbatim. |
| `OpError.exit` | The CLI's exit code (1 invalid, 3 refused by gate, 4 no server, 5 sign-in ended). |
| `OpError.field` | The form field the refusal concerns, when there is one (redisplay next to it). |

Rules:
- Inputs that are secrets are `SecretString` and are never `Debug`-printed, logged or returned.
- An operation that edits a file either writes a file that still loads or writes nothing (the CLI's
  rule today).

## Task (engine, in memory)

| Field | Meaning |
|---|---|
| `id` | `tk_` + 16 random base32 characters. |
| `kind` | `test`, `signin`, `adapter_review`, `adapter_build`, `adapter_rebuild`, `catalogue_install`, `proxy_fixed`, `quota_poll`. |
| `subject` | What it acts on (e.g. `anthropic/work`, `combo:fast`). |
| `started`, `ended` | Times. |
| `progress` | Lines, in order (the CLI's streamed lines for tests). |
| `result` | `running`, `done { text }`, or `failed { text }`. |
| `input` | Sign-in only: whether it waits for a pasted code (`paste`) or a device approval (`device { code, link }`). |

State transitions: `running → done | failed`. The table keeps the newest 200 tasks and at most 4
running at once. A fifth start is refused with `busy: 4 tasks are running; try again when one ends`.

## Flash (dashboard, in memory)

| Field | Meaning |
|---|---|
| `id` | 128-bit random, base32. |
| `session` | The session it belongs to (another session can't read it). |
| `kind` | `done { text }`, `once { label, value }` (new key), `refused { text, field, kept_fields }`. |
| `expires` | Created + 5 minutes. |

It is consumed on first read. `kept_fields` never holds a write-only field.

## Form token

`hex(HMAC-SHA256(process_key, session_cookie_value))[..32]`, recomputed on every request. Nothing
is stored. It dies with the session.

## Change log entry (`<home>/dashboard-log.jsonl`)

```json
{"at":"2026-10-22T14:03:11+02:00","op":"accounts.remove","subject":"anthropic/work","result":"done","text":"anthropic/work: removed"}
```

| Field | Values |
|---|---|
| `op` | `<area>.<op>`, one of the ids in `contracts/dashboard-forms.md`. |
| `subject` | The typed subject: `<provider>/<account>`, a key name, a provider id, `<harness>@<version>`, a target name, a proxy name, or a setting path. |
| `result` | `done`, `saved_not_applied`, `started`, `refused`. |
| `text` | `Done.text` or `OpError.text`. |

The file is 0600 and keeps the newest 1,000 lines. It is rewritten when it reaches 1,200.

## Provider visibility (`config.toml`)

```toml
[providers.openrouter]
enabled = false   # routing skips it (default true)
hidden = true     # left out of model lists and the Providers grid (default false)
```

## Custom compatible provider (`plugins/<id>.toml`)

Built from the form fields: `id` (lowercase letters, digits and dashes, up to 32 characters, not an
existing provider), `name`, `style` (`openai-chat` or `anthropic-messages`), `base_url` (https, or a
private http(s) URL only when private endpoints are on), and `models` (1 to 200 ids). The result is
a schema-2 plugin with one text section in that style, API-key auth in the style's header, and a
static model list. It must pass `check_user_plugin` before it is written.

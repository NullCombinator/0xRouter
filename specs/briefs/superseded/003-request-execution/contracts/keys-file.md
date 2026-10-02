# Contract: `$NULLROUTER_HOME/keys.toml`

**Feature**: [spec.md](../spec.md) FR-004a, FR-006–FR-010 | **Research**:
[R4](../research.md#r4-accounts-and-access-keys-file-secrets-validation-fr-004a-fr-006fr-010)

This file is operator state. It is never a plugin, and no plugin can reference it.

```toml
schema = 1

[[access_key]]
agent  = "laptop"
key    = { env = "NR_KEY_LAPTOP" }   # or a literal string (then the file must be 0600)

[[access_key]]
agent  = "ci"
key    = "nr-2c1f…"                  # literal, at least 16 characters
active = false                       # default true

[[connection]]
provider = "anthropic"               # id or alias
name     = "personal"
api_key  = { env = "ANTHROPIC_API_KEY" }

[[connection]]
provider   = "cloudflare-ai"
name       = "main"
api_key    = "cf-…"
account_id = "0123abcd…"             # required: the URL contains {accountId}

[[connection]]
provider = "openrouter"
name     = "backup"
api_key  = { env = "OPENROUTER_KEY" }
active   = false
```

## Fields

**Top level**: `schema` (required, `1`), `access_key` (array), `connection` (array).
Unknown keys are rejected.

**`[[access_key]]`**

| Key | Required | Rule |
|---|---|---|
| `agent` | yes | `[A-Za-z0-9._-]{1,64}`; unique |
| `key` | yes | string (≥ 16 chars) or `{ env = "NAME" }`; resolved value unique across access keys |
| `active` | no | bool, default `true` |

**`[[connection]]`**

| Key | Required | Rule |
|---|---|---|
| `provider` | yes | a registry id or alias; the provider must be executable in this slice |
| `name` | yes | non-empty; unique per provider |
| `api_key` | yes | string (non-empty) or `{ env = "NAME" }` |
| `active` | no | bool, default `true` |
| `account_id` | when the URL needs it | non-empty; no `/ ? #` or whitespace |

`{ env = "NAME" }` accepts only the `env` key. The variable must be set and non-empty
when the file is loaded, and it is re-read on every reload.

## Permission rule (FR-007)

If any `key` or `api_key` is a literal, the file's mode must not grant group or other
permissions (`mode & 0o077 == 0`). Otherwise load fails:

```text
keys.toml: holds literal secrets but is readable by group/others (mode 0644); chmod 600 or use { env = "..." }
```

## Errors

Every error is reported, not just the first. The format is 002's
`file:line:col path: rule`. Error messages never contain a key value.

```text
keys.toml:12:12 connection[1].provider: unknown provider "cloudfare-ai"
keys.toml:12:12 connection[1].provider: provider "azure" is not executable in this slice (specialized executor)
keys.toml:20:1  connection[2]: provider "cloudflare-ai" needs account_id ({accountId} in URL)
keys.toml:24:11 connection[3].api_key: environment variable OPENROUTER_KEY is not set
keys.toml:30:1  connection[4].name: duplicate connection "main" for provider "openrouter"
keys.toml:3:10  access_key[0].key: shorter than 16 characters
keys.toml:9:10  access_key[1].key: same key as access_key[0]
```

| Situation | At `serve` start | On reload |
|---|---|---|
| file absent | loads with zero keys and a warning; every client request gets 401 | same |
| any error | fatal: exit 1 with the errors | rejected: the previous state is kept, and the errors are returned (FR-010) |

## Provider not executable: reasons

The error names one of these reasons:

- `oauth`
- `no-auth free provider`
- `web-cookie`
- `specialized executor`
- `no openai or claude transport`

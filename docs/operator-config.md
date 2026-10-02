# Operator configuration

The operator's state lives in one directory, `$ZEROROUTER_HOME` (default `~/.0router`):

```text
$ZEROROUTER_HOME
├── config.toml     # unified models, per-provider settings, plugin decisions, server settings
├── accounts.toml   # provider accounts and their secrets (mode 0600)
├── keys.toml       # agent key digests (mode 0600)
├── plugins/        # user and installed community plugins (*.toml, top level only), see plugins.md
└── run/
    └── operator.sock   # the running server's operator socket (mode 0600)
```

Nothing here is required to load. A missing directory or file means no unified models,
default settings, no user plugins, no accounts and no keys. Write `accounts.toml` and
`keys.toml` with the CLI rather than by hand.

Full reference: [`specs/002-provider-model-registry/contracts/operator-config.md`](../specs/002-provider-model-registry/contracts/operator-config.md)
and slice 003's [`operator-cli.md`](../specs/003-request-pipeline/contracts/operator-cli.md).

## Running the server

```bash
zerorouter accounts add anthropic main        # paste the API key on stdin
zerorouter keys issue claude-code-laptop      # prints the agent key once
zerorouter serve                              # foreground; logs to stderr, redacted
```

`serve` listens on `127.0.0.1:20129` unless `--listen` or `config.toml` says otherwise:

```toml
[server]
listen = "127.0.0.1:20129"
```

It refuses to start if `accounts.toml` or `keys.toml` is readable by group or others, and
names the file and the `chmod` that fixes it. Clients point their base URL at the server
and send the agent key wherever their API style carries one (`Authorization: Bearer`,
`x-api-key`, `x-goog-api-key`, …); see [api-styles.md](api-styles.md).

## Accounts

An account is one secret for one provider. A provider can have several; they are tried in
`order`, and an account that is rate-limited or failing cools down while the others serve.

```bash
zerorouter accounts add openrouter main                    # secret from stdin, never argv
zerorouter accounts add openrouter ci --env OPENROUTER_KEY # read from the environment at load
zerorouter accounts add openrouter backup --order 1
zerorouter accounts list                                   # name, order, …last4 or env:VAR, state
zerorouter accounts disable openrouter backup              # or enable, remove
```

```toml
# accounts.toml
schema = 1
[[account]]
provider = "anthropic"
name = "main"
secret = "sk-ant-…"              # or secret = { env = "ANTHROPIC_KEY_MAIN" }
order = 0
```

A secret is sent only to its own provider's endpoints, and only in that provider's auth
header. Logs, records and error bodies never carry it.

## Agent keys

Every client request carries an agent key. 0router stores only a digest and the last four
characters, so a lost key can't be shown again: issue a new one.

```bash
zerorouter keys issue claude-code-laptop      # prints the key (0r-…) once
zerorouter keys list                          # id, name, …last4, created, revoked
zerorouter keys revoke claude-code-laptop     # by name or id
```

The key's id is the agent's identity: provider session ids are derived from it, and
records name it.

## When a stream breaks

When a provider's stream fails after some output has reached the client, 0router either
**restarts** the answer on the next target (the client sees a note and the new answer), or
ends the stream with an **error event** in the client's style.

```toml
[pipeline]
break_behaviour = "restart"   # restart | error_event
```

```bash
zerorouter behaviour set-break error_event              # the operator default
zerorouter keys set-break claude-code-laptop restart    # per key; `default` clears it
```

## Private endpoints

Plugin URLs may not point at loopback, private, link-local or metadata addresses,
`localhost`, `*.local` or `*.internal`, and the address a host resolves to is checked again
at request time. To route to a local model server, opt in:

```toml
allow_private_endpoints = true
```

## Request records

The running server keeps a record of each request: agent, target, the provider and
account that served it, every attempt with its outcome, time to first token and total,
usage, and anything left out when translating across styles.

```bash
zerorouter records list --limit 20            # newest first; --provider, --model, --json
zerorouter records show rq_01JAB3…           # the full record with attempts
```

Records live in the server's memory. `records` needs a running server and exits 4 without
one. Error bodies sent to clients carry the record id, so a failure can be looked up.

## Addressing models

Clients name a target in one of two ways:

- **`provider/model`**: a direct target. `provider` is an id or alias (`kr`, `kiro`), and
  the model part may contain further `/` (`openrouter/meta-llama/llama-3`).
- **A bare name**: a **unified model** you declared. A bare name never falls back to
  a provider's model, so `claude-sonnet-4.5` alone is not found unless you declared it.

```bash
zerorouter resolve kr/claude-sonnet-4-5 --json
zerorouter model kr claude-sonnet-4-5          # what the provider declares about it
zerorouter providers --capability tts
```

## Declaring a unified model

A unified model is one name backed by one or more provider models, listed in order.

```toml
schema = 1

[[unified_model]]
name = "sonnet"                  # non-empty, no "/"
kind = "llm"                     # optional; members must not conflict with it
members = [
  { provider = "kr",         model = "claude-sonnet-4-5" },           # id or alias
  { provider = "openrouter", model = "anthropic/claude-sonnet-4.5" },
]
```

Check it:

```bash
zerorouter resolve sonnet
# unified sonnet:
#   0. kiro claude-sonnet-4-5 → upstream claude-sonnet-4.5
#   1. openrouter anthropic/claude-sonnet-4.5 → upstream anthropic/claude-sonnet-4.5
```

Each member's upstream id is resolved once, at load. Rules:

- every member model must be declared by its provider, unless that provider is
  passthrough;
- a provider appears at most once per unified model;
- typed members must agree on `kind` (untyped members never conflict).

## Per-provider settings

```toml
[provider.openai]
allow_uncatalogued_models = false   # default true
```

With the default, `openai/brand-new-model` is forwarded unchanged even though openai does
not declare it. With `false`, it is a not-found error. Passthrough providers accept any
model either way.

## Replacing a bundled provider

A user plugin whose `id` matches a bundled provider stays **pending**: the bundled
provider remains active and `check` reports the conflict. Decide in `config.toml`:

```toml
[plugin_decisions]
gemini-cli = "replace"   # the user plugin becomes active; its settings above still apply
kiro       = "decline"   # bundled kiro stays; the user file is reported as declined
```

## Errors, startup, and reload

Every error is reported as `file:line:col path: rule`:

```text
config.toml:4:1 unified_model[0].members[1].provider: unknown provider "xx"
```

- **At startup**:
  - an invalid `config.toml` is fatal;
  - an invalid user plugin is skipped and reported, and everything else loads;
  - a unified model that needs a skipped plugin is dropped and reported.
- **On reload**:
  - the whole new state is built and validated first;
  - any error rejects the reload, and the previous state keeps serving;
  - in-flight requests keep the snapshot they started with;
  - nothing watches the files. Each mutating command (`accounts`, `keys`, `behaviour`,
    `plugins install`/`uninstall`) writes its file atomically and then asks the running
    server to reload over the operator socket. It prints `applied` when the server
    acknowledged, or `saved; applies at next start` when no server is running. A hand
    edit applies at the next start or the next such reload.

`zerorouter check` prints the load report:
- provider counts;
- pending and declined conflicts;
- withheld credentials;
- skipped plugins;
- dropped unified models.

# Operator configuration

The operator's state lives in one directory, `$ZEROROUTER_HOME` (default `~/.0router`):

```text
$ZEROROUTER_HOME
├── config.toml     # unified models, per-provider settings, plugin decisions
└── plugins/        # user plugins (*.toml, top level only), see plugins.md
```

Nothing here is required. A missing directory or file means no unified models, default
settings, and no user plugins.

Full reference: [`specs/002-provider-model-registry/contracts/operator-config.md`](../specs/002-provider-model-registry/contracts/operator-config.md).

## Addressing models

Clients name a target in one of two ways:

- **`provider/model`**: a direct target. `provider` is an id or alias (`kr`, `kiro`), and
  the model part may contain further `/` (`openrouter/meta-llama/llama-3`).
- **A bare name**: a **unified model** you declared. A bare name never falls back to
  a provider's model, so `claude-sonnet-4.5` alone is not found unless you declared it.

```bash
zerorouter-cli resolve kr/claude-sonnet-4-5 --json
zerorouter-cli model kr claude-sonnet-4-5          # what the provider declares about it
zerorouter-cli providers --capability tts
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
zerorouter-cli resolve sonnet
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
  - nothing watches the files, so reload is always explicit.

`zerorouter-cli check` prints the load report:
- provider counts;
- pending and declined conflicts;
- withheld credentials;
- skipped plugins;
- dropped unified models.

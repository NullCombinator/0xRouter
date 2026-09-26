# Contract: Operator Home and `config.toml`

**Consumers**: the operator (hand-edited), the loader, the future dashboard (which will
write this file).
**Entity mapping**: [data-model.md § Operator state](../data-model.md#operator-state)

## Layout

```text
$ZEROROUTER_HOME  (default: ~/.0router)
├── config.toml
└── plugins/
    └── *.toml        # user plugins; top level only, other extensions ignored
```

A missing home, config file, or plugins directory is not an error. It means no unified
models, default settings, and no user plugins.

## `config.toml`

```toml
schema = 1

# ── Unified models (FR-014 – FR-016) ─────────────────────────────
[[unified_model]]
name = "sonnet-4.5"              # no "/" allowed
kind = "llm"                     # optional
members = [
  { provider = "cc",        model = "claude-sonnet-4-5" },     # alias or id
  { provider = "openrouter", model = "anthropic/claude-sonnet-4.5" },
]

[[unified_model]]
name = "embed"
members = [{ provider = "openai", model = "text-embedding-3-large" }]

# ── Per-provider settings (Clarify Q1) ───────────────────────────
[provider.openai]
allow_uncatalogued_models = false   # default true

# ── Bundled-plugin conflict decisions (FR-013) ───────────────────
[plugin_decisions]
gemini-cli = "replace"  # user plugins/gemini-cli.toml becomes active
kiro   = "decline"      # bundled kiro stays active
```

Unknown keys are rejected, as they are in plugins.

## Validation

| Rule | Error (shape) |
|---|---|
| `name` contains `/` or is empty | `config.toml:4 unified_model[0].name: must be non-empty and must not contain "/"` |
| Duplicate unified model name | `…unified_model[2].name: duplicate of unified_model[0]` |
| Member provider unknown | `…unified_model[0].members[1].provider: unknown provider "xx"` |
| Member model not in catalogue (non-passthrough) | `…members[0].model: "foo" is not declared by provider "claude"` |
| Same provider twice in one unified model | `…members[2]: provider "claude" already a member` |
| Conflicting kinds | `…members[1]: kind "embedding" conflicts with unified_model kind "llm"` |
| `[provider.<id>]` for an unknown provider | `…provider.nope: unknown provider` |
| `plugin_decisions.<id>` not a bundled id, or value not `replace`/`decline` | `…plugin_decisions.foo: …` |

Member resolution uses the same alias index and model lookup as requests. The
`upstream_id` for each member is resolved once, when the file loads.

## Load report

Each load (startup or reload) produces a report. It is shown by `zerorouter-cli check`
and later by the dashboard. It lists:
- pending conflicts (a user plugin shadowed by a bundled one with no decision yet);
- declined user plugins;
- credentials withheld under FR-012a, with the offending URL;
- user plugins skipped at startup, with their errors.

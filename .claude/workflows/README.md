# 0router Workflows

Orchestration programs that chain agents in phases. Invoked by the Claude Code harness
workflow runner as `Workflow({ name: "<name>", args: { ... } })`.

## Runtime environment

Each workflow file is a JS module executed by the harness with top-level `await` support.
The following globals are injected at runtime:

| Global | Type | Description |
|---|---|---|
| `args` | `any` | The `args` object (or string) passed to `Workflow({…})` |
| `log(msg)` | `(string) => void` | Emit a log line visible in the workflow output |
| `phase(name)` | `(string) => void` | Mark the start of a named phase in the UI |
| `agent(prompt, opts)` | `async (string, AgentOpts) => any` | Invoke a subagent; returns the parsed response matching `opts.schema` |
| `pipeline(items, fn)` | `async (T[], (T) => any) => any[]` | Map over items, running `fn` for each; items within a batch may run in parallel |

### `AgentOpts`

```ts
{
  label: string,         // Short label shown in the UI for this agent call
  phase: string,         // Phase name (for grouping in the UI)
  schema: JSONSchema,    // Expected return type; the agent is prompted to match it
  agentType?: string,    // Agent name from .claude/agents/ by name: frontmatter value
                         // (defaults to general-purpose when omitted)
}
```

The `agentType` field references agents by their `name:` frontmatter value. Each agent's
`model:`, `effort:`, and `tools:` frontmatter automatically applies when it is invoked
this way. Use the right agent for the task:

- `js-to-rust-porter` — 9router JS → Rust translation
- `rust-engineer` — general Rust implementation
- `performance-engineer` — implementing and benchmarking optimizations
- `perf-hypothesis-explorer` — read-only performance hypothesis investigation (no code changes)
- `architect-reviewer` — system design review

### `pipeline` concurrency

`pipeline(items, fn)` processes items respecting the concurrency limit. The harness
controls actual parallelism. Pass items in an array; the harness may run them in
sequential or parallel batches depending on the `concurrency` argument where supported.

## Workflows

| File | Input | Purpose |
|---|---|---|
| `port-module.js` | `{ js_path, rust_out? }` | 4-phase: analysis → Rust impl → parity tests → audit. Single file. |
| `port-layer.js` | `{ layer, rust_base?, concurrency? }` | Fan out port-module over all JS files in a directory layer. |
| `audit-rust-port.js` | `{ rust_path }` | Locate JS counterpart, run 7-section parity audit, triage findings. |
| `optimize-perf.js` | `{ target, benchmark_cmd?, reference_impl?, speedup_target? }` | Benchmaxxing loop: baseline → hypotheses → implement → correctness → compete → refactor. |
| `refactor-pass.js` | `{ target, benchmark_cmd? }` | ≥20% SLoC reduction with zero criterion regression gate. |

## Scope note

rtk (9router's token compression suite: compressMessages, headroom, pxpipe, caveman,
ponytail) is **out of scope** for 0router. Do not create workflows or invoke port-module
for any file under `ref/9router/open-sse/rtk/` or `ref/9router/src/lib/headroom/`.

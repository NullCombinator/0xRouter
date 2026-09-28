# Contract: Adapter kit and sandbox ABI

The adapter kit (`zerorouter-adapter-kit`) is the one crate a third-party adapter may depend on.
This contract covers:
- the Rust API that adapter authors write against;
- the WASM ABI between the module and the core;
- the limits the sandbox enforces.

Research: [R2](../research.md#r2-where-an-adapter-runs-and-what-it-sees),
[R3](../research.md#r3-sandbox-limits), [R5](../research.md#r5-checking-an-adapters-edits-before-the-guardrail),
[R9](../research.md#r9-kit-abi-and-upgrades-fr-032).

## Author API (Rust)

```rust
use zerorouter_adapter_kit::{export, Adapter, Context, Edits, Input, Reason};

struct MyHarness;

impl Adapter for MyHarness {
    // Required. Called once per upstream attempt, with the parts of the client-style request
    // that match `request.selectors` in adapter.toml.
    fn on_request(ctx: &Context, input: &Input, out: &mut Edits);

    // Optional, default no-op. Called once for a non-stream response body.
    fn on_response(ctx: &Context, input: &Input, out: &mut Edits) {}

    // Optional, default no-op. Called per stream event when `response.events = true`.
    fn on_event(ctx: &Context, input: &Input, out: &mut Edits) {}
}

export!(MyHarness);
```

| Kit item | Purpose |
|---|---|
| `Context` | The attempt context, read-only (fields in [data-model.md](../data-model.md#attemptcontext-sent-to-an-adapter)) |
| `Input` | The matched parts. `input.parts()` yields `(path: &Path, value: &serde_json::Value)` |
| `Path` | A concrete path, with `child(key)`, `index(i)` and `Display` |
| `Edits` | `out.remove(path, Reason)` and `out.convert(path, value, Reason)`. These are the only ways to change content |
| `Reason` | The closed enum of reason codes ([data-model.md](../data-model.md#reasoncode-closed-defined-by-the-kits-abi-version)) |
| `log!(…)` | Debug text to the core's log at `debug` level. It is capped at 512 bytes per call and 8 calls per invocation, and the redactor runs over it. It never reaches records |
| `export!` | A `macro_rules!` macro that generates the ABI exports. Authors never write `#[no_mangle]` or `unsafe` |

The kit re-exports `serde_json`, so adapters use it without depending on it. It offers no I/O,
clock, randomness, environment or threads, and the target has none either.

## Module ABI (core WASM module, `wasm32-unknown-unknown`)

### Custom section

`zr.abi`: 4 bytes, the little-endian `u32` ABI major version. Required. Current: `1`.

### Imports (module `zr`)

| Name | Signature | Behaviour |
|---|---|---|
| `abi_version` | `() -> i32` | Returns the host's ABI major version |
| `log` | `(ptr: i32, len: i32)` | Capped and rate-limited; a debug log line |

**Any other import, from any module, refuses the module at load** (alert `module_refused`,
bound keys work as plain clients). This includes WASI, `env` and every other import.

### Exports

| Name | Signature | Required |
|---|---|---|
| `memory` | memory | yes |
| `zr_alloc` | `(len: i32) -> i32` | yes |
| `zr_on_request` | `(ptr: i32, len: i32) -> i64` | yes |
| `zr_on_response` | `(ptr: i32, len: i32) -> i64` | if the manifest has response selectors |
| `zr_on_event` | `(ptr: i32, len: i32) -> i64` | if `response.events = true` |

A missing required export refuses the module at load. Extra exports are ignored.

### Call sequence

1. The host instantiates a fresh instance from the cached `InstancePre`.
2. It calls `zr_alloc(len)`, then writes the input JSON at the returned pointer.
3. It calls `zr_on_*(ptr, len)`. The result packs `(out_ptr << 32) | out_len`. `0` means no
   edits.
4. It reads the output JSON and drops the instance.

**Input JSON**:

```json
{"ctx": {"direction": "request", "provider": "openrouter", "target_style": "openai-chat",
         "same_style": true, "model": "anthropic/claude-sonnet-4", "model_type": "text",
         "capabilities": {"vision": true, "file_input": false, "reasoning": true},
         "stream": true, "attempt": 1},
 "parts": [{"path": "messages[2].reasoning_content", "value": "…"}]}
```

**Output JSON**:

```json
{"edits": [{"op": "remove", "path": "messages[2].reasoning_content",
            "kind": "removed", "reason": "target_rejects_field"}]}
```

If no part matches the selectors, the host skips the call and records `not_run`
(`no_selector_match`).

## Limits (defaults from `config.toml` `[adapters]`)

| Limit | Default | On breach |
|---|---|---|
| Wall time per request or response call | 20 ms (epoch deadline, async yield) | `failed{deadline}` |
| Wall time per event call | 2 ms | `failed{deadline}` |
| Linear memory | 64 MiB | `failed{memory}` |
| Input size | 16 MiB | The call is skipped, and the outcome is `failed{invalid_output: input_too_large}` |
| Output size | 16 MiB | `failed{invalid_output: output_too_large}` |
| Edits | 1,024 | `failed{invalid_output: too_many_edits}` |
| Trap (panic, unreachable, out-of-bounds) | — | `failed{trap}` |

Every failure continues the attempt without that adapter's edits and raises an
`adapter_failed` alert (FR-018). Failures never mark the adapter suspect.

## Edit checks and guardrail

These run in the core after the call, in this order:
1. **Edit checks.** Each edit must be under a declared selector, exist in the body, not overlap
   another edit, have a kind that matches its operation, and use a known reason code. Any
   failure is `failed{invalid_output: <rule>}`.
2. **Apply.** The edits are applied to a copy of the body.
3. **Guardrail.** For third-party adapters only. It decodes the copy with the manifest's style,
   and checks that tool calls, tool definitions, tool results, opaque blocks and unplaced keys
   in the edited body or event are a sub-multiset of the original's. On a violation, the
   original goes on, the version is marked `suspect`, and a `guardrail` alert and a
   `GuardrailEvent` are written.

## Versioning

- `KIT_ABI` major changes when the ABI, the JSON shapes, or the removal of a reason code
  changes. The host supports the current major and the one before it (R9).
- Kit minor versions add reason codes or helpers, and never change the ABI.
- An adapter's `adapter.toml` `kit` requirement must match the vendored kit, or the build
  refuses the adapter with `kit_version_unavailable`.

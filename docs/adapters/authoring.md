# Writing a harness adapter

A harness adapter is a small Rust crate for one client harness, such as Claude Code. It changes a
request, and sometimes a response, so that the harness's quirks don't reach a provider that
rejects them. The core compiles the crate to WASM, runs it in a sandbox, and checks every change
it makes.

An adapter is code, but it can only read the matched parts of a body and return a list of
edits. It has no network, no files and no secrets. See
[operating.md](operating.md#the-safety-model) for what the sandbox and the guardrail do.

The worked example is `adapters/community/claude-code/`.

## Layout

```text
<package>/
├── Cargo.toml
├── adapter.toml
└── src/
    ├── lib.rs
    └── (other .rs files)
```

Files outside this layout are refused (`file_not_allowed`). A package is a directory, or a
`.tar.gz` for `nullrouter adapters install`. It may hold at most 64 files, 256 KiB in total, and
no `.rs` file over 64 KiB. Directories may nest at most 8 deep.

## `Cargo.toml`

```toml
[package]
name = "nr-adapter-example"
version = "0.1.0"
edition = "2024"

[dependencies]
nullrouter-adapter-kit = "1"
```

- `[package]` needs `name`, `version` and `edition`.
- `[dependencies]` holds exactly one entry, `nullrouter-adapter-kit`. A `path`, `git` or
  `registry` key on it is refused (`dependency_source`). Any other dependency is refused
  (`foreign_dependency`).
- `[features]` may only contain `default = []`.
- These are refused: a `build` key, `proc-macro` or `proc_macro`, and the tables `[patch]`,
  `[replace]`, `[workspace]`, `[profile]`, `[lib]` and `[[bin]]`. A `build.rs` file is refused too.

## `adapter.toml`

```toml
harness = "example-harness"
style = "anthropic-messages"
kit = "1"
summary = "Example quirks: drops a field the provider rejects"

[request]
selectors = ["messages[*]"]

[response]
selectors = []
events = false
```

| Key | Rule |
|---|---|
| `harness` | Starts with a lowercase letter, then 2 to 32 lowercase letters, digits or `-`. Not `hermes`, which is built in, and not `opencode`, `grok-build` or `zcode`, which are reserved (`harness_invalid`) |
| `style` | The client style the harness speaks: `openai-chat`, `anthropic-messages`, `openai-responses` or `gemini`. Anything else is `style_unknown` |
| `kit` | The kit version the package needs. Today `"1"` |
| `summary` | At most 200 characters |
| `request.selectors` | 1 to 32 selectors. The hook runs only on parts that match one |
| `response.selectors` | 0 to 32 selectors. With none, the adapter never sees responses |
| `response.events` | `true` runs the adapter on each stream event, with the response selectors |

Unknown keys are refused (`manifest_invalid`).

## Selectors

A selector is a path into the client's JSON body, such as `messages[*]` or `tools`.

- Segments are object keys, `[N]` for one array index, or `[*]` for any index or key.
- At most 8 segments per selector.
- The host sends the hook only the parts of the body that match a selector.

An edit to a path that no selector covers is refused at run time (`outside_selector`).

## Writing the hook

```rust
use nullrouter_adapter_kit::{export, Adapter, Context, Edits, Input, Reason};

struct Example;

impl Adapter for Example {
    fn on_request(_ctx: &Context, input: &Input, out: &mut Edits) {
        for (path, _value) in input.parts() {
            out.remove(path, Reason::TargetRejectsField);
        }
    }
}

export!(Example);
```

- `on_request` is required. The host calls it once per upstream attempt.
- `on_response` is optional, for a non-stream response body. `on_event` is optional, for each
  stream event, and only runs when `response.events = true`.
- A method you don't override makes no edits for that direction.
- Don't write `#[no_mangle]`, `#[export_name]`, `#[link_section]`, `extern` blocks or `unsafe`
  yourself. `export!` generates the ABI exports, and the gate refuses the attributes and blocks
  in your own source.

### The kit's items

Import them from `nullrouter_adapter_kit`.

| Item | Purpose |
|---|---|
| `Adapter` | The trait. `on_request` is required; `on_response` and `on_event` default to no edits |
| `Context` | The attempt, read-only. See below |
| `Input` | The matched parts. `input.parts()` yields `(&Path, &serde_json::Value)` |
| `Path` | A concrete path into the body. `Path::root()`, `child(key)`, `index(i)`, and `Display` such as `messages[3].content[1]` |
| `Edits` | `out.remove(&path, reason)` and `out.convert(&path, value, reason)`. These are the only ways to change content |
| `Reason` | The closed set of reason codes below |
| `log!(…)` | A debug line for the host's log. Each line is cut at 512 bytes, and an invocation may write 8 lines. The host redacts the output |
| `export!(T)` | Generates the ABI exports for one type that implements `Adapter` |

The kit re-exports `serde_json`, so you don't need to depend on it.

### `Context`

| Field | Type |
|---|---|
| `direction` | `request`, `response` or `event` |
| `provider` | The provider id the attempt targets |
| `target_style` | The wire style of the target endpoint |
| `same_style` | Whether the client style and the target style match |
| `model` | The upstream model id |
| `model_type` | The model's type, as a string |
| `capabilities` | `vision`, `file_input` and `reasoning`, each `Option<bool>`. `None` is unknown |
| `stream` | Whether the request streams |
| `attempt` | The attempt number |

The context has no key, header, agent id, session id or record id.

### Edits and reason codes

- `out.remove(&path, reason)` makes a `removed` edit.
- `out.convert(&path, value, reason)` makes a `converted` edit that replaces the value at `path`.

The host refuses the whole edit list for a call when:

- a path is outside every declared selector (`outside_selector`), or doesn't exist (`path_missing`);
- two paths overlap, where one is a prefix of the other (`overlap`);
- there are more than 1,024 edits (`too_many_edits`);
- a replacement value is over 4 MiB (`value_too_large`);
- the edit kind does not match the reason's use (`kind_mismatch`), or the reason is unknown (`unknown_reason`).

Each of these is an invalid output. The original body goes on for that attempt. Only the
guardrail marks a version suspect, so an invalid output alone never does.

The reason codes are a closed set for ABI 1:

| Code | Use it when |
|---|---|
| `target_rejects_field` | The target provider rejects a field the client sent |
| `target_cannot_carry_block` | The target style can't carry this kind of block |
| `foreign_block` | A block came from another model or provider, such as a thinking block with another signature |
| `format_conversion` | Content is rewritten into the target's format |
| `param_unsupported_by_model` | A request parameter is not supported by this model |
| `empty_after_removal` | A text block or message is empty after removals |
| `duplicate_tool` | A built-in tool duplicates an equivalent tool |
| `role_not_accepted` | The target doesn't accept this role |

## Limits

| Limit | Value | When it's hit |
|---|---|---|
| Time per request or response call | 20 ms (`request_deadline_ms`) | The call fails with `deadline` |
| Time per event call | 2 ms (`event_deadline_ms`) | The call fails with `deadline` |
| Linear memory | 64 MiB (`memory_mib`) | The call fails with `memory` |
| Input, or output, from the host or the module | 16 MiB | The call is an invalid output (`input_too_large` or `output_too_large`) |
| Edits in one call | 1,024 | An invalid output (`too_many_edits`) |
| Replacement value | 4 MiB | An invalid output (`value_too_large`) |
| Trap (panic, unreachable, out-of-bounds) | none | The call fails with `trap` |

A failed call, of any kind, leaves the original body in place for that attempt. Only the
guardrail marks a version suspect.

The first three are defaults the operator can change in the `[adapters]` settings. See
[operating.md](operating.md#configuration). Write for the defaults: an adapter that needs more
time or memory than they allow fails on most installs.

## What the gate refuses

The gate runs on `nullrouter adapters install` before the package is built. It reports every
problem it finds, not just the first. Each line has this form:

```text
refused: <code> at <file>[:<line>]: <message>
```

| Code | Refused when |
|---|---|
| `file_not_allowed` | A file outside the layout, a symlink, a hard link, a special file, a directory nested too deep, or a path that can't be read |
| `binary_file` | A file that isn't UTF-8 text |
| `too_large` | More than 64 files, more than 256 KiB in total, or a `.rs` file over 64 KiB |
| `manifest_missing` | `Cargo.toml`, `adapter.toml` or `src/lib.rs` is absent |
| `manifest_invalid` | An unknown or ill-typed key in `adapter.toml` or `Cargo.toml`, a `summary` over 200 characters, or a missing `[package]` |
| `harness_invalid` | A malformed, built-in or reserved harness name |
| `style_unknown` | `style` is not one of the client styles |
| `selector_invalid` | A selector with bad syntax, more than 8 segments, or a count outside 1–32 (request) or 0–32 (response) |
| `foreign_dependency` | A dependency other than `nullrouter-adapter-kit` |
| `dependency_source` | The kit dependency has a `path`, `git`, `registry` or other source key |
| `build_script` | A `build` key in `Cargo.toml`, or a `build.rs` file |
| `proc_macro` | `proc-macro` or `proc_macro` in `Cargo.toml` |
| `cargo_table_not_allowed` | A table other than the allowed ones, or a `[features]` other than `default = []` |
| `parse_error` | A `.rs` file that doesn't parse |
| `unsafe_code` | An `unsafe` block, fn, impl or trait, or `unsafe` in a macro's input |
| `extern_block` | An `extern` block, or `#[link]` |
| `extern_crate` | `extern crate` of anything but `std`, `core`, `alloc` or the kit |
| `abi_attribute` | `#[no_mangle]`, `#[export_name]` or `#[link_section]` |
| `path_attribute` | `#[path]` |
| `forbidden_macro` | `include!`, `include_str!`, `include_bytes!`, `env!`, `option_env!`, `asm!`, `global_asm!` or `concat_idents!`. The names are refused anywhere, with or without `!`: as a path segment, in a `use` (renamed or not) or as a token in any macro input, so no alias or `macro_rules!` forwarding can reach them. A variable named `env` is refused too |
| `opaque_blob` | A base64-like run over 256 characters, an integer array over 256 elements, or an integer array that encodes over 128 bytes |

## Building locally

The builder, `nullrouter-builder`, is a separate binary. The core runs it as a child process.

### Setup

```bash
nullrouter-builder setup
```

`setup` installs the pinned toolchain (1.93.1), vendors the kit's dependencies, and writes its
cache under `$NULLROUTER_HOME/builder/`. Set `NULLROUTER_HOME` if you don't want the default
`~/.0router`.

### Running one build

The builder reads one JSON job on stdin and writes one JSON result on stdout.

```bash
echo '{"source_dir": "/abs/path/to/package", "out_dir": "/abs/path/to/out", "kit": "1", "abi": 1}' \
  | nullrouter-builder build
```

| Field | Meaning |
|---|---|
| `source_dir` | The package directory |
| `out_dir` | Where `module.wasm` and `build.json` go on success |
| `kit` | The kit requirement, such as `"1"` |
| `abi` | The ABI the caller wants, `1` today |

Unknown fields are refused. The builder does not run the gate. Run `nullrouter adapters install`
for the full check.

On success, the result has `ok`, `source_fp`, `wasm_hash`, `kit_abi` and `toolchain`, and
`out_dir` holds `module.wasm` and `build.json`. On failure, the result has `ok: false`, an `error`
and sometimes a `detail`:

| `error` | Meaning |
|---|---|
| `compile` | The source doesn't compile. `detail` has up to 40 lines of compiler output |
| `nondeterministic` | Two builds gave different WASM |
| `timeout` | The build passed 120 s of CPU time |
| `kit_version_unavailable` | The `kit` requirement doesn't match the vendored kit |
| `toolchain_missing` | `setup` has not run, or a required tool is not on `PATH` |

The build runs with 120 s of CPU time and 2 GiB of address space.

### Host tests

`adapters/community/` has a `.cargo/config.toml` that points the kit at the workspace copy in
`crates/nullrouter-adapter-kit`. From a package directory there, `cargo test` runs the host
tests. The root workspace excludes `adapters/`, so those builds don't touch the core's build.

### Trying a package

```bash
nullrouter adapters install ./example
nullrouter adapters show example-harness
nullrouter adapters review example-harness <version-id>
```

An install that passes the gate is stored, then built. It then waits for a review. See
[operating.md](operating.md) for each step.

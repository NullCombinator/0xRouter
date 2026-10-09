# Contract: Adapter package and validation gate

What a third-party adapter submission is, and exactly what the gate refuses. The gate runs in the
core, before any build, on local installs and catalogue installs alike (FR-031). It lists every
reason it finds (FR-011).

Research: [R7](../research.md#r7-validation-gate-for-adapter-source).

## Layout

```text
<package>/
├── Cargo.toml          # required
├── adapter.toml        # required
├── src/
│   ├── lib.rs          # required
│   └── **/*.rs         # optional modules
├── README.md           # optional
├── CHANGELOG.md        # optional
└── LICENSE*            # optional
```

The install input is a directory, or a `.tar.gz` archive with this layout at its root or
under a single top-level directory.

## `Cargo.toml` (whole file; anything else is refused)

```toml
[package]
name = "nr-adapter-claude-code"
version = "0.3.0"
edition = "2024"
license = "MIT"                  # optional; also description, authors, repository

[dependencies]
nullrouter-adapter-kit = "1"
```

The kit resolves from the builder's local registry, not from crates.io
([R8](../research.md#r8-the-builder), Kit source). A package carries no `Cargo.lock`. The
builder supplies its own pinned lock file.

## `adapter.toml`

```toml
harness = "claude-code"
style = "anthropic-messages"
kit = "1"
summary = "Claude Code quirks: foreign thinking and server-tool blocks, haiku params"

[request]
selectors = ["thinking", "output_config", "model", "messages[*]", "tools"]

[response]
selectors = []
events = false
```

The fields and rules are in [data-model.md](../data-model.md#adaptermanifest-adaptertoml).

## Gate rules and refusal messages

Refusal output is one line per reason: `refused: <code> at <location>: <message>`. Codes are
stable, and tests match them against golden `.expected` files in
`crates/nullrouter-adapters/tests/gate/invalid/`.

| Code | Refused when |
|---|---|
| `file_not_allowed` | Any file outside the layout. Also a symlink, hard link, device file, absolute or `..` path |
| `binary_file` | A file that isn't valid UTF-8, or contains NUL |
| `too_large` | Total over 256 KiB, more than 64 files, or a `.rs` file over 64 KiB |
| `manifest_missing` | `Cargo.toml`, `adapter.toml` or `src/lib.rs` is absent |
| `manifest_invalid` | Unknown or ill-typed keys in `adapter.toml` |
| `harness_invalid` | A malformed harness name, or one that is built in or reserved |
| `style_unknown` | `style` isn't a loaded client style |
| `selector_invalid` | Selector syntax, count or depth out of bounds |
| `foreign_dependency` | Any dependency other than `nullrouter-adapter-kit`, in any dependency table |
| `dependency_source` | The kit dependency has `path`, `git`, `registry` or `features` |
| `build_script` | A `build` key, or a `build.rs` file |
| `proc_macro` | `[lib] proc-macro`, or `proc-macro = true` anywhere |
| `cargo_table_not_allowed` | `[patch]`, `[replace]`, `[workspace]`, `[profile]`, `[[bin]]`, `[[example]]`, `[[test]]`, `[[bench]]`, `[lib]` (any key), `[features]` other than `default = []`, `links` |
| `parse_error` | A `.rs` file doesn't parse (`syn`) |
| `unsafe_code` | An `unsafe` block, fn, impl or trait |
| `extern_block` | `extern "…" { … }`, or `#[link]` |
| `extern_crate` | `extern crate` of anything but `std`, `core`, `alloc` or the kit |
| `abi_attribute` | `#[no_mangle]`, `#[export_name]` or `#[link_section]` |
| `path_attribute` | `#[path = …]` on a module |
| `forbidden_macro` | `include!`, `include_str!`, `include_bytes!`, `env!`, `option_env!`, `asm!`, `global_asm!`, `concat_idents!`. The names are refused anywhere, with or without `!`: as a path segment, in a `use` (renamed or not) or as a token in any macro input, so no alias or `macro_rules!` forwarding can reach them. A variable named `env` is refused too |
| `opaque_blob` | A string or byte-string literal with a base64 or hex run over 256 characters. Also an integer-array literal with more than 256 elements, or one encoding more than 128 bytes as hex or bytes |

Several reasons in one submission produce several lines. The gate never stops at the first.

## After the gate

1. `source_fp` is computed over the gated tree, and the tree is stored under
   `adapters/<harness>/<version-id>/source/`.
2. The builder job (`nullrouter-builder`, JSON on stdin):

   ```json
   {"source_dir": "…/source", "out_dir": "…", "kit": "1", "abi": 1}
   ```

   and its result on stdout:

   ```json
   {"ok": true, "source_fp": "sha256:…", "wasm_hash": "sha256:…", "kit_abi": 1,
    "toolchain": "1.93.1-wasm32-unknown-unknown"}
   {"ok": false, "error": "compile", "detail": "error[E0425]: … (first 40 lines)"}
   ```

   The builder recomputes `source_fp`, and the core refuses a result whose `source_fp`
   differs.
3. **Build refusals:** `compile`, `nondeterministic` (the two builds differ), `timeout`,
   `kit_version_unavailable`, `toolchain_missing`.

# Implementation Plan: Provider Entity & Unified Model Registry

**Branch**: `002-provider-model-registry` | **Date**: 2026-09-26 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `specs/002-provider-model-registry/spec.md`

## Summary

The first compiling slice of 0router is a Rust library, `zerorouter-registry`. It loads
provider entities from strict, data-only TOML plugins, applies operator state from
`~/.0router/config.toml` (unified models, per-provider settings, and replace/decline
decisions), and answers lookups:
- `<provider>/<model>` → one provider plus its upstream ID;
- a bare name → a declared unified model's members;
- `(provider, model)` → model info.

Snapshots are immutable and swapped atomically on explicit reload.

The 121 bundled plugins are generated from 9router's *evaluated* registry by a Node dev
tool. The four hardcoded OAuth client secrets move into a core credential table, bound
to their OAuth hosts. Parity is checked against oracle fixtures regenerated from the
pinned `ref/9router`. A thin `zerorouter-cli` exposes `check`, `validate`, `resolve`, and
`model` for operators and the quickstart.

## Technical Context

**Language/Version**: Rust stable, edition 2024 (MSRV 1.85). Node ≥ 22 is used only by
the dev-time generator.

**Primary Dependencies**: `serde`, `toml`, `serde_path_to_error`, `thiserror`,
`arc-swap`, `url`, `regex-lite`. Dev: `serde_json`, `criterion`. CLI: `clap`.
([research R12](research.md#r12-crates))

**Storage**: Files only. Bundled plugins are embedded in the binary. User plugins live
in `$ZEROROUTER_HOME/plugins/*.toml` and operator state in `$ZEROROUTER_HOME/config.toml`.
([R7](research.md#r7-operator-owned-state-location-and-format))

**Testing**: `cargo test`, split into four kinds:
- unit tests;
- `parity` (against the 9router oracle fixtures);
- `gate` (a corpus of invalid plugins);
- `reload` (concurrency).

Performance is covered by a Criterion benchmark, `resolve`.

**Target Platform**: Linux (developer machine); nothing platform-specific.

**Project Type**: Library crate plus a thin CLI in a Cargo workspace.

**Performance Goals**:
- full load under 50 ms;
- `resolve()` p50 under 1 µs, with no allocation beyond the returned upstream ID.

These are well inside SC-006. ([R11](research.md#r11-performance))

**Constraints**:
- No async runtime in the library. Reload does blocking I/O, so callers use
  `spawn_blocking`.
- No `unsafe`.
- No secrets in any plugin-visible type.

**Scale/Scope**:
- 121 bundled providers and 935 bundled models;
- tens of user plugins and unified models expected.

**Prerequisite (blocking `/speckit-implement`)**: the `claude-0router` Landlock gate
denies `~/.cargo` and `~/.rustup`, so `cargo` cannot run from this identity. The gate
change is the user's call. ([R1](research.md#r1-toolchain-availability))

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-checked after Phase 1 design.*

| Principle | Status | How this plan complies |
|---|---|---|
| **I. Plugin Safety** | ✅ Pass | See the notes below the table |
| **II. Routing Fidelity** | ✅ N/A, not blocked | No routing decision in this slice. The per-provider entity and snapshot model leave room for amortization parameters and per-agent cache state to be added later without schema breaks (`schema = 1` versioning) |
| **III. Unified Models & Provider Entities** | ✅ Pass | See the notes below the table |
| **IV. Scope Discipline** | ✅ Pass | No request bodies are touched; the registry only resolves names |
| **V. Streaming-Native SSE** | ✅ N/A | No I/O on the request path |
| **VI. Reference-Informed Behavior** | ✅ Pass | See the notes below the table |
| **VII. Trustworthy Model Tests** | ✅ N/A | The uncatalogued-by-default setting avoids pulling working models out of use |
| **VIII. Latency Observability** | ✅ N/A | Resolution is sub-microsecond and benchmarked; request latency is measured in later slices |
| Arch: Rust only, Tokio rules | ✅ Pass | Sync library, and reload is documented for `spawn_blocking` |
| Arch: No self-registration | ✅ Pass | Static tables and `LazyLock`; no `inventory` |
| Arch: Bundled provider secrets | ✅ Pass | The 4 active secrets are extracted. trae's is not bundled, because trae is disabled in the ref |
| Workflow: Benchmark gate | ✅ Pass | `benches/resolve.rs` sets the baseline |
| Workflow: Parity-audit gate | ⚠️ Scheduled | `/rust-parity-audit` on `lookup.rs` before the PR (task) |

**I. Plugin Safety**:
- `deny_unknown_fields` on every plugin struct.
- Core behaviour is chosen only by name from closed enums (quirks, auth hooks, formats,
  capability kinds).
- Secret-key denylists on free-form maps and secret checks on URLs.
- Bundled plugins pass the same `validate()`.
- The credential table is core-only and bound to OAuth hosts (FR-012a).
- Plugins never read files, run code, or reach the network. The loader reads them.

**III. Unified Models & Provider Entities**:
- One file is one entity, with capability sections present only when offered.
- Unified models are the routing target, and a bare name resolves only to a declared
  unified model.
- Combos are out of scope, but they will reference unified-model names, which cannot
  contain `/`.

**VI. Reference-Informed Behavior**:
- Lookup semantics (suffix stripping, dash/dot tolerance, the upstream-ID algorithm) are
  checked against a model-lookup oracle generated by running 9router's own functions.
- Transport, alias, and OAuth views are checked against regenerated fixtures.
- Deliberate deviations are listed in [R10](research.md#r10-where-0router-deliberately-differs-from-9router-lookups)
  and asserted explicitly.
- Nothing follows 9router's file order.

**Post-design re-check**: ✅ No violations. Nothing is needed in Complexity Tracking.

## Project Structure

### Documentation (this feature)

```text
specs/002-provider-model-registry/
├── plan.md              # This file
├── research.md          # Phase 0: R1–R12
├── data-model.md        # Phase 1: entities, validation, state machines
├── quickstart.md        # Phase 1: runnable validation guide
├── contracts/
│   ├── plugin-schema.md     # Plugin TOML format + rejection rules
│   ├── operator-config.md   # $ZEROROUTER_HOME layout + config.toml
│   └── registry-api.md      # Library API + zerorouter-cli
├── checklists/
│   └── requirements.md
└── tasks.md             # Phase 2 (/speckit-tasks, not created here)
```

### Source Code (repository root)

```text
Cargo.toml                          # [workspace]
crates/
├── zerorouter-registry/
│   ├── Cargo.toml
│   ├── build.rs                    # embeds plugins/bundled/*.toml via include_str! table
│   ├── src/
│   │   ├── lib.rs                  # RegistryHandle, Registry, re-exports
│   │   ├── schema/                 # serde structs: plugin.rs, transport.rs, oauth.rs,
│   │   │                           #   model.rs, capability.rs, config.rs, enums.rs
│   │   ├── validate/               # gate.rs (FR-007–010), secrets.rs (R5), errors.rs
│   │   ├── credentials/
│   │   │   ├── mod.rs              # SecretString, host binding (FR-012a)
│   │   │   └── bundled.rs          # GENERATED: 4-entry static table
│   │   ├── load.rs                 # bundled + user + config → candidate; conflicts
│   │   ├── registry.rs             # Registry snapshot, indices, ArcSwap handle, reload
│   │   ├── lookup.rs               # suffix, tolerance, upstream ID (9router parity)
│   │   ├── resolve.rs              # target classification, Resolution/NotFound
│   │   └── views.rs                # composed transport / alias / oauth parity views
│   ├── tests/
│   │   ├── parity.rs               # vs tests/fixtures/9router/*.json
│   │   ├── gate.rs  + gate/{valid,invalid}/*.toml
│   │   ├── secrets.rs              # no table value appears in any plugin
│   │   ├── unified.rs              # US3
│   │   └── reload.rs               # FR-024–026, SC-007
│   └── benches/
│       └── resolve.rs
└── zerorouter-cli/
    ├── Cargo.toml
    └── src/main.rs                 # check · validate · resolve · model · providers
plugins/
└── bundled/*.toml                  # GENERATED: 121 files
tools/
└── gen-bundled/
    └── generate.mjs                # evaluates ref/9router → plugins, credentials, fixtures
tests/
└── fixtures/
    └── 9router/                    # GENERATED oracle: providers, alias, oauth-urls, lookup
```

**Structure Decision**:
- A Cargo workspace with `crates/`, so later slices (routing, execution, server) can be
  added as sibling crates that depend on `zerorouter-registry`.
- Generated artefacts are committed. Each header records the `ref/9router` SHA, and
  `generate.mjs` is the only writer.
- Parity fixtures live at the repo root (`tests/fixtures/9router/`) because later slices
  will reuse the same oracle.

## Complexity Tracking

No constitution violations. Two design choices worth noting for review:

| Choice | Why | Simpler alternative rejected because |
|---|---|---|
| Node generator + committed output | The 9router registry must be *evaluated*: 14 files import computed constants | `build.rs` running Node would make every Rust build depend on Node and the ref checkout; hand-written TOML (121 files, 935 models) would drift |
| Oracle regenerated, not the committed 9router baselines | `providers-baseline.json` in the ref is already stale (claude User-Agent 2.1.258 vs 2.1.280) | Using the stale file would fail parity on correct behaviour, and editing `ref/` breaks its role as a read-only oracle |

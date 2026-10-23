# Implementation Plan: Functional Dashboard

**Branch**: `014-functional-dashboard` | **Date**: 2026-10-22 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `specs/014-functional-dashboard/spec.md`; scope brief
`specs/briefs/2026-10-22-functional-dashboard.md`.

## Summary

The dashboard's disabled controls become working forms. Every change the CLI makes moves into one
shared module of operations (`nullrouter_server::ops`, research R1), which the CLI and the
dashboard's form handlers both call, so a browser change *is* the CLI change. The change logic
leaves `crates/nullrouter-cli/src/cmd/*.rs`; those files keep only argument parsing and printing.

Forms post to `/do/<area>/<op>` and answer with POST-redirect-GET (R2). Each post is guarded by a
per-session form token, an origin check and SameSite (R3). Long changes run as engine tasks that the
page shows on reload (R4). Sign-in reuses the engine's flows and hands the browser back to the
dashboard (R5). Five changes that have no CLI command gain one (R6), and browser changes are logged
to a bounded file (R7). Pages stay server-rendered maud with no script.

## Technical Context

**Language/Version**: Rust 1.93.1 (edition 2024), as the workspace.

**Primary Dependencies**: existing ones only. axum 0.8 (forms via `axum::Form`, through
serde_urlencoded), maud, tokio, serde, `toml`, `sha2` (the form token's HMAC-SHA256 is built on it in
a few lines; no `hmac` crate), and `getrandom` (keys). `config.toml` keeps being edited by the CLI's
own text editor (`edit` in `cmd/connection.rs:251`, which preserves the operator's comments). That editor
moves to `ops` with the rest.

**Storage**: the operator home's existing files (`accounts.toml`, `config.toml`, `keys.toml`,
`proxies.toml`, `tokens/`, `plugins/`, `adapters/`), plus `dashboard-log.jsonl` (R7). Tasks and
flashes are in memory.

**Testing**: `cargo test` per crate. Each `ops` function gets unit tests. One parity suite
(`crates/nullrouter-cli/tests/dashboard_parity.rs`) performs every FR-002 change through the browser
routes and through the CLI on twin homes and compares the homes (SC-002, SC-003). Dashboard tests
cover the form token, origin, PRG and flash (SC-005), a secret-marker sweep (SC-004), confirm windows
(SC-007), and that pages still have no script (SC-008). The engine bench checks SC-006.

**Target Platform**: Linux and macOS, loopback only (009).

**Project Type**: a CLI plus a local web service, inside the existing workspace.

**Performance Goals**: SC-006: router-overhead p95 stays within 10% of the 013 baseline while the
dashboard posts changes as fast as it can.

**Constraints**: no script; CSP unchanged except that it already forbids every script; no new crate
in the request path; secrets never leave the write-only fields' handler except to the CLI's own
secret store.

**Scale/Scope**: about 60 operations across 14 CLI command files; 9 pages gain forms; 6 new CLI
subcommands (R6).

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-checked after Phase 1 design.*

| Principle | Check | Status |
|---|---|---|
| I. Plugin Safety | A custom provider is a data plugin written as TOML and passed through the same gate as any user plugin (`check_user_plugin`), with private endpoints refused unless the operator turns them on (FR-021). Adapter install, review and approve stay the 004 pipeline: the browser only starts the same steps, and the operator still decides the review. Secrets are written only to the existing secret stores. | PASS |
| II. Routing Fidelity | No routing change. Provider enable/hide (R6) filters routing candidates in the same way as account enable/disable. | PASS |
| III. Unified Models and Provider Entities | Unified models and combos keep their `config.toml` declarations. New CLI twins edit them; there is no second store. | PASS |
| IV. Scope Discipline | No content handling. | PASS |
| V. Streaming-Native SSE | Untouched. Dashboard work runs off the request path (R8). | PASS |
| VI. Reference-Informed Behavior | 9router's dashboard makes these changes through React and API routes. We keep its pages, names and controls, and replace the mechanism with forms (user constraint). | PASS |
| VII. Trustworthy Model Tests | The browser starts the same `test.plan`/`test.run`. The billed-calls confirm is kept (FR-009). Verdict rules are unchanged. | PASS |
| VIII. Latency Observability | Untouched. SC-006 guards router overhead. | PASS |

Post-design re-check: PASS. `ops` sits in `nullrouter-server`, which already depends on the engine
and registry. The dashboard depends on the server, as today. No plugin gains any capability.

## Project Structure

### Documentation (this feature)

```text
specs/014-functional-dashboard/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── dashboard-forms.md     # every /do/<area>/<op> route, its fields, its redirect
│   └── operator-cli.md        # the new CLI commands (R6) and the unchanged ones' contract
└── tasks.md                   # /speckit-tasks
```

### Source Code (repository root)

```text
crates/nullrouter-server/src/
├── ops/                       # NEW: every change, one module per area (R1)
│   ├── mod.rs                 # Ctx, Done, OpError, apply() (reload in-process or by socket), describe_loss()
│   ├── accounts.rs            # add, priority, enable/disable, exclusive, remove; signin (task)
│   ├── keys.rs                # issue, revoke, tag, set_break, set_adapter
│   ├── routing.rs             # behaviour, routing set/unset(-plugin), window
│   ├── connection.rs  proxy.rs  quota.rs (interval, poll, ack)
│   ├── plugins.rs             # install, uninstall, private endpoints; providers enable/disable/hide/show, add_compatible
│   ├── adapters.rs            # install, review, build, approve, reject, clear, rebuild, remove, review settings; catalogue install; alerts ack
│   ├── targets.rs             # unified add/set/remove, combos add/set/remove (R6)
│   └── tests.rs               # test plan/run (task), verdicts mark/clear/settings
crates/nullrouter-engine/src/
├── tasks.rs                   # NEW: Tasks table (R4)
└── (routing filter for provider enabled/hidden, R6)
crates/nullrouter-registry/src/
└── compatible.rs              # NEW: the custom compatible plugin's TOML from its form fields (R6)
crates/nullrouter-dashboard/src/
├── forms.rs                   # NEW: form token, origin check, Form extractors, PRG, flash store (R2, R3)
├── changes.rs                 # NEW: /do/<area>/<op> handlers → ops → log → 303
├── log.rs                     # NEW: dashboard-log.jsonl writer and reader (R7)
├── components.rs              # + form, field, secret field, select, switch, confirm window
└── pages/*.rs                 # disabled controls → forms; task panels; Console Log list
crates/nullrouter-cli/src/cmd/ # parsing + printing only; calls ops; new subcommands (R6)
```

**Structure Decision**: the existing workspace. `ops` goes in `nullrouter-server` beside `views`, so
reads and changes share a crate and the operator socket. The dashboard keeps its own listener and
gains no dependency on the CLI.

## Phases for tasks

1. **Foundation**:
   - `ops` skeleton (Ctx, OpError with the CLI's exit codes, apply);
   - the Tasks table;
   - the dashboard's forms layer (token, origin, PRG, flash) and the change log;
   - the form components.
2. **US1 accounts and keys**: move the accounts and keys commands into `ops`, then build the forms
   on Providers (account window) and Endpoint & Key. Sign-in runs as a task.
3. **US2 routing settings**: behaviour, routing, connection, proxy and quota move to `ops`, then the
   forms on Settings, Quota Tracker, Proxy Pools and the account window.
4. **US3 targets and tests**: the new unified and combo CLI twins and their `ops`; Combo page forms;
   tests and verdicts as tasks.
5. **US4 plugins and adapters**: the plugins and adapters commands move to `ops`; the new provider
   visibility, add-compatible and private-endpoints commands; side panel and settings window forms.
6. **US5 Console Log**: the log page and `dashboard log`.
7. **Polish**:
   - the parity suite (SC-002/003);
   - the secret sweep (SC-004);
   - CSRF tests (SC-005);
   - SC-006 bench;
   - docs (`docs/operator-config.md`, `docs/dashboard/`);
   - the security review.

Each `cmd/*.rs` move is behaviour-preserving: the existing CLI tests must pass unchanged before the
area's forms are added.

## Complexity Tracking

No constitution violations to justify.

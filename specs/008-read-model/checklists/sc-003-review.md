# SC-003 review: no read builds its own answer

Reviewed after the US1 moves, on `008-read-model`. `grep` for `crate::open`, `operator::call`,
`Accounts::load`, `Keys::load`, `TokenStore::load`, `records::read`, `records::get` and
`history::read` in `crates/nullrouter-cli/src/cmd/`.

| File | Listed read | Now |
|---|---|---|
| `accounts.rs` | `accounts list` | fetches, calls `views::accounts`, renders |
| `quota.rs` | `quota`, `quota history` | fetches, calls `views::quota`, renders |
| `routing.rs` | `routing` | fetches, calls `views::routing`, renders |
| `records.rs` | `records list`, `records show` | fetch, call `views::records`, render |
| `providers.rs`, `model.rs` | `providers`, `model` | call `views::providers`, `views::model`, render |
| `plugins.rs` | `plugins list` | calls `views::plugins`, renders |
| `keys.rs` | `keys list` | calls `views::keys`, renders |
| `check.rs` | `check` | fetches, calls `views::check`, renders |
| `resolve.rs` | `resolve` | calls `views::resolve`, renders |
| `validate.rs` | (not listed) | unchanged |

What is left in those files that still reads a file or the socket is a change, not a read: `accounts add`
and `signin` (the registry, to bind hosts), `quota poll` and `interval`, `routing set`/`unset`/`window`
(`edit_account`), `records prune` and `forget`, and the writes of `keys`. Each needs the current file
to edit it, which is not an answer a front end shows.

Result: pass.

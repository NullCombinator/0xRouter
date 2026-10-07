# Contract: Operator socket ops

These follow the existing table in `crates/nullrouter-server/src/operator.rs`. Each op is one
JSON line in and one JSON line out.

| Request | Response |
|---|---|
| `{"op":"live.snapshot"}` | `{"ok":true,"as_of":RFC3339,"paused_proxies":[{name,since,reason}],"in_flight":[LiveEntry…]}`. Entries are newest first. See [cli.md](cli.md#nullrouter-live---json-us2) for the fields. No prompt, answer, header or secret |
| `{"op":"connection.view","provider"?}` | `{"ok":true,"providers":[{id,timeouts:{connect,headers,first_token,stall}:{ms\|null,source},reuse,http2,retry:{…},proxy:{name\|null,level,paused},models:[…],accounts:[{name,proxy,level}]}]}` |
| `{"op":"proxy.fixed","name"}` | `{"ok":true,"reachable":true}` and the pause cleared, or `{"ok":true,"reachable":false,"reason"}` |
| `{"op":"records.list",…}` (exists) | each record gains `slowest:{phase,ms,side,in_progress}` or `null` |
| `{"op":"records.get","id"}` (exists) | each attempt gains `timing` and `phases` ([record.md](record.md)) |
| `{"op":"reload"}` (exists) | also reloads `proxies.toml` and connection settings; clears pauses whose proxy definition or assignment changed |

`live.snapshot` takes the live table's lock only long enough to clone the entries' `Arc`s
(R9), so a slow socket client can't hold up requests (FR-020).

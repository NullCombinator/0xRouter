# Contract: Operator CLI and operator channel

**Feature**: [spec.md](../spec.md) FR-025–FR-028 | **Research**:
[R12](../research.md#r12-operator-channel-and-cli-fr-026fr-028)

The existing commands (`check`, `validate`, `resolve`, `model`, `providers`) are
unchanged. They do not need a running server. `check` also validates `keys.toml` when it
exists and prints connection and access-key counts, never values.

## `zerorouter-cli serve`

```text
zerorouter-cli serve [--listen ADDR] [--observations-cap N]
```

| Option | Env | Default |
|---|---|---|
| `--listen` | `ZEROROUTER_LISTEN` | `127.0.0.1:20129` |
| `--observations-cap` | `ZEROROUTER_OBSERVATIONS_CAP` | `10000` (at least 1) |
| — | `ZEROROUTER_HOME` | `~/.0router` |
| — | `FETCH_CONNECT_TIMEOUT_MS` | 60000 (9router parsing) |
| — | `STREAM_STALL_TIMEOUT_MS` | 360000 (9router parsing) |
| — | `RUST_LOG` | `info` |

**Start-up**

1. Load the registry and `keys.toml` (fatal on error).
2. Bind the operator socket.
3. Bind the listener.
4. Print one line to stderr:

   ```text
   zerorouter: serving on 127.0.0.1:20129 (47 connectable providers, 2 connections, 1 access key); operator socket ~/.0router/run/operator.sock
   ```

**Stopping**: on SIGINT or SIGTERM, stop accepting new requests, let in-flight requests
run for up to 10 s, then exit 0.

**Exit codes**:

| Code | Meaning |
|---|---|
| 0 | clean shutdown |
| 1 | load error |
| 2 | usage error |
| 3 | bind error (the address is in use, or another server owns the socket) |

## `zerorouter-cli reload`

It connects to the operator socket and sends `reload`.

- **Success**: prints 002's load report (as `check` does) plus the keys summary, and
  exits 0.
- **Rejected**: prints every error and exits 1. The server keeps serving the previous
  state.
- **No server**: prints `no running server at <socket>` and exits 3.

## `zerorouter-cli obs`

```text
zerorouter-cli obs [--provider P] [--unified U] [--agent A] [--session S]
                   [--endpoint chat|messages|count_tokens|embeddings|models]
                   [--since T] [--until T] [--include-count-tokens]
                   [--limit N] [--json]
```

- `T` is RFC 3339, or relative (`15m`, `2h`, `1d`).
- The default `--limit` is 50, newest first. The summary always covers the whole
  filtered set.
- Output without `--json`:

```text
provider=anthropic  count=42 success=40  ttft p50=412ms p95=1.9s  total p50=6.1s p95=18.4s
tokens: input 18 240 · output 9 812 · cache read 402 118 · cache write 21 004  (reported by 40/42)

AT                    AGENT   SESSION        TARGET                       CONN      STATUS  OUTCOME              TTFT    TOTAL   IN     OUT   CR      CW
2026-09-27T10:02:11Z  laptop  claude:7f3a…   anthropic/claude-sonnet-4-5  personal  200     success              388ms   5.2s    412    233   11022   —
…
```

- `—` means "not reported".
- Exit codes: 0 (including zero matches), 1 (an unknown provider or unified name in a
  filter), 3 (no server).

## Operator channel protocol

- **Transport**: the Unix socket `$ZEROROUTER_HOME/run/operator.sock`.
  - The directory is 0700 and the socket is 0600.
  - A peer whose uid is not the server's is closed without a response.
- **Framing**: one UTF-8 JSON object per line. There is one request per connection, and
  the server closes after responding.

**Requests**

```json
{"op":"status"}
{"op":"reload"}
{"op":"observations","filter":{"provider":"anthropic","unified":null,"agent":null,"session":null,"endpoint":null,"since":"2026-09-27T09:00:00Z","until":null,"include_count_tokens":false,"limit":50}}
```

**Responses**

```json
{"ok":true,"status":{"generation":3,"listen":"127.0.0.1:20129","observations":812,"cap":10000,"connections":2,"access_keys":1}}
{"ok":true,"report":{ /* 002 LoadReport, serialized */ },"keys":{"connections":2,"access_keys":1}}
{"ok":false,"errors":["keys.toml:12:12 connection[1].provider: unknown provider \"cloudfare-ai\""]}
{"ok":true,"summary":{ /* ObservationSummary */ },"observations":[ /* Observation, newest first */ ]}
```

`Observation` and `ObservationSummary` follow [data-model.md](../data-model.md). The
JSON field names are exactly the data-model field names. No response ever contains key
material.

# Contract: operator socket ops

Added to the table in `crates/nullrouter-server/src/operator.rs`. All requests are one NDJSON
line; all responses are one line except `test.run`.

| Request | Response |
|---|---|
| `{"op":"test.plan","target"?,"account"?,"all"?}` | `{"ok":true,"pairs":[{provider,account,model,type,skip?}],"calls":{"text":N,…}}`; `target` is `provider/model`, a unified model or a combo; a combo counts as 1 call of its kind |
| `{"op":"test.run","target"?,"account"?,"all"?}` | streamed: one `{"event":"result","result":TestResult}` line per pair (or one `{"event":"combo","result":ComboResult}`), then `{"ok":true,"done":{"pass":N,"broken":N,"unknown":N,"skipped":N}}`. Closing the connection cancels calls not yet sent |
| `{"op":"verdicts.list","provider"?,"account"?,"model"?,"state"?}` | `{"ok":true,"verdicts":[Verdict + {"waiting"?:reason}]}` |
| `{"op":"verdicts.set","provider","account","model","state":"broken"\|"clear","note"?}` | `{"ok":true}`, or `{"ok":false,"error":"no verdict for …"}` on clearing an untested pair |

`TestResult`, `ComboResult` and `Verdict` are as in [data-model.md](../data-model.md). Reasons
pass through the redactor; no secret, prompt or generated output crosses the socket.

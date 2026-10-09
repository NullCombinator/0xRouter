# Hostile adapter corpus (spec 004, US3)

Each directory is one attack. `module.wat` is the module the suite runs (assembled with the `wat` crate at test time; nothing here is a checked-in `.wasm`). `adapter.toml` is its manifest, `expect.toml` what the suite (T066) asserts, and the JSON file named by `input` the body it runs on. `source/` is a kit package for the operator-run build (`tools/build-adapter-fixtures.sh`); net, file, env and bad_output have none because safe kit code cannot express them. Nothing in CI builds `source/`.

`detail` is the last code the alert shows: a not-run reason, a failure kind or invalid-output rule, or a guardrail rule.

- `net`: imports wasi_snapshot_preview1.sock_open to open a socket. Must happen: not run, no_approved_version, alert module_refused.
- `file`: imports wasi_snapshot_preview1.path_open to read the filesystem. Must happen: not run, no_approved_version, alert module_refused.
- `env`: imports wasi_snapshot_preview1.environ_get to read the environment. Must happen: not run, no_approved_version, alert module_refused.
- `secret_probe`: scans its whole input for sk-, Bearer and NR-SENTINEL-, logs and writes any hit into `user`. Must happen: runs with no changes; no secret reaches it.
- `loop`: spins forever in zr_on_request; stopped at the 20 ms deadline. Must happen: failed (deadline), alert adapter_failed.
- `memory_bomb`: grows linear memory to 128 MiB, past the 64 MiB cap. Must happen: failed (memory), alert adapter_failed.
- `trap`: executes `unreachable`. Must happen: failed (trap), alert adapter_failed.
- `bad_output`: answers text that is not JSON. Must happen: failed (not_json), alert adapter_failed.
- `out_of_selector`: removes `max_tokens` while its only selector is `model`. Must happen: failed (outside_selector), alert adapter_failed.
- `add_tool_call_request`: replaces the user turn with an assistant turn that calls run_shell. Must happen: blocked (tool_call_added), alert guardrail, version marked suspect.
- `change_tool_args`: rewrites the arguments of the existing get_weather call. Must happen: blocked (tool_call_changed), alert guardrail, version marked suspect.
- `add_tool_call_response`: adds a run_shell tool call to a non-stream answer. Must happen: blocked (tool_call_added), alert guardrail, version marked suspect.
- `add_tool_call_event`: starts a run_shell tool call inside a stream event. Must happen: blocked (tool_call_added), alert guardrail, version marked suspect.
- `add_tool_def`: adds a run_shell tool definition. Must happen: blocked (tool_def_added), alert guardrail, version marked suspect.
- `rewrite_tool_result`: rewrites the content of the tool result for call c1. Must happen: blocked (tool_result_changed), alert guardrail, version marked suspect.
- `add_unplaced_field`: adds a key no style rule reads (`x_smuggled`) to a tool definition. Must happen: blocked (unplaced_added), alert guardrail, version marked suspect.
- `legit_removal`: removes `max_tokens` and `temperature`: a removal the guardrail must let through. Must happen: runs and the core takes its edits.

# Parity audit: hermes and the Claude Code adapter (T093)

Protocol: `.claude/skills/rust-parity-audit/SKILL.md`. Oracle: `ref/9router` at `39e36d3`.
Parity is judged on chatCore's request path (`open-sse/handlers/chatCore.js`). A quirk that only
shows up off that path is Low and accepted. The decisions already taken in research.md R4, R15
and R17 are accepted deviations, so they are listed here as Low.

Result: **0 Critical, 0 High, 0 Medium, 15 Low** (11 Claude Code, 4 hermes). Neither file was
changed. The Claude Code adapter's source, and so its `source_fp`, are unchanged.

## 1. Scope

| Rust | 9router | Purpose |
|---|---|---|
| `crates/nullrouter-adapters/src/builtin/hermes.rs` | `open-sse/translator/concerns/paramSupport.js:30-32,57-64` via `executors/default.js:78` (echoed reasoning); `services/combo.js:145-157` and `translator/concerns/modality.js:66-75` (images and attachments) | Remove echoed reasoning for providers that reject it; convert Ollama-style images and attachments to content parts |
| `adapters/community/claude-code/src/lib.rs` (+ `adapter.toml`) | `open-sse/translator/formats/claude.js:204-321` (`normalizeClaudePassthrough`), `utils/claudeSignature.js:9-41`, `utils/toolDeduper.js:6-47`, called at `chatCore.js:203` and `:219-224` | Claude Code quirks on the anthropic-messages body |

Side effects: none in either file. Both return edits, and the core applies and records them.

Request-path callers in 9router:
- `normalizeClaudePassthrough` runs only when `clientTool === "claude"` and the request is a native
  passthrough (`chatCore.js:165-166,203`). Passthrough means the provider is `claude`, `anthropic`
  or `anthropic-compatible-*` (`clientDetector.js:7-8,62-65`).
- `dedupeTools` runs on `translatedBody.tools` for every Claude client, on every target
  (`chatCore.js:219`).
- `stripUnsupportedParams` runs in `DefaultExecutor.transformRequest` (`default.js:78`) for every
  client. groq, mistral and cerebras have no executor of their own, so they reach it.

## 2. Interface parity

| JS | Rust | Notes |
|---|---|---|
| `normalizeClaudePassthrough(body, model)` | `ClaudeCode::on_request(ctx, input, out)` | `model` is `ctx.model` (the upstream id). 9router uses `stripThinkingSuffix(upstreamModel)`, and the haiku test is unaffected by the suffix ✓ |
| `isValidClaudeSignature(raw)` | `is_valid_claude_signature(Option<&Value>)` | E-form and R-form both handled, using Node's lenient base64 (`node_base64`) ✓ |
| `hasForeignServerToolUseId` / `CLAUDE_SERVER_TOOL_USE_ID` | `is_native_server_tool_id` | `^srvtoolu_[a-zA-Z0-9_]+$`. JS `$` without `m` anchors only at the end of input ✓ |
| `dedupeTools(tools)` | `dedupe_tools` | The 3 rules, the `name \|\| function.name` lookup, prefix regexes as `Pattern::Prefix` ✓ |
| `stripUnsupportedParams` (`dropMessageFields` rules) | `hermes::on_request` + `REJECTS_ECHOED_REASONING` | Same 3 providers and 3 fields, assistant turns only, `!== undefined` (so `null` counts as present) ✓ |

## 3. Error contracts

- 9router throws in three places on malformed input:
  - a non-string truthy `signature` (`claudeSignature.js:10`, outside the `try`);
  - a `null` message (`claude.js:231`);
  - a `null` block (`claude.js:270`).

  The throw fails the request. Rust treats the bad signature as invalid and leaves `null`
  messages and blocks alone. It doesn't panic and doesn't `unwrap` outside tests (CC-7, CC-8).
- hermes has no error path: what it can't convert stays in its field.

## 4. State and mutation

- 9router changes the body in place. The adapters return edits instead, so the client body stays
  untouched and a fallback attempt reruns against the original. This matches 9router's
  copy-on-write intent for the fold (`claude.js:244-246`).
- No edits overlap:
  - a turn is written either as per-block removals, a `content` replace, a whole-message
    replace, or a whole-message removal;
  - folded system messages are removed at their own indices.

## 5. Streaming

Not applicable. The Claude Code adapter declares no response selectors and `events = false`.
hermes has no response side.

## 6. Configuration surface

All values match 9router:
- haiku test: `/haiku/i` (ASCII case-insensitive `contains`);
- thinking budget: `10000`;
- signature limits: `MAX_CLAUDE_SIGNATURE_LEN` is 32 MiB, measured in UTF-16 units; marker `0x12`;
- the three `DEDUP_RULES`;
- hermes's three `STRIP_RULES` entries.

## 7. Translator routes

Not applicable.

## 8. Fixture trace (by hand, against `tests/fixtures/9router/adapters/claude-code/*`)

| Fixture | Rust path | Result |
|---|---|---|
| thinking-foreign-signature | `drop_foreign`: `CiQ…` is not E/R, so it's dropped. `EkAK…` decodes to `0x12` first, so it's kept | = out |
| redacted-thinking-foreign-signature | `[1]` foreign signature dropped, `[3]` kept, `[5]` has no signature → `None` → dropped | = out |
| server-tool-use-foreign-id | `call_9f2c41d7` and `call_a07be3` are dropped and their ids collected. The results pass drops `[1].content[2]` (web_search_tool_result) and `[4].content[0]` (tool_result). `[3]` is left empty and removed. Two user turns stay adjacent | = out |
| server-tool-use-srvtoolu | native ids, no edits | = out |
| mid-conversation-system | `[1]` is folded into `[0]` (content replace). `[3]` follows an assistant turn, so it becomes its own user turn (message replace). `[4]` is blank and removed | = out |
| bare-content-block | `[0]` and `[1]` are wrapped, and `cache_control` is dropped | = out |
| adaptive-thinking-haiku | `thinking` converted | = out |
| output-config-effort-haiku | `output_config` has only `effort`, so the whole key is removed | = out |
| empty-text-and-empty-messages | `[0]`, `[2]` and `[3]` removed; `[1].content[0]` and `[4].content[1]` removed | = out |
| duplicate-tools | exa → WebSearch, WebFetch and mcp__workspace__web_fetch go. browsermcp → Claude_in_Chrome goes. `tavily_crawl` is not a trigger | = out (= `dedupe_stripped`) |
| thinking-placeholder | `[3].content[0]` (foreign) removed; no placeholder | = out, minus the two placeholders (CC-1, accepted) |

The adapter's unit tests inline these fixtures, because the gate refuses `include_str!`.

## Findings

| id | severity | file:line | 9router ref | finding | decision |
|---|---|---|---|---|---|
| CC-1 | Low | `adapters/community/claude-code/src/lib.rs:277-292` | `translator/formats/claude.js:286-288` | No thinking placeholder is put in front of a tool-use turn that lacks valid thinking when thinking is enabled | accepted (R15, R17: an adapter never adds content) |
| CC-2 | Low | `lib.rs:31-41` | `chatCore.js:165,203,219`; `utils/clientDetector.js:35` | The handling is bound to the key's harness, not chosen by detecting the client from its User-Agent or `x-app` header | accepted (R15, R17, FR-002) |
| CC-3 | Low | `lib.rs:36` | `utils/clientDetector.js:7-8,62-65` | Step 5 runs only for `provider == "anthropic"` on the same style. 9router's passthrough also covers `anthropic-compatible-*` and `claude`. In 0router a Claude subscription is an account on `anthropic` (slice 005), so only anthropic-compatible providers differ | accepted (R15 oracle details) |
| CC-4 | Low | `lib.rs:204-211` | `translator/request/claude-to-openai.js:172-231` (no case for these blocks) | On non-Anthropic targets, `server_tool_use` and `web_search_tool_result` are removed and recorded. 9router's translator drops them silently, so the wire outcome is the same | accepted (R15 step 6, R17) |
| CC-5 | Low | `lib.rs:204-211` | `claude-to-openai.js:199-217` | On non-Anthropic targets, a `tool_result` that references a removed server-tool id stays and becomes an orphan tool message. 9router's translator emits the same orphan | accepted (same as the oracle on its request path) |
| CC-6 | Low | `lib.rs:35-40` | `chatCore.js:203` (passthrough only); `claude-to-openai.js:153-155` | Steps 1-4 and 7 run on every target, while 9router runs them only on passthrough. On translated paths, 9router's translator turns a mid-conversation system message into a separate string user turn instead of folding it. The haiku downgrade also applies to, for example, openrouter haiku | accepted (R15 scopes only steps 5 and 6; T082 decision) |
| CC-7 | Low | `lib.rs:375-376` | `utils/claudeSignature.js:9-13` | 9router throws on a non-string truthy `signature`, which fails the request. 0router treats the block as foreign and removes it | accepted (9router crash path) |
| CC-8 | Low | `lib.rs:147-164,195-197` | `claude.js:231,270` | 9router throws on a `null` message or `null` block. 0router leaves them for the codec | accepted (9router crash path) |
| CC-9 | Low | `lib.rs:320-349` | n/a (records) | In a wrapped or folded turn, blocks removed by steps 5 to 7 go out inside the `content` replace. They are recorded as `format_conversion` or `role_not_accepted`, not `foreign_block` or `empty_after_removal`. The wire body is the same | accepted (a recording detail; an edit can't touch both a parent and its child) |
| CC-10 | Low | `adapters/community/claude-code/adapter.toml:7` | n/a | The manifest selects `messages[*]`. R15 lists `messages[*].role` and `messages[*].content`, but whole-message replace and removal need the wider selector | accepted (research.md R15 wording is stale) |
| CC-11 | Low | `lib.rs:470-483` | `chatCore.js:219-224` | Dedupe reads the client's tools before encoding, while 9router reads `translatedBody.tools`. Names only differ under claudeCloaking's renaming, which is intentionally absent | accepted |
| H-1 | Low | `crates/nullrouter-adapters/src/builtin/hermes.rs:57-70` | `executors/default.js:78`; `translator/concerns/paramSupport.js:30-32,57-64` | Echoed reasoning is removed only on a hermes key. 9router removes it in core for any client | accepted (R4, R17) |
| H-2 | Low | `hermes.rs:83-185` | `translator/concerns/modality.js:66-75`; `services/combo.js:145-157` | `images` and attachments are converted to content parts. 9router deletes them for a model that can't read them, and otherwise forwards them as is | accepted (R4, R17) |
| H-3 | Low | `hermes.rs:64` | `executors/default.js:78` | Removal happens only on same-style attempts. groq, mistral and cerebras are all openai-chat, and hermes talks openai-chat, so the difference can't be reached | accepted |
| H-4 | Low | `hermes.rs:142-185` | n/a (no 9router counterpart) | Not a parity finding, a note against R4: images are converted whatever `ctx.capabilities.vision` says, and only `application/pdf` becomes a `file` part. Other MIME types stay in their field, and slice 003 decides about the target | accepted (matches R4's "left unconverted" rule; a spec-owner note) |


## 9. Test coverage

- The Claude Code unit tests cover all 11 fixtures, plus:
  - non-Anthropic targets;
  - the signature predicate, including Node base64 edge cases;
  - JS `trim`, `String` and truthiness semantics;
  - the fold into a turn without `content`;
  - non-object messages.
- hermes is covered by `crates/nullrouter-adapters` and the engine tests (research R16). This
  audit made no test changes.

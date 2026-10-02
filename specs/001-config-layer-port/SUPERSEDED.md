# SUPERSEDED — see note

This spec was authored on 2026-09-26 under the `opusplan` setting, which ran execution
on `claude-sonnet-4-6`. It targets an exact file-by-file parity port of 9router's
`config/` layer, which the constitution v2.0.0 (amended same day) no longer mandates.

**Known errors** (from a cross-check against ref/9router source):

1. "No secrets in declarations" — false. Five 9router registry files hardcode OAuth
   `clientSecret`. Bundled plugins must strip these into a core credential table.
2. Unmatched 4xx (not 401/402/403/429) → "30s transient" — wrong. Source:
   `accountFallback.js:57-59` returns `{shouldFallback:false, cooldownMs:0}` instead.
3. The 30-min cap is not a config-layer rule on retry-after. It lives in
   `src/sse/services/auth.js:257` and applies only to `resetsAtMs`.
4. Kiro queries work only via alias `"kr"`, not `"kiro"`.
5. Alias→transport lookup is not in config — it's `services/model.js::resolveProviderAlias`.
6. Thinking-suffix regex leaves nested parens unstripped. Unknown model returns baseId+suffix,
   not None.
7. `getModelType` returns null when no kind/type, not "llm".
8. Model field is `targetFormat`, not `format`.
9. Categories are `apikey/oauth/freeTier/free/webCookie`. No `local`.
10. Only `id` + `category` are required. 29 entries have no transport.
11. Registry has ~92 providers with transport (121 total), not "40+".
12. Registry files can't be regex-parsed — 15 contain functions. Use `snapshot-providers.mjs`.
13. Retry values have no env overrides. `parseInt` accepts "120000abc".
14. Crate name `0router-config` is invalid (starts with digit). Use `nullrouter-config`.
15. FR-009 locks built-ins — contradicts the 2026-09-26 decision (bundled providers are plugins
    the user can replace).
16. The plan's constitution check cites "FR-004" for the parity audit, but FR-004 is
    error classification.

**What to do**: Run `/speckit-specify` on Opus 5.5 for a new spec targeting the
"provider entity & unified model registry" slice, incorporating these findings and the
init.md-wins direction. See the plan file for the full input.

The parity-oracle tests to reuse are in `ref/9router/tests/__baseline__/` and
`ref/9router/tests/unit/`.

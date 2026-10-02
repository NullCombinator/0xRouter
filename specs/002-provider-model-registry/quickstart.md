# Quickstart: Validating the Provider & Unified Model Registry

Runnable checks that prove the slice works end to end. Each one maps to a spec story or
success criterion. Commands run from the repo root.

## Prerequisites

1. **Toolchain reachable from the `claude-0router` identity** ([research R1](research.md#r1-toolchain-availability)):
   `cargo --version` must succeed inside the gate.
2. **Node ≥ 22**: only needed to regenerate bundled plugins or oracle fixtures.
3. A scratch operator home, so your real `~/.0router` is untouched:
   ```bash
   export NULLROUTER_HOME="$(mktemp -d)"
   ```

## 1. Regenerate the bundled set and the oracle (only after updating `ref/9router`)

```bash
node tools/gen-bundled/generate.mjs
git diff --stat plugins/bundled tests/fixtures/9router crates/nullrouter-registry/src/credentials
```

**Expect**:
- 121 files in `plugins/bundled/`;
- 4 entries in the credential table (antigravity, gemini-cli, gemini, iflow);
- the header line of every bundled file names the `ref/9router` SHA.

## 2. Parity with 9router (US1, US2 · SC-001, SC-004)

```bash
cargo test -p nullrouter-registry --test parity
```

**Expect**: all pass. The tests assert:
- 83 composed transports equal `tests/fixtures/9router/providers.json`;
- 113 alias tokens resolve as in `alias.json`, and `qw`, `dv`, `devin`, `devin-cli`
  return not-found;
- the 83-entry id-to-alias map and all OAuth URL groups match, except that `mimo-free`
  maps to itself: 0router drops its `mmf` alias, which collides with another provider;
- all 2493 rows of `lookup.json` match: the 987 declared models of the 100 catalogued
  providers plus 1506 edge inputs. The three `mmf` rows for undeclared models expect
  not-valid, because `mmf` is no longer a passthrough alias.

## 3. No secrets in plugins (US1 scenario 4 · SC-002)

```bash
grep -rlE 'GOCSPX-|REDACTED_IFLOW_OAUTH_CLIENT_SECRET|client_secret' plugins/ && echo FAIL || echo OK
cargo test -p nullrouter-registry --test secrets   # scans every bundled file for all 4 table values
```

**Expect**: `OK`.

## 4. Validation gate (US4 · SC-003)

```bash
cargo test -p nullrouter-registry --test gate
cargo run -q -p nullrouter-cli -- validate crates/nullrouter-registry/tests/gate/invalid/*.toml
```

**Expect**:
- the tests pass;
- the CLI exits 1 and prints one error per file, each in the form
  `file:line:col field.path: rule`.

The corpus has at least one file per rule in
[contracts/plugin-schema.md § Rejected content](contracts/plugin-schema.md#rejected-content-validation-gate).

## 5. Declare and resolve a unified model (US3 · SC-005)

```bash
cat > "$NULLROUTER_HOME/config.toml" <<'EOF'
schema = 1
[[unified_model]]
name = "sonnet"
members = [
  { provider = "kr", model = "claude-sonnet-4-5" },
  { provider = "openrouter", model = "anthropic/claude-sonnet-4.5" },
]
EOF
cargo run -q -p nullrouter-cli -- resolve sonnet --json
```

**Expect**:
- two members, in declaration order;
- the `kiro` member's upstream ID is dot-normalised (`claude-sonnet-4.5`, via tolerance);
- the `openrouter` member (passthrough) keeps the requested ID.

## 6. Addressing rules (Clarify Q1, Q2)

```bash
cargo run -q -p nullrouter-cli -- resolve kr/claude-sonnet-4-5 --json          # direct, catalogued
cargo run -q -p nullrouter-cli -- resolve openai/brand-new-model --json         # direct, uncatalogued → allowed by default
cargo run -q -p nullrouter-cli -- resolve claude-sonnet-4.5; echo "exit=$?"     # bare, undeclared → exit=2
printf '\n[provider.openai]\nallow_uncatalogued_models = false\n' >> "$NULLROUTER_HOME/config.toml"
cargo run -q -p nullrouter-cli -- resolve openai/brand-new-model; echo "exit=$?" # → exit=2
```

## 7. Conflicts and credential binding (US4 scenarios 5–7 · FR-012a)

```bash
mkdir -p "$NULLROUTER_HOME/plugins"
sed 's#oauth2.googleapis.com#evil.example#' plugins/bundled/gemini-cli.toml \
  | sed '/^# Generated/d' > "$NULLROUTER_HOME/plugins/gemini-cli.toml"
cargo run -q -p nullrouter-cli -- check                     # conflict pending; bundled gemini-cli active
printf '\n[plugin_decisions]\ngemini-cli = "replace"\n' >> "$NULLROUTER_HOME/config.toml"
cargo run -q -p nullrouter-cli -- check                     # user gemini-cli active; credential WITHHELD (evil.example)
```

## 8. Reload safety (FR-024 – FR-026 · SC-007)

```bash
cargo test -p nullrouter-registry --test reload
```

**Expect**: pass. The test runs repeated reloads while many threads resolve targets. It
checks that no lookup fails or sees a mixed snapshot, and that a reload with a broken
file leaves the old snapshot serving.

## 9. Performance gate (Constitution · SC-006)

```bash
cargo bench -p nullrouter-registry --bench resolve
```

**Expect**:
- full load under 50 ms;
- `resolve` p50 under 1 µs.

Record the numbers as the Criterion baseline for later slices:
`cargo bench -p nullrouter-registry --bench resolve -- --save-baseline slice-002`.

Measured for slice 002 (release build, median): load ≈ 15 ms; resolve direct ≈ 465 ns,
direct with suffix ≈ 869 ns, unified ≈ 48 ns, uncatalogued ≈ 226 ns, not found ≈ 88 ns.

# Contract: Adapter catalogue

The catalogue is an index file in a 0router-owned public repository (clarify Q6). Authors are
listed by a pull request that maintainers merge. There is no service, no accounts and no uploads.
The catalogue lists source only, and being listed grants no trust: every install goes through the
gate, build, review and operator decision (FR-031).

Research: [R12](../research.md#r12-catalogue).

## Location

- File: `catalogue/index.toml` in this repository.
- Default URL (`config.toml` `[adapters] catalogue_url`): the repository's raw URL for that file
  on the default branch. Operators may point it at a mirror.
- The Claude Code adapter's source lives at `adapters/community/claude-code/`. Its archives are
  release assets of this repository.

## Format

```toml
schema = 1

[[entry]]
harness = "claude-code"
summary = "Claude Code: foreign thinking/server-tool blocks, haiku params, duplicate tools"
homepage = "https://github.com/…/0router/tree/main/adapters/community/claude-code"
style = "anthropic-messages"

[[entry.version]]
semver = "0.1.0"
source = "https://github.com/…/0router/releases/download/adapter-claude-code-0.1.0/claude-code-0.1.0.tar.gz"
sha256 = "…"                         # of the archive bytes
source_fp = "sha256:…"               # of the canonical unpacked tree (data-model.md)
kit = "1"
```

Rules, checked by CI in this repository and again by the core on fetch:
- `harness` is unique. It is not built in and not reserved.
- `semver` is unique per entry.
- `source` is HTTPS.
- `sha256` is 64 hex digits.
- Unknown keys are refused (`deny_unknown_fields`).
- The index is at most 1 MiB.

## When 0router contacts the catalogue

Only on these operator commands, never in the background, at startup, or on a timer (FR-031,
SC-012):

| Command | Requests |
|---|---|
| `nullrouter catalogue list` / `show <harness>` | the index |
| `nullrouter catalogue install <harness> [<semver>]` | the index, then one archive |
| `nullrouter catalogue check` | the index |

## Fetch and verify

1. **Fetch** with the core's HTTP client, under the SSRF rules (003 R17):
   - HTTPS only;
   - redirects are followed only to the same host, at most 3;
   - index ≤ 1 MiB, archive ≤ 2 MiB;
   - 30 s timeout.
2. The archive's SHA-256 must equal `sha256`. On a mismatch, the fetch is refused with
   `catalogue_hash_mismatch`, and nothing is unpacked.
3. **Unpack** safely, into a temp dir under `$NULLROUTER_HOME/adapters/.staging/`.
   - Refused: symlinks, hard links, device files, absolute or `..` paths, more than 64
     entries, more than 256 KiB unpacked.
   - A single top-level directory is stripped.
4. The canonical `source_fp` must equal `source_fp`, or the install is refused with
   `catalogue_fp_mismatch`.
5. The gate runs, and the rest follows as for a local install
   ([adapter-package.md](adapter-package.md)). The version's `origin` records the URL.

## `check` output

```
harness      installed  active    catalogue newest
claude-code  0.1.0      0.1.0     0.2.0   (update available: nullrouter catalogue install claude-code 0.2.0)
```

`check` never installs anything (US4-4).

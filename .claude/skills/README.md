# 1router project skills

Curated Claude Code skills, project-scoped: they load only for sessions in this
repo. Sources and review provenance:

## Development methodology (13 skills) — from obra/superpowers

Copied from the Palace-pinned local plugin cache v3.6.2
(`/home/ali/.claude/plugins/cache/superpowers-marketplace/superpowers/3.6.2`,
plugin.json: name superpowers, MIT, Jesse Vincent), not re-fetched from
upstream: the Palace has been running this exact pinned copy; upstream HEAD
(2026-09-25, commit 8ca22dba) adds nothing required here.

brainstorming, test-driven-development, systematic-debugging,
root-cause-tracing (+ find-polluter.sh), verification-before-completion,
writing-plans, executing-plans, subagent-driven-development,
requesting-code-review (+ code-reviewer.md template), receiving-code-review,
using-git-worktrees, finishing-a-development-branch, writing-skills
(+ anthropic-best-practices.md, graphviz-conventions.dot,
persuasion-principles.md).

Note: bodies cross-reference each other with a `superpowers:` prefix that does
not resolve here (no plugin installed); treat those as pointing at the sibling
directory of the same name in this repo.

## Anthropic official (2 skills) — from anthropics/skills

Fetched via git blob API at repo HEAD `33375500` (2026-09-24, "Update
claude-api skill: link refusal billing to the docs"), Apache-2.0 (LICENSE.txt
in each dir). Every file verified by exact byte-size match against the git
tree listing at that commit.

- **skill-creator** — authoring, eval-driven iteration, benchmarking, and
  packaging of skills (init.md: hard constraints and out-of-scope will be set
  after first run "via the spec-driven toolkit"; this is the toolkit-building
  skill).
- **webapp-testing** — Playwright testing of local web apps with a
  server-lifecycle helper (init.md first milestone: "it works for me" — the
  9router UI is part of that).

## Considered and excluded (from the same X post's list)

- **claude-api** (anthropics/skills) — its own contract halts on non-Anthropic
  provider markers; wrong inside a multi-provider LLM router.
- **context7** (upstash/context7) — glue for Upstash's hosted docs-MCP
  service; not wired on this LAN. Bodies read for the record.
- **claude-mem** (thedotmack/claude-mem) — a parallel memory engine; would
  fight the Palace agentmemory estate ([[one-router-isolated-identity-2026-09-25]]).
- ui-ux-pro-max, taste-skill, transitions.dev, marketing/social packs,
  finance/small-business/legal plugin bundles — design/office/business
  domains, irrelevant to a Rust LLM router.

## Review status (2026-09-25)

All 15 SKILL.md bodies read end-to-end before install; all 21 support files
decoded from pinned blob SHAs (byte-size verified) and scanned: agents/*.md,
references/, and both HTML viewers' script blocks read in full; Python scripts
scanned for network/exec/secret patterns — only expected localhost serving,
webbrowser open, Google Fonts links, one SRI-hashed CDN script, and `claude -p`
subprocess calls consistent with each script's stated purpose. No exfiltration,
no prompt injection, no obfuscation found.

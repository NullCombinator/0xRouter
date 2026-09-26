# 0router identity — isolation artifacts

The `claude-0router` identity (isolated Claude Code for 0router development)
runs with two enforced layers:

1. **Config isolation** — `CLAUDE_CONFIG_DIR=~/.claude-0router`; inherited
   `ANTHROPIC_*` env stripped by the launcher (env credentials rank above
   every other auth source; an inherited BASE_URL would reroute the identity).
2. **Filesystem confinement** — `0router-gate`, a Landlock allowlist gate
   (kernel-enforced, inherited by all child processes, no root required).
   Everything in `$HOME` outside the 0router runtime is denied, including
   `~/.claude`, `~/.claude-alt`, `~/.ssh`, and `~/.config` except git.

## Files

- `0router-gate.c` — gate source. Rebuild:
  `gcc -O2 -Wall -Wextra -o ~/.claude-0router/bin/0router-gate identity/0router-gate.c`
  Fail-closed on any setup error (exit 126, target never runs).
- `0router-gate` — compiled binary, deployed copy at `~/.claude-0router/bin/`.
- `claude-0router-launcher` — the launcher; deployed at `~/.local/bin/claude-0router`.
  Its `RULES` line is the single place to extend the allowlist (add a rule,
  never loosen to `/`).

## Ruleset (proven 2026-09-26)

- rw: `~/.claude-0router`, `~/Desktop/0router`, `/tmp`, `/dev/null`, `/dev/tty`, `/dev/pts`, `/dev/shm`, `/dev/ptmx` (PTY creation — daemon bg workers die "exit 1 before init" without it)
- ro: system trees (`/usr /bin /sbin /lib /lib64 /etc /opt /proc /dev /sys`),
  `~/.nvm`, `~/.local/share/claude`, `~/.local/lib/palace-mcp`, `~/.local/bin`,
  `~/Desktop` (read-only — project browsing; writes outside 0router denied), `~/.gitconfig`, `~/.config/git`, `~/.local/share/uv` (code-review-graph lives there), `~/docker/agentmemory/0router`
- Everything else denied. Proven: foreign reads rc=2, git OK in repo,
  `claude --version` under gate, fail-closed 126 when gate binary absent.

**Gate ABI notes:**
- `LANDLOCK_ACCESS_FS_TRUNCATE` is ABI 3 (not ABI 2). The gate correctly gates it at `abi >= 3`.
- `LANDLOCK_ACCESS_FS_IOCTL_DEV` is ABI 5; the earlier `LANDLOCK_ACCESS_FS_IOCTL` fallback referenced a never-merged constant and has been removed.
- The launcher previously listed `ro:~/Desktop` twice (duplicate); one has been removed.

Auth (2026-09-26): subscription `/login` — OAuth in
`~/.claude-0router/.credentials.json`, auto-refresh; apiKeyHelper/.token retired.

---
name: implement-dispatch
description: "Runs as a mandatory before_implement hook of /speckit-implement in this repo. Makes the main session an orchestrator: each task goes to the cheapest subagent tier that can do it (Haiku task-implementer by default), with review and escalation. Not for use outside /speckit-implement."
user-invocable: false
---
# Dispatch /speckit-implement tasks by tier

For the rest of this `/speckit-implement` run, the main session (Sonnet 5.5 `medium`) orchestrates and does not write the slice's code itself. Steps 1 to 5 and 8 to 9 of the implement outline still apply. This skill replaces how step 6 executes each task.

**Why:** a main session that writes every task grows its context with each one and hits the usage limit partway through a slice. Opus already did the judgment in plan and tasks, so most tasks are well specified enough for Haiku 5.5. A subagent's context is discarded when it finishes; only its short report comes back.

## 1. Route each task

Take the first rule that matches:

1. **Main session**: marking `tasks.md`, edits to the slice's own spec docs, git.
2. **`js-to-rust-porter`** (Opus): the task ports a named `ref/9router` module or asks for parity with it. Run one at a time, never in parallel.
3. **`rust-engineer`** (Sonnet): the task involves any of the following:
   - a numerical or statistical algorithm (fitting, covariance, confidence bounds, linear algebra beyond a stub);
   - shared state or concurrency (`ArcSwap`, locks, channels, `tokio::select!`, cancellation);
   - the streaming relay or the attempt loop;
   - security-relevant code: access keys, secrets, SSRF, plugin validation, the dashboard's access check;
   - `unsafe`;
   - a signature change whose impact radius (`get_impact_radius_tool`) reaches more than three files.
4. **`task-implementer`** (Haiku): everything else. That covers module skeletons, types and enums with given fields, serde and `FromStr`/`Display`, tests whose assertions the task spells out, wiring with given signatures, CLI and dashboard glue, plugin and style data files, and docs.

Don't split a task that spans several rules; route it by its hardest part.

## 2. Brief the subagent

Give each subagent only what its task needs. Don't paste whole design docs; name the sections.

```
Feature: <FEATURE_DIR>
Task <ID>, verbatim:
<task line from tasks.md>

Files you may touch: <paths from the task; add the mod.rs or lib.rs a new module needs>
Design to read: <plan.md § ..., data-model.md § ..., research.md R..., contracts/...>
Done before you: <IDs of tasks this one builds on, with the items they added>
Rules: no cargo, no git, don't edit tasks.md. Report in your agent's format.
```

`rust-engineer` and `js-to-rust-porter` get the same brief plus: "You cannot run cargo here: read carefully and use LSP diagnostics. CI compiles. Reply in under 250 words: status, files, open doubts."

## 3. Review each result

The orchestrator reviews before marking a task `[X]`:

1. Run `git diff --stat`, then read the diff of the touched files. Don't re-read whole files.
2. Check that:
   - only the allowed files changed;
   - everything the task text lists is there;
   - tests assert what the task says, with nothing weakened or ignored;
   - there are no obvious compile errors (unresolved names, missing `use`, wrong arity, moved values);
   - there is no `todo!()` or `unwrap()` the task didn't ask for.
3. Don't run cargo. CI compiles and tests.

**Escalation:**

- **Haiku `DONE`, review passes:** mark `[X]`.
- **Haiku `DONE`, review fails:** re-dispatch to `task-implementer` once, with the specific defects listed.
- **Second failure, or `BLOCKED`/`NEEDS_ESCALATION` from Haiku:** send to `rust-engineer` with the brief, the current diff and the reasons.
- **`rust-engineer` fails or reports a design gap:** stop the run and report to the user. Don't go up to Opus without asking.

## 4. Parallelism

- `[P]` tasks with disjoint files: up to three `task-implementer` runs at once.
- Sonnet and Opus subagents: one at a time.
- Tasks that touch the same file are always sequential.

## 5. Checkpoints

- At the end of each phase, commit that phase's work on the slice branch (conventional message with the task IDs) and report the phase in under ten lines.
- Pushing still needs the user's yes. Ask once per phase, unless the user has given standing approval for this slice.
- After a push, read the CI result with the `github` MCP tools. Failures in Haiku-written code go to `task-implementer` with the log excerpt, then follow the escalation rules above.

## 6. Completion report

Add a tier table to the completion report:

| Task | Routed to | Finished by | Retries |
|---|---|---|---|

Then save one line to agentmemory (main instance) in this form:

> slice NNN: H tasks to Haiku, E escalated, S to Sonnet, O to Opus; first-pass CI failures in Haiku code: F

The escalation rate decides whether the routing rules in step 1 need tightening.

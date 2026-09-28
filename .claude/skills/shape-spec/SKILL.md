---
name: shape-spec
description: "Use before /speckit-specify while 0router's core is still being built. Claude reads the project's current status, suggests the next slice, then tests that suggestion with the user claim by claim until it matches their intent, and outputs a /speckit-specify command built only from confirmed claims."
argument-hint: "Optional: a steer in your own words (e.g. 'request execution', 'I want retry in'). Empty = Claude suggests from the current status."
user-invocable: true
disable-model-invocation: false
---

# Shape the next slice before /speckit-specify

## Why this exists

0router is not greenfield: 9router already exists, and the core is being built slice by
slice. Until the core is complete, the user relies on Claude to suggest what comes next
and how to scope it. Claude knows the project's status and the reference. The user
knows what they want the product to be.

That reliance only works if Claude's suggestion is tested before it becomes a spec. In
slice 003, Claude's suggested text said "There is no retry and no fallback" and made the
harness↔vendor "native pair" a core feature. The user pasted it, trusting it, and the
spec drifted from what they wanted. `/speckit-clarify` then asked detailed questions
inside that framing and made the drift harder to see.

This skill keeps Claude as the one who suggests, and makes every part of the suggestion
earn its place. **Claude proposes; nothing reaches the final command until the user has
confirmed it.**

**Lifespan.** This skill is for the core-construction phase. Once the status snapshot
shows every core capability as shipped, say so, and tell the user they can move to
specifying freely with `/speckit-specify` against the shipped product.

Built on the `brainstorming` skill (one question at a time, multiple choice, a
recommendation first, YAGNI). This skill replaces brainstorming's ending: no design doc,
no worktree, no plan.

## Rules

1. **Suggest from evidence.** Every suggestion rests on the status snapshot (step 0):
   what is shipped, what is missing, and what depends on what. Never suggest from
   9router's structure alone.
2. **Your suggestion starts as unconfirmed.** Every claim you put forward enters the
   ledger as `C?` and stays out of the final command until the user confirms it.
3. **Scope limits get the most scrutiny.** Every "out of scope", "no X", "placeholder",
   "only Y" or "deferred" clause is confirmed on its own, with its user-visible
   consequence ("With retry out, a single 429 from the provider reaches the client as an
   error. OK for this slice?"). Never confirm limits as a block.
4. **Ask from the user's side.** The options are what the operator or client sees, waits
   for, or has kept about them. How 9router maps to 0router is your job: mention 9router
   only as a short reason for your recommendation, never as the choice itself.
5. **Core, plugin, or bonus.** For each capability, settle whether it is core behaviour,
   plugin-declared data that the core interprets, or a bonus for later. Do not assume
   9router's placement.
6. **Recorded directions win.** If a claim contradicts the user's recorded directions
   (agentmemory, init.md, the constitution), point it out and ask. Do not resolve it
   silently.
7. **One question per turn**, with `AskUserQuestion`, recommended option first. Group up
   to 4 questions only when they are independent yes/no confirmations of ledger rows.

## Claim ledger

A claim is one atomic statement the spec will rely on: a capability in, a capability
out, a constraint, a user-visible behaviour, a success signal.

| Tag | Source | Allowed in the final command? |
|---|---|---|
| `U` | The user said it (keep a short verbatim quote) | yes |
| `C✓` | Claude suggested it, and the user confirmed it explicitly | yes |
| `C?` | Claude suggested it, not yet confirmed | **no**. Confirm it or drop it |
| `M` | A recorded user direction in agentmemory (cite the date) | yes, but re-confirm when it limits scope |
| `K` | init.md or the constitution | yes |
| `P` | A parity decision the user will not notice | no. It goes in the brief as a note for research.md |

Silence is not confirmation, and neither is "sounds good" to a long block. Confirmation
means the user accepted that specific row.

## Process

### 0. Status snapshot (before speaking)

Build the evidence for your suggestion:

- **Core map.** List the core capabilities from init.md ("What it is", "Why it
  exists") and the constitution's principles: unified models, cache-aware routing,
  per-agent isolation, windowed amortization, request execution, retry and fallback,
  latency observability, model tests, combos, non-text model types, plugin-declared
  providers, and the rest. Mark each **shipped / partial / absent**, with evidence:
  a spec dir, a crate or module, tests, a commit. Use `code-review-graph-0router` and
  the git log. Do not rely on memory alone.
- **Promises.** The deferred items in each earlier spec's `Out of scope` and in the
  earlier briefs in `specs/briefs/`, plus the agentmemory slot `pending_items`. Name
  the slice each one was deferred to.
- **Drift and dead ends.** Any `SUPERSEDED.md`, any uncommitted or abandoned spec dir,
  and why it went wrong (`memory_smart_search` "User direction").
- **Directions.** agentmemory slots `project_context` and `guidance`, plus the recorded
  user directions. These become `M` rows; the relevant init.md and constitution parts
  become `K` rows.
- **Dependencies.** Which absent capability blocks which. What is the smallest next
  slice that you can show working end to end, and that unblocks the most?
- **Reference.** How 9router handles the candidate area, using `code-review-graph`.
  This informs your recommendation; do not lecture the user on it.

### 1. Suggest

Show the core map compactly (a table: capability, status, evidence, one line each).
Then:

- **Recommend** the next slice: its user-visible result, why it comes now (what it
  unblocks, what it builds on), and what it leaves for later.
- **Offer** 1–2 alternatives, one line each, with their trade-off.
- If the user gave a steer as an argument, record it as `U` rows and shape the
  recommendation around it. If the steer conflicts with the dependency order, say so
  and let the user decide.

Once the user picks a slice, write your full suggestion for it as ledger rows, all
`C?`, and move to step 2.

### 2. Hammer

Work through the `C?` rows with these probes, in order. Skip a probe only if the
ledger already settles it.

1. **Outcome**: "When this slice is done, you can ___. Is that the result you want?"
   Then ask what would make the user say the slice failed.
2. **Boundary**: every capability the slice touches, sorted into core /
   plugin-declared / bonus / later (rule 5). Include the ones you would leave out.
3. **Each scope limit**: one confirmation per limit, with its consequence (rule 3).
   Record which later slice takes each deferred item.
4. **Contradictions**: every place a row disagrees with an `M` or `K` row, or with an
   earlier spec.
5. **Premortem**: list the 3 most likely ways your suggestion could still be wrong
   ("I'm assuming X because Y. If that's wrong, Z happens."). Ask about each.
6. **Size**: if the confirmed scope cannot be shown working end to end by one small set
   of user stories, propose a split and let the user choose.

After any answer that changes the picture, update the ledger (and your suggestion)
before the next question.

### 3. Play back

Say the slice back in plain language, at most 150 words, with no spec vocabulary.
Then show the ledger as a table (tag, claim, source/quote). Ask: "Does this match what
you want?" If no, go back to step 2 for the rows that are wrong.

### 4. Compose

Write the `/speckit-specify` text **from confirmed rows only**:

- the slice name and purpose;
- the capabilities in scope, as user-visible behaviour (WHAT and WHY, not HOW);
- constraints (`K`, `M`, and user-stated ones);
- `Out of scope:` confirmed limits only, each with where it goes ("→ slice NNN" or
  "→ later");
- a last line, `Scope brief: specs/briefs/<file>.md`, so clarify, plan and analyze can
  check themselves against it.

Parity oracle files may be named in one `Oracle:` sentence. They are test sources and
do not add scope.

### 5. Trace check

Under the command, map each sentence of the command to the ledger rows behind it.
Remove any sentence that has no `U`/`C✓`/`M`/`K` row, or take it back to step 2. Then
ask the user to approve the final text.

### 6. Save and hand off

- Write `specs/briefs/YYYY-MM-DD-<short-name>.md` containing: the core map, the
  playback, the ledger, the `P` notes for research.md, the final command, and the
  trace. (`specs/briefs/` has no number prefix, so speckit ignores it.) The next run
  starts from this core map.
- `memory_save` each new or changed user direction to **both** agentmemory instances.
  Subagents in plan and implement read only the team instance.
- Print the final command in a fenced block for the user to copy. **Do not run
  `/speckit-specify` yourself** unless the user asks.
- Remind the user once: in `/speckit-clarify`, an answer that would contradict a
  confirmed ledger row is a signal to stop and revisit the brief, not to accept it.

## Red flags: stop and go back

- You are about to write "the user wants…" about a `C?` row.
- The command has a sentence you cannot trace to a confirmed row.
- An "out of scope" line was confirmed only as part of a block.
- The recommendation cites 9router's structure but not the core map.
- A question's options are 9router layers or functions, not outcomes.
- You are drafting the command before the playback was confirmed.

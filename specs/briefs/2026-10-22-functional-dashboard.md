# Scope brief: functional dashboard (the last slice before the first release)

Shaped with `/shape-spec` on 2026-10-22, in an Opus 5.5 `high` session. It supersedes the "look only"
end state of 009/010 (009 FR-013/FR-014 were right for those slices). The user's direction of
2026-10-19 put this slice on the path to the first release.

In `/speckit-clarify`, `/speckit-plan` and `/speckit-analyze`, an answer that contradicts a
confirmed row below means stop and revisit this brief. Don't accept it.

## Core map (at `main` `b58ba80`, 2026-10-22)

| Capability | Status | Evidence |
|---|---|---|
| Provider plugins, unified models, four client API styles, non-text types | shipped | 002, 003 |
| Request execution, retry, fallback | shipped | 003 |
| Account sign-in, quota polling | shipped | 005 |
| Cache-aware routing, per-agent isolation, windowed amortization | shipped | 006 |
| Shared read model | shipped | 008 |
| Dashboard, look only (pages, sidebar, style guide) | shipped; mockup parity on `look/mockup-parity` | 009 |
| Usage and latency summaries, landscape, harness tags | shipped | 010 |
| Model tests, combos | shipped | 011 |
| Quota fit | shipped | 012 |
| Latency phases, live view, connection settings, proxies | shipped | 013 |
| Client-side adapters (hermes, sandboxed third-party, Claude Code) | shipped | 004 |
| Changes from the browser | **absent**: every page control is disabled with a CLI hint | → this slice |
| Whole-system live run with real accounts | open (user) | 004 T031/T084, 012 T072, 009 T072/T075, 010 T052, 011 T054/T056, 013 T066/T070 |
| Housekeeping chat | absent | → later |
| Latency trends, latency per unified model | absent | → latency slice 2 |

## Playback (confirmed 2026-10-22)

After this slice, you run 0router entirely from the browser. Anything you can change in the CLI,
you can change on its page: add, sign in and remove accounts; issue and revoke client keys and set
their harness tag; set account order, behaviour, meter overrides, proxies and connection settings;
install, enable, hide or uninstall provider plugins, and create your own OpenAI- or
Anthropic-compatible provider; install, approve and clear harness adapters; build unified models
and combos; run model tests, combo tests and Test All. Each change runs exactly what the CLI command
does: same checks, same error text, applied at once. Anything that can't be undone first opens a
window naming what you'd lose. Console Log lists every change you made in the browser. Pages stay
server-rendered Rust with no JavaScript, on this machine only, behind the same token. API keys are
typed once and never shown back. Long tasks say "started, reload to see". The chat box stays off.

## Ledger

| # | Tag | Claim | Source |
|---|---|---|---|
| 1 | U | Browser changes in all four areas: accounts and keys (add, OAuth sign-in, remove; issue, revoke, harness tag), routing settings (order/priority, behaviour, meter overrides, proxies, connection), plugins and adapters (install, enable, hide, uninstall plugins; install, approve review, clear adapters), combos and tests (unified models, combos, model tests, combo tests, Test All) | user picked all four, 2026-10-22 |
| 2 | U | One slice, not split | "One big slice" |
| 3 | U | All Rust, server-rendered, no JavaScript, strict CSP; changes are HTML forms, each change reloads the page | "I'd rathered all program in RUST including the UI"; confirmed "Server-rendered Rust, no JS" |
| 4 | C✓ | One dashboard token for reading and writing; every form CSRF-guarded | "One token, forms CSRF-guarded" |
| 5 | C✓ | An API key is pasted once in a write-only field, never shown back; OAuth accounts sign in through the provider's page in the browser | "Yes, paste once, never shown back" |
| 6 | C✓ | Long tasks (tests, adapter builds) show "started, reload to see"; a meta refresh may reload for you | "Yes, reload to see" |
| 7 | C✓ | The housekeeping chat box stays disabled ("not built yet") → later | "Still out, later" |
| 8 | C✓ | A browser change runs exactly what the CLI command does: same checks, same error text shown on the page, applied at once | "Yes, same as the CLI" |
| 9 | C✓ | Irreversible actions (remove account, revoke key, uninstall plugin) open a confirm window naming what is lost | "A confirm window" |
| 10 | C✓ | Console Log records every change made in the browser (what, when; never a secret), with a CLI twin | "Yes, on Console Log" |
| 11 | C✓ | "Add … Compatible" creates a custom OpenAI- or Anthropic-compatible provider as a checked plugin file, appearing at once, with a CLI twin | "Yes, in" |
| 12 | M | Every fact a page shows has a CLI twin; this machine only; 9router's look and page names; light theme | 2026-10-05 brief rows 3, 5, 6, 9 |
| 13 | K | No secret or prompt is shown; plugins see nothing new; a dashboard fault never slows or breaks client requests | constitution I; 009 FR-012 |
| 14 | M | The first release comes only after this slice and the user's whole-system live run | user direction 2026-10-19 |

## P notes for research.md

- The CLI's change commands are the single path: the dashboard should call the same functions the
  CLI and operator socket call (as the read side does through `nullrouter_server::views`), so rows 8
  and 13 hold by construction.
- 9router's dashboard makes these changes with client-side React and API routes; 0router keeps
  server-rendered forms (row 3). POST-redirect-GET avoids resubmission on reload.
- The CSP keeps `script-src` absent; `form-action 'self'` should be added.
- OAuth sign-in in the browser reuses 005's flows; the callback lands on the dashboard's port or the
  existing loopback callback.
- Cookie scope Low L1 from 007 (host-, not port-scoped cookies) matters more once the token writes:
  research should revisit it with the CSRF design.

## Final command

```
/speckit-specify Functional dashboard: the operator runs 0router entirely from the browser. Every change the CLI can make, the dashboard's pages can make too, in four areas: accounts and keys (add an account by pasting its API key once, sign an OAuth account in through the provider's page, remove an account; issue and revoke client keys and set their harness tag); routing settings (account order and priority, behaviour, meter overrides, proxies, connection settings); plugins and adapters (install, enable, hide and uninstall provider plugins; create a custom OpenAI- or Anthropic-compatible provider from the Providers page's "Add … Compatible" button; install harness adapters, approve their review, and clear a suspect adapter); combos and tests (declare unified models and combos; run model tests, combo tests and Test All and see each verdict). A change made in the browser runs exactly what the matching CLI command does: the same checks, the same error text shown on the page, applied at once; creating a custom provider gets a CLI twin. Anything that can't be undone (removing an account, revoking a key, uninstalling a plugin) first opens a confirm window naming what will be lost. Console Log lists every change made in the browser (what and when, never a secret), with a CLI twin. Long tasks such as tests and adapter builds say they started and show their result on reload. Constraints: the pages stay server-rendered Rust with no JavaScript, and each change is a form that reloads the page; one dashboard token covers reading and writing, and every form is protected against cross-site posting; the dashboard serves this machine only; an API key is never shown back once saved; no page shows a secret or prompt; every fact a page shows keeps a CLI twin; 9router's look and page names stay; a dashboard fault never slows or breaks client requests. Out of scope: the housekeeping chat box stays disabled (→ later). Scope brief: specs/briefs/2026-10-22-functional-dashboard.md
```

## Trace

| Command sentence | Rows |
|---|---|
| Runs 0router entirely from the browser; every CLI change, four areas listed | 1, 2 |
| Paste API key once; OAuth through the provider's page | 5 |
| Custom compatible provider from "Add … Compatible" | 11 |
| Same checks, same error text, applied at once; custom provider CLI twin | 8, 11 |
| Confirm window for irreversible actions | 9 |
| Console Log with CLI twin, never a secret | 10, 13 |
| Long tasks started / result on reload | 6 |
| Server-rendered Rust, no JS, form reloads page | 3 |
| One token, cross-site posting protection | 4 |
| This machine only; no secret/prompt; CLI twin; 9router look; never slows requests | 12, 13 |
| Out of scope: chat box | 7 |

# Specification Quality Checklist: Provider Entity & Unified Model Registry

**Purpose**: Validate specification completeness and quality before proceeding to planning
**Created**: 2026-09-26
**Feature**: [spec.md](../spec.md)

## Content Quality

- [x] No implementation details (languages, frameworks, APIs)
- [x] Focused on user value and business needs
- [x] Written for non-technical stakeholders
- [x] All mandatory sections completed

## Requirement Completeness

- [x] No [NEEDS CLARIFICATION] markers remain
- [x] Requirements are testable and unambiguous
- [x] Success criteria are measurable
- [x] Success criteria are technology-agnostic (no implementation details)
- [x] All acceptance scenarios are defined
- [x] Edge cases are identified
- [x] Scope is clearly bounded
- [x] Dependencies and assumptions identified

## Feature Readiness

- [x] All functional requirements have clear acceptance criteria
- [x] User scenarios cover primary flows
- [x] Feature meets measurable outcomes defined in Success Criteria
- [x] No implementation details leak into specification

## Notes

- Q1 resolved 2026-09-26: clients may address a provider's model directly as
  `<provider>/<model>` (FR-017).
- "No implementation details": TOML and the baseline file names are kept deliberately.
  TOML is fixed by Constitution I, and the baseline files are the parity oracle required
  by Constitution VI. No crate, language, or library choices appear.
- "Non-technical stakeholders": the audience for this slice is the operator and plugin
  author; the terms used (provider, alias, model ID, upstream ID) are the ones they see.
- Source-verified against ref/9router on 2026-09-26: 121 registry entries / 83 with
  transport; categories apikey 78, oauth 19, freeTier 18, free 4, webCookie 2; 14 registry
  files compute values from imports; client secrets in 4 active providers (antigravity,
  gemini-cli, gemini, iflow). trae and devin-cli are disabled in 9router and are not
  bundled (found during /speckit-plan). `providers-baseline.json` embeds 4 secrets.
- Corrections to 001's SUPERSEDED.md found during verification: registry files do not
  contain functions (they import computed constants), and 38 entries — not 29 — have
  no transport.

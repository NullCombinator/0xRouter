# 0router (null router)

Intention, written 2026-09-25 before any code. This file is the seed of the project: what it is, why it exists, and where its boundaries will come from later.

## What it is

A Rust implementation of the 9router project core.

## Why it exists

To make 9router faster, customizable, and to enrich it:

- **Latency observability** — latency becomes something measured and visible, not something noticed.
- **Claude form of connection**
- **Nested combos** — handled more efficiently.
- **Robust model tests in providers** — provider-level model tests become trustworthy.
- **Testable combos** — combos gain test capability of their own, not just the providers inside them.
- **Community-driven provider plugins** — providers become plugins, so users customize the default providers they want to use.
- **Beyond text generation** — a model manager, not a text-generation manager: first-class image, audio, embedding, decision, and other model types.

## Hard constraints

Not set yet. These will be defined after the first run, with a spec-driven toolkit helper.

## Out of scope

Not set yet. Same: set after the first run, via the spec-driven toolkit.

## First milestone

Fully implemented, and it works for me, at least.

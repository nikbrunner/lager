# Domain docs

This repo uses a single-context layout:

- `CONTEXT.md` at the repo root holds domain vocabulary.
- `docs/adr/` holds architectural decisions.

## Before exploring

Read `CONTEXT.md` and ADRs relevant to the area being explored.

If either is absent, proceed silently. The `domain-modeling` skill creates
these documents when terms or decisions are resolved.

## Vocabulary

Use the terms defined in `CONTEXT.md` when naming domain concepts in
issues, proposals, hypotheses, and tests.

If a concept is missing, reconsider the terminology or note the gap for
`domain-modeling`.

## Decision conflicts

Explicitly flag proposals that contradict an existing ADR.
Identify the ADR and explain why the decision merits reconsideration.

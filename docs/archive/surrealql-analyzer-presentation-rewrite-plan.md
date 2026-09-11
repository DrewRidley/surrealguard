# SurrealQL Analyzer deck rewrite plan

## Diagnosis

The revised talk is clearer than the original, but it still feels like a project-status deck. It explains the analyzer, adapters, CI, LSPs, and roadmap, but the emotional center is not sharp enough.

The strongest material is:

- the split brain
- concrete ways SurrealDB projects break
- diagnostics appearing directly where the broken query lives
- typed result shapes

The weaker material is:

- repeated architecture framing
- too many internal implementation stages
- roadmap language arriving before the audience fully cares
- surfaces/adapters/LSP content feeling like project plumbing instead of user payoff

## Proposed new thesis

SurrealDB gives you a powerful database language, but app tooling treats that language like an opaque string. SurrealQL Analyzer makes SurrealQL visible to the tools developers already rely on: CI, editors, type systems, and review workflows.

Short version:

Make SurrealQL visible before runtime.

## Proposed rewrite structure

1. Title
   - SurrealQL Analyzer
   - Make SurrealQL visible before runtime.

2. The actual user problem
   - Your database knows the schema.
   - Your app compiler does not.
   - The seam is every query string, migration, event, function, and graph traversal.

3. A tiny concrete failure
   - Rename `email` to `primary_email`.
   - App still ships `SELECT email FROM person`.
   - The compiler is happy. Runtime is not.

4. The split brain visual
   - App code on one side.
   - SurrealDB schema on the other.
   - Query boundary in the middle.

5. More failure modes
   - renamed field
   - wrong param type
   - missing table
   - graph edge drift
   - result shape drift
   - hidden database code drift

6. What SurrealQL Analyzer is
   - A static analyzer for SurrealQL inside real projects.
   - It reads schema, migrations, database code, and app queries.
   - It reports breakage before deploy.

7. The bridge
   - Schema contract + query contract -> analyzer -> diagnostics, params, result shapes, spans.

8. What a user gets first
   - `surrealql-analyzer check`
   - local / pre-commit / CI failure with source spans.

9. Diagnostics in code
   - unknown field
   - wrong param type
   - missing edge table
   - include the exact lines and hints.

10. Typed outputs
   - result shape inference
   - param requirements
   - generated or projected host-language types.

11. Why it is hard
   - SurrealQL is not just SELECT.
   - It includes schema, events, functions, permissions, relations, graph traversal, and flexible result shapes.
   - This explains why a real core analyzer is needed.

12. How it works, compact
   - parse with spans
   - build catalog
   - analyze semantics
   - emit diagnostics/types/params/spans

13. Where it shows up
   - CLI now
   - Rust/TS/Python adapters next
   - host LSPs after that
   - SurrealQL LSP/editor integrations later

14. Roadmap
   - Phase 1: core analysis
   - Phase 2: adapter-ready outputs
   - Phase 3: editor-facing tooling

15. Close
   - SurrealDB keeps the language.
   - SurrealQL Analyzer gives the language to the rest of the toolchain.

## Rewrite principle

The new deck should be user-payoff first, implementation second.

A random SurrealDB user should understand the talk even if they know nothing about SurrealQL Analyzer internals. Architecture only earns a slide after the failure and the user-visible output are obvious.

## Concrete changes from current v2

- Move `What it is` later, after at least one concrete failure.
- Add a simple single-failure slide before the split-brain diagram.
- Merge or shorten `Pipeline`, `Surfaces`, and `CI guardrails`; they currently overlap.
- Replace “Analysis phases” as a standalone explanation with a compact “why this needs a real analyzer” slide.
- Make CLI/CI the first concrete output, before LSP/editor claims.
- Keep future editor support, but make it explicitly downstream of adapters.
- Reduce the visual style slightly: less neon SaaS, more code/editor clarity.

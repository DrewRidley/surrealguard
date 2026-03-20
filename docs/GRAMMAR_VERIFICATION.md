# Grammar Verification Plan

## Current State
- 111 corpus tests, 2 failing (DEFINE FIELD block clauses, Closure With Cast And Chain)
- Grammar has been modified organically — block-as-value, cast with {}, destructuring
- No systematic verification against SurrealDB's actual parser

## Verification Strategy

### Phase 1: Fix Existing Failures
Fix the 2 failing corpus tests caused by our grammar changes.

### Phase 2: Corpus Expansion from SurrealDB Source
SurrealDB's own test suite contains thousands of valid SurrealQL queries.
Extract them and verify our grammar accepts them without ERROR nodes.

Sources:
- `surrealdb/core/src/syn/` — parser tests
- `surrealdb/sdk/tests/` — integration tests
- SurrealDB documentation examples

### Phase 3: Differential Testing
Build a harness that:
1. Takes a .surql input
2. Parses with tree-sitter → check for ERROR nodes
3. Parses with SurrealDB's parser → check for success/failure
4. Compare: if SurrealDB accepts it, tree-sitter should too

### Phase 4: Fuzzing
Use grammar-aware fuzzing to generate random valid SurrealQL and verify:
- No tree-sitter crashes
- No unexpected ERROR nodes on valid input
- Consistent parse tree shapes

## Type Inference Specification

### Principle
Every SurrealQL expression has a deterministic type that can be inferred
from the schema context. The analyzer should compute this type and use it
for diagnostics.

### Expression Type Rules (to be formalized)
See TYPE_INFERENCE_SPEC.md

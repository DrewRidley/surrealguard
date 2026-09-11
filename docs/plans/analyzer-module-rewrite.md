# Analyzer module rewrite

> **Superseded (2026-07-03).** The analyzer-tree layout and per-construct
> ownership described here still stand, but the input contract does not:
> analyzers no longer target raw CST nodes. See
> `2026-07-03-typed-ast-lowering.md` — analyzers consume a typed, lowered
> AST, and the "target analyzer signature shape" section below is obsolete.

## Direction

Move SurrealQL Analyzer from one broad semantic walker toward a statement/function-family analyzer tree.

The existing implementation stays live while slices migrate into `crates/workspace/src/analyzer/`.

## Principles

- One file per discrete SurrealQL construct where practical.
- Statement analyzers assume the caller already classified the CST node as that statement kind.
- Function analyzers are grouped by SurrealQL function namespace, similar to Surrealix, but must use upstream `surrealdb_types::Kind` and SurrealQL Analyzer response-shape facts instead of a custom type hierarchy.
- Diagnostics are appended through shared analyzer context, not invented per module.
- Unknown/dynamic/unsupported constructs produce explicit partial facts instead of fake certainty.
- Host adapters remain downstream of the plain `.surql` core.

## Skeleton

```text
analyzer/
  context.rs          shared catalog/env/diagnostic/result contracts
  statement.rs        top-level dispatch only
  data/               SELECT/CREATE/UPDATE/UPSERT/DELETE/INSERT/RELATE
  schema/             DEFINE/REMOVE/ALTER/INFO/USE and catalog effects
  flow/               BLOCK/IF/FOR/LET/RETURN/THROW source-ordered env
  expression/         literals, paths, binary ops, objects, arrays, calls, subqueries
  function/           built-in function namespaces
```

## Event / block placement

- `DEFINE EVENT` belongs under `schema::define` because it updates catalog/database-code facts.
- Event body / `THEN` statement analysis should reuse `flow::block` / `statement` once extracted.
- Blocks, IF, FOR, LET, RETURN belong under `flow` because their main job is source-ordered environment and return-value analysis.

## Target analyzer signature shape

Callers hand the top-level analyzer a tree-sitter statement node. The parser already bounds that node to the statement, usually up to the semicolon. The analyzer reads the first statement keyword from the node's source text and dispatches internally. Callers should not pass a separate statement-kind enum or pre-classified kind.

Each construct-specific module should converge on this shape:

```rust
fn analyze_statement(ctx: &mut AnalysisContext<'_>, node: Node<'_>) -> AnalyzedKind;
fn analyze_select(ctx: &mut AnalysisContext<'_>, node: Node<'_>) -> AnalyzedKind;
fn analyze_function_call(ctx: &mut AnalysisContext<'_>, node: Node<'_>, args: &[AnalyzedKind]) -> AnalyzedKind;
```

`AnalysisContext` owns schema/catalog facts, locals, params, row context, source/span helpers, and diagnostics. The return value should stay small: what the construct evaluates to, usually an upstream `surrealdb_types::Kind`, `Unknown`, or `None`.

## Migration order

1. Introduce context/fact contracts without changing behavior.
2. Move built-in function signatures into `analyzer::function`.
3. Move expression fact inference into `analyzer::expression`.
4. Move `SELECT` response-shape and field diagnostics into `analyzer::data::select`.
5. Move mutation assignability/return shapes into `analyzer::data::{create,update,upsert,delete,insert,relate}`.
6. Move schema catalog extraction into `analyzer::schema`.
7. Replace `semantic.rs` orchestration with `analyzer::statement` dispatch.

## Non-goals for the skeleton commit

- No behavioral migration yet.
- No new custom type hierarchy.
- No host adapters.
- No broad rewrite of existing tests.

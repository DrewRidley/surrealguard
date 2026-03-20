# Analyzer Rewrite Plan

## Principle
Each statement module is self-contained. No shared "resolve" god function.
Shared code is limited to: Context, Diagnostic, type utilities, parser helpers.

## Module Structure

```
crates/analyzer/src/
  lib.rs                — analyze(), analyze_with_context()
  context.rs            — Context, TableDef, FieldDef, FunctionDef, Scope
  diagnostic.rs         — Diagnostic, Code, Severity
  types.rs              — Kind utilities, is_assignable, is_valid_cast, display
  span.rs               — Span
  parser.rs             — tree-sitter helpers (child_by_kind, find_all, node_text)
  schema.rs             — extract_schema() — populates Context from DEFINE statements
  hints.rs              — inlay hint collection

  statements/
    mod.rs              — analyze_all() dispatcher
    select.rs           — self-contained SELECT analysis
    create.rs           — self-contained CREATE analysis
    update.rs           — self-contained UPDATE analysis
    delete.rs           — self-contained DELETE analysis
    insert.rs           — self-contained INSERT analysis
    upsert.rs           — self-contained UPSERT analysis
    relate.rs           — self-contained RELATE analysis
    define.rs           — DEFINE TABLE/FIELD/FUNCTION analysis
    remove.rs           — REMOVE analysis
    let_stmt.rs         — LET binding analysis
    for_stmt.rs         — FOR loop analysis
    if_stmt.rs          — IF/ELSE analysis
    control.rs          — BREAK/CONTINUE/THROW
    info.rs             — INFO FOR analysis
    alter.rs            — ALTER TABLE analysis

  functions/
    mod.rs              — resolve_builtin() dispatcher
    string.rs           — string:: namespace
    array.rs            — array:: namespace
    math.rs             — math:: namespace
    crypto.rs           — crypto:: namespace
    ...                 — one per namespace
```

## What each statement module contains

Each statement module (e.g., select.rs) has:

```rust
/// Analyze a SELECT statement. Returns the result type.
pub fn analyze(node: &Node, source: &str, ctx: &mut Context) -> Kind {
    // 1. Extract the FROM table
    // 2. Process the select clause (fields, *, VALUE, aliases)
    // 3. Process WHERE clause (validate condition)
    // 4. Process ORDER BY, GROUP BY, LIMIT, etc.
    // 5. Compute and return the result type
}

// Private helpers — only used by SELECT
fn resolve_select_fields(...) -> BTreeMap<String, Kind> { ... }
fn resolve_where_condition(...) { ... }
fn resolve_from_table(...) -> Option<String> { ... }
fn validate_omit_fields(...) { ... }
// etc.

#[cfg(test)]
mod tests {
    // Comprehensive tests for every SELECT feature
}
```

## What's shared vs statement-specific

### Shared (in lib, context, types, parser):
- Context: get_table, get_field, build_table_type, scope operations
- Type checks: is_assignable, is_valid_cast, is_numeric, display_kind
- Parser: child_by_kind, find_all, node_text, Span::from_node
- Diagnostic emission: ctx.emit(Diagnostic::error/warning/hint)

### Statement-specific (NOT shared):
- Expression resolution for that statement's context
- Field validation for that statement's target table
- Result type computation
- Clause processing (WHERE, SET, CONTENT, RETURN, etc.)

### Common patterns extracted as small utilities:
- resolve_literal(node, source) → Kind — for string/int/bool/null literals
- resolve_function_call(node, source, ctx) → Kind — dispatches to functions/
- resolve_binary_op(left, op, right) → Kind — type checking for operators

These are thin utilities, not a 2500-line resolve_expr. Each is <50 lines.

## Build Order

1. Foundation: context.rs, types.rs, parser.rs, diagnostic.rs, span.rs
2. Schema: schema.rs (extract DEFINE statements)
3. SELECT: statements/select.rs with 20+ tests
4. CREATE: statements/create.rs with 15+ tests
5. UPDATE: statements/update.rs with 15+ tests
6. DELETE, INSERT, UPSERT, RELATE
7. LET, FOR, IF, control flow
8. DEFINE (validation), REMOVE, ALTER, INFO
9. Function modules (string, array, math, etc.)
10. Hints module

## Test Pattern

Every test follows:
```rust
#[test]
fn select_specific_fields() {
    let mut ctx = test_schema();
    let typ = analyze_query(&mut ctx, "SELECT name, age FROM user");
    assert_type!(typ, array<{ name: string, age: int }>);
}
```

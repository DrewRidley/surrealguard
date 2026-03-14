# SurrealGuard LSP Integration — Design Document

## Vision

Deliver rust-analyzer-level DX for SurrealQL. Every hover, diagnostic, and hint should
feel precise, contextual, and helpful. No noise, no confusion, no broken squiggles.

## Architecture

### Single Source of Truth

SurrealGuard is the SOLE provider of type-checking diagnostics.
The LSP's built-in semantic diagnostics are disabled when surrealguard is active.

Diagnostic pipeline:
```
Source text
  → tree-sitter parse → syntax errors (parse failures only)
  → surrealguard analyze → type errors, schema violations, scope issues
  → publish to editor
```

### AST-Aware Hover (rust-analyzer pattern)

Current (broken): `cursor position → token string → name lookup → hover`
Target: `cursor position → tree-sitter node → AST context → resolve → hover`

```rust
// 1. Find the AST node at cursor position
let node = tree.root_node().descendant_for_point_range(ts_point, ts_point);

// 2. Walk up to understand context
match node.kind() {
    "identifier" => {
        let parent = node.parent();
        match parent.kind() {
            "from_clause" => show_table_hover(node_text),
            "field_assignment" => show_field_hover(node_text, table_from_statement),
            "graph_predicate" => show_relation_hover(node_text),
            "where_clause" => show_field_hover(node_text, table_from_statement),
            _ => show_generic_symbol_hover(node_text),
        }
    }
    "variable_name" => show_variable_hover(node_text, scope),
    "custom_function_name" => show_function_hover(node_text),
    "builtin_function_name" => show_builtin_hover(node_text),
    "wildcard" => show_wildcard_expansion(table_from_statement),
    _ => None,
}
```

### Diagnostic Span Precision

**Rule**: Underline EXACTLY what's wrong, never the whole statement.

```
✗ Bad:  UPDATE user SET age = 'twenty';
        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^  ← entire statement

✓ Good: UPDATE user SET age = 'twenty';
                              ^^^^^^^^  ← just the wrong value
```

Implementation pattern — find the specific child node:
```rust
// For type mismatch on assignment: span the VALUE, not the assignment
let value_node = &children[children.len() - 1];
let span = Span::from_node(value_node);

// For missing required field: span the TABLE NAME, not the statement
let target_span = find_all(node, "create_target")
    .first()
    .map(|n| Span::from_node(n));

// For wrong arg type: span the SPECIFIC ARGUMENT
let arg_node = &arg_nodes[arg_index];
let span = Span::from_node(arg_node);
```

### Cross-File Related Info

When the analyzer builds context from multiple files, definitions carry spans
from their source files. Related info ("defined here") must either:
1. Include the correct file URI + byte offset → clickable link to definition
2. Be omitted if we can't resolve the source file

To implement: add `source_file: Option<String>` to the analyzer's Context definitions,
populated during cross-file schema loading.

## Hover Specs

### Table
```surql
TABLE user SCHEMAFULL
  name: string
  age: int
  email: string
```
Comment text if present. Nothing else.

### Field (in query context)
```surql
FIELD user.age TYPE int
```
Shows which table the field belongs to. Readonly/computed indicators if applicable.

### Built-in Function
```surql
fn string::len(string) -> number
```
Returns the length of a string in characters.

[SurrealDB docs](https://surrealdb.com/docs/surrealql/functions/database/string#stringlen)

### Custom Function
```surql
fn fn::greet($name: string) -> string
```
Comment text if present.

### Variable
```surql
$adults: array<{ name: string, age: int }>
```
Shows inferred type from the binding expression.

### Graph Edge (`->wrote->`)
Hovering `wrote` shows:
```surql
TABLE wrote TYPE RELATION FROM user TO post
  created_at: datetime
```

### Wildcard (`*` in SELECT)
Hovering `*` in `SELECT * FROM user` shows:
```
Expands to: name, age, email, profile, tags
```

### Keywords
No hover. rust-analyzer doesn't show hovers for `fn`, `let`, `struct` keywords.

## Inlay Hints

### LET bindings
```surql
LET $adults: array<{ name: string, age: int }> = SELECT * FROM user;
LET $count: int = 42;
LET $greeting: string = "hello world";
```

Only show for non-trivial inferred types. Skip:
- `Kind::Any` (unknown)
- Obvious literals where the type is already visible

### Statement result types (optional, lower priority)
After SELECT/CREATE keywords, show the return type if known.

## Error Message Style Guide

Follow rustc conventions:
- Use backticks around types and identifiers: `` expected `int`, found `string` ``
- Use "expected X, found Y" format for type mismatches
- Include "help:" suggestions for fixable errors
- Include "defined here" links pointing to the exact definition site
- Be specific: "field `age` on table `user`" not just "field `age`"

## Implementation Phases

### Phase 1: Span Precision (in surrealguard-analyzer)
- Tighten all diagnostic spans to underline specific tokens
- For type mismatch: underline the value expression
- For missing field: underline the table target
- For wrong arg type: underline the specific argument
- For undefined field: underline the field name

### Phase 2: AST-Aware Hover (in surreal-language-server)
- Parse tree-sitter tree in hover handler
- Find node at cursor position
- Walk up tree to determine context
- Resolve hover based on AST context, not token string
- Support: tables, fields in queries, graph edges, variables, functions, `*`

### Phase 3: Cross-File Definition Links
- Track source file in analyzer Context
- Emit related info with file URIs
- Enable "go to definition" across files

### Phase 4: Polish
- Ensure inlay hints render correctly in Zed
- Add quick fixes (code actions) for common errors
- Syntax highlighting fixes in tree-sitter grammar

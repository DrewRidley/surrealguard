# SurrealGuard Analyzer — Implementation Plan

## Architecture

```
crates/analyzer/
├── src/
│   ├── lib.rs              # Public API: analyze(), AnalysisResult
│   ├── types.rs            # Our own type system (no surrealdb dep)
│   ├── span.rs             # Byte-offset spans from tree-sitter
│   ├── diagnostic.rs       # Diagnostics with spans, codes, suggestions
│   ├── context.rs          # Schema registry + variable scopes
│   ├── parser.rs           # Tree-sitter CST helpers
│   ├── schema.rs           # DEFINE TABLE/FIELD/INDEX extraction
│   ├── resolve.rs          # Expression → Type resolution (THE HEART)
│   ├── scope.rs            # Variable scoping (global, block, closure, implicit)
│   ├── permissions.rs      # Permission clause validation
│   ├── statements/         # Per-statement analyzers
│   │   ├── mod.rs          # Statement dispatcher
│   │   ├── select.rs       # SELECT: fields, aliases, graphs, destructure, FETCH, OMIT
│   │   ├── create.rs       # CREATE: param inference from CONTENT/SET
│   │   ├── update.rs       # UPDATE: SET/CONTENT/MERGE/PATCH/REPLACE
│   │   ├── delete.rs       # DELETE
│   │   ├── insert.rs       # INSERT: VALUES, objects
│   │   ├── upsert.rs       # UPSERT
│   │   ├── relate.rs       # RELATE: relation resolution, FROM/TO inference
│   │   ├── define.rs       # DEFINE analysis + validation
│   │   └── control.rs      # IF/FOR/RETURN/THROW/BREAK/CONTINUE
│   └── functions/          # Built-in function type signatures
│       ├── mod.rs          # Dispatcher by namespace
│       ├── array.rs        # array::* (flatten, map, filter, etc.)
│       ├── string.rs       # string::* (concat, split, etc.)
│       ├── math.rs         # math::* (abs, ceil, etc.)
│       ├── crypto.rs       # crypto::* (argon2, bcrypt, etc.)
│       ├── time.rs         # time::* (now, format, etc.)
│       ├── duration.rs     # duration::*
│       ├── object.rs       # object::* (keys, values, entries)
│       ├── type_fns.rs     # type::* and type::is::*
│       ├── parse.rs        # parse::*
│       ├── rand.rs         # rand::*
│       ├── geo.rs          # geo::* (distance, area, etc.)
│       ├── http.rs         # http::* (get, post, etc.)
│       ├── encoding.rs     # encoding::*
│       ├── meta.rs         # meta::* (id, tb)
│       ├── search.rs       # search::*
│       ├── session.rs      # session::*
│       └── vector.rs       # vector::*
```

## Core Components

### 1. Expression Resolver (`resolve.rs`)

The heart of the analyzer. Given any CST `value` node, determines its Type.

**Handles:**
- **Literals**: string, int, float, bool, none, null, datetime, duration, uuid
- **Variables**: `$name` → lookup in scope chain (block → function → global → implicit)
- **Identifiers**: field names → lookup in current table context
- **Paths**: `a.b.c` → chain resolution through nested types
- **Binary expressions**: `a + b` → numeric coercion, `a AND b` → bool, `a == b` → bool
- **Function calls**: `string::len(x)` → dispatch to function signatures
- **Cast expressions**: `<int>x` → explicit type, validate source is castable
- **Subqueries**: `(SELECT ...)` → recursive analysis
- **Closures**: `|$v| expr` → resolve param types from context, analyze body
- **Graph paths**: `->knows->person` → relation traversal with type nesting
- **Arrays**: `[1, 2, 3]` → infer element type from contents
- **Objects**: `{ a: 1, b: "x" }` → literal object type
- **Record IDs**: `person:john` → record<person>
- **Ranges**: `1..10` → range type
- **Destructuring**: `a.{ x, y }` → subset of object fields
- **Method calls**: `"hello".uppercase()` → dispatch as string function

### 2. Scope System (`scope.rs`)

Tracks variable bindings at different levels:

- **Global scope**: `DEFINE PARAM $endpoint VALUE '...'`
- **Block scope**: `LET $x = ...` (within `{ }`)
- **Loop scope**: `FOR $item IN $list { ... }` (element type inferred from list)
- **Closure scope**: `|$v: string| ...` (typed or inferred params)
- **Event scope**: `$event`, `$before`, `$after`, `$value`, `$input`
- **Query scope**: `$this` (current record), `$parent` (outer query)
- **Auth scope**: `$auth` (authenticated user record)

Scope is a stack — inner blocks shadow outer bindings.

### 3. Permission Analysis (`permissions.rs`)

Validates PERMISSIONS clauses make sense:
- `$this` resolves to the table's record type
- `$auth` resolves to the auth scope type
- Field access validation: `$this.email` checks that the table has an `email` field
- Type checking in comparisons: `$auth.role == 'admin'` validates types match

### 4. Custom Function Analysis

`DEFINE FUNCTION fn::greet($name: string) { RETURN "Hello, " + $name; }`
- Extract parameter types from declaration
- Analyze function body to determine return type
- Register in context for use in queries
- Validate calls match declared signature

## Diagnostic Codes

| Code   | Category        | Description |
|--------|----------------|-------------|
| SG001  | Reference      | Table not found |
| SG002  | Reference      | Field not found on table |
| SG003  | Type           | Type mismatch |
| SG004  | Type           | Cannot cast between types |
| SG005  | Schema         | Assignment to readonly field |
| SG006  | Schema         | Missing required field in CONTENT |
| SG007  | Schema         | Field not defined on schemafull table |
| SG008  | Reference      | Parameter not found |
| SG009  | Reference      | Function not found |
| SG010  | Permission     | Permission field reference invalid |
| SG011  | Scope          | Variable used before definition |
| SG012  | Scope          | Variable shadows outer binding |
| SG013  | Type           | Ambiguous type (strict mode) |
| SG014  | Type           | Return type mismatch in function |
| SG015  | Graph          | Invalid relation traversal |
| SG016  | Graph          | Table is not a relation |
| SG017  | Type           | Operator not valid for types |
| SG018  | Schema         | Duplicate field definition |
| SG019  | Type           | Wrong number of function arguments |
| SG020  | Type           | Wrong argument types for function |

## Phases

### Phase 1: Foundation ← START HERE
- [x] types.rs, span.rs, diagnostic.rs (from v2)
- [ ] Restructure into crates/analyzer
- [ ] scope.rs — variable scope stack
- [ ] resolve.rs — expression type resolution (literals, variables, paths, binary ops)
- [ ] schema.rs — DEFINE TABLE/FIELD extraction (from v2)
- [ ] statements/mod.rs — statement dispatcher

### Phase 2: Statement Analysis
- [ ] select.rs — field resolution, aliases, wildcards, WHERE, ORDER, LIMIT
- [ ] create.rs — param inference from CONTENT/SET
- [ ] update.rs — SET/CONTENT/MERGE variants
- [ ] delete.rs, insert.rs, upsert.rs
- [ ] relate.rs — relation resolution
- [ ] control.rs — IF/FOR/RETURN

### Phase 3: Advanced Features
- [ ] Graph traversal analysis (->table->, destructuring)
- [ ] FETCH clause resolution
- [ ] Custom function analysis (DEFINE FUNCTION)
- [ ] Closure type inference
- [ ] Permission validation
- [ ] Subquery analysis

### Phase 4: Function Signatures
- [ ] All 17 function namespaces
- [ ] ~60+ individual function type mappings
- [ ] Argument count/type validation

### Phase 5: Strict Mode & Polish
- [ ] Strict mode: error on ambiguous types
- [ ] Unused variable warnings
- [ ] Shadowing warnings
- [ ] Suggestion engine (did you mean?)
- [ ] ariadne integration for CLI rendering

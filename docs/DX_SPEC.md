# SurrealGuard DX Specification

## Philosophy

Every interaction should feel like rust-analyzer for SurrealQL.
The developer should never wonder "what does this mean?" or "why is this highlighted?"
Every pixel of feedback should be intentional, precise, and helpful.

---

## 1. Architecture

### Option A: Standalone LSP (recommended)
Build our own LSP from scratch using `tower-lsp`. The existing ForetagInc LSP has a
different data model and goals. Fighting it wastes time.

**Our LSP provides**: diagnostics, hover, inlay hints, go-to-definition, completions.
**Tree-sitter provides**: syntax highlighting, parsing.
**Surrealguard-analyzer provides**: type inference, schema validation, diagnostics.

### Option B: Clean module in existing LSP
Keep using ForetagInc LSP but with a clean separation layer. Disable their semantic
analysis entirely and route everything through surrealguard.

### Workspace Model
```
.surrealguard.toml (optional)
schema/
  tables.surql      → schema definitions
  functions.surql   → custom functions
queries/
  api.surql         → queries to analyze
migrations/
  001_init.surql    → also analyzed
```

All `.surql` files in workspace are parsed. DEFINE statements build the schema context.
Every file is analyzed against the full workspace schema.

Each definition tracks its **source file + byte offset** so "defined here" links
always point to the right place.

---

## 2. Diagnostics

### Span Rules

**THE RULE**: Underline the smallest meaningful token that's wrong.

```
✗ Wrong:  UPDATE user SET age = 'twenty';
          ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^   ← whole statement

✓ Right:  UPDATE user SET age = 'twenty';
                                ^^^^^^^^   ← just the value
```

Specific span targets:
| Error | Underline |
|-------|-----------|
| Type mismatch in SET | The value expression (`'twenty'`) |
| Missing required field | The table name in CREATE (`user`) |
| Undefined field on schemafull | The field name (`nonexistent`) |
| Wrong function arg type | The specific argument (`age`) |
| Wrong function arg count | The argument list (`(a, b, c)`) |
| Unknown table | The table identifier (`ghost`) |
| Invalid graph edge | The target table in the graph path (`file`) |
| Duplicate field assignment | The second occurrence of the field name |

### Message Format

Follow rustc conventions exactly:

```
expected `int`, found `string`
```

Not:
```
cannot assign type `string` to field `age` of type `int` with `=`
```

More examples:
```
expected `int`, found `string`                          ← type mismatch
`string::len` takes 1 argument but 2 were supplied      ← wrong arg count
`string::len` expects `string` for argument 1, found `int` ← wrong arg type
field `ghost` is not defined on schemafull table `user` ← undefined field
missing field `profile` in CREATE on `user`             ← missing required
relation `wrote` connects `user` → `post`, not `user`  ← invalid graph edge
```

### Severity

- **Error** (red squiggle): Will fail at runtime. Type mismatches, undefined schemafull fields,
  invalid graph edges, wrong arg count.
- **Warning** (yellow squiggle): Likely a bug. Missing required field, wrong arg type,
  comparing incompatible types.
- **Hint** (no squiggle, just faded): Style suggestions. Variable shadowing,
  unnecessary optional chaining. **Hints should NEVER show squiggles.**

### Related Information ("defined here")

Only include when:
1. The definition is in the SAME file, OR
2. We can provide a valid cross-file URI + position

Format:
```
expected `int`, found `string`
  ╰─ `age` defined as `int` here          → links to DEFINE FIELD age ON user TYPE int
```

Never show related info that points to garbage. If we can't guarantee correctness, omit it.

### Suggestions ("help:")

Only when there's an obvious fix:
```
field `emal` is not defined on table `user`
  help: did you mean `email`?

`string::len` expects `string`, found `int`
  help: convert with `<string>age`
```

---

## 3. Hover

### Design Principles
- Show exactly what the symbol IS — its definition
- Use `surql` fenced code blocks for syntax-highlighted definitions
- No metadata noise (no "Source:", "Confidence:", "Inferred from...")
- Keep it short — if it doesn't fit in a tooltip, it's too long

### Table Hover

Hovering `user` in any context where it's a table reference:

```surql
DEFINE TABLE user SCHEMAFULL
  name: string
  age: int
  email: string
  active: bool
  tags: array<string>
  profile: record<profile>
```

- Shows `DEFINE TABLE` prefix (matches actual SurrealQL syntax)
- Shows `SCHEMAFULL`/`SCHEMALESS` if set
- Lists only EXPLICITLY defined fields (no inferred garbage)
- Fields sorted alphabetically
- Comment below code block if present

### Relation Table Hover

Hovering `wrote` in `->wrote->post`:

```surql
DEFINE TABLE wrote TYPE RELATION FROM user TO post
  created_at: datetime
```

- Shows the full TYPE RELATION FROM ... TO ... clause
- Shows relation-specific fields

### Field Hover (in queries)

Hovering `age` in `WHERE age > 18`:

```surql
DEFINE FIELD age ON user TYPE int
```

Hovering `name` in `SELECT name FROM user`:

```surql
DEFINE FIELD name ON user TYPE string
```

If the field has READONLY, DEFAULT, ASSERT, show them:
```surql
DEFINE FIELD status ON user TYPE string DEFAULT 'active' READONLY
```

### Built-in Function Hover

Hovering `string::len` or `len` inside `string::len(name)`:

```surql
string::len(value: string) -> number
```

Returns the length of a string in characters.

[SurrealDB docs](https://surrealdb.com/docs/surrealql/functions/database/string#stringlen)

- Signature in a code block
- One-line summary
- Link to the specific function docs (with anchor)
- No `fn` prefix (it's not a keyword in SurrealQL)

### Custom Function Hover

Hovering `fn::greet`:

```surql
DEFINE FUNCTION fn::greet($name: string) {
  RETURN "Hello, " + $name;
}
```

- Shows the full function definition as written
- Or if too long, shows signature + comment

### Variable Hover

Hovering `$adults`:

```
$adults: array<{ name: string, age: int, email: string, ... }>
```

- Shows the inferred type
- For complex types, abbreviate with `...`

### Wildcard Hover

Hovering `*` in `SELECT * FROM user`:

```surql
-- Expands to all fields on `user`:
  name: string
  age: int
  email: string
  active: bool
  tags: array<string>
  profile: record<profile>
```

### Keyword Hover

**No hover for keywords.** `DEFINE`, `SELECT`, `WHERE`, `SET` don't need tooltips.
rust-analyzer doesn't show hovers for `fn`, `let`, `struct`.

### Special Variables

Hovering `$auth`, `$this`, `$value`, `$event`:

```
$auth: object
```

Built-in variable representing the authenticated user's session data.

---

## 4. Inlay Hints

### LET Bindings

Show inferred type after the variable name:

```surql
LET $adults: array<{ name: string, age: int }>  = SELECT * FROM user WHERE age >= 18;
LET $greeting: string  = "hello world";
LET $count: int  = 42;
```

**Skip when**:
- Type is `any` (unknown)
- Type is obvious from the literal (maybe — debatable)

### Statement Result Types (optional)

Could show after SELECT/CREATE keywords:

```surql
SELECT  → array<{ name: string, age: int }>  * FROM user;
```

But this might be too noisy. **Discuss: do we want this?**

### Parameter Types in Functions

```surql
DEFINE FUNCTION fn::greet($name: string ) {
```

Already has type annotation — no hint needed.

---

## 5. Go to Definition

Clicking (Cmd+click or F12) on:

| Symbol | Goes to |
|--------|---------|
| Table name in FROM/CREATE/UPDATE | DEFINE TABLE statement |
| Field name in SET/WHERE | DEFINE FIELD statement |
| Function name | DEFINE FUNCTION statement |
| Variable name | LET statement that binds it |
| Graph edge table | DEFINE TABLE TYPE RELATION statement |
| Record link (`user:123`) | DEFINE TABLE statement for `user` |

**Cross-file**: If the definition is in another file, jump to that file + line.
This is the killer feature — it makes the workspace feel cohesive.

---

## 6. Completions

### After `FROM` / `CREATE` / `UPDATE` / `DELETE`
List all known tables with their schema mode:
```
user          SCHEMAFULL
post          SCHEMAFULL
wrote         RELATION
profile       SCHEMAFULL
```

### After `SET` / in `WHERE` clause
List fields from the target table:
```
name          string
age           int
email         string
```

### After `->` in graph traversal
List relation tables:
```
wrote         RELATION FROM user TO post
follows       RELATION FROM user TO user
```

### After `::` in function calls
List functions in that namespace:
```
string::len         (string) -> number
string::uppercase   (string) -> string
string::lowercase   (string) -> string
```

### After `$`
List known variables in scope:
```
$adults       array<{ ... }>
$auth         object
$this         object
```

---

## 7. Syntax Highlighting

This is handled by tree-sitter + highlights.scm in the Zed extension.

### Problems to Fix
- Single-letter keywords (`keyword_m`) matching inside identifiers
- Inconsistent coloring of identifiers vs keywords in hover code blocks
- Short keywords (`IN`, `AS`, `ON`, `BY`) potentially matching inside words

### Color Scheme (semantic)
| Element | Color Role |
|---------|-----------|
| Keywords (SELECT, DEFINE, FROM, WHERE) | keyword (purple) |
| Table/field identifiers | variable (white/light) |
| Type names (string, int, bool) | type (teal/cyan) |
| String literals | string (green) |
| Number literals | number (orange) |
| Built-in functions (string::len) | function (yellow) |
| Variables ($auth, $name) | variable.special (blue) |
| Operators (=, >, AND, OR) | operator |
| Comments (-- text) | comment (grey) |

---

## 8. Implementation Plan

### Phase 1: Clean Foundation
1. Decide: standalone LSP or module in existing?
2. Set up the workspace model (file tracking, schema context building)
3. Implement diagnostic pipeline with correct spans
4. Implement related info with file-aware URIs

### Phase 2: Hover
1. AST-aware hover (tree-sitter node at cursor → context resolution)
2. Table, field, function, variable, wildcard, graph edge hovers
3. Clean formatting — no noise

### Phase 3: Inlay Hints
1. LET binding type hints
2. Test with Zed to ensure correct rendering

### Phase 4: Go to Definition + Completions
1. Cross-file go-to-definition
2. Context-aware completions

### Phase 5: Syntax Highlighting
1. Fix highlights.scm issues
2. Ensure hover code blocks render with correct colors

---

## Open Questions

1. **Standalone vs module?** Building our own LSP is cleaner but more work.
   Using the existing one means fighting their data model.

2. **Statement result type hints?** Show `→ array<{...}>` after SELECT?
   Helpful but potentially noisy.

3. **Cross-file "defined here" links?** Worth the complexity? Or just
   rely on go-to-definition?

4. **How to handle schemaless tables?** Show warnings for unknown fields
   or stay silent since anything goes?

5. **Live SurrealDB connection?** The existing LSP supports fetching
   schema from a running database. Do we want to support this too?

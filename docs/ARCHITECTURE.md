# SurrealGuard v2 Architecture

## Vision

SurrealGuard is a cross-language static analysis and type inference engine for SurrealQL. It parses SurrealQL — both standalone `.surql` files and queries embedded in host language code — performs semantic analysis against a declared schema, and delivers type information and diagnostics through each language's native tooling. No generated files. No watch mode. Types just appear.

## The Problem

SurrealDB's official parser discards span (source location) information before producing its AST. The `Span` type exists internally in `surrealdb-core`'s parser (`syn/token/mod.rs`), but it is stripped from all AST nodes before they are returned. This makes it impossible to emit precise, location-aware diagnostics using SurrealDB's own parser.

Additionally, SurrealDB's AST types live in `surrealdb-core`, which is explicitly marked as an unstable internal API with no semver guarantees. Depending on it is fragile.

The codegen-to-file approach (v1) works but introduces friction:
- Generated files are visible artifacts that must be managed (committed or gitignored)
- File-watching can drift out of sync
- An extra build step developers forget to run
- Feels foreign to the host language's development workflow

## The Solution

### Tree-sitter for Parsing

Use a tree-sitter grammar for SurrealQL as the parser foundation. Tree-sitter provides:

- **Full CST with spans on every node** — byte offsets, line/column, ranges
- **Error recovery** — partial parses still produce usable trees
- **Incremental reparsing** — only re-parses changed regions on edits
- **Language injection** — tree-sitter natively supports parsing embedded languages within host language code (e.g., SurrealQL inside a TypeScript template literal or Rust string literal)
- **Editor integration** — tree-sitter grammars are already the standard for syntax highlighting in Zed, Neovim, Helix, and others
- **No dependency on SurrealDB internals** — the grammar is independently maintained and follows SurrealQL's syntax, not its internal representation

This decouples surrealguard from `surrealdb-core` entirely. We own the parser, we own the spans, we own the error recovery.

### Language Injection: Parsing SurrealQL Inside Host Code

Tree-sitter's language injection system allows parsing embedded SurrealQL within host language source files. This means surrealguard can analyze queries written inline in any supported host language:

```typescript
// Tree-sitter parses the TypeScript file, identifies the template literal
// inside surql`...`, then injects the SurrealQL grammar to parse its contents.
const users = await db.query(surql`
    SELECT name, email FROM user WHERE age > $min_age
`);
```

```rust
// Same concept: the proc macro string literal is parsed as SurrealQL.
let users = db.query(surql!(
    "SELECT name, email FROM user WHERE age > $min_age"
)).await?;
```

```python
# Tagged string identified and injected.
users = await db.query(surql("SELECT name, email FROM user WHERE age > $min_age"))
```

The injection queries are defined per host language grammar. Each host language adapter specifies the pattern (function name, macro invocation, tagged template, etc.) that signals "this string contains SurrealQL."

## Core Architecture

```
                                        Diagnostics & Types
                                               |
                                               v
 .surql files ──┐                    ┌─────────────────────┐
                ├── tree-sitter ──>  │  surrealguard-core  │
 host code ─────┘   (with injection) │                     │
                                     │  - Schema registry  │
                                     │  - Type resolver     │
                                     │  - Diagnostic engine │
                                     │  - Permission model  │
                                     └────────┬────────────┘
                                              │
                          ┌───────────────────┼───────────────────┐
                          │                   │                   │
                          v                   v                   v
                   ┌─────────────┐   ┌──────────────┐   ┌──────────────┐
                   │ Rust        │   │ TypeScript   │   │ Python       │
                   │ proc macro  │   │ LS plugin    │   │ type plugin  │
                   └─────────────┘   └──────────────┘   └──────────────┘
                          │                   │                   │
                          v                   v                   v
                   rustc/rust-analyzer   tsserver/vscode     pyright/mypy
```

### Layer 1: Tree-sitter SurrealQL Grammar

A tree-sitter grammar (`tree-sitter-surrealql`) that produces a CST for SurrealQL with full span information on every node.

Responsibilities:
- Lexing and parsing SurrealQL into a concrete syntax tree
- Providing byte-offset spans for every node
- Recovering from syntax errors to produce partial trees
- Serving as the injection grammar for host language parsers

The grammar is independently versioned and tracks SurrealQL's evolving syntax.

### Layer 2: surrealguard-core (Analysis Engine)

The semantic analysis engine. Takes a tree-sitter CST and performs:

**Schema Registry**
- Loads and indexes DEFINE TABLE, DEFINE FIELD, DEFINE INDEX statements
- Builds complete table types from field definitions
- Tracks relation tables (from/to types)
- Resolves field paths through nested structures

**Type Resolver**
- Infers return types for statements (SELECT, CREATE, UPDATE, DELETE, RELATE, INSERT, UPSERT)
- Infers parameter types from usage context (e.g., `$min_age > age` where `age` is `int` implies `$min_age: int`)
- Resolves graph traversals (`->relation->table`)
- Handles FETCH expansion, destructuring, aliases, OMIT, VALUE
- Resolves built-in function return types

**Permission Model**
- Tracks PERMISSIONS clauses on fields
- Models auth-dependent return shapes (fields that are only present for certain roles)
- Emits types that honestly represent conditional access:
  - Fields gated by permissions become optional in the output type
  - Metadata annotations indicate which permission is required

**Diagnostic Engine**
- Emits errors, warnings, and hints with precise source locations (from tree-sitter spans)
- Schema violations: referencing undefined tables/fields
- Type mismatches: wrong parameter types, incompatible operations
- Permission warnings: accessing fields that require elevated auth
- Deprecation hints: using deprecated fields or patterns

Diagnostic format:
```
schema/user.surql:5:12  error[E0201]: field `email` expects type `string`, got `number`
  |
5 |     DEFINE FIELD email ON user TYPE number;
  |                              ^^^^^^ expected `string` based on usage in queries/get_user.surql:3
  |
  = note: field is used as `string` in SELECT email FROM user
```

### Layer 3: Language Adapters

Thin, per-language adapters that deliver surrealguard-core's analysis results through each language's native compile-time or editor-time mechanism.

#### Rust: Proc Macro

```rust
use surrealguard::surql;

// At compile time:
// 1. Proc macro extracts the query string
// 2. Calls surrealguard-core to analyze against the schema
// 3. Emits a typed wrapper — compile error if schema violations found

let users: Vec<User> = db.query(surql!(
    "SELECT name, email FROM user WHERE age > $min_age",
    // $min_age inferred as i64 from schema
)).await?;

// The surql!() macro expands to something like:
// {
//     struct __SurqlQuery;
//     impl SurqlTyped for __SurqlQuery {
//         type Result = Vec<UserProjection>;
//         type Params = SurqlParams_min_age;
//     }
//     SurqlQuery::<__SurqlQuery>::new("SELECT ...")
// }
```

The proc macro reads the schema path from `surrealguard.toml` at compile time. This is exactly the pattern SQLx uses. No generated files — `cargo build` just works.

#### TypeScript: Language Service Plugin

A TypeScript language service plugin that intercepts `surql` tagged template literals and provides:

- **Hover types**: hover over `surql\`...\`` to see the inferred return type and parameter types
- **Completions**: table names, field names, function names inside the template
- **Diagnostics**: inline errors for schema violations, type mismatches
- **Go to definition**: jump from a field reference in a query to its DEFINE FIELD in the schema

```typescript
// The plugin intercepts this and provides type information to the editor.
// No codegen. The TS language service resolves the types live.
const users = await db.typed(surql`
    SELECT name, email FROM user WHERE age > $min_age
`);
// Editor infers: users: Array<{ name: string, email: string }>
// Editor infers: $min_age: number
```

Implementation: a `tsserver` plugin (or VS Code extension using `vscode.typescript-language-features`) that communicates with surrealguard-core via a native binary or WASM module.

#### C#: Source Generator

```csharp
// Roslyn source generator analyzes [Surql] attributed strings at compile time.
[Surql("SELECT name, email FROM user WHERE age > $min_age")]
public partial class GetUsersQuery : ISurqlQuery<GetUsersResult, GetUsersParams> { }

// The source generator emits the GetUsersResult and GetUsersParams types.
// dotnet build just works.
```

#### Python: Type Checker Plugin

A Pyright or MyPy plugin that resolves `surql()` calls:

```python
users = await db.query(surql("SELECT name, email FROM user"))
# Pyright sees: list[TypedDict('UserResult', {'name': str, 'email': str})]
```

#### Java: Annotation Processor

```java
@Surql("SELECT name, email FROM user WHERE age > $min_age")
public interface GetUsersQuery extends SurqlQuery<GetUsersResult, GetUsersParams> {}

// Annotation processor generates the result/param types during javac.
```

#### PHP: PHPStan Extension

A PHPStan extension that resolves `surql()` call return types during static analysis.

#### Fallback: File Codegen

For languages or environments where a native integration isn't available, the file-based codegen approach remains as a fallback. This is the v1 approach — still useful, just not the primary path.

## Permission-Dependent Types

A key capability is expressing that query results may have different shapes depending on the authenticated user's role. The analysis engine tracks PERMISSIONS clauses from the schema and reflects them in the emitted types.

### Schema

```surql
DEFINE TABLE user SCHEMAFULL;
DEFINE FIELD name ON user TYPE string;
DEFINE FIELD email ON user TYPE string
    PERMISSIONS
        FOR select WHERE $auth.role = "admin" OR id = $auth.id;
DEFINE FIELD ssn ON user TYPE string
    PERMISSIONS
        FOR select WHERE $auth.role = "admin";
```

### Emitted Types

The type system in each language represents permission-gated fields as optional, with metadata for tooling:

**TypeScript:**
```typescript
type GetUserResult = {
    name: string;
    email: string | undefined;  // requires: admin | owner
    ssn: string | undefined;    // requires: admin
};
```

**Rust:**
```rust
pub struct GetUserResult {
    pub name: String,
    pub email: Option<String>,  // gated by: admin | owner
    pub ssn: Option<String>,    // gated by: admin
}
```

The LSP/plugin layer can provide hover documentation explaining *why* a field is optional:
```
email: string | undefined
  Permission-gated: visible when $auth.role = "admin" OR id = $auth.id
```

## Tree-sitter Grammar Requirements

The `tree-sitter-surrealql` grammar must cover:

### Statements
- Data: SELECT, CREATE, UPDATE, DELETE, INSERT, UPSERT, RELATE
- Schema: DEFINE (TABLE, FIELD, INDEX, ANALYZER, FUNCTION, SCOPE, TOKEN, EVENT, TYPE)
- Control: IF/ELSE, FOR, RETURN, THROW, BREAK, CONTINUE
- Transaction: BEGIN, COMMIT, CANCEL
- Other: LET, INFO, LIVE, KILL, SHOW, SLEEP, USE, OPTION

### Expressions
- Values: strings, numbers, booleans, null, none, arrays, objects, durations, datetimes, UUIDs, geometry literals
- Identifiers: plain, backtick-quoted, bracket-quoted
- Parameters: `$param`
- Idioms: field access (`foo.bar`), array access (`foo[0]`), graph traversal (`->edge->node`)
- Destructuring: `field.{ sub1, sub2 }`
- Operators: arithmetic, comparison, logical, IS, IS NOT, CONTAINS, CONTAINSALL, etc.
- Functions: built-in (`string::lowercase(...)`) and custom
- Subqueries: `(SELECT ...)`
- Record IDs: `table:id`, `table:⟨complex-id⟩`
- Casting: `<int>`, `<string>`, `<datetime>`, etc.
- Type expressions: `TYPE string`, `TYPE option<int>`, `TYPE array<string, 10>`

### Injection Points

Each host language's tree-sitter grammar needs injection queries to identify SurrealQL regions:

```scheme
; TypeScript injection — surql tagged template
((call_expression
  function: (identifier) @_fn
  arguments: (template_string) @injection.content)
 (#eq? @_fn "surql")
 (#set! injection.language "surrealql"))

; Rust injection — surql!() macro
((macro_invocation
  macro: (identifier) @_name
  (token_tree (string_literal) @injection.content))
 (#eq? @_name "surql")
 (#set! injection.language "surrealql"))

; Python injection — surql() call
((call
  function: (identifier) @_fn
  arguments: (argument_list (string) @injection.content))
 (#eq? @_fn "surql")
 (#set! injection.language "surrealql"))
```

## LSP Server

surrealguard ships an LSP server that provides diagnostics and intelligence for both `.surql` files and SurrealQL embedded in host code.

### Capabilities

- **textDocument/publishDiagnostics** — schema violations, type errors, permission warnings
- **textDocument/hover** — inferred types for expressions, fields, parameters
- **textDocument/completion** — table names, field names, built-in functions, keywords
- **textDocument/definition** — jump from field usage to DEFINE FIELD
- **textDocument/references** — find all queries that reference a field or table
- **textDocument/rename** — rename a field across schema and all queries
- **textDocument/codeAction** — quick fixes (add missing DEFINE FIELD, fix type annotation)

### Editor Integration

The LSP server works in any editor that supports LSP:
- **Zed**: native tree-sitter + LSP support, language injection built in
- **Neovim**: via nvim-lspconfig + tree-sitter injections
- **VS Code**: via extension wrapping the LSP
- **Helix**: native tree-sitter + LSP support

For embedded SurrealQL in host code, the LSP leverages the editor's tree-sitter language injection to identify SurrealQL regions, then runs surrealguard-core analysis on those regions with the appropriate byte offset mapping.

## Implementation Roadmap

### Phase 1: Foundation
- [ ] Build or adopt `tree-sitter-surrealql` grammar
- [ ] Rewrite surrealguard-core to consume tree-sitter CST instead of `surrealdb::sql::parse`
- [ ] Implement diagnostic engine with span-aware error reporting
- [ ] Schema registry (DEFINE TABLE, DEFINE FIELD)

### Phase 2: LSP
- [ ] LSP server with diagnostics for `.surql` files
- [ ] Hover types for expressions and parameters
- [ ] Completions for tables, fields, functions
- [ ] Go-to-definition for field references

### Phase 3: Language Adapters (Priority Order)
- [ ] TypeScript language service plugin
- [ ] Rust proc macro
- [ ] Python type plugin (Pyright)
- [ ] C# source generator

### Phase 4: Advanced Analysis
- [ ] Permission-dependent type modeling
- [ ] Graph traversal type resolution
- [ ] Built-in function type signatures (complete coverage)
- [ ] Custom function analysis (DEFINE FUNCTION)
- [ ] Transaction / LET variable scoping
- [ ] IF/ELSE branch type narrowing

### Phase 5: Language Injection
- [ ] Tree-sitter injection queries for TypeScript, Rust, Python, C#, Java, PHP, Go
- [ ] LSP support for embedded queries in host code
- [ ] Cross-file analysis (schema + queries + host code)

## Design Principles

1. **Own the parser.** Depending on SurrealDB's internal parser means no spans, no error recovery, and breakage on every patch release. A tree-sitter grammar gives us full control.

2. **Integrate natively.** Generated files are a last resort. Each language has a mechanism for compile-time type injection — use it.

3. **Be honest about types.** If a field might be absent due to permissions, say so. Don't pretend the type system is simpler than reality.

4. **Spans on everything.** Every diagnostic, every inferred type, every suggestion must point to a precise source location. This is the entire reason for tree-sitter.

5. **Incremental by default.** Tree-sitter reparses incrementally. The analysis engine should follow suit — re-analyze only what changed.

6. **Schema is the source of truth.** The DEFINE statements in `.surql` files define the contract. Everything else is derived.

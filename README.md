# SurrealGuard 🛡️

SurrealGuard is a static analysis and type checking system for SurrealQL queries, providing compile-time safety and type inference for your SurrealDB applications.

## Motivation

Working with SurrealDB's query language (SurrealQL) in TypeScript/JavaScript applications can be challenging due to:

- Lack of compile-time type safety
- No automated parameter type inference
- Missing schema validation before runtime
- Difficulty maintaining type definitions as schemas evolve

SurrealGuard aims to solve these problems by providing:

- Static analysis of SurrealQL queries against your schema
- Automatic type generation for query results
- Parameter type inference
- Early error detection for schema violations

## Features

### Schema Analysis ✅
- [x] DEFINE TABLE validation
- [x] DEFINE FIELD type checking
- [x] Nested object structures
- [x] Array types
- [x] Record links
- [ ] Custom types
- [ ] DEFINE ANALYZER
- [ ] DEFINE FUNCTION
- [ ] DEFINE INDEX
- [ ] DEFINE SCOPE/TOKEN
- [ ] DEFINE EVENT

### Query Analysis ✅
- [x] SELECT statements (including FETCH)
- [x] CREATE/INSERT
- [x] UPDATE/UPSERT
- [x] DELETE
- [x] RELATE
- [x] Graph traversals
- [ ] Nested queries
- [ ] Functions and expressions
- [ ] IF/ELSE conditions
- [ ] RETURN statements
- [ ] Transactions (BEGIN/COMMIT/CANCEL)
- [ ] LET variables
- [ ] INFO statements
- [ ] LIVE queries

### Type Generation 🏗️
- [x] TypeScript output
- [ ] JavaScript with JSDoc
- [ ] Rust
- [ ] Other languages (Go, Python, etc.)

### Developer Experience 🛠️
- [x] CLI tool with watch mode
- [x] Project configuration
- [ ] VS Code extension
- [ ] Error messages with suggestions
- [ ] Query formatting
- [ ] Parameter Inference

## Project Structure

```
surrealguard/
├── crates/
│   ├── cli/          # Command line interface
│   ├── core/         # Analysis engine
│   ├── codegen/      # Code generation
│   └── macros/       # Internal proc macros
└── examples/         # Usage examples
```

## Getting Started

1. Install the CLI:
```bash
cargo install surrealguard
```

2. Initialize a new project:
```bash
surrealguard init
```

3. Configure your schema and query paths in `surrealguard.toml`:
```toml
[schema]
path = "schema/surrealql/"

[queries]
path = "queries/surrealql/"
```

4. Generate types:
```bash
surrealguard run
```

## Example

```typescript
// Your schema
DEFINE TABLE user SCHEMAFULL;
    DEFINE FIELD name ON user TYPE string;
    DEFINE FIELD age ON user TYPE number;
    DEFINE FIELD posts ON user TYPE array<record<post>>;

// Your query
const query = "SELECT name, age, posts.* FROM user FETCH posts";

// Generated types
interface User {
    name: string;
    age: number;
    posts: Post[];
}
```

## Current Limitations

- Complex functions and expressions not yet supported
- No transaction analysis
- Limited to basic schema definitions
- Missing support for some SurrealQL features
- Type generation limited to TypeScript

## Contributing

Contributions are welcome! See our [Contributing Guide](CONTRIBUTING.md) for details.

## License

MIT

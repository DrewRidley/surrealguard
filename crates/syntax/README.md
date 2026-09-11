# surrealql-analyzer-syntax

The syntax layer for [SurrealQL Analyzer](https://github.com/surrealdb/analyzer).

Everything that reads SurrealQL source text lives here: tree-sitter parsing
(via the vendored `surrealql-analyzer-tree-sitter-surrealql` grammar), the typed,
span-carrying AST, and the lowering pass between them. Downstream analysis
consumes only the `ast::*` values — spans survive lowering because SurrealDB's
own parser discards them.

The public API is not yet stable; pin an exact version.

## License

MIT OR Apache-2.0.

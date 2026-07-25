# surrealguard-syntax

The syntax layer for [SurrealGuard](https://github.com/DrewRidley/surrealguard).

Everything that reads SurrealQL source text lives here: tree-sitter parsing
(via the vendored `surrealguard-tree-sitter-surrealql` grammar), the typed,
span-carrying AST, and the lowering pass between them. Downstream analysis
consumes only the `ast::*` values — spans survive lowering because SurrealDB's
own parser discards them.

The public API is not yet stable; pin an exact version.

## License

MIT OR Apache-2.0.

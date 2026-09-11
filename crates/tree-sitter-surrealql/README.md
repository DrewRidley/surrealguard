# surrealql-analyzer-tree-sitter-surrealql

Vendored [tree-sitter](https://tree-sitter.github.io/) grammar for
[SurrealQL](https://surrealdb.com/docs/surrealql), with thin Rust bindings, used
by [SurrealQL Analyzer](https://github.com/surrealdb/analyzer).

## Vendoring

The grammar sources — `grammar.js`, `src/parser.c`, `src/scanner.c`,
`src/grammar.json`, `src/node-types.json`, and the `src/tree_sitter/*.h` headers
— are vendored **verbatim** from the merged official grammar
[`surrealdb/surrealql-tree-sitter`](https://github.com/surrealdb/surrealql-tree-sitter)
at commit `f40a0ede750081b8d49feb40516c28cc49f1cce4`. SurrealQL Analyzer maintains the
thin Rust binding shim in `bindings/rust/` on top.

## Crate name

The crates.io name `tree-sitter-surrealql` is already taken by an unrelated
archived crate, so this package is published as
**`surrealql-analyzer-tree-sitter-surrealql`**. The library keeps the conventional
name via `[lib] name = "tree_sitter_surrealql"`, so it is imported unchanged:

```rust
let language = tree_sitter_surrealql::LANGUAGE;
let mut parser = tree_sitter::Parser::new();
parser.set_language(&language.into()).expect("Failed to set language");
```

## License

MIT — see [`LICENSE`](LICENSE). The grammar sources originate from
`surrealdb/surrealql-tree-sitter`, whose `tree-sitter.json` metadata declares
MIT.

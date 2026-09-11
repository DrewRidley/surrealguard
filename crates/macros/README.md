# surrealql-analyzer-macros

The compile-time SurrealQL proc-macros behind
[SurrealQL Analyzer](https://github.com/surrealdb/analyzer).

- `surql!("…")` checks a query at compile time and expands to its text as a
  `&'static str`.
- `query!("…", name = value, …)` checks a query and expands to a
  `surrealql_analyzer_rs::Query<T>` whose `T` is the inferred, nameless result type,
  with each parameter pinned to the Rust type its inferred kind decodes into.
- `query_file!("path.surql", name = value, …)` is `query!` with the SurrealQL
  read from a file at compile time, resolved relative to the crate root.

All three run the SurrealQL Analyzer engine against your project schema during
compilation, so an invalid query is a `cargo check` error — no build script and
no language server required.

Most users should depend on
[`surrealql-analyzer-rs`](https://crates.io/crates/surrealql-analyzer-rs), which re-exports
all three macros together with the runtime types (`Query<T>`, `Rows`, `Error`)
their expansions refer to.

## License

MIT OR Apache-2.0.

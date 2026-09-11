# surrealql-analyzer-workspace

Workspace, schema, and analysis-orchestration layer for
[SurrealQL Analyzer](https://github.com/surrealdb/analyzer).

This crate owns the middle of the pipeline: it holds the source registry and
config, extracts the schema catalog from `DEFINE`/`REMOVE` statements, runs type
inference over the lowered AST (`analyzer::*`), and produces the findings the CLI
and LSP consume. It also backs editor features (hover, go-to-definition,
completion suggestions).

The public API is not yet stable; pin an exact version. Most users should reach
for the [`surrealql-analyzer`](https://crates.io/crates/surrealql-analyzer) CLI or
[`surrealql-analyzer-rs`](https://crates.io/crates/surrealql-analyzer-rs) instead.

## License

MIT OR Apache-2.0.

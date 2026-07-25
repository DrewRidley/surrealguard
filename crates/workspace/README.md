# surrealguard-workspace

Workspace, schema, and analysis-orchestration layer for
[SurrealGuard](https://github.com/DrewRidley/surrealguard).

This crate owns the middle of the pipeline: it holds the source registry and
config, extracts the schema catalog from `DEFINE`/`REMOVE` statements, runs type
inference over the lowered AST (`analyzer::*`), and produces the findings the CLI
and LSP consume. It also backs editor features (hover, go-to-definition,
completion suggestions).

The public API is not yet stable; pin an exact version. Most users should reach
for the [`surrealguard`](https://crates.io/crates/surrealguard) CLI or
[`surrealguard-rs`](https://crates.io/crates/surrealguard-rs) instead.

## License

MIT OR Apache-2.0.

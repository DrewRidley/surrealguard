# surrealql-analyzer-lsp

The Language Server Protocol implementation for
[SurrealQL Analyzer](https://github.com/surrealdb/analyzer).

The `surrealql-analyzer-lsp` binary serves the analyzer over stdio: it tracks
workspace documents, runs analysis through `surrealql-analyzer-workspace`, and
converts findings into LSP diagnostics, with hover and go-to-definition backed
by the same inference. Point any LSP-capable editor at the binary to get live
SurrealQL diagnostics and type information.

Every diagnostic also carries suppression quick fixes: silence it at its site
with a `-- surrealql-analyzer: allow(...)` directive, or across the workspace with a
`[lints]` entry in `surrealql-analyzer.toml`. In a host file (`.ts`, `.svelte`) the
site fix is offered only where a comment line can be inserted without breaking
the string literal the query lives in — elsewhere only the workspace fix is
offered.

The public API is not yet stable; pin an exact version.

## License

MIT OR Apache-2.0.

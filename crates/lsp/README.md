# surrealguard-lsp

The Language Server Protocol implementation for
[SurrealGuard](https://github.com/DrewRidley/surrealguard).

The `surrealguard-lsp` binary serves the analyzer over stdio: it tracks
workspace documents, runs analysis through `surrealguard-workspace`, and
converts findings into LSP diagnostics, with hover and go-to-definition backed
by the same inference. Point any LSP-capable editor at the binary to get live
SurrealQL diagnostics and type information.

The public API is not yet stable; pin an exact version.

## License

MIT OR Apache-2.0.

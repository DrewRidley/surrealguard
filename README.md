# SurrealGuard 🛡️

SurrealGuard is a static analyzer and type-inference engine for SurrealQL. It parses your `.surql` schema and queries with span-preserving syntax, infers the response type of every statement, and reports contract violations — unknown tables and fields, kind mismatches, invalid graph traversals, misused clauses, bad function calls — before anything reaches a running SurrealDB instance.

## Why

SurrealDB tolerates a lot at runtime: cross-kind comparisons order by kind instead of failing, coercions succeed silently, misspelled fields return `NONE`. Those behaviors are exactly where bugs hide. SurrealGuard's premise is contract-first: every construct has a contract (what the author must mean for the statement to make sense), and the analyzer reports violations of that contract even when the engine would happily execute the query.

SurrealDB's own parser discards spans before producing its AST, so it cannot power an analyzer or editor tooling. SurrealGuard parses with tree-sitter into a typed, span-carrying AST and runs all analysis on that.

## What it does today

- **Full statement coverage** — every SurrealQL statement kind lowers to a typed AST and is analyzed: SELECT (projections, graph traversals, FETCH/SPLIT/GROUP/OMIT), the six mutations, RELATE, LET/RETURN/IF/FOR/blocks, transactions, DEFINE/REMOVE/ALTER, LIVE SELECT/KILL, and the rest.
- **Type inference** — response kinds as upstream `surrealdb_types::Kind` values: closed object literals for known rows, unions from IF/ELSE, record-link and graph-edge shapes, function return types (the full builtin table plus `fn::` declarations), closure and subquery inference, and constant-value evaluation.
- **A contract catalog of ~80 diagnostics** — one code per contract, allocated in families (1xxx schema references, 2xxx types, 3xxx graph, 4xxx statement misuse, 5xxx functions, 6xxx parameters, 7xxx lints, 8xxx version compatibility). The registry lives in `docs/plans/2026-07-07-diagnostic-catalog.md`; severities are intrinsic to each finding, and consumers apply policy (warnings-as-errors, lint levels) at their edge, rustc-style.
- **Parameter constraints for hosts** — every `$param` a source reads is exported with the kind and value domain its uses imply (`UPDATE user SET age = $age` → `age: int`; `LIMIT $n` → non-negative int; `type::field($f)` → one of the table's field paths). Host adapters enforce these at the call site.
- **Byte-precise spans** on every finding, suitable for editor squiggles.

## Surfaces

- **CLI** — `surrealguard check` analyzes the workspace (`--json` for machine output; exit code reflects post-policy errors); `surrealguard init` writes a starter config.
- **LSP** — `surrealguard-lsp` publishes diagnostics over stdio.
- Host adapters (Rust macros, TypeScript, Python, Go) are the next phase, built on the parameter-constraint and response-kind exports. MCP tooling is planned.

## Project structure

```
surrealguard/
├── crates/
│   ├── syntax/        # tree-sitter parsing, typed AST, lowering
│   ├── diagnostics/   # finding types, code catalog, severity policy
│   ├── workspace/     # schema index, analyzers, analysis pipeline
│   ├── cli/           # surrealguard binary
│   └── lsp/           # surrealguard-lsp binary
├── docs/
│   ├── DESIGN.md      # maintained source of truth
│   └── plans/2026-07-07-diagnostic-catalog.md  # the contract registry
└── tree-sitter-surrealql/  # grammar (path dependency, forked)
```

## Getting started

```bash
cargo install --path crates/cli

# in your project
surrealguard init
surrealguard check          # human output
surrealguard check --json   # machine output
```

Point the config at your schema and query directories; the workspace analyzes every `.surql` source in order, so schema definitions are visible to the queries that follow them.

## Status

Under active development on the `redesign-v3-foundation` branch. The core engine (typed AST, full inference, contract diagnostics, parameter constraints) is complete; host adapters and grammar-conformance hardening are in progress. See `docs/DESIGN.md` for the current state and roadmap.

## License

MIT

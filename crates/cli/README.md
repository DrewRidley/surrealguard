# surrealql-analyzer

The command-line interface for [SurrealQL Analyzer](https://github.com/surrealdb/analyzer) —
a static analyzer and type-inference engine for SurrealQL.

```bash
surrealql-analyzer init      # write a starter surrealql-analyzer.toml
surrealql-analyzer check     # analyze schema + queries, report findings (--json for machine output)
surrealql-analyzer generate  # emit the typed TypeScript client + query registry
surrealql-analyzer watch     # check, then regenerate, on every change — run it beside your dev server
```

`check` discovers `.surql` sources through the globs in `surrealql-analyzer.toml`,
splits them into the schema set (DEFINE/REMOVE catalog) and the query set,
runs the analyzer, and reports findings as rustc-style blocks (or JSON). The
exit code reflects post-policy errors, so it drops into CI directly.

`watch` is the development loop: it runs `check` across the whole workspace,
and regenerates the TypeScript registry when — and only when — that check
passes, so a half-typed save never overwrites types that worked. It repaints
the screen on every run, names what changed, and ends in a PASS/WARN/FAIL band.
`--check-only` skips the write; `check --watch` and `generate --watch` still
watch one verb each.

## Colour

Human output is coloured on a terminal and plain everywhere else, so
`surrealql-analyzer check > report.txt` is clean text. `--no-color`, `NO_COLOR` and
`TERM=dumb` each turn it off explicitly. `--json` is a machine contract — one
document, one exit code — and is never decorated.

See the [SurrealQL Analyzer repository](https://github.com/surrealdb/analyzer)
for the full workflow, schema conventions, and configuration reference.

## License

MIT OR Apache-2.0.

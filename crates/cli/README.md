# surrealguard

The command-line interface for [SurrealGuard](https://github.com/DrewRidley/surrealguard) —
a static analyzer and type-inference engine for SurrealQL.

```bash
surrealguard init      # write a starter surrealguard.toml
surrealguard check     # analyze schema + queries, report findings (--json for machine output)
surrealguard generate  # emit the typed TypeScript client + query registry
```

`check` discovers `.surql` sources through the globs in `surrealguard.toml`,
splits them into the schema set (DEFINE/REMOVE catalog) and the query set,
runs the analyzer, and reports findings as rustc-style blocks (or JSON). The
exit code reflects post-policy errors, so it drops into CI directly.

See the [SurrealGuard repository](https://github.com/DrewRidley/surrealguard)
for the full workflow, schema conventions, and configuration reference.

## License

MIT OR Apache-2.0.

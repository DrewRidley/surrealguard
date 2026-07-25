# surrealguard-macros

The compile-time SurrealQL proc-macros behind
[SurrealGuard](https://github.com/DrewRidley/surrealguard).

- `surql!("…")` checks a query at compile time and expands to its text as a
  `&'static str`.
- `query!("…")` checks a query and expands to a `surrealguard_rs::Query<T>`
  whose `T` is the inferred, nameless result type.

Both run the SurrealGuard analyzer against your project schema during
compilation, so an invalid query is a `cargo check` error — no build script and
no language server required.

Most users should depend on
[`surrealguard-rs`](https://crates.io/crates/surrealguard-rs), which re-exports
both macros together with the runtime types (`Query<T>`, `RecordLink<T>`) the
`query!` expansion refers to.

## License

MIT OR Apache-2.0.

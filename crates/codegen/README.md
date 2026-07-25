# surrealguard-codegen

TypeScript type generation from
[SurrealGuard](https://github.com/DrewRidley/surrealguard) analysis results.

Two layers: `ts_type` renders one `surrealdb_types::Kind` as a TypeScript type,
and `render_registry` emits the generated module — a literal-keyed registry
mapping each embedded query to its result type, substitution tuple, and
named-parameter object, plus the `surql` tag and `SurqlQuery` carrier the host
code consumes. Value conventions match the SurrealDB SDK: datetimes are `Date`,
records/uuids/durations are (branded) strings, `NONE` is `undefined`.

The public API is not yet stable; pin an exact version.

## License

MIT OR Apache-2.0.

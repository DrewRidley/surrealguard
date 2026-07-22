# surrealguard-rs

**Compile-time-checked, typed SurrealQL for Rust.** The `query!` macro runs the
[SurrealGuard](https://github.com/DrewRidley/surrealguard) analyzer against your
schema *during compilation* — a wrong table, unknown field, bad arity, or kind
mismatch is a `cargo check` error, and the result type is generated from the
inferred response. No build script, no language server, no runtime schema fetch.

```rust
use surrealguard_rs::query;

let users = query!("SELECT name, age FROM user");
//  users: Query<Vec<{ name: String, age: i64 }>>  ← nameless struct, inferred

let bad = query!("SELECT name, ssn FROM user");
//  error: SurrealGuard rejected this query:
//    [E1002] unknown field `ssn` on table `user`
```

The result type has no user-facing name — you get nested field access without
ever writing a type, the same way sqlx's `query!` returns an anonymous record.

## Schema

The macro resolves your schema at compile time from, in order:

1. the `SURREALGUARD_SCHEMA` environment variable (a `.surql` file or a
   directory), relative to `CARGO_MANIFEST_DIR` unless absolute;
2. otherwise a convention path under the crate root: `schema/`, `migrations/`,
   then `schema.surql`.

A directory contributes every `.surql`/`.surrealql` file, **sorted by name**, so
zero-padded migrations (`0001_*.surql`, `0002_*.surql`) apply in order. Editing a
schema file forces a rebuild. With no schema configured, queries are still
checked for everything that doesn't depend on one (syntax, function arity,
operators, …).

## Macros

- **`query!("…")`** — check *and* return a typed `Query<T>` whose `T` is the
  inferred, nameless result type.
- **`surql!("…")`** — check and expand to the query text as a `&'static str`
  (the lighter form when you only want validation).

## Types

Results derive `serde::Deserialize`. `Kind` maps to Rust as: `datetime` →
`chrono::DateTime<Utc>`, `uuid` → `uuid::Uuid`, `option<T>` → `Option<T>`, arrays
→ `Vec<T>`, closed objects → nested structs, open/`any` → `serde_json::Value`.
Executing a `Query<T>` against a live database uses the official `surrealdb` Rust
SDK.

## License

MIT OR Apache-2.0.

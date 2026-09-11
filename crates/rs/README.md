# surrealql-analyzer-rs

**Compile-time-checked, typed SurrealQL for Rust.** The `query!` macro runs the
[SurrealQL Analyzer](https://github.com/surrealdb/analyzer) analyzer against your
schema *during compilation* — a wrong table, unknown field, bad arity, or kind
mismatch is a `cargo check` error, the result type is generated from the
inferred response, and the query's parameters are checked against the kinds
their uses imply. No build script, no language server, no runtime schema fetch.

```rust
use surrealql_analyzer_rs::query;

let adults = query!("SELECT name, age FROM user WHERE age >= $min", min = 18)
    .fetch_all(&db)
    .await?;

for row in adults {
    println!("{} is {}", row.name, row.age);   // String, i64 — inferred
}
```

The result type has no user-facing name — you get nested field access without
ever writing a type, the same way sqlx's `query!` returns an anonymous record.

```rust
let bad = query!("SELECT name, ssn FROM user");
//  error: SurrealQL Analyzer rejected this query:
//    [E1002] `user` has no field `ssn`
```

## Executing

| method            | returns            | available when                      |
| ----------------- | ------------------ | ----------------------------------- |
| `fetch`           | `T`                | always — `T` is the analyzed shape  |
| `fetch_all`       | `Vec<T::Row>`      | the query yields a row set          |
| `fetch_one`       | `T::Row`           | the query yields a row set          |
| `fetch_optional`  | `Option<T::Row>`   | the query yields a row set          |
| `execute`         | `()`               | always                              |

Each takes the connection as an argument (`&Surreal<C>`, any engine), so one
`Query<T>` value can be run against whatever connection is in hand.

`fetch` is the shape-preserving verb: it returns exactly what the analyzer
inferred — a `Vec` for a plain `SELECT`, an `Option` for `SELECT … FROM ONLY`,
a scalar for `RETURN`, a tuple for several statements. The row-set verbs are
gated on the `Rows` trait, so asking for rows from a query that has none is a
compile error rather than a runtime one:

```rust
query!("RETURN 1 + 1").fetch_all(&db).await?;
//  error[E0277]: this query does not return a set of rows
//    = note: use `.fetch(&db)` to get the value this query actually returns
```

This is a guarantee the SDK cannot give on its own: `take::<Option<T>>(0)`
against a multi-row `SELECT` is a runtime error there, because nothing knows the
query's cardinality until it runs. Here it is known at compile time.

## Parameters

Parameters are supplied by name and checked against the kind the analyzer
inferred from their use sites. There is no unchecked bind — every one of these
is a compile error:

```rust
query!("SELECT name FROM user WHERE age > $min", min = 18)     // ok
query!("SELECT name FROM user WHERE age > $min", min = "18")   // wrong type
query!("SELECT name FROM user WHERE age > $min")               // missing `min`
query!("SELECT name FROM user", limit = 10)                    // no such parameter
```

The value is pinned through `Into<T>`, so the obvious literals work (`&str` for
a `string` parameter, an integer literal for an `int`) while unrelated types are
rejected. When the analyzer cannot infer a parameter's kind, the bind accepts
any `SurrealValue` — an uninferred parameter is honestly unchecked rather than
falsely narrowed.

## Multiple statements

When more than one statement responds, `T` is a tuple of their types in source
order. Statements that do not respond (a bare `LET`, a `DEFINE`) still occupy a
slot in the SDK's response, and that is accounted for, so the tuple lines up
with what you wrote:

```rust
let (users, posts) = query!("LET $n = 1; SELECT name FROM user; SELECT title FROM post;")
    .fetch(&db)
    .await?;
```

## Macros

- **`query!("…", name = value, …)`** — check *and* return a typed `Query<T>`.
- **`query_file!("path.surql", name = value, …)`** — the same, with the
  SurrealQL read from a file at compile time. The path is relative to the crate
  root (as `sqlx::query_file!` does, unlike `include_str!`).
- **`surql!("…")`** — check and expand to the query text as a `&'static str`
  (the lighter form when you only want validation).

## Schema

The macros resolve your schema at compile time from, in order:

1. the `SURREALQL_ANALYZER_SCHEMA` environment variable (a `.surql` file or a
   directory), relative to `CARGO_MANIFEST_DIR` unless absolute;
2. otherwise a convention path under the crate root: `schema/`, `migrations/`,
   then `schema.surql`.

A directory contributes every `.surql`/`.surrealql` file, **sorted by name**, so
zero-padded migrations (`0001_*.surql`, `0002_*.surql`) apply in order. Editing a
schema file forces a rebuild. With no schema configured, queries are still
checked for everything that doesn't depend on one (syntax, function arity,
operators, …).

## Types

Generated types implement `surrealdb_types::SurrealValue` — which is what the
SDK's `take` requires, *not* `serde::Deserialize`. The SDK coerces nothing
between value variants, so each mapping is the exact type the server's value
decodes into; an approximate one would compile and then fail at runtime.

| SurrealQL kind      | Rust type                    |
| ------------------- | ---------------------------- |
| `bool`              | `bool`                       |
| `int`               | `i64`                        |
| `float`             | `f64`                        |
| `decimal`           | `Decimal`                    |
| `number`            | `Number`                     |
| `string`            | `String`                     |
| `datetime`          | `Datetime`                   |
| `duration`          | `Duration`                   |
| `uuid`              | `Uuid`                       |
| `bytes`             | `Bytes`                      |
| `regex`             | `Regex`                      |
| `record<t>`         | `RecordId`                   |
| `geometry`          | `Geometry`                   |
| `option<T>`         | `Option<T>`                  |
| `array<T>` / `set`  | `Vec<T>`                     |
| closed `object`     | a nested, nameless struct    |
| `any` / open object | `Value`                      |

Two decoding details the generated code handles that the SDK's own derive does
not: an **absent** field decodes into an `Option<T>` as `None` (the server omits
`NONE`-valued fields), and a **`NULL`** field does too (the SDK's `Option<T>`
accepts only `Value::None`). A decode failure names the field.

## Errors

`surrealql_analyzer_rs::Error` carries the text of the query that failed, which
neither the SDK's error nor `sqlx::Error` does:

```text
expected at least one row, got none
  in: SELECT name FROM user WHERE age > $min;
```

`ErrorKind` distinguishes `Database`, `Decode { statement, source }` and
`RowNotFound`.

## Feature flags

- **`runtime`** (default) — pulls in the `surrealdb` SDK and enables `fetch*` /
  `execute`. Turn it off (`default-features = false`) for checking and typing
  with no SDK dependency; `Query::decode` still decodes a response obtained
  another way.

The SDK is depended on with `default-features = false`, so this crate never
forces an engine or protocol on you — your own `surrealdb` dependency picks
those and Cargo unifies the two.

## Tests

`cargo test` runs the typing and decoding tests, which need no database. The
execution tests are `#[ignore]`d because they need a server:

```sh
surreal start --user root --pass root --bind 127.0.0.1:8111 memory &
cargo test -p surrealql-analyzer-rs -- --ignored
```

## Known limitation

The `dec` numeric suffix (`9.99dec`) does not currently parse inside a `SET`
clause, and `RETURN 9.99dec` infers `float` rather than `decimal`. Use a cast —
`<decimal> 9.99` — until the grammar covers it.

## License

MIT OR Apache-2.0.

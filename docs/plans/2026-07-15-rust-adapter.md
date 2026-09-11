# SurrealQL Analyzer Rust Adapter — Design

Status: Phases 1–3 and 4a built and green (2026-07-17); 4b (live execution) pending a
dependency decision. Companion to the TypeScript runtime design
(`2026-07-15-typescript-runtime.md`).

## Why Rust is the best-fit host

Rust has real compile-time macros, so it collapses the *entire* TypeScript toolchain —
`surrealql-analyzer generate`, the `.d.ts` registry, and even the separate LSP-for-inline-errors
— into a single proc-macro. `surrealql-analyzer-workspace` is already a plain callable library
(`analyze_query`, `analyze_source`), so a `surql!` macro can run the real analyzer during
`cargo check` and turn findings into `compile_error!`s. **The compiler is the checker.**

The mental anchor is sqlx's `query!` — compile-time-checked queries generating an
anonymous result struct — but SurrealQL Analyzer checks *statically* against the analyzer + your
`.surql` schema, with **no live database connection at build time** (strictly better than
sqlx). Types stay structural/per-query and macro-generated (un-nameable, like sqlx's
records) — never hand-named models.

## Layering

- **`surrealql-analyzer-macros`** (`crates/macros`) — the `surql!` proc-macro. General-purpose,
  framework-agnostic. Compile-time check → spanned errors; result-struct generation.
- **`surrealql-analyzer-rs`** (planned) — client glue: `db.query(surql!(…))` typed execution
  over the official `surrealdb` crate. Framework-agnostic.
- **`surrealql-analyzer-dioxus`** (planned) — reactive integration as its own crate (not a
  feature of core), so the core stays UI-agnostic: one-shot → `use_resource`; live →
  `Signal<Vec<Row>>` fed by the SDK notification stream, reconciled by `id`, dropped on
  unmount. Bevy is the same story via a system draining the stream into components.

## Phases

1. **Compile-time checking — DONE.** `surql!("…")` parses the string literal, runs
   `analyze_query` on it, and emits a `compile_error!` (spanned at the literal) for every
   error-severity finding; valid queries expand to the query text as `&'static str`.
   Verified: `surql!("RETURN string::len();")` → `error: [E5002] string::len expects 1
   argument, found 0`; `surql!("SELECT * FROM ;")` → `error: [S0001] syntax error`; valid
   queries compile. Schemaless today, which already covers syntax/function/arity/operator
   and every other schema-independent contract. fmt/clippy clean.
2. **Result-struct generation — `query!("…")`, nameless by default. [BUILT — `crates/macros`,
   `generate.rs`; tests green.]** The macro maps the
   inferred response `Kind` → `#[derive(serde::Deserialize)]` structs defined **inside the
   expansion's own block scope**, and returns the value. The type escapes; its name does
   not — so the user gets full (nested) field access and never writes a type name:
   ```rust
   let users = query!("SELECT name, address.city FROM user");
   for u in &users { println!("{} {}", u.name, u.address.city); }
   ```
   Verified against rustc: block-local structs escape with nested field access. No `Name`
   argument, no per-query module, no `build_query!`. This is the primary surface.
   Kind→Rust scalars: `Datetime`→`chrono::DateTime<Utc>`, `Duration`→`std::time::Duration`,
   `Uuid`→`uuid::Uuid`, `Bytes`→`Vec<u8>`, integer→`i64`, float→`f32`/`f64`,
   `option<T>`→`Option<T>`, array→`Vec<T>`, record link→`RecordLink<…>`, `Either`→a
   generated (block-local) enum. Nested objects → nested block-local structs.

   **Surrealix reality check:** the precursor (`DrewRidley/surrealix`) only ever *implemented*
   the **named** `build_query!(Name, "…")` form (semantic naming: root=table name `User`,
   nested=parent-path-composed `UserAddress`, deduped, per-query module). Its `query/`
   module is empty stubs — the nameless `query!` was only ever commented sketches. So the
   semantic-naming scheme is NOT for the default nameless path; keep it in reserve only for:

   - **Optional named form (escape hatch), when you must name the type in a signature** —
     e.g. a Dioxus `#[derive(Props)]` field or a `fn foo(x: UserAddress)`. A nameless type
     cannot be written in a signature, so this is the one case that needs names. There, use
     the Surrealix scheme (semantic names from field lineage) so sub-structs are reusable
     "without scope creep". **ADAPTATION:** Surrealix read `field.meta.original_path` off a
     custom `TypeAST`; SurrealQL Analyzer uses upstream `Kind` (no lineage) and must NOT
     reintroduce a type hierarchy — reconstruct paths from the query's **projection idioms +
     FROM table** in the lowered AST (`SELECT address.city FROM user` spells
     `user`→`address`→`city`).
3. **Schema awareness. [BUILT — `crates/macros/src/schema.rs`; verified.]** Resolves the
   project schema at compile time (`schema::load`): the `SURREALQL_ANALYZER_SCHEMA` env var (file
   or directory, relative to `CARGO_MANIFEST_DIR` unless absolute), else convention paths
   `schema/`, `migrations/`, then `schema.surql`. A directory contributes every
   `.surql`/`.surrealql` file **sorted by path**, so zero-padded migrations apply in order;
   sources are added as `file://…` (which sort before the `virtual://` query, so schema is
   analyzed first) and run through `analyze_workspace`. Each schema file is emitted as an
   `include_bytes!` in the expansion so editing the schema forces a rebuild (proc-macro
   output is otherwise only rebuilt when the call site changes). Nothing found → schemaless.
   Verified: a two-file migrations dir where `age` is defined only in `0002_*.surql` types
   `SELECT name, age FROM user` correctly (in-order load); `SELECT name, ssn FROM user` →
   compile error `[E1002] unknown field ssn on table user`; without the env var the same
   query correctly fails `[E1001] unknown table user`.
4. **Runtime crate `surrealql-analyzer-rs` + execution.**

   - **4a — runtime crate + typed results. [BUILT — `crates/rs`; tests green.]** New crate
     re-exports the macros and holds the types the generated code refers to (so a user crate
     needs only `surrealql-analyzer-rs`): a `_rt` module re-exporting `serde`/`serde_json`/`chrono`/
     `uuid`; `Query<T>` (carries the validated text + the inferred `T`); `RecordLink<T>`
     scaffolding. `query!("…")` now expands to `::surrealql_analyzer_rs::Query::<T>::new(text)` with
     `T` the nameless result struct, `#[derive(serde::Deserialize)]` via the re-exported serde
     (`#[serde(crate = "::surrealql_analyzer_rs::_rt::serde")]`). Real Kind→Rust mappings:
     `Datetime`→`chrono::DateTime<Utc>`, `Uuid`→`uuid::Uuid`, open/`any`/geometry/range→
     `serde_json::Value`, else as before. Verified via `Query::<T>::from_json`: object/nested/
     array queries deserialize real JSON into the nameless typed struct with correct values
     (`row.name: String`, `row.user.name`, `Vec<row>`). The query macro tests moved to
     `crates/rs/tests` (they need the runtime crate); `surql!` tests stay in `crates/macros`.

   - **4b — live execution. [DEFERRED by decision; recipe proven via kv-mem spike.]** Drew's
     call: keep the `surrealdb` client OUT of the shipped crate for now (WASM/Bevy dep weight)
     — `surrealql-analyzer-rs` stays the type+checking layer. But a throwaway `kv-mem` spike proved
     the full chain works: `query!("SELECT name, age FROM user")` → checked against
     `schema.surql` at compile time → executed on an embedded `Surreal<Mem>` → deserialized
     into the nameless struct → `[("ada", 42), ("lin", 30)]`.

     **Proven recipe** (for when execution lands, behind an `execute` feature or a downstream
     crate):
     ```rust
     async fn run<T: DeserializeOwned, C: Connection>(q: &Query<T>, db: &Surreal<C>) -> Result<T> {
         let mut resp = db.query(q.text).await?;
         let value: surrealdb::types::Value = resp.take(0usize)?;   // note: 0usize
         let json = value.into_json_value();                       // clean serde_json::Value
         Ok(serde_json::from_value(json)?)
     }
     ```
     **Gotchas found:** SurrealDB 3.x extracts via its own `SurrealValue` trait, not serde, so
     `take::<Vec<OurStruct>>` won't compile; and `Value`'s *plain* serde output is its internal
     enum-tagged form (`{"Array":[…]}`), unusable. The fix is `Value::into_json_value()` →
     clean JSON → `serde_json::from_value::<T>` — which lets our existing serde-derived,
     nameless result structs decode directly, no `SurrealValue` derive needed. The nameless
     `T` threads through `Query<T>` into `from_value::<T>`. `Value` path is `surrealdb::types::Value`
     (`surrealdb` re-exports `surrealdb_types as types`).
5. **`surrealql-analyzer-dioxus`.** Reactive signals + live-as-`Signal` with subscribe/KILL tied
   to component lifecycle.

## Known limitations

- **Spans**: on stable Rust the error span covers the whole string literal (the message
  carries each finding's code + text). Precise sub-literal spans need the unstable
  `proc_macro_span` API — a nightly-gated enhancement, not required for correctness.
- **Build cost**: the macro pulls `surrealql-analyzer-workspace` (and thus tree-sitter) in as a
  build dependency and runs analysis per invocation. Analysis is fast (one query); this is
  the same shape as sqlx-macros and is acceptable.

## Rollout note

`crates/macros` depends on `surrealql-analyzer-workspace` by `{ path, version }` like the other
crates, so it inherits the same crates.io gate (the grammar must be published first).

# @surrealguard/client

Typed SurrealQL client. `SurrealGuardClient` **extends** the official
[`surrealdb`](https://www.npmjs.com/package/surrealdb) SDK's `Surreal` class, so
you get every SDK method plus typed queries. A string-literal query passed to
`query` resolves its result and parameter types from the declarations
`surrealguard generate` emits — no wrapper syntax, no manual types.

```ts
// One import: the generated file re-exports a ready-to-use client and loads
// the typed query registry.
import { SurrealGuardClient } from "./surrealguard.generated";

const db = new SurrealGuardClient();
await db.connect("ws://localhost:8000/rpc");
await db.use({ namespace: "app", database: "app" });

// SurrealDB returns one result per statement, so destructure the first
// statement's result. Params are required exactly when the query reads them.
const [users] = await db.query("SELECT name FROM user WHERE team = $team", { team: "red" });
users[0].name; // string
```

## Live queries

`db.live(...)` describes a live query, returning a `LiveDescriptor<Row>` the
framework adapters (`@surrealguard/svelte`, `@surrealguard/next`) consume. The
row type is inferred from the same registry as `query`:

```ts
// Note the parentheses: the query is an argument, not a *tagged* template —
// TypeScript widens a tagged template's text to `string`, which would lose the
// row type. `db.live(`…`)` and `db.live("…")` are both fully typed.
const users = db.live(`SELECT * FROM user`);
users.sql; // "LIVE SELECT * FROM user"  (the `LIVE` prefix is added for you)
```

A registered query resolves a typed row; a non-registered one degrades to
`unknown` (never `any`). The SDK's own `db.live(table)` subscription still works.

`surrealdb` is a peer dependency — install it alongside this package. Dynamic
(non-literal) strings resolve to the SDK's `unknown[]` and still run. The typed
form depends on `surrealguard generate` output, which augments the
`SurqlRegistry` interface in this package.

Part of [SurrealGuard](https://github.com/DrewRidley/surrealguard). Licensed
under MIT OR Apache-2.0.

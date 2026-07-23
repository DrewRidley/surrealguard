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

`surrealdb` is a peer dependency — install it alongside this package. Dynamic
(non-literal) strings resolve to the SDK's `unknown[]` and still run. The typed
form depends on `surrealguard generate` output, which augments the
`SurqlRegistry` interface in this package.

Part of [SurrealGuard](https://github.com/DrewRidley/surrealguard). Licensed
under MIT OR Apache-2.0.

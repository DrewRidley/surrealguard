# @surrealguard/client

Typed SurrealQL client. A string-literal query passed to `query` resolves its
result and parameter types from the declarations `surrealguard generate` emits —
no wrapper syntax, no manual types.

```ts
import { SurrealGuardClient, fromSurreal } from "@surrealguard/client";
import Surreal from "surrealdb";
// import "./surrealguard.generated"; // emitted by `surrealguard generate`

const surreal = new Surreal();
await surreal.connect("ws://localhost:8000");
const db = new SurrealGuardClient(fromSurreal(surreal));

// Result type inferred; params required exactly when the query reads them.
const users = await db.query("SELECT name FROM user WHERE team = $team", { team: "red" });
users[0].name; // string
```

Dynamic (non-literal) strings resolve to `unknown` and still run. The typed form
depends on `surrealguard generate` output, which augments the `SurqlRegistry`
interface in this package.

Part of [SurrealGuard](https://github.com/DrewRidley/surrealguard). Licensed
under MIT OR Apache-2.0.

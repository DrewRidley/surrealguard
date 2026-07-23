# @surrealguard/query

Framework-agnostic reactive core for SurrealGuard. Wraps a
[`@surrealguard/client`](https://www.npmjs.com/package/@surrealguard/client)
with a cache, reference-counted subscriptions, live-query reconciliation, and
SSR hydration. The framework packages (`@surrealguard/next`,
`@surrealguard/svelte`) are thin bindings over what it returns.

```ts
import { QueryClient } from "@surrealguard/query";
import { SurrealGuardClient } from "./surrealguard.generated";

const db = new SurrealGuardClient();
await db.connect("ws://localhost:8000/rpc");
const qc = new QueryClient(db);

// A plain query resolves once; a LIVE query keeps updating in place.
const observable = qc.observe("LIVE SELECT * FROM user", { initialData });
observable.subscribe((state) => render(state.data));
```

- **Dedup:** N subscribers to the same `(sql, params)` share one subscription.
- **Live reconciliation:** change notifications applied by record `id`
  (CREATE → append, UPDATE → replace, DELETE → remove).
- **SSR:** `dehydrate()` on the server, `hydrate()` on the client — no refetch flash.

Part of [SurrealGuard](https://github.com/DrewRidley/surrealguard). Licensed
under MIT OR Apache-2.0.

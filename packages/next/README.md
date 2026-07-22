# @surrealguard/next

Next.js / React bindings for SurrealGuard. Fetch typed data in a Server
Component and hand it to a client component's `useLiveQuery`, which seeds from
that data (no hydration gap), subscribes on mount, reconciles live
notifications, and releases the subscription on unmount.

```tsx
"use client";
import { useLiveQuery } from "@surrealguard/next";

export function Users({ qc, initialData }) {
  const { data, status } = useLiveQuery(qc, "LIVE SELECT * FROM user", { initialData });
  if (status === "loading") return <Spinner />;
  return <ul>{data.map((u) => <li key={String(u.id)}>{u.name}</li>)}</ul>;
}
```

Built on [`@surrealguard/query`](https://www.npmjs.com/package/@surrealguard/query).
`react >= 18` is a peer dependency.

Part of [SurrealGuard](https://github.com/DrewRidley/surrealguard). Licensed
under MIT OR Apache-2.0.

# @surrealguard/next

Next.js / React bindings for SurrealGuard. Provide the typed client via context,
then call `useLiveQuery` in a client component — it returns `{ data, status,
error }` backed by `useSyncExternalStore`. A `LIVE SELECT …` keeps `data`
reconciled as notifications arrive; the (reference-counted) subscription is
released on unmount.

```tsx
// app/providers.tsx
"use client";
import { SurrealGuardProvider } from "@surrealguard/next";
import { db } from "@/lib/db";
export function Providers({ children }: { children: React.ReactNode }) {
  return <SurrealGuardProvider client={db}>{children}</SurrealGuardProvider>;
}
```

```tsx
// app/users/users-list.tsx
"use client";
import { useLiveQuery } from "@surrealguard/next";
export function UsersList({ initialData }: { initialData?: User[] }) {
  // `db` comes from context; the row type is inferred from the query.
  const { data, status } = useLiveQuery((db) => db.live(`SELECT * FROM user`), { initialData });
  if (status === "loading") return <Spinner />;
  return <ul>{data.map((u) => <li key={String(u.id)}>{u.name}</li>)}</ul>;
}
```

`db.live(...)` takes the query as an argument (note the parentheses) so its
literal type — and thus the row type — is preserved. `data` is fully typed for a
registered query and `unknown[]` for a non-registered one (never `any[]`).

## SSR / RSC: seed on the server, go live on the client

`@surrealguard/next/server` is server-safe (no `"use client"`). `queryServer`
runs the underlying `SELECT` once; pass the rows to `useLiveQuery`'s `initialData`.

```tsx
// app/users/page.tsx  (Server Component)
import { queryServer } from "@surrealguard/next/server";
import { db } from "@/lib/db";
import { UsersList } from "./users-list";
export default async function Page() {
  const users = await queryServer(db, db.live(`SELECT * FROM user`));
  return <UsersList initialData={users} />;
}
```

For whole-cache transport use `dehydrate(db)` on the server and `hydrate(db, state)`
on the client. Pass an explicit client with `useLiveQuery(fn, { client })` to
bypass context.

Built on [`@surrealguard/query`](https://www.npmjs.com/package/@surrealguard/query).
`react >= 18` is a peer dependency.

Part of [SurrealGuard](https://github.com/DrewRidley/surrealguard). Licensed
under MIT OR Apache-2.0.

# @surrealguard/query

The framework-agnostic reactive core behind
[`@surrealguard/svelte`](https://www.npmjs.com/package/@surrealguard/svelte) and
[`@surrealguard/next`](https://www.npmjs.com/package/@surrealguard/next). It
wraps a [`@surrealguard/client`](https://www.npmjs.com/package/@surrealguard/client)
with a result cache, reference-counted subscriptions, live-query reconciliation,
and SSR hydration.

Use it directly when you are binding SurrealGuard to a framework it has no
adapter for, or when you want the cache without any framework at all. If you are
on Svelte or Next, use those packages instead — they are thin wrappers over what
this returns.

## Install

```sh
npm i @surrealguard/query @surrealguard/client surrealdb
npm i -D surrealguard
```

## Generate the types first

The row types come from `surrealguard generate`, which analyzes the queries in
your source against your `.surql` schema and writes a module that re-exports the
client along with a registry keyed by each query's exact text. See
[`@surrealguard/client`](https://www.npmjs.com/package/@surrealguard/client) for
the full setup; the short version is:

```sh
npx surrealguard init                                    # writes surrealguard.toml
npx surrealguard generate --out src/surrealguard.generated.ts
```

Re-run it whenever a query or the schema changes. `generate` and `check` support
a watch mode (`--watch`) that stays running and regenerates on save;
`surrealguard generate --help` lists the flags your installed version has.

## Use

```ts
import { getQueryClient } from "@surrealguard/query";
import { SurrealGuardClient } from "./surrealguard.generated";

const db = new SurrealGuardClient();
await db.connect("ws://localhost:8000/rpc");
await db.use({ namespace: "app", database: "app" });

const qc = getQueryClient(db);

// `db.live(...)` returns a typed descriptor, so `state.data` is Array<{ name; age }>.
const people = qc.observeLive(db.live(`SELECT name, age FROM person`));

const unsubscribe = people.subscribe((state) => {
  if (state.status === "error") console.error(state.error);
  else for (const person of state.data) console.log(person.name, person.age);
});
```

`subscribe` delivers the current state synchronously, then again on every change,
and returns an unsubscribe function. The first subscriber starts the query; the
last one to leave tears the live subscription down. The cached rows survive that
teardown, so re-subscribing renders immediately.

`getQueryClient(db)` returns **one** `QueryClient` per underlying connection,
cached in a `WeakMap`. That matters: the reference counting that lets N
subscribers share one `LIVE SELECT` only works if everyone resolves the same
core. `new QueryClient(db)` is exported too, but constructing a second one for
the same connection quietly opens a second subscription for every query.

## What it does

- **Dedup** — the cache key is `(sql, params)`, with params serialised in sorted
  key order so two callers writing the same bindings in a different order still
  share an entry.
- **Live reconciliation** — a `LIVE SELECT` resolves to a live-query id, which
  the core subscribes to; each notification is applied to the array by record
  `id` (CREATE appends, UPDATE replaces in place, DELETE removes).
- **SSR** — `prime(descriptor)` runs a live query's underlying `SELECT` once on
  the server and caches the rows under the *live* key, so the client's first
  render is gap-free and then upgrades to live. `dehydrate()` / `hydrate()` move
  the whole cache across the boundary.

## API

| Method | What it does |
| --- | --- |
| `getQueryClient(db)` | The `QueryClient` for a connection. Prefer this over `new QueryClient(db)`. |
| ``qc.observeLive(db.live(`…`), opts?)`` | Typed reactive handle. The entry point adapters use. |
| `qc.observe(sql, opts?)` | Same, for a raw SQL string. Rows are untyped — no registry lookup. |
| `qc.fetch(sql, params?)` | One-shot query, cached, outside the reactive layer. |
| `qc.prime(descriptor, params?)` | Run a live query's `SELECT` once and cache under the live key (SSR seed). |
| `qc.dehydrate()` / `qc.hydrate(state)` | Snapshot / restore the cache. |

`observe` and `observeLive` take `{ params, initialData }`; `initialData` seeds
the entry so `status` starts at `"success"` instead of `"loading"`.

Part of [SurrealGuard](https://github.com/DrewRidley/surrealguard). Licensed
under MIT OR Apache-2.0.

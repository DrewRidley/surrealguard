# @surrealguard/client

A typed SurrealQL client. You write real SurrealQL; `surrealguard generate`
analyses it against your schema and emits the types.

```ts
const people = await db.run(allPeople);
//    ^? Array<{ id: RecordId<"person">; name: string; age: number }>
```

No query builder, no second language, no manual `db.query<Person[]>(...)` cast.

## How it works

`surrealguard generate` scans your source for query text, analyses each query
against your `.surql` schema, and writes one file:

```ts
declare module "@surrealguard/client" {
  interface SurqlRegistry {
    "SELECT id, name FROM person WHERE team = $team": {
      result: [Array<{ id: RecordId<"person">; name: string }>];
      params: { team: RecordId<"team"> };
    };
  }
}
```

A registry keyed by *exact query text*, read by one conditional generic. That is
the whole mechanism. Two rules keep it honest:

- **There is no permissive `string` overload.** A literal is also a `string`, so
  a fallback overload would rescue every mis-call into `unknown`. There is none.
- **A miss degrades to `unknown`, never `any`.** Asserted in `test-d/`.

## Install

```sh
npm install @surrealguard/client surrealdb
```

`surrealguard.toml`:

```toml
[sources]
schema  = ["schema/**/*.surql"]
queries = ["queries/**/*.surql"]
```

```sh
surrealguard generate --out src/lib/surrealguard.generated.ts
```

## A query is a value

Query text lives in exactly one place:

```ts
// src/lib/queries.ts
import { defineQuery, defineLive } from "./surrealguard.generated";

export const allPeople  = defineQuery("SELECT id, name, age FROM person");
export const peopleOf   = defineQuery("SELECT id, name FROM person WHERE team = $team");
export const addPerson  = defineQuery("CREATE person SET name = $name, joined = $joined");
export const livePeople = defineLive("SELECT id, name, age FROM person");
export const liveTeam   = defineLive("SELECT id, name FROM person WHERE team = $team");
```

`defineQuery` infers the string literal and reads the registry exactly as
`db.query` does — same guarantee, same single conditional generic. The gain is
that the literal is written *once*, so an SSR seed and the component that
consumes it cannot drift apart byte-for-byte and silently miss the cache.

A query text the registry does not contain is a **hard compile error**, because
it means exactly one thing — the generated file is stale:

```
TS2345: Argument of type 'SurqlError<"this query is not in the generated
  registry - run `surrealguard generate`">' is not assignable to …
```

That also catches the case where reformatting a query changed its key.
`defineQuery.unchecked("…")` opts out and degrades to `unknown[]`.

## The client

```ts
// src/lib/db.ts
import { createClient } from "./surrealguard.generated";

export const db = createClient({
  url: "ws://localhost:8000/rpc",
  namespace: "app",
  database: "app",
});
```

The connection opens **lazily**, on first use, so a module-level `db` is safe and
no route has to remember to `await db.connect(...)`.

```ts
// One-shot. Single-statement queries resolve to their rows — no destructure.
const people = await db.run(allPeople);
const red    = await db.run(peopleOf, { team: new RecordId("team", "red") });

// Multi-statement queries keep the per-statement tuple. Nothing is hidden.
const [names, ages] = await db.run(twoStatements);

// A scalar result is a scalar.
const count = await db.run(peopleCount);   // ^? number

// Live, in vanilla JS, with no other package.
const stop = db.watch(livePeople, (rows) => render(rows));
//    ^? rows: Array<{ id: RecordId<"person">; name: string; age: number }>

// Writes, and telling the reactive layer about them.
await db.run(addPerson, { name: "ada", joined: new Date() });
await db.invalidate(allPeople);
await db.invalidate(peopleOf.with({ team }));   // just that binding
```

Params are required exactly when the query reads them, and forbidden when it
does not:

```ts
await db.run(peopleOf);                      // TS2554: missing params
await db.run(allPeople, { team });           // TS2554: this query takes none
await db.run(peopleOf, { team: "team:red" }); // TS2322: string is not RecordId
```

## Values are the SDK's values

This is the part most likely to surprise you, and it is deliberate. The SDK
decodes SurrealQL values as **its own classes**, so that is what the generated
types say:

| SurrealQL | TypeScript |
| --- | --- |
| `record<team>` | `RecordId<"team">` |
| `datetime` | `Date` |
| `duration` | `Duration` |
| `uuid` | `Uuid` |
| `decimal` | `Decimal` |

`row.id` is a `RecordId`, not a string — `row.id.startsWith(...)` is a compile
error rather than a runtime one. `datetime` is a native `Date` because
`createClient` sets `codecOptions.useNativeDates`.

It matters more for **writing** than for reading. A `RecordId` parameter encodes
to a record link on the wire; a plain string encodes to a SurrealQL string:

```
encode(new RecordId("team","red"))  ->  c8 82 …   (CBOR tag 8: a record link)
encode("team:red")                  ->  68 …      (an untagged text string)
```

So `WHERE team = $team` matches with the class and returns nothing without it.

Construct params from the generated import:

```ts
import { RecordId } from "./surrealguard.generated";
await db.run(peopleOf, { team: new RecordId("team", "red") });
```

### Crossing a serialisation boundary

Class instances do not survive SvelteKit's `load` (devalue) or a React Server
Component's props. `Json<T>` is the projection that does — it is the SDK's own
`Jsonify`, so the mapping is theirs:

```ts
const rows = await db.runJson(allPeople);
//    ^? Array<{ id: `person:${string}`; name: string; age: number }>
```

`db.runJson`, `preload`, and the whole reactive layer (`@surrealguard/query`,
`@surrealguard/svelte`, `@surrealguard/next`) are `Json<T>`-shaped for this
reason. `db.run` is not.

## Errors

```ts
import { SurrealGuardError } from "./surrealguard.generated";

try {
  await db.run(peopleOf, { team });
} catch (error) {
  if (error instanceof SurrealGuardError) {
    console.error(error.query, error.params);
    // The SDK's own typed error is kept, not flattened:
    if (error.cause instanceof AuthenticationError) redirectToLogin();
  }
}
```

## Escape hatches

```ts
db.surreal                                   // the raw SDK instance
db.surreal.query(surql`SELECT * FROM ${t}`)  // fully dynamic, SDK-typed
await db.query("SELECT name FROM person")    // literal form -> the statement tuple
fromSurreal(existingSurreal)                 // wrap a connection you already own
```

`db.query(text, params)` is the lower-level form: it returns the full
per-statement tuple and never unwraps. `db.run` is the same guarantee one level
up.

If you use `fromSurreal`, construct the `Surreal` with
`codecOptions: { useNativeDates: true }` — that is the one thing `createClient`
does for you which cannot be recovered afterwards.

## Why compose the SDK instead of extending it

`class SurrealGuardClient extends Surreal` does not compile. The SDK already owns
`run` (RPC function invocation), `subscribe` (the event emitter) and
`invalidate` — and `Surreal.invalidate()` *logs the session out*. tsc reports
TS2416 on each. So the SDK instance lives at `db.surreal`, the session methods
worth having (`use`/`signin`/`signup`/`authenticate`/`close`) are forwarded, and
the data vocabulary is ours.

## API

| Export | |
| --- | --- |
| `createClient(options)` | the client; connects lazily |
| `fromSurreal(surreal)` | wrap a `Surreal` you already own |
| `defineQuery(text)` / `defineLive(text)` | name a query; `.unchecked` opts out of the registry |
| `db.run` / `db.runJson` / `db.runLiveOnce` | execute |
| `db.watch(live, onRows, onError?)` | subscribe; returns an unsubscribe |
| `db.invalidate(...queries)` | tell the reactive layer a write happened |
| `db.query(text, params?)` | the literal form; per-statement tuple |
| `preload(db, query)` | server-fetched, serialisable, self-describing |
| `SurrealGuardError` | `{ query, params, cause }` |
| `RecordId` / `Uuid` / `Duration` / `Decimal` | the SDK value classes |
| `Json<T>` / `Rows<R>` / `SurqlQuery` / `SurqlLive` / `Preloaded<T>` | types |

## Licence

MIT OR Apache-2.0

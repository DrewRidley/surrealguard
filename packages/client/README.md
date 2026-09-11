# @surrealdb/analyzer-client

A typed SurrealQL client. You write the SurrealQL you already know;
`surrealql-analyzer generate` analyses it against your schema and types the result.

```ts
const [people] = await db.query("SELECT id, name, age, team FROM person");
//     ^? Array<{ age: number; id: RecordId<"person">; name: string; team: RecordId<"team"> }>
```

That is the API. No query builder, no second language, no `db.query<Person[]>(…)`
cast, and nothing to wrap the string in. `person.nope` is a compile error;
`WHERE team = $team` demands a `RecordId<"team">` param and refuses a string.

## Setup

Three steps. Skip any one of them and you get `any` with no error, so none of
them are optional — see [When everything is `any`](#when-everything-is-any).

**1. Install.** `@surrealdb/analyzer-client` is a real runtime dependency *and* the
module the generated file augments by name. `surrealdb` is its peer.

```sh
npm install @surrealdb/analyzer-client surrealdb
npm install -D surrealql-analyzer typescript
```

**2. Point it at your schema.** `npx surrealql-analyzer init` writes a commented
`surrealql-analyzer.toml`; the part that matters is:

```toml
[sources]
schema  = ["schema/**/*.surql"]
queries = ["queries/**/*.surql"]
```

```surql
-- schema/schema.surql
DEFINE TABLE team SCHEMAFULL;
DEFINE FIELD name ON team TYPE string;

DEFINE TABLE person SCHEMAFULL;
DEFINE FIELD name ON person TYPE string;
DEFINE FIELD age  ON person TYPE int;
DEFINE FIELD team ON person TYPE record<team>;
```

**3. Generate — with `--out`.** Bare `generate` writes to the *workspace root*,
which is almost never where your imports point. Pass the path that matches how
you import it:

| Project | Command | Import as |
| --- | --- | --- |
| Vanilla TS | `npx surrealql-analyzer generate --out src/surrealql-analyzer.generated.ts` | `./surrealql-analyzer.generated` |
| SvelteKit | `npx surrealql-analyzer generate --out src/lib/surrealql-analyzer.generated.ts` | `$lib/surrealql-analyzer.generated` |
| Next (`src/`) | `npx surrealql-analyzer generate --out src/surrealql-analyzer.generated.ts` | `@/surrealql-analyzer.generated` |
| Next (no `src/`) | `npx surrealql-analyzer generate --out surrealql-analyzer.generated.ts` | `@/surrealql-analyzer.generated` |

Put it in `package.json` so it is one command and one path forever:

```json
{ "scripts": { "generate": "surrealql-analyzer generate --out src/surrealql-analyzer.generated.ts" } }
```

Commit the generated module — it is what makes a fresh checkout type-check with
no build step. Re-run it whenever the schema or a query changes.

## The client

```ts
// src/db.ts
import { createClient } from "./surrealql-analyzer.generated";

export const db = createClient({
  url: "ws://localhost:8000/rpc",
  namespace: "app",
  database: "app",
});
```

Import `createClient` **from the generated file**. That import is what loads the
`declare module` augmentation; importing it from `@surrealdb/analyzer-client` directly
compiles fine and gives you `unknown[]` forever.

The connection opens **lazily**, on first use, so a module-level `db` is safe and
no route has to remember to `await db.connect(...)`. Import it anywhere — a
component, a route handler, a plain module. There is nothing to provide, inject
or wire up.

## `db.query` — the form you already know

`query` resolves the per-statement tuple, exactly as SurrealDB returns it. One
statement, one element:

```ts
const [people] = await db.query("SELECT id, name, age, team FROM person");
for (const person of people) console.log(person.name, person.age.toFixed(0));
```

Params are required exactly when the text reads them, and typed:

```ts
const [red] = await db.query("SELECT id, name FROM person WHERE team = $team", {
  team: new RecordId("team", "red"),
});

await db.query("SELECT id, name FROM person WHERE team = $team");
// TS2554: Expected 2 arguments, but got 1.

await db.query("SELECT id, name FROM person WHERE team = $team", { team: "team:red" });
// TS2322: Type 'string' is not assignable to type '_RecordId<"team">'.
```

Multi-statement text keeps its tuple; nothing is hidden:

```ts
const [names, ages] = await db.query("SELECT name FROM person; SELECT age FROM person");
```

A string that is **not** in the registry — built at runtime, or written since the
last `generate` — resolves to `unknown[]` with optional bindings. It still runs.
It is never `any`.

## Naming a query, when a name earns its keep

`defineQuery` infers the same string literal and reads the same registry through
the same conditional generic, so **it buys no extra type safety**. What it buys
is a value:

```ts
// src/queries.ts — the one place these texts live
import { defineQuery, defineLive } from "./surrealql-analyzer.generated";

export const allPeople    = defineQuery("SELECT id, name, age, team FROM person");
export const peopleOf     = defineQuery("SELECT id, name FROM person WHERE team = $team");
export const namesAndAges = defineQuery("SELECT name FROM person; SELECT age FROM person");
export const addPerson    = defineQuery("CREATE person SET name = $name, age = $age, team = $team");
export const livePeople   = defineLive("SELECT id, name, age, team FROM person");
export const liveTeam     = defineLive("SELECT id, name FROM person WHERE team = $team");
```

Reach for it when:

- **Two places must agree on one query** — an SSR `preload` in a `load` function
  and the component that subscribes to it. Write the text twice and they drift
  by a byte, the cache key stops matching, the seed is silently discarded and the
  page refetches: no error, no warning, no type failure. A value cannot drift.
- **You want the single-statement result unwrapped.** `db.run` returns rows
  directly, with no destructure.
- **Something needs to hold on to the query** — `db.watch`, `db.invalidate`, a
  mutation's `invalidates` list.

Otherwise `db.query` is the whole story and this file need not exist.

```ts
const people = await db.run(allPeople);           // rows, no destructure
const red    = await db.run(peopleOf, { team });  // params checked
const [names, ages] = await db.run(namesAndAges); // multi-statement keeps its tuple

const stop = db.watch(livePeople, (rows) => render(rows));
//    ^? rows: Array<{ age: number; id: RecordId<"person">; name: string; … }>

await db.run(addPerson, { name: "ada", age: 36, team });
await db.invalidate(allPeople);                 // every binding of that query
await db.invalidate(peopleOf.with({ team }));   // just that binding
```

A query text the registry does not contain is a **hard compile error** on
`defineQuery`, because for a literal it means exactly one thing — the generated
file is stale:

```
TS2345: Argument of type 'SurqlError<"this query is not in the generated
  registry - run `surrealql-analyzer generate`">' is not assignable to …
```

That also catches the case nobody recognises as an edit: reformatting a query
changes its text, so it changes its key. `defineQuery.unchecked("…")` opts out
and degrades to `unknown[]`.

## When everything is `any`

A type generator that silently produces `any` is worse than no type generator.
There are exactly four ways to get there, and all four are silent on *your*
code. If a result is `any`, it is one of these:

1. **`@surrealdb/analyzer-client` is not installed.** The generated file says
   `declare module "@surrealdb/analyzer-client"`. If that specifier does not resolve,
   TypeScript reports `TS2664: Invalid module name in augmentation` **inside the
   generated file** — which you would never open — and drops the entire
   registry. Every lookup then falls back. `npm ls @surrealdb/analyzer-client`.
2. **You imported from `@surrealdb/analyzer-client` instead of the generated file.**
   The augmentation loads with the import. Import `createClient`, `defineQuery`
   and `RecordId` from `./surrealql-analyzer.generated`.
3. **The generated file is somewhere else.** Bare `generate` writes to the
   workspace root. If your import points at `src/lib/` and the file is at the
   root, you now have two of them and they will drift. One `--out`, in
   `package.json`, forever.
4. **A `.svelte` script has no `lang="ts"`.** Svelte does not typecheck an
   untyped script block at all — `svelte-check` reports zero errors on
   `people[0].nope.definitely.not.a.field`. Every snippet in these docs says
   `lang="ts"` because copying the whole block is the point.

None of these produce an error on the line you wrote. All four are worth ruling
out before anything else.

## Values are the SDK's values

The SDK decodes SurrealQL values as **its own classes**, so that is what the
generated types say:

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
Construct params from the generated import — it re-exports the classes so this
needs no second package:

```ts
import { RecordId } from "./surrealql-analyzer.generated";
await db.query("SELECT id, name FROM person WHERE team = $team", {
  team: new RecordId("team", "red"),
});
```

### Crossing a serialisation boundary

Class instances do not survive SvelteKit's `load` (devalue) or a React Server
Component's props. `Json<T>` is the projection that does — it is the SDK's own
`Jsonify`, so the mapping is theirs:

```ts
const rows = await db.runJson(allPeople);
//    ^? Array<{ age: number; id: `person:${string}`; name: string; team: `team:${string}` }>
```

`db.runJson`, `preload`, and the whole reactive layer (`@surrealdb/analyzer-query`,
`@surrealdb/analyzer-svelte`, `@surrealdb/analyzer-next`) are `Json<T>`-shaped for this
reason. `db.query` and `db.run` are not.

## Errors

```ts
import { SurrealQLAnalyzerError } from "./surrealql-analyzer.generated";

try {
  await db.run(peopleOf, { team });
} catch (error) {
  if (error instanceof SurrealQLAnalyzerError) {
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
defineQuery.unchecked("SELECT " + table)     // a query no registry could hold
fromSurreal(existingSurreal)                 // wrap a connection you already own
```

If you use `fromSurreal`, construct the `Surreal` with
`codecOptions: { useNativeDates: true }` — that is the one thing `createClient`
does for you which cannot be recovered afterwards.

## How it works

`surrealql-analyzer generate` scans your source for query text — `db.query("…")`,
`defineQuery("…")`, `defineLive("…")` — analyses each against your schema, and
writes one file:

```ts
declare module "@surrealdb/analyzer-client" {
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

### Why compose the SDK instead of extending it

`class SurrealQLAnalyzerClient extends Surreal` does not compile. The SDK already owns
`run` (RPC function invocation), `subscribe` (the event emitter) and
`invalidate` — and `Surreal.invalidate()` *logs the session out*. tsc reports
TS2416 on each. So the SDK instance lives at `db.surreal`, the session methods
worth having (`use`/`signin`/`signup`/`authenticate`/`close`) are forwarded, and
the data vocabulary is ours.

## API

| Export | |
| --- | --- |
| `createClient(options)` | the client; connects lazily |
| `db.query(text, params?)` | the literal form; per-statement tuple |
| `db.run` / `db.runJson` / `db.runLiveOnce` | execute a named query; single-statement unwraps |
| `db.watch(live, onRows, onError?)` | subscribe; returns an unsubscribe |
| `db.invalidate(...queries)` | tell the reactive layer a write happened |
| `defineQuery(text)` / `defineLive(text)` | name a query; `.unchecked` opts out of the registry |
| `preload(db, query)` | server-fetched, serialisable, self-describing |
| `fromSurreal(surreal)` | wrap a `Surreal` you already own |
| `SurrealQLAnalyzerError` | `{ query, params, cause }` |
| `RecordId` / `Uuid` / `Duration` / `Decimal` | the SDK value classes |
| `Json<T>` / `Rows<R>` / `SurqlQuery` / `SurqlLive` / `Preloaded<T>` | types |

## Licence

MIT OR Apache-2.0

# TypeScript package API redesign — design proposal

Status: **proposal**, not built. Target: `@surrealguard/{client,query,svelte,next}` 0.5.0.
Scope: the *published surface*. Nothing here proposes changing the analyzer, and nothing
here weakens the literal-key type guarantee.

Everything in §3 was compiled against real `tsc` 5.9.3 and a real `surrealdb@2.0.8`
`.d.ts` before it was written down; §9 lists exactly what typechecked and what did not.

---

## 0. Verdict up front

The **foundation is right and should not be touched**: a generated `interface SurqlRegistry`
keyed by exact query text, read by one conditional generic, degrading to `unknown` (never
`any`). That mechanism is the product. It is also, measurably, the best-typed thing in this
space — Supabase can't type a `.select("a,b")` string against a schema this precisely, and
gql.tada needs a whole document-node encoding to get the same result.

What is wrong is everything *around* it:

1. **The unit of reuse is a string literal, but a string literal cannot be reused.** Every
   consumer (SSR `load`, the component, a future invalidation) has to retype the same query
   text byte-for-byte or silently miss the cache. The SvelteKit example already does this
   — the same `SELECT name, age, team FROM person` appears in `+page.ts` and `+page.svelte`.
   This is the single biggest structural problem, and it is fixable without giving up one
   bit of type safety.
2. **The generated value types do not match what the SDK actually hands back**, in both
   directions. This is a soundness bug, not an ergonomics complaint (§2.1, P5).
3. **The Svelte surface is not Svelte 5.** It uses runes internally, but its *inputs* are
   snapshots — a query whose params come from `$derived` state cannot re-run. There is also
   no one-shot query primitive at all.
4. **There is no mutation, no invalidation, and no vanilla-JS live subscription.** These are
   not undocumented; they do not exist.

The proposal is: keep the registry, add a **query reference** (`defineQuery` /
`defineLive`) as the unit that flows through the system, and rebuild the adapters on top of
it. `db.query("literal", params)` stays, unchanged, forever.

One thing worth stating before the criticism starts, because it reframes how much of this
matters: **every generic in the official SurrealDB SDK is an unverified cast.**
`surql<R extends unknown[] = unknown[]>` never infers `R` from its template — the tag only
does injection-safe binding. `db.live<T>(…)` uses `T` for the *builder's* field names and
nothing else; the delivered `LiveMessage.value` is `Record<string, unknown>`, so `T` never
reaches the payload. Typed live rows do not exist for SurrealDB outside this repo. The
adapters are not a nice-to-have wrapper around a typed SDK; they are the only place the types
are real, which is exactly why their surface being awkward is expensive.

---

## 1. What is actually good — keep these

Not everything needs redesigning. These are load-bearing and correct:

| Thing | Why it stays |
| --- | --- |
| `interface SurqlRegistry` keyed by exact text + one conditional generic | The whole guarantee. Proven by `packages/client/test-d/query.test-d.ts`, re-proven in §9. |
| **No permissive `string` overload**, ever | A literal is also a `string`; a fallback overload rescues every mis-call into `unknown`. This rule is correct and this proposal does not break it. |
| Degrade to `unknown`, never `any` | Asserted with `IsAny<T>` in three test-d files. Keep the assertions. |
| The per-statement response tuple | Truthful to the SDK. Multi-statement queries genuinely return one result per statement. The tuple stays; §3 only *unwraps* it at a new, differently-named call site. |
| `@surrealguard/query`'s refcounted live core | This is the hard part and it is done: N subscribers share one `LIVE SELECT`, one `KILL` at zero, reconcile by record `id` (CREATE→append, UPDATE→replace, DELETE→remove). Four vitest tests cover it. The proposal reuses it wholesale. |
| Live is a *separate explicit* call, not a `query()` that sniffs `LIVE` | One method returning two different things is worse. Keep the split. |
| Context-provided client with an explicit `{ client }` override | Standard, correct, matches every comparable library. |
| `useSyncExternalStore` in the React adapter | The correct primitive. Keep it. |
| Framework-agnostic core in its own package | Correct layering. `@surrealguard/query` should stay independently usable. |
| The generated file re-exporting `SurrealGuardClient` so one import loads the augmentation | Genuinely nice. Users never write a side-effect import. Keep it and extend it (§3.1). |

---

## 2. Honest assessment, per package

### 2.1 `@surrealguard/client`

**Smallest path from install to a typed result today.** Six concepts:

```ts
// 1. surrealguard.toml   2. surrealguard generate   3. the generated file
import { SurrealGuardClient } from "../surrealguard.generated";  // 4. import from *there*, not the package
const db = new SurrealGuardClient();
await db.connect("ws://localhost:8000/rpc");                     // 5. connect, manually
const [rows] = await db.query("SELECT name FROM person");        // 6. destructure statement 0
```

That is not bad. Steps 1–4 are inherent to codegen and are the same shape as
kysely-codegen or Supabase's `gen types`. Steps 5–6 are where it starts to grate.

**P1 — every single-statement query is destructured.** `const [rows] = await db.query(...)`
appears in every example, every README, every doc page. It is truthful (the SDK really does
return one result per statement) but ~97% of queries are one statement, so the truthful
shape taxes the common case to serve the rare one. Compare: Kysely `.execute()` → rows,
Drizzle `.findMany()` → rows, Supabase `{ data }` → rows.

**P2 — `db.live()` produces an inert object, and there is no vanilla way to subscribe.**

```ts
const users = db.live(`SELECT * FROM user`);   // => { sql: "LIVE SELECT * FROM user" }
// ...now what?
```

The only way to actually receive rows is:

```ts
import { getQueryClient } from "@surrealguard/query";        // a different package
getQueryClient(db).observeLive(users).subscribe((s) => { /* s.data */ });
```

Three concepts (`getQueryClient`, `observeLive`, `Observable`) in a package the vanilla user
was never told to install, none of it in the client's README. **This is the "the vanilla js
one even" complaint, and it is real: `@surrealguard/client` alone cannot do a live query.**

**P3 — `live()` overrides an SDK method with incompatible semantics.** `Surreal.live(table)`
returns `ManagedLivePromise<T>` — an awaitable subscription. `SurrealGuardClient.live(text)`
returns a plain descriptor object. A `SurrealGuardClient` is therefore not substitutable for
a `Surreal` for anyone who calls `.live()`. The class also inherits the SDK's `live` for
`Table` arguments through a third overload, so one method name now means two things.

**P4 — no connection lifecycle.** Nothing awaits readiness; every example calls
`await db.connect(...)` by hand, and the SvelteKit and Next examples **never call it at
all** (`examples/sveltekit/src/lib/db.ts` is `export const db = new SurrealGuardClient()`,
and no route connects it). Convex, Supabase and TanStack all take config at construction and
connect lazily. We should too.

**P5 — the generated value types do not match SDK runtime values.** This is the most
serious finding in this document and it is not an ergonomics issue.

The codegen conventions (`crates/codegen/src/lib.rs::ts_type`) say: record link →
`RecordId<T> = string & { __table?: T }`, datetime → `Date`, duration/uuid → `string`,
decimal → `number`. What `surrealdb@2.0.8` actually decodes (verified from
`dist/surrealdb.d.ts`):

| SurrealQL kind | We generate | SDK default decode | SDK `.json()` decode (`Jsonify<T>`) |
| --- | --- | --- | --- |
| `record<team>` | `string & brand` | `RecordId` **class instance** | `` `team:${string}` `` |
| `datetime` | `Date` | `DateTime` **class** (unless `codecOptions.useNativeDates`) | `string` |
| `duration` | `string` | `Duration` **class** | `string` |
| `uuid` | `string` | `Uuid` **class** | `string` |
| `decimal` | `number` | `Decimal` **class** | `string` |

Two concrete consequences:

- *Reading:* `row.id.startsWith("team:")` typechecks and throws at runtime — `row.id` is a
  `RecordId` object. `row.created.getTime()` typechecks and throws — it's a `DateTime`.
- *Writing:* the flagship example passes `"team:red" as RecordId<"team">` as a param. Because
  it is a plain JS string, the SDK encodes a **SurrealQL string**, and
  `WHERE team = $team` compares `record<team>` against `"team:red"` — never true. The
  headline demo of the whole project silently returns zero rows.

There is a third consequence that hits SSR: SvelteKit serialises `load` return values with
devalue, which rejects arbitrary class instances. `loadLive(db, db.live(\`SELECT ... FROM
person\`))` returns rows containing `RecordId` instances, so the SvelteKit example is very
likely a runtime failure the moment it meets a real database. (It has never been run against
one — `examples/sveltekit` currently does not even typecheck: its generated file predates the
per-statement tuple change, so `const [rows] = …` destructures a *row*, and
`pnpm --filter @surrealguard-example/sveltekit exec tsc --noEmit` fails with TS2339
`Property 'map' does not exist on type '{ name: string; }'`.)

**P6 — subclassing `Surreal` makes the SDK's namespace ours to dodge, forever.** I tried to
add `run`, `subscribe` and `invalidate` to a `class SurrealGuardClient extends Surreal` and
tsc rejected all three: `Surreal.run(name, args)` is RPC function invocation,
`Surreal.subscribe(event, listener)` is the event emitter, and `Surreal.invalidate()` **logs
the session out**. Real compiler output:

```
src/client.ts(42,3): error TS2416: Property 'run' in type 'SurrealGuardClient' is not
  assignable to the same property in base type 'Surreal'.
src/client.ts(54,3): error TS2416: Property 'subscribe' in type 'SurrealGuardClient' is not
  assignable to the same property in base type 'Surreal'.
```

Taken names include `create delete insert invalidate live liveOf merge patch query raw
relate reset run select set signin signup subscribe unset update upsert use`. Every good
name for a data API is already spoken for, and the SDK can take more at any minor version.

**P7 — no mutations, no invalidation.** `db.query("CREATE …")` works and is typed, but
nothing in `@surrealguard/query` can be told a write happened. Non-live cached queries stay
stale forever. `QueryClient` has no `invalidate`, no `refetch`, no `setData`.

**P8 — errors are raw SDK errors.** No query text, no params, no code. Debugging a failed
query means finding it yourself.

**Genuinely fine, leave alone:** the `ParamsArg` conditional-rest trick; the `unknown`
fallback; `BoundQuery` pass-through; the tuple result; the generated-file re-export.

### 2.2 `@surrealguard/query`

The core is the strongest package here. `observe`/`observeLive` refcounting, reconcile-by-id
and dehydrate/hydrate are all correct and tested. Gaps:

**P9 — `data` is always `Row[]`.** `QueryState<Row> { data: Row[] }` and
`Entry.state.data` default to `[]`. A query returning a scalar (`RETURN count(…)`), a single
record, or an object cannot be observed. `fetch<R extends Row>` further constrains R to
`Record<string, unknown> & { id?: unknown }`. Live queries genuinely are row streams; one-shot
queries are not, and the core conflates them.

**P10 — the cache never evicts.** `stop()` kills the subscription but keeps the entry
("Keep the last data cached for a fast re-subscribe"). There is no TTL, no `gcTime`, no
manual eviction, no size bound. A long-lived SPA with parameterised queries grows without
limit.

**P11 — no `invalidate` / `refetch` / `setData` / retry.** Once an entry errors it stays
errored until the last subscriber leaves and a new one arrives.

**P12 — the cache key is the full query text**, so `dehydrate()` ships every executed query's
source to the browser inside the SSR payload. Minor, but it is bytes and it is your schema
shape on the wire.

### 2.3 `@surrealguard/svelte`

Before-code, taken verbatim from `examples/sveltekit`:

```svelte
<!-- +layout.svelte -->
<script lang="ts">
  import { setClient } from "@surrealguard/svelte";
  import { db } from "$lib/db";
  setClient(db);
</script>
```
```ts
// +page.ts
import { loadLive } from "@surrealguard/svelte";
import { db } from "$lib/db";
export async function load() {
  const people = await loadLive(db, db.live(`SELECT name, age, team FROM person`));
  return { people };
}
```
```svelte
<!-- +page.svelte -->
<script lang="ts">
  import { liveQuery } from "@surrealguard/svelte";
  let { data } = $props();
  const people = liveQuery(
    (db) => db.live(`SELECT name, age, team FROM person`),
    { initial: data.people },
  );
</script>
{#each people.data as person (person.team)}<li>{person.name}</li>{/each}
```

Count the concepts: `setClient`, `loadLive`, `db.live`, the `(db) => …` callback, the
descriptor, `{ initial }`, and manual prop threading of `data.people`. Seven, for one list.

**P13 — the query text is written twice and must match byte-for-byte.** `+page.ts` and
`+page.svelte` both spell out `SELECT name, age, team FROM person`. Change one and the
`queryKey` no longer matches, so the SSR seed is silently discarded and the page refetches —
with no error, no warning, and no type failure. This is the structural flaw the redesign
exists to fix.

**P14 — params are not reactive.** `LiveQueryOptions.params` is read once, at construction.
There is no thunk/getter form. In Svelte 5 the normal shape of a parameterised query is

```svelte
const people = liveQuery((db) => db.live(`… WHERE team = $team`), {
  params: { team: page.params.team },   // ← snapshot; navigating to another team does nothing
});
```

This is a solved problem elsewhere and we are on the wrong side of it. `@tanstack/svelte-query`
cut a **whole major version (v6)** for exactly this migration: options went from
`StoreOrVal<T> = T | Readable<T>` to `Accessor<T> = () => T`, and the result went from a
`Readable` store to a plain reactive object. `convex-svelte` independently arrived at the
same `T | (() => T)`. Our `{ params }` object is precisely the pre-runes shape.
**This is the single biggest framework-fit failure in the package.**

**P15 — there is no one-shot query primitive.** `liveQuery` is the only reactive export, and
`db.live()` always prefixes `LIVE`. A component that wants "fetch this once, show a
spinner, show an error" has nothing to call — it must go through `load` or hand-roll
`$state` + `await`. This is the most common data-fetching need in any app and the package
does not serve it.

**P16 — `liveQuery` calls `$effect`, so it only works inside a component.** Sharing a query
from a `.svelte.ts` module (the idiomatic Svelte 5 way to share reactive state) throws
`effect_orphan` unless the caller wraps it in `$effect.root` and manages disposal.
Svelte 5.7+ ships `createSubscriber` from `svelte/reactivity` precisely for wrapping
external subscriptions in a way that works anywhere and tears down automatically —
confirmed present in the repo's svelte 5.56.7. We should be using it.

**P17 — no mutation helper**, so writes bypass the package entirely and nothing invalidates.

**Genuinely fine:** the returned object exposing `.data` / `.status` / `.loading` / `.error`
as getters is correct Svelte 5 (no `$` prefix, reads track). Keep that shape.

### 2.4 `@surrealguard/next`

```tsx
// app/providers.tsx
"use client";
<SurrealGuardProvider client={db}>{children}</SurrealGuardProvider>

// app/users/page.tsx  (Server Component)
const users = await queryServer(db, db.live(`SELECT * FROM user`));
return <UsersList initialData={users} />;

// users-list.tsx
"use client";
const { data } = useLiveQuery((db) => db.live(`SELECT * FROM user`), { initialData });
```

Same duplicated-literal problem as Svelte (P13), plus:

**P18 — a module-level client is shared across requests on the server.** `lib/db.ts` exports
`new SurrealGuardClient()`; Next imports that module into the server runtime, so one
connection — and one auth session, one `getQueryClient` cache — is shared by every concurrent
request and every user. Any `signin()` mutates global state. This needs a per-request server
client (React `cache()` scoping) and the docs need to say so loudly.

**P19 — the most common Next data pattern is unsupported.** In App Router, the default way
to read data is `const rows = await something()` *inside a Server Component*, shipping zero
client JS. Nothing in `@surrealguard/next` serves that; the only server export is
`queryServer`, which takes a `LiveDescriptor` and exists to seed a client hook.

**P20 — no Suspense/streaming story.** No `useSuspenseQuery` equivalent, no `loading.tsx`
integration, no `use()` of a server-passed promise (which in App Router is the idiomatic way
to stream a slow query into a client component).

**P21 — `queryServer` is misnamed**: it does not run a query, it primes a live descriptor.

**Genuinely fine:** `useSyncExternalStore` is the correct primitive; memoising on
`[client, descriptor.sql, paramsKey]` is correct; the `/server` export condition without
`"use client"` is correct.

---

## 3. The proposal

One new idea, applied consistently: **a query is a value, not a string.**

```ts
const activePeople = defineQuery("SELECT id, name, age FROM person WHERE active = true");
```

`defineQuery<Q extends string>(text: Q)` infers `Q` as the string literal exactly like
`db.query` does, looks `Q` up in `SurqlRegistry`, and returns a `SurqlQuery<Result, Params>`.
**The guarantee is unchanged** — same registry, same literal inference, same single
conditional generic, still no `string` overload. The literal is simply written *once*, in a
module, instead of once per consumer.

Everything else follows from that.

### 3.1 `@surrealguard/client`

```ts
// src/lib/queries.ts  — the one place query text lives
import { defineQuery, defineLive } from "./surrealguard.generated";

export const allPeople   = defineQuery("SELECT id, name, age FROM person");
export const peopleOf    = defineQuery("SELECT id, name FROM person WHERE team = $team");
export const addPerson   = defineQuery("CREATE person SET name = $name, joined = $joined");
export const livePeople  = defineLive("SELECT id, name, age FROM person");
export const liveTeam    = defineLive("SELECT id, name FROM person WHERE team = $team");
```

```ts
// src/lib/db.ts  — setup
import { createClient } from "./surrealguard.generated";

export const db = createClient({
  url: "ws://localhost:8000/rpc",
  namespace: "app",
  database: "app",
});                       // connects lazily; run/watch await readiness
```

```ts
// one-shot query — no destructure, params required exactly when the query reads them
const people = await db.run(allPeople);
//    ^? Array<{ id: RecordId<"person">; name: string; age: number }>
const red = await db.run(peopleOf, { team: teamId });

// multi-statement queries keep the per-statement tuple — nothing is hidden
const [names, ages] = await db.run(twoStatements);

// a scalar result is a scalar, not an array
const count = await db.run(peopleCount);   //  ^? number

// live query, vanilla JS, no other package required
const stop = db.watch(livePeople, (rows) => render(rows));
//    ^? rows: Array<{ id: RecordId<"person">; name: string; age: number }>

// mutation + invalidation
await db.run(addPerson, { name: "ada", joined: new Date() });
await db.invalidate(allPeople);            // all bindings of that query
await db.invalidate(peopleOf.with({ team: teamId }));   // just that binding

// errors carry context
try { await db.run(peopleOf, { team: teamId }); }
catch (e) {
  if (e instanceof SurrealGuardError) console.error(e.query, e.params, e.cause);
}

// escape hatches, all still here
db.surreal.signin({ ... });                          // the raw SDK instance
await db.query("SELECT name FROM person");           // the 0.4 literal form, unchanged
await db.surreal.query(surql`SELECT * FROM ${table}`);  // fully dynamic
```

Signatures (all compiled — §9):

```ts
export declare function defineQuery<Q extends string>(text: Q):
  SurqlQuery<ResultOf<Q>, ParamsOf<Q>>;
export declare function defineLive<Q extends string>(text: Q):
  SurqlLive<RowOf<Rows<ResultOf<Q>>>, ParamsOf<Q>>;

export interface SurqlQuery<R, P extends Record<string, unknown> = Bound> {
  readonly text: string;
  readonly params: Record<string, unknown> | undefined;
  readonly key: string;                    // stable: text + stably-serialised params
  with(params: P): SurqlQuery<R, Bound>;   // bind -> "no params remaining"
}

interface SurrealGuardClient {
  readonly surreal: Surreal;               // escape hatch
  ready(): Promise<void>;
  run    <R, P>(q: SurqlQuery<R, P>, ...args: ParamsArg<P>): Promise<Rows<R>>;
  runJson<R, P>(q: SurqlQuery<R, P>, ...args: ParamsArg<P>): Promise<Json<Rows<R>>>;
  watch  <Row>(q: SurqlLive<Row, Bound>, onRows: (rows: Row[]) => void,
               onError?: (e: SurrealGuardError) => void): () => void;
  invalidate(...queries: AnyQuery[]): Promise<void>;
}

type Rows<R>      = R extends readonly [infer Only] ? Only : R;  // 1 statement -> unwrap
type ParamsArg<P> = P extends Record<string, never> ? []
                  : Record<string, unknown> extends P ? [params?: P]
                  : [params: P];
```

Five deliberate decisions in there:

- **`Rows<R>` unwraps only single-element tuples.** `[Array<X>]` → `Array<X>`; `[A, B]` stays
  `[A, B]`; `unknown[]` stays `unknown[]`. The common case loses the destructure, the
  multi-statement case keeps its honesty. `db.query` is untouched and still returns the tuple.
  (Note the SDK's own rule for multi-statement: `.collect()` rejects if *any* statement failed,
  and per-statement success/failure is only visible through `.responses()`. `db.run` inherits
  the reject-on-any behaviour; `db.responses(q)` should be added later for the other case.)
- **`key` is branded with its result type**, borrowing TanStack v5's `DataTag`:
  `type QueryKey<T> = string & { readonly [dataTag]: T }`. That makes the imperative cache API
  type itself with no annotations — `client.getData(q.key)` returns `Rows<R> | undefined` for
  free. Compiled, §9 check 16.
- **A `"skip"` sentinel for conditional queries**, borrowed from Convex's
  `OptionalRestArgsOrSkip`: `createLive(() => enabled ? liveTeam.with({team}) : "skip")`
  typechecks and keeps the row type. Without it, "don't run this query yet" has no expression
  in a thunk-based API. Compiled, §9 check 15.
- **`ParamsArg` gained an "open record" branch** so a *non-registered* query takes optional
  params instead of required ones. The current `ParamsArg` gets this wrong — a dynamic query's
  params type is `Record<string, unknown>`, which is not `Record<string, never>`, so today it
  would be *required*. (Today it never surfaces because dynamic strings take the
  `[bindings?: …]` branch of `ArgsOf`; the query-ref form needs it in `ParamsArg` itself.)
- **`.with()` is the only way to make a query "bound"**, and adapters accept only bound
  queries. That gives one uniform rule ("a query handed to a component must have its params")
  enforced by the type system, and it makes the cache key computable at the call site.

**Composition instead of subclassing (the one genuinely contentious call).** §2.1 P6 shows
`extends Surreal` costs us `run`, `subscribe` and `invalidate` — and `invalidate` colliding
with "log out" is a hazard, not just an inconvenience. Two options:

- **A. Keep `class SurrealGuardClient extends Surreal`.** Free SDK compatibility, zero
  migration. Cost: rename the new methods to something the SDK hasn't claimed (`execute`,
  `watch`, `refresh`) and accept that any future SDK minor can collide with us.
- **B. `createClient()` returns an object with `.surreal`.** *(recommended.)* Our vocabulary
  is ours. Cost: `db.signin(...)` becomes `db.surreal.signin(...)` unless we re-export the
  handful of session methods (`connect/close/use/signin/signup/authenticate` — we should),
  and `SurrealGuardClient` is no longer structurally a `Surreal`.

Recommend **B**, with `fromSurreal(existing)` for people who already own a connection and
`class SurrealGuardClient extends Surreal` kept exported and deprecated through 0.5.x.

**Should a registry *miss* be an error?** `db.query` must tolerate a non-literal (dynamic
strings are legal), so it degrades to `unknown`. `defineQuery` is different: it *always*
receives a literal, so a miss means exactly one thing — **the generated file is stale**.
graphql-codegen's client preset handles the same situation with a fallback overload returning
`unknown` whose JSDoc reads *"The query argument is unknown! Please regenerate the types."* —
a soft fail visible only on hover. We can do better, using Supabase's `ParserError` trick
(`type Err<M extends string> = { __brand: true } & M`), which renders the message inside the
type error:

```ts
type Strict<Q extends string> = Q extends keyof SurqlRegistry
  ? SurqlQuery<ResultOf<Q>, ParamsOf<Q>>
  : SurqlError<"this query is not in the generated registry - run `surrealguard generate`">;
```
```
TS2345: Argument of type 'SurqlError<"this query is not in the generated registry -
  run `surrealguard generate`">' is not assignable to parameter of type 'SurqlQuery<…>'.
```

Compiled, §9 check 14. Recommendation: **hard-fail by default** for `defineQuery`/`defineLive`
(a stale registry is a bug, not a mode), with `defineQuery.unchecked("…")` for the rare
deliberate case. `db.query`'s soft `unknown` fallback stays exactly as it is.

**`SurrealGuardError` wraps, it does not replace.** The SDK ships a real hierarchy —
`SurrealError → ServerError → { QueryError, ValidationError, AuthenticationError,
NotFoundError, NotAllowedError, … }` plus a separate `SqonError` family for value parsing.
`SurrealGuardError` should carry `{ query, params, cause }` and keep `cause` as the original
typed SDK error, so `e.cause instanceof AuthenticationError` still works. Do not flatten it.

**Value types — pin one mode and make the generated types true.** Recommended:

- `createClient` sets `codecOptions.useNativeDates: true`, so codegen's `Date` is correct.
- Codegen emits the SDK's real classes for the rest: `RecordId<"team">`, `Uuid`, `Duration`,
  `Decimal` imported from `surrealdb`. Reading is then correct, and — more importantly —
  a `RecordId` param is *encoded as a record link*, so `WHERE team = $team` matches.
- Anything crossing a serialisation boundary goes through `Json<T>` (= the SDK's own
  `Jsonify<T>`): `runJson`, `preload`, and the whole reactive/SSR layer. `Json<RecordId<"p">>`
  is `` `p:${string}` ``, `Json<Date>` is `string`. Compiled, §9.
- Optional `[codegen] record_ids = "string"` restores the 0.4 branded-string shape for people
  who want plain JSON everywhere. Document that a string param cannot match a record link;
  do not make it the default.

### 3.2 `@surrealguard/query` (core)

Same engine, four additions and one generalisation:

```ts
class QueryClient {
  observe<R>(q: AnyQuery, opts?): Observable<QueryState<R>>;   // R no longer forced to Row[]
  invalidate(...queries: AnyQuery[]): Promise<void>;           // refetch live entries, drop the rest
  refetch(q: AnyQuery): Promise<void>;
  setData<R>(q: SurqlQuery<R, Bound>, updater: (prev: Rows<R>) => Rows<R>): void;  // optimistic
  getData<T>(key: QueryKey<T>): T | undefined;                 // types itself from the branded key
  mutate<R, P>(q: SurqlQuery<R, P>, params: P, opts?: { invalidates?: AnyQuery[] }): Promise<Rows<R>>;
  dehydrate(): DehydratedState;
  hydrate(state: DehydratedState): void;
  // new: bounded cache
  constructor(client, opts?: { gcTime?: number; maxEntries?: number });
}
```

`QueryState` becomes a real discriminated union so `status` narrows:

```ts
type QueryState<T> =
  | { status: "pending"; data: T | undefined; error: undefined }
  | { status: "success"; data: T;             error: undefined }
  | { status: "error";   data: T | undefined; error: SurrealGuardError };
```

`observe` keys off `query.key` (precomputed on the ref) rather than re-serialising params on
every call, which also makes "did the SSR seed match?" a value comparison rather than a
string-equality accident.

### 3.3 `@surrealguard/svelte`

```svelte
<!-- +layout.svelte : unchanged -->
<script lang="ts">
  import { setClient } from "@surrealguard/svelte";
  import { db } from "$lib/db";
  setClient(db);
  let { children } = $props();
</script>
{@render children()}
```

```ts
// +page.ts : SSR. `preload` returns a serialisable payload that REMEMBERS which query it is.
import { preload } from "@surrealguard/svelte";
import { db } from "$lib/db";
import { livePeople } from "$lib/queries";

export async function load() {
  return { people: await preload(db, livePeople) };
}
```

```svelte
<!-- +page.svelte : the query text appears nowhere. The key cannot drift. -->
<script lang="ts">
  import { createLive } from "@surrealguard/svelte";
  let { data } = $props();
  const people = createLive(data.people);
</script>

{#if people.error}
  <p class="error">{people.error.message}</p>
{:else}
  <ul>{#each people.data as p (p.id)}<li>{p.name} — {p.age}</li>{/each}</ul>
{/if}
```

Reactive params — the thunk form, which is the whole point:

```svelte
<script lang="ts">
  import { page } from "$app/state";
  import { createLive, createQuery, createMutation } from "@surrealguard/svelte";
  import { liveTeam, allPeople, addPerson } from "$lib/queries";

  // re-subscribes whenever page.params.team changes; old subscription is KILLed
  const team = createLive(() => liveTeam.with({ team: page.params.team }));

  // one-shot, with loading/error — the primitive that does not exist today
  const roster = createQuery(allPeople);

  const add = createMutation(addPerson, { invalidates: [allPeople, liveTeam] });
</script>

{#if roster.loading}<Spinner />{/if}
<button onclick={() => add.mutate({ name: "ada", joined: new Date() })} disabled={add.pending}>
  Add
</button>
```

Signatures:

```ts
// the thunk keeps it reactive; "skip" is the conditional-query sentinel (Convex)
type Source<Q> = Q | (() => Q | "skip") | "skip";

function createQuery<R>(source: Source<SurqlQuery<R, Bound>> | Preloaded<Json<Rows<R>>>,
                        options?: { initial?: Json<Rows<R>> }): QueryHandle<Json<Rows<R>>>;
function createLive <Row>(source: Source<SurqlLive<Row, Bound>> | Preloaded<Json<Row>[]>,
                        options?: { initial?: Json<Row>[] }):   LiveHandle<Json<Row>>;
function createMutation<R, P>(q: SurqlQuery<R, P>,
                        options?: { invalidates?: AnyQuery[]; onSuccess?(d: Rows<R>): void }
                        ): MutationHandle<R, P>;

interface QueryHandle<T> { readonly data: T | undefined; readonly error: SurrealGuardError | undefined;
                           readonly loading: boolean; readonly status: "pending"|"success"|"error";
                           refetch(): Promise<void>; }
interface LiveHandle<Row> { readonly data: Row[];   /* always an array: a live query is a row stream */
                            readonly error: SurrealGuardError | undefined;
                            readonly loading: boolean; readonly status: "pending"|"success"|"error"; }
interface MutationHandle<R, P> { mutate(...args: ParamsArg<P>): void;
                                 mutateAsync(...args: ParamsArg<P>): Promise<Rows<R>>;
                                 readonly data: Rows<R> | undefined;
                                 readonly error: SurrealGuardError | undefined;
                                 readonly pending: boolean; }
```

The `create*` (reactive primitive) / `use*` (context, imperative) naming split is TanStack
Svelte v6's convention and we should match it exactly: `createQuery`/`createLive`/
`createMutation` alongside `useClient`. It is free consistency for anyone arriving from
TanStack.

Implementation notes that matter for idiom:

- Use `createSubscriber` from `svelte/reactivity`, **not** `$effect`. It works outside a
  component (so a shared query can live in a `.svelte.ts` module), subscribes lazily on first
  read inside a tracking scope, and tears down automatically. This removes P16 outright.
- `LiveHandle.data` is `Row[]` and starts `[]` (no `?? []` in markup). `QueryHandle.data` is
  `T | undefined`, because a one-shot query's result may be a scalar and there is nothing
  honest to default it to.
- Ship `@surrealguard/svelte/transport` — a SvelteKit `transport` hook entry
  (`export const transport = { RecordId: {...}, DateTime: {...}, Duration: {...}, Uuid: {...},
  Decimal: {...} }`) so users who prefer SDK-class fidelity over `Json<>` can pass raw values
  through `load` without devalue rejecting them. The `Json<>` default means most people never
  need it; the people who do currently have no option at all.

### 3.4 `@surrealguard/next`

```tsx
// lib/db.server.ts — per-request server client, NOT module-global
import { cache } from "react";
import { createClient } from "@/surrealguard.generated";
export const getDb = cache(() => createClient({ url: process.env.SURREAL_URL!, ... }));
```

```tsx
// app/people/page.tsx — Server Component. Zero client JS for the static path.
import { getDb } from "@/lib/db.server";
import { allPeople, livePeople } from "@/lib/queries";
import { preload } from "@surrealguard/next/server";
import { PeopleList } from "./people-list";

export default async function Page() {
  const people = await getDb().run(allPeople);            // plain RSC read, fully typed
  const preloaded = await preload(getDb(), livePeople);   // seed for the live client component
  return (
    <>
      <StaticRoster rows={people} />
      <PeopleList preloaded={preloaded} />
    </>
  );
}
```

```tsx
// app/people/people-list.tsx
"use client";
import { useLive, useMutation } from "@surrealguard/next";
import { addPerson, allPeople, livePeople } from "@/lib/queries";
import type { Preloaded, Json, RowOf } from "@surrealguard/next";

export function PeopleList({ preloaded }: { preloaded: Preloaded<Json<Person>[]> }) {
  const people = useLive(preloaded);             // hydrates, then upgrades to live
  const add = useMutation(addPerson, { invalidates: [allPeople, livePeople] });
  if (people.error) return <Error error={people.error} />;
  return <>
    <ul>{people.data.map((p) => <li key={p.id}>{p.name}</li>)}</ul>
    <button onClick={() => add.mutate({ name: "ada", joined: new Date() })}>Add</button>
  </>;
}
```

Streaming, for a slow query, using the App Router idiom (`use()` a server-created promise):

```tsx
// page.tsx (server)
const promise = getDb().runJson(slowReport);          // not awaited
return <Suspense fallback={<Skeleton />}><Report data={promise} /></Suspense>;
// report.tsx (client)  ->  const rows = use(data);
```

`useQuery` / `useLive` take the ref (or a `Preloaded`) **directly** — no thunk. React does not
need one, because the ref already carries a stable `key` string, so the hook's `useMemo`
dependency is `[client, source.key]` and the "did my deps change" problem disappears. This is
the one place Svelte and React diverge, and they diverge because the frameworks do.

`@surrealguard/next/server` exports `preload`, `dehydrate`, `hydrate`, and nothing that needs
`"use client"`. `queryServer` stays as a deprecated alias for one minor.

---

## 4. Trade-offs — what this gives up

**Does it keep the literal-key type guarantee end to end? Yes, and it is compiled (§9).**
`defineQuery<Q extends string>(text: Q)` is the same inference site as `db.query<Q extends
string>(query: Q, …)`. There is still no permissive `string` overload anywhere on the path;
a non-literal (`let` variable, concatenation) degrades to `SurqlQuery<unknown[],
Record<string, unknown>>` — `unknown`, never `any`, asserted with `IsAny<T>`. Param checking
survives every hop: `defineQuery` → `.with()` → `db.run` → `createMutation.mutate`. Result
types survive every hop including `Preloaded` across the SSR boundary.

What we actually give up:

1. **The query text moves out of the call site.** `createLive(data.people)` does not show you
   what it queries; you follow the import. That is the cost of writing the literal once, and
   it is the same trade every mature client makes (tRPC procedures, Convex's `api.x.y`,
   TanStack's `queryOptions()`). Mitigation: the LSP already knows the mapping, and hover on a
   query ref can show the text and its inferred type — a small, high-value LSP feature.
2. **`defineQuery` is a new extraction sink.** `crates/embed/src/typescript.rs` today
   recognises `` surql`…` ``, `surql("…")` and any `.query(…)` / `.live(…)` member call.
   `defineQuery("…")` matches none of them. This needs either a ~3-line addition to
   `is_surql_tag`, or (better) a config key: `[codegen] sinks = ["db.query", "db.live",
   "defineQuery", "defineLive", "surql"]`. *Note the cheap fallback: naming the factory
   `surql(…)` would work today with zero Rust change — it is recognised already — at the cost
   of colliding with the SDK's own `surql` template-tag export.*
3. **`defineLive` is analysed without its `LIVE` prefix**, so `LIVE`-specific restrictions
   (no `ORDER BY`, no `LIMIT`, no `GROUP BY`) are not checked by the type layer. Fix in
   codegen, not in TS: `defineLive` sinks should be analysed *twice* — once bare (for the row
   type and the SSR seed key) and once `LIVE`-prefixed (for diagnostics only). Related: the
   `` `LIVE ${Q}` `` branch in `packages/client/src/live.ts::LiveRowOf` is **dead code today**
   — the extractor stores the argument text verbatim, so no generated file ever contains a
   `LIVE …` key (confirmed: `examples/sveltekit/surrealguard.generated.ts` has none). It
   should be deleted.
4. **Truthful value types are a breaking change to generated output** (§5). `row.id` stops
   being a string. This is the price of `WHERE team = $team` actually matching.
5. **`Json<T>` in the reactive layer means the reactive row type differs from the `db.run`
   row type.** `db.run(q)` gives `RecordId` instances; `createLive(q).data` gives
   `` `person:${string}` `` strings. That is a real inconsistency and the honest alternative
   (class instances everywhere + a SvelteKit `transport` + a Next serialiser) is more moving
   parts and breaks plain `JSON.stringify` for anyone rolling their own SSR. The proposal
   picks JSON-by-default in the reactive layer and ships the transport hook for the other
   camp. **This is the weakest point of the proposal; it deserves Drew's ruling.**
6. **Two ways to pass params** (`db.run(q, params)` inline vs `q.with(params)`). Both are
   checked against the same `P`, so nothing is unsound, but it is two ways.
7. **`AnyQuery` (`SurqlQuery<any, any>`) in `invalidates: AnyQuery[]`** is intentionally
   untyped — invalidation targets do not need to agree with the mutation's own types. That is
   the only `any` in the proposal and it is contained to a phantom position.
8. **Overloads vs unions.** I first wrote `createLive` as two overloads (ref | preloaded) and
   the rejection message for an unbound query was `TS2769: No overload matches this call`.
   Rewritten as a single union parameter, the same mistake reports
   `TS2345: Argument of type 'SurqlLive<{…}, { team: RecordId<"team"> }>' is not assignable to
   parameter of type 'Preloaded<…> | Source<SurqlLive<{…}, Bound>>'`. Use the union. Error
   quality is API design.

---

## 5. Migration path and breaking-change assessment

The packages are at 0.4.0 with, realistically, near-zero external users. That is an argument
for making the break now and making it clean — but the cost of *additive* here is genuinely
low, so most of it can be additive anyway.

### 5.1 Non-breaking (ships in 0.5.0, nothing to change)

| Addition | Notes |
| --- | --- |
| `defineQuery` / `defineLive` / `SurqlQuery` / `SurqlLive` | new exports |
| `createClient` / `fromSurreal` / `SurrealGuardError` | new exports |
| `db.run` / `db.runJson` / `db.watch` / `db.invalidate` | on the *new* client object |
| `createQuery` / `createMutation` / `preload` (svelte) | new exports |
| `useQuery` / `useLive` / `useMutation` / `preload` (next) | new exports |
| `QueryClient.invalidate` / `refetch` / `setData` / `mutate` | additive on the core |
| `[codegen] out` + `[codegen] sinks` in `surrealguard.toml` | new optional keys; existing configs keep working |

`db.query("literal", params)` and the whole `SurqlRegistry` mechanism are untouched.

### 5.2 Breaking (0.5.0)

| Break | Blast radius | Mitigation |
| --- | --- | --- |
| **Generated value types: `RecordId`/`Uuid`/`Duration`/`Decimal` become SDK classes** | Any code doing string ops on `row.id`, or passing a string param where a record is expected (which was already broken at runtime) | `[codegen] record_ids = "string"` restores the old shape; codemod is mechanical (`"team:red"` → `new RecordId("team","red")`) |
| `SurqlRegistry` `result` values now assumed to be the per-statement tuple everywhere | Already true since the multi-statement change; `examples/sveltekit/surrealguard.generated.ts` is stale and does not typecheck today | Regenerate |
| `QueryState` becomes a discriminated union (`data` is `T \| undefined` in `pending`) | Anyone reading `state.data` before checking `status` | Type error, not a runtime break |
| `QueryClient.observe` no longer forces `R extends Record<string, unknown>` / `data: Row[]` | Only affects direct core users | Widening; existing calls keep compiling |
| `liveQuery` (svelte) / `useLiveQuery` (next) / `loadLive` / `queryServer` / `db.live()` | The current public surface of both adapters | **Deprecate, do not delete, in 0.5.0**; delete in 0.6.0. Each is ≤15 lines re-expressed over the new core |
| `class SurrealGuardClient extends Surreal` stops being the recommended client | The class stays exported and working | `fromSurreal(new SurrealGuardClient())` bridges; deprecation warning in the docs, not at runtime |
| `LiveRowOf`'s `` `LIVE ${Q}` `` branch removed | None — dead code | — |

### 5.3 Mechanical migration

```ts
// 0.4                                            // 0.5
const users = liveQuery((db) =>                   const users = createLive(livePeople);
  db.live(`SELECT * FROM user`),                  // + export const livePeople =
  { initial: data.users });                       //     defineLive("SELECT * FROM user")
                                                  //   in lib/queries.ts

await loadLive(db, db.live(`SELECT * FROM user`)) await preload(db, livePeople)

const [rows] = await db.query("SELECT …")          const rows = await db.run(theQuery)
```

A codemod is feasible for the common shapes (`db.live(\`X\`)` → hoist to `defineLive("X")`),
but with this user count, a migration guide is enough.

---

## 6. Prior art

Signatures below were read out of installed `.d.ts` files (versions given), not from docs.

| Library | Its answer to "how do I name a query and reuse it?" | Reactivity / live | Mutations + invalidation | SSR | Verdict |
| --- | --- | --- | --- | --- | --- |
| **TanStack Query v5** (`@tanstack/react-query@5.101.4`) | `queryOptions({ queryKey, queryFn })` — a *value* passed to `useQuery`, `prefetchQuery`, `invalidateQueries`. The returned `queryKey` is intersected with `DataTag<Key, TData, TError>`, two `unique symbol` phantoms, so `getQueryData(opts.queryKey)` types itself | none in the push sense — poll/`refetchInterval`/`invalidate`. Live data means `setQueryData` by hand | `useMutation` + `invalidateQueries({ queryKey })`, **prefix**-matched | `prefetchQuery` (deliberately un-awaited, to stream) → `dehydrate()` → `<HydrationBoundary>` → `useSuspenseQuery` | **Follow**: `queryOptions` ≡ our query ref; prefix invalidation ≡ `invalidate(q)` vs `invalidate(q.with(p))`; **`DataTag` ≡ our branded `key`** (§3.1). **Reject** the authored `queryKey` array — its documented failure mode is exactly divergent keys (`['user',id]` vs `['users',id]`) producing silent duplicate cache entries. Ours is *derived* from the query, so it cannot diverge. Their errors-at-the-edge shape (fetcher throws, hook returns `{data,error}`) is what we do too. |
| **TanStack Svelte** (`@tanstack/svelte-query@6.1.38` — a **separate v6 major**) | `createQuery(() => ({ queryKey, queryFn }))` | — | — | — | **This settles the Svelte question.** v5 → v6 *is* the runes migration: options went from `StoreOrVal<T> = T \| Readable<T>` to `Accessor<T> = () => T`, and the result went from `Readable<QueryObserverResult>` (read `$query.data`) to a plain reactive object (read `query.data`). The docs state the rule flatly: *"the arguments to the `create*` functions must be wrapped in a function to preserve reactivity."* Our `LiveQueryOptions.params` object is the v5 shape we should never have shipped. Also **follow their naming split**: `create*` for reactive primitives, `use*` for context/imperative. |
| **Convex** (`convex@1.42.3`, `convex-svelte@0.14.0`) | Generated `api.messages.list` — a `FunctionReference<Type, Vis, Args, Return>` whose four type params are **entirely phantom**. `useQuery(api.messages.list, { channel })`; `OptionalRestArgsOrSkip<F>` makes args required-or-forbidden *and* accepts the string `"skip"` | always live; there is **no `invalidateQueries` in the whole API** because the server tracks each query's read set | `useMutation(...).withOptimisticUpdate(fn)`, where an async `fn` collapses the param type to the string literal `"Optimistic update handlers must be synchronous"` — the error message *is* the type | `preloadQuery()` in an RSC → `usePreloadedQuery(preloaded)` in a client component | **Follow, heavily**: `preloadQuery`/`usePreloadedQuery` is our `preload`/`createLive(preloaded)` (§3.3) — the cleanest answer anywhere to "server fetched it, client keeps it live, nobody retypes the key". `"skip"` is our conditional-query sentinel. Their `useQuery_experimental` discriminated union (`{status:'pending'} \| {status:'success',data} \| {status:'error',error}`) is our `QueryState`. **`convex-svelte` independently landed on the same `T \| (() => T)` thunk as TanStack v6** — that convention is settled, and we should adopt it rather than invent. **Reject** always-live (SurrealDB `LIVE SELECT` has real restrictions) and automatic invalidation (needs a reactive server). |
| **tRPC v11** (`@trpc/tanstack-react-query@11.18.0`) | `trpc.post.list.queryOptions(input)` — plus per-procedure `queryKey`, `queryFilter`, `mutationOptions`, `subscriptionOptions`, and per-path `pathKey`/`pathFilter`. Keys carry TanStack's `DataTag` with **both** output and error types | SSE via `httpSubscriptionLink` | delegated to TanStack | `createTRPCOptionsProxy({ router, ctx })` on the server calls in-process; `{ client }` goes over HTTP — **same call sites** | **Follow the structural lesson, which is the biggest one in this table:** v11's headline change was *deleting* its bespoke hook layer. It no longer wraps `useQuery`; it **emits options objects** the host library consumes. If we ever want a TanStack integration, the right shape is `surqlOptions(ref)` → pass to their `useQuery`, not a parallel hook universe. **Reject** the router — our "procedure name" is the query text. |
| **Drizzle** (`drizzle-orm@0.45.2`) | Schema *is* TypeScript; the builder accumulates config into a phantom `_` property and `$inferSelect` is a declared-but-nonexistent instance property | only in the expo-sqlite adapter | plain awaits | none | **Reject** the builder — reproducing SurrealQL's surface is a second language to learn and a second thing to keep correct; our differentiator is that you write real SurrealQL. **Heed the cost warning**: a cited benchmark puts Drizzle at ~41k type instantiations vs Prisma's ~428 on the same schema. Deep conditional-type machinery is not free, which is an argument for our *generated table* over a type-level parser. |
| **Kysely** (`kysely@0.29.4`) | `db.selectFrom('person').select(['person.name as n']).execute()` over a codegen'd `DB` interface. The select string is decomposed by a **cascade of template-literal `infer` patterns**, most-specific first, then re-keyed by extracted alias | none | plain awaits | none | **Follow** `.execute()` returning rows, not a wrapper. **Note for later**: `ColumnType<Select, Insert, Update>` modelling one column as three projections is directly applicable to SurrealDB's `VALUE`/`DEFAULT`/`READONLY` asymmetry, which our codegen currently ignores. Their `DrainOuterGeneric` wrapper exists purely to fight instantiation blowup — same warning as Drizzle. |
| **Prisma** (`prisma@7.9.0`) | `prisma.user.findMany({ include: { posts: true } })`; `SelectSubset<T, Args>` preserves the args literal so `GetResult<$Payload, T>` narrows the return | none | plain awaits, `$transaction` | none | **Reject** the client. **Take two lessons**: (1) errors with stable codes (`PrismaClientKnownRequestError.code === 'P2002'`) beat raw driver errors — hence `SurrealGuardError` keeping the SDK's typed `cause`; (2) Prisma 7 had to break every user to *un*-conflate type-source, migration-source and runtime config, which had all lived in `schema.prisma`. Our `surrealguard.toml` is drifting the same way — `[codegen]` should stay clearly separate from `[sources]` and `[analysis]`. Also relevant: `$queryRaw<User[]>` is a manual, unchecked generic — the same hole the SurrealDB SDK has. |
| **Supabase JS v2** (`@supabase/supabase-js@2.110.8`) | `createClient<Database>()` from generated types; `.from('person').select('id, name, posts(title)')` — the select string is parsed by a genuine **1,867-line type-level recursive-descent parser** (`ParseQuery` → `Ast.Node[]` → `ProcessNodes` against the schema) | `.channel().on('postgres_changes', …)` — a completely separate, untyped API | `.insert()/.update()`; no cache, no invalidation | `@supabase/ssr`: `createServerClient(url, key, { cookies: { getAll, setAll } })`, per-request | **Follow** the per-request server client (our P18) and **`ParserError<M> = { error: true } & M`**, which puts a human-readable message *into* the type error — we use it for the stale-registry case (§3.1). Their two-stage parse-then-resolve split is the right architecture *if* one ever writes a type-level SurrealQL parser; we don't have to, because Rust already parsed it. **Reject** `{ data, error }` on every call: their most-cited daily complaint is that `data` stays `T \| null` even after checking `error`, because the pair is not a discriminated union. If we ever return a pair, it must discriminate. Also note their type system's documented failure modes — "Type instantiation is excessively deep" on nested joins, wrong cardinality on relations — which a generated table simply cannot have. |
| **SurrealDB JS SDK 2.x** (`surrealdb@2.0.8`; there is **no 3.x line** — dist-tags are `latest 2.0.8`, `beta 2.0.0-beta.2`) | `db.query<[Person[]]>("…")`; `` surql`…` `` → `BoundQuery<R>`; `db.live<T>(new Table('users'))`; value classes `RecordId`(`.table`/`.id`, not `.tb`)/`DateTime`/`Duration`/`Uuid`/`Decimal`; `.json()` → `Jsonify<T>` | `live()`/`liveOf()`, subscription is both `AsyncIterable<LiveMessage>` **and** `.subscribe(handler) => unsubscribe` | none | none | **The central fact: every generic in this SDK is an unverified cast.** `surql<R extends unknown[] = unknown[]>` never infers `R` from the template — the tag does injection-safe binding only. `live<T>` uses `T` only for the *builder's* field names; the delivered `LiveMessage.value` is `Record<string, unknown>`, so **`T` never reaches the payload**. That is the hole we fill, and it is bigger than I assumed: our typed live rows are not a nicety, they are the only typed live rows that exist for SurrealDB. **Follow**: `Jsonify<T>` is the SDK's own serialisation answer, so `Json<T>` is *their* mapping (§3.1); keep `BoundQuery` pass-through; copy the dual iterable/`subscribe()` shape for `db.watch`. |
| **gql.tada 1.11.3 / graphql-codegen client-preset 6.1.0** | `graphql("query { … }")` — a **call form**, not a tagged template, because TS widens a tagged template's cooked text to `string` (TS#33304). codegen emits one overload **per document** plus a `string → unknown` fallback overload *declared first*; gql.tada instead runs a real tokenizer + recursive-descent parser in conditional types and uses its generated `interface setupCache` registry as an **optional cache** | n/a | n/a | n/a | **Follow, and note we already do**: `defineQuery("…")` is the same call form for the same reason. Their `documents` map keyed by document string is our `SurqlRegistry`. Two sharp findings: (1) **neither normalises the key** — reformatting a query changes it, whitespace and all; that constraint is inherent and we should stop worrying about it and instead make staleness *loud* (§3.1). (2) **gql.tada's registry is a cache, ours is a contract.** A miss costs them compile time; a miss costs us correctness. We cannot become a cache without a type-level SurrealQL parser (which Supabase shows is ~1,900 lines and Drizzle/Kysely show is expensive), so the right move is the opposite: make a miss a **hard type error** with a readable message. Their `TadaDocumentNode` riding on a real `DocumentNode` — brand the runtime value, don't invent a client contract — is also why `SurqlQuery` carries `text`/`params`/`key` as real runtime fields with the types as phantoms. |

**The through-line:** every library that got this right introduced a *value* that names a
query and carries its types (`queryOptions`, `api.x.y`, `trpc.x.y.queryOptions`, a
`TadaDocumentNode`). We are the only one still passing a bare string to every consumer. That
is the gap.

**Ideas adopted from this survey that were not in the first draft of §3:** `DataTag`-branded
cache keys (TanStack), the `"skip"` sentinel (Convex), `ParserError<M>` message-in-the-type
(Supabase), hard-fail on a registry miss (the gql.tada-vs-codegen contrast), keeping the SDK's
typed error as `cause` (Prisma), and the `create*`/`use*` naming split (TanStack Svelte v6).

---

## 7. Staged plan

Ordered by ergonomic gain per unit of churn. Stages 1–3 ship without breaking anyone.

**Stage 1 — the query ref (largest gain, additive).**
`defineQuery` / `defineLive` / `SurqlQuery` / `SurqlLive` / `.with()` / `db.run` in
`@surrealguard/client`, plus the `[codegen] sinks` config key and the `is_surql_tag`
addition in `crates/embed`. Nothing existing changes. This alone kills the duplicated-literal
problem and the `const [rows] =` papercut.
*Also do here:* delete the dead `` `LIVE ${Q}` `` branch; analyse `defineLive` sinks twice
(bare + `LIVE`-prefixed) so live-only restrictions get diagnosed.

**Stage 2 — the value-type fix (small diff, fixes a correctness bug).**
`codecOptions.useNativeDates: true`; codegen emits SDK `RecordId`/`Uuid`/`Duration`/`Decimal`;
`Json<T>` = `Jsonify<T>` exported. Breaking to generated output, but it turns a silently-wrong
demo into a working one. Do it before anyone depends on the old shape.

**Stage 3 — vanilla live + errors (closes the "vanilla JS" hole).**
`db.watch(live, cb)`, `SurrealGuardError` with query/params attached, `createClient` with lazy
connect. `@surrealguard/client` becomes usable on its own for the first time.

**Stage 4 — core additions.** `QueryClient.invalidate` / `refetch` / `setData` / `mutate`,
the discriminated `QueryState`, non-array results, bounded cache (`gcTime`, `maxEntries`).

**Stage 5 — Svelte rebuild.** `createQuery` / `createLive` / `createMutation` / `preload` on
`createSubscriber`, thunk `Source<Q>` form, `@surrealguard/svelte/transport`. Deprecate
`liveQuery` / `loadLive` (keep them working as thin shims).

**Stage 6 — Next rebuild.** `useQuery` / `useLive` / `useMutation` / `preload` /
per-request `cache()` server client, RSC-first docs, the `use()`-a-promise streaming
example. Deprecate `useLiveQuery` / `queryServer`. **Ship a `examples/next` app** — there
isn't one today, which is why the Next package's problems are the least understood.

**Stage 7 — polish and optional integrations.** LSP hover on a query ref showing text +
inferred type; a codemod for `db.live(\`X\`)` → `defineLive("X")`; `@surrealguard/query` cache
devtools; and — worth evaluating rather than assuming — a `surqlOptions(ref)` helper that
emits a TanStack `queryOptions` object, following tRPC v11's "emit options, don't wrap hooks"
move. That would let a TanStack user adopt typed one-shot queries without leaving their cache,
while our `createLive`/`useLive` keeps the half TanStack has no answer for.

Independent of all of this: `--watch` (in flight on this branch) is the other half of the
loop — a query ref you can't regenerate on save is still annoying.

---

## 8. Open questions for Drew

1. **Composition vs subclass** (§3.1 option A/B). Recommendation: composition. The
   `invalidate` collision — where the SDK's meaning is "log out" — is the strongest argument.
2. **`Json<T>` in the reactive layer, or SDK classes everywhere + transport hooks?** (§4.5).
   Recommendation: `Json<T>` by default, transport hook shipped for the other camp.
3. **Is `defineQuery` the right name?** `surql("…")` works with today's extractor and no Rust
   change, but collides with the SDK's `surql` tag export. `defineQuery`/`defineLive` cost ~3
   lines of Rust and read better.
4. **Should `defineLive` reject a projection without `id` at the type level?** It compiles
   (§9, check 11) and produces a self-documenting error, but a *diagnostic* from the analyzer
   ("a live query's projection must include `id`, or rows cannot be reconciled") is the
   contract-first answer. Probably both.
5. **Hard-fail or soft-fail on a registry miss in `defineQuery`?** Recommendation: hard-fail
   with a `ParserError`-style message (§3.1) and an `unchecked` escape hatch. This is the one
   place where we can be *stricter* than gql.tada and graphql-codegen, because unlike them our
   analysis happens in Rust and cannot silently fall back to a type-level parse.
6. **Do we want a `surqlOptions(ref)` adapter for TanStack Query** instead of (or alongside)
   our own `createQuery`? tRPC v11 deleted its own hooks in favour of exactly this. Ours has to
   exist regardless — live subscriptions have no TanStack equivalent — but the one-shot half
   could be delegated rather than duplicated.

---

## 9. What was compiled

All of it, against `typescript@5.9.3` and a real `surrealdb@2.0.8` in `/tmp/apidesign-ts`,
with `strict` + `noUncheckedIndexedAccess`. Positives had to typecheck; every negative is a
`@ts-expect-error` line, so tsc fails if it *stops* being an error. Negatives were then
re-run with the directives stripped, to confirm each produces a real, specific error rather
than passing for an unrelated reason.

**Green — the proposed surface typechecks:**

| # | Claim | Evidence |
| --- | --- | --- |
| 1 | `defineQuery("literal")` infers the literal and resolves `result` + `params` from `SurqlRegistry` | `Equal<typeof people, Array<{ id: RecordId<"person">; name: string; age: number }>>` |
| 2 | Single-statement `db.run` unwraps the tuple; multi-statement keeps it; a scalar stays a scalar | `Rows<[Array<X>]> = Array<X>`; `[A,B]` stays; `Rows<[number]> = number` |
| 3 | Params required exactly when the query reads them | `db.run(byTeam, { team })` ✅ |
| 4 | Non-registered query degrades to `unknown[]`, and `IsAny<…>` is `false` | asserted |
| 5 | `.with(params)` type-checks the bind and returns a query with **no** remaining params | `Parameters<typeof bound.with>[0]` is `Record<string, never>` |
| 6 | Adapters reject an **unbound** query at compile time | `createLive(liveByTeam)` → TS2345 |
| 7 | The reactive thunk form `createLive(() => ref.with({…}))` preserves the row type | asserted |
| 8 | `Preloaded<T>` carries the result type across the SSR boundary; `createLive(preloaded)` / `useLive(preloaded)` recover it with no second reference to the query text | asserted |
| 9 | `Json<T>` (= the SDK's `Jsonify<T>`) maps `RecordId<"person">` → `` `person:${string}` `` and `Date` → `string` | `Equal<Json<Row>, { id: \`person:${string}\`; name: string; joined: string }>` |
| 10 | Mutation params are checked (`mutate({ name, joined })`) | asserted |
| 11 | A live query whose projection omits `id` can be rejected at the type level with a named reason | `LiveOf<Row>` conditional compiles and rejects |
| 12 | An **untagged template literal** `defineQuery(\`SELECT …\`)` keeps its literal type (a `const` string variable does too; a `let` degrades to `unknown[]`) | asserted, all three |
| 13 | `db.query("literal", params)` is **unchanged** — same result tuple, same param enforcement | back-compat block compiles |
| 14 | A **stale-registry miss can hard-fail with a readable message** (`SurqlError<M> = { … } & M`) | `TS2345: Argument of type 'SurqlError<"this query is not in the generated registry - run \`surrealguard generate\`">' is not assignable to …` |
| 15 | The `"skip"` sentinel works in both direct and thunk position and preserves the row type | `createLive(() => enabled ? ref : "skip")` ✅ |
| 16 | A `DataTag`-branded key types imperative cache reads with zero annotation | `getData(q.key)` infers `Array<{ name: string }> \| undefined` |

**Negatives — each rejected, with the real error:**

```
TS2554  db.run(byTeam)                                   missing required params
TS2322  db.run(byTeam, { team: "team:red" })             string is not RecordId<"team">
TS2554  db.run(allPeople, { team })                      params on a param-free query
TS2339  (await db.run(allPeople))[0]!.nope               unknown result field
TS2322  byTeam.with({ team: 123 })                       wrong param type at bind time
TS2561  byTeam.with({ tea: teamId })                     unknown param name ("did you mean 'team'?")
TS2345  createLive(liveByTeam)                           unbound live query
TS2345  createLive(allPeople)                            one-shot query passed to a live adapter
TS2345  createQuery(byTeam)                              unbound query
TS2345  preload(db, liveByTeam)                          unbound live query
TS2554  create.mutate()                                  missing mutation params
TS2322  create.mutate({ name: 1, … })                    wrong mutation param type
TS2339  people.data[0]!.nope                             unknown row field
```

**Red — what did *not* compile, and what that told us:**

- `class SurrealGuardClient extends Surreal { run(...); subscribe(...) }` → **TS2416** on both.
  The SDK already owns those names (and `invalidate`). This is what turned §3.1's
  composition-vs-subclass question from a preference into an argument. *(Reproduced in
  `/tmp/apidesign-ts/collision/`.)*
- The first `ParamsArg` made params **required** for non-registered queries, because
  `Record<string, unknown>` is not `Record<string, never>`. Fixed with an explicit open-record
  branch (§3.1) — a real bug the proposal would have shipped.
- Two-overload `createLive` produced `TS2769: No overload matches this call`; the union form
  produces a message that names the actual mismatch. The proposal specifies the union.

**Not proven here (needs a live database, not a compiler):** that `RecordId` params actually
match `record<team>` on the wire; that SvelteKit's devalue rejects the current `loadLive`
payload; that reconcile-by-id behaves under real notification ordering. §2.1 P5 and §2.3 P13
are inferences from the SDK's own `.d.ts` and from devalue's documented behaviour, and each
should be confirmed against a running `surreal start` before Stage 2 lands.

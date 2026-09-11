/**
 * Type-level tests for the query-reference contract. Compiling this file IS the
 * test: every positive line must typecheck, and every `@ts-expect-error` line
 * must fail — tsc errors if an expected-error line ever starts compiling, so
 * these are guarantees, not documentation.
 *
 * Pure type checks; `main` is never called, so no connection opens.
 */

import {
  createClient,
  defineLive,
  defineQuery,
  fromSurreal,
  preload,
  RecordId,
  SurrealQLAnalyzerError,
  type Bound,
  type Json,
  type Preloaded,
  type QueryKey,
  type Rows,
  type SurqlQuery,
} from "../src/index.js";
import type { Equal, Expect, IsAny } from "./assert.js";

// Stand in for what `surrealql-analyzer generate` emits. Each `result` is the
// per-statement response tuple: one element per statement, `null` for a
// non-responder. A single-statement query is a one-element tuple.
//
// Augmented through the package name, exactly as the generated file does, and
// not through `../src/registry.js` where the interface is declared. TypeScript
// merges an augmentation into a *clone* of its target; when one interface is
// augmented through two different specifiers — the declaring module and a
// re-export of it — it ends up with two clones, and which one a given file
// sees depends on program order. With `test/` in the program that split made
// `defineQuery` in `gen/consumer.test-d.ts` report every golden query as "not
// in the registry" while indexing `SurqlRegistry` still found them. One
// specifier, one merged interface.
declare module "@surrealdb/analyzer-client" {
  interface SurqlRegistry {
    "SELECT id, name, age FROM person": {
      result: [Array<{ id: RecordId<"person">; name: string; age: number }>];
      params: Record<string, never>;
    };
    "SELECT id, name FROM person WHERE team = $team": {
      result: [Array<{ id: RecordId<"person">; name: string }>];
      params: { team: RecordId<"team"> };
    };
    "SELECT name FROM person; SELECT age FROM person": {
      result: [Array<{ name: string }>, Array<{ age: number }>];
      params: Record<string, never>;
    };
    "RETURN count(SELECT id FROM person)": {
      result: [number];
      params: Record<string, never>;
    };
    "CREATE person SET name = $name, joined = $joined": {
      result: [Array<{ id: RecordId<"person">; name: string; joined: Date }>];
      params: { name: string; joined: Date };
    };
  }
}

const allPeople = defineQuery("SELECT id, name, age FROM person");
const peopleOf = defineQuery("SELECT id, name FROM person WHERE team = $team");
const twoStatements = defineQuery("SELECT name FROM person; SELECT age FROM person");
const peopleCount = defineQuery("RETURN count(SELECT id FROM person)");
const addPerson = defineQuery("CREATE person SET name = $name, joined = $joined");
const livePeople = defineLive("SELECT id, name, age FROM person");
const liveTeam = defineLive("SELECT id, name FROM person WHERE team = $team");

const db = createClient({ url: "ws://localhost:8000/rpc" });

/** Stands in for `QueryClient.getData` — it reads its type from the key alone. */
declare function getData<T>(key: QueryKey<T>): T | undefined;
const team = new RecordId("team", "red");

// --- Rows<R>: the tuple unwraps only when there is exactly one statement -----
type _one = Expect<Equal<Rows<[Array<{ a: 1 }>]>, Array<{ a: 1 }>>>;
type _two = Expect<Equal<Rows<[number, string]>, [number, string]>>;
type _scalar = Expect<Equal<Rows<[number]>, number>>;
type _dynamic = Expect<Equal<Rows<unknown[]>, unknown[]>>;

async function main() {
  // 1. A literal resolves result + params from the registry, and `db.run`
  //    unwraps the single-statement tuple — no `const [rows] =`.
  const people = await db.run(allPeople);
  type _people = Expect<
    Equal<typeof people, Array<{ id: RecordId<"person">; name: string; age: number }>>
  >;
  people[0]!.name.toUpperCase();
  people[0]!.age.toFixed(0);

  // 2. Params are required exactly when the query reads them, and typed.
  const red = await db.run(peopleOf, { team });
  red[0]!.name.length;

  // 3. A multi-statement query keeps the per-statement tuple. Nothing is hidden.
  const [names, ages] = await db.run(twoStatements);
  names[0]!.name.toUpperCase();
  ages[0]!.age.toFixed(0);

  // 4. A scalar result stays a scalar — not wrapped in an array.
  const count = await db.run(peopleCount);
  type _count = Expect<Equal<typeof count, number>>;

  // 5. `.with()` binds params and reports "nothing remaining", which is what
  //    the adapters require.
  const bound = peopleOf.with({ team });
  type _bound = Expect<Equal<Parameters<typeof bound.with>[0], Bound>>;
  type _boundIsQuery = Expect<
    Equal<typeof bound, SurqlQuery<[Array<{ id: RecordId<"person">; name: string }>], Bound>>
  >;
  const boundRows = await db.run(bound);
  type _boundRows = Expect<
    Equal<typeof boundRows, Array<{ id: RecordId<"person">; name: string }>>
  >;

  // 6. The key is branded with its result type (TanStack's `DataTag` trick), so
  //    an imperative cache read types itself with no annotation.
  type _tagged = Expect<
    Equal<
      ReturnType<typeof getData<Array<{ id: RecordId<"person">; name: string; age: number }>>>,
      Array<{ id: RecordId<"person">; name: string; age: number }> | undefined
    >
  >;
  const cached = getData(allPeople.key);
  type _cached = Expect<
    Equal<typeof cached, Array<{ id: RecordId<"person">; name: string; age: number }> | undefined>
  >;
  // A key is still a plain string at runtime.
  const key: string = allPeople.key;
  void [key, cached];

  // 7. `Json<T>` is the SDK's own mapping: a RecordId becomes `table:${string}`,
  //    a Date becomes a string. This is what crosses an SSR boundary.
  type Row = { id: RecordId<"person">; name: string; joined: Date };
  type _json = Expect<Equal<Json<Row>, { id: `person:${string}`; name: string; joined: string }>>;
  const asJson = await db.runJson(allPeople);
  type _runJson = Expect<
    Equal<typeof asJson, Array<{ id: `person:${string}`; name: string; age: number }>>
  >;

  // 8. Live queries carry the row type, and `db.watch` needs no other package.
  const stop = db.watch(livePeople, (rows) => {
    type _liveRow = Expect<
      Equal<(typeof rows)[number], { id: RecordId<"person">; name: string; age: number }>
    >;
    rows[0]!.name.toUpperCase();
  });
  stop();

  // 9. `defineQuery.unchecked` degrades to `unknown[]` — never `any`.
  const dynamic = defineQuery.unchecked("SELECT " + "1");
  const dynamicRows = await db.run(dynamic);
  type _notAny = Expect<Equal<IsAny<typeof dynamicRows>, false>>;
  type _unknown = Expect<Equal<typeof dynamicRows, unknown[]>>;
  // …and its params are OPTIONAL, not required: we know nothing about them, so
  // demanding an object the caller cannot construct would be wrong.
  await db.run(dynamic);
  await db.run(dynamic, { anything: 1 });

  // 10. The literal `db.query` form is unchanged: still the per-statement tuple,
  //     still param-checked.
  const [tupleRows] = await db.query("SELECT id, name, age FROM person");
  tupleRows[0]!.age.toFixed(0);
  await db.query("SELECT id, name FROM person WHERE team = $team", { team });

  // 11. Errors carry the query and its params.
  try {
    await db.run(peopleOf, { team });
  } catch (error) {
    if (error instanceof SurrealQLAnalyzerError) {
      const query: string | undefined = error.query;
      const params: Record<string, unknown> | undefined = error.params;
      const cause: unknown = error.cause;
      void [query, params, cause];
    }
  }

  // 12. Invalidation targets are query refs — the whole query, or one binding.
  await db.invalidate(allPeople);
  await db.invalidate(peopleOf.with({ team }), livePeople);

  // 13. `preload` carries the result type across a serialisation boundary, as
  //     plain values, so the consumer needs no second reference to the text.
  const seededLive = await preload(db, livePeople);
  type _seededLive = Expect<
    Equal<
      typeof seededLive,
      Preloaded<Array<{ id: `person:${string}`; name: string; age: number }>>
    >
  >;
  const seededOne = await preload(db, allPeople);
  type _seededOne = Expect<
    Equal<
      typeof seededOne,
      Preloaded<Array<{ id: `person:${string}`; name: string; age: number }>>
    >
  >;

  // 14. A live query's underlying SELECT can be run once, un-subscribed.
  const seedRows = await db.runLiveOnce(livePeople);
  type _seedRows = Expect<
    Equal<typeof seedRows, Array<{ id: RecordId<"person">; name: string; age: number }>>
  >;

  // 15. `fromSurreal` wraps a connection you already own.
  const wrapped = fromSurreal(db.surreal);
  await wrapped.run(allPeople);

  // --- negatives: each must fail to compile --------------------------------

  // @ts-expect-error missing required params
  await db.run(peopleOf);
  // @ts-expect-error a plain string is not a RecordId — this is the bug that
  // silently returned zero rows, now caught by the compiler
  await db.run(peopleOf, { team: "team:red" });
  // @ts-expect-error params passed to a param-free query
  await db.run(allPeople, { team });
  // @ts-expect-error unknown result field
  (await db.run(allPeople))[0]!.nope;
  // @ts-expect-error wrong param type at bind time
  peopleOf.with({ team: 123 });
  // @ts-expect-error unknown param name
  peopleOf.with({ tea: team });
  // A registry miss is a HARD failure, not a silent degrade to `unknown`. The
  // message rides inside the type, so the compiler prints the remedy:
  //   TS2345: Argument of type 'SurqlError<"this query is not in the generated
  //   registry - run `surrealql-analyzer generate`">' is not assignable to …
  const stale = defineQuery("SELECT nope FROM nowhere");
  // @ts-expect-error the generated registry has no such query
  await db.run(stale);
  // @ts-expect-error a live adapter will not take a one-shot query
  db.watch(allPeople, () => {});
  // @ts-expect-error an unbound live query cannot be watched
  db.watch(liveTeam, () => {});
  // @ts-expect-error missing mutation params
  await db.run(addPerson);
  // @ts-expect-error wrong mutation param type
  await db.run(addPerson, { name: 1, joined: new Date() });
  // @ts-expect-error an unbound live query cannot be preloaded
  await preload(db, liveTeam);
  // @ts-expect-error a RecordId is not a string
  people[0]!.id.startsWith("person:");

  void [twoStatements, count, red, boundRows, asJson, stale, seededLive, seededOne, seedRows];
}
void main;

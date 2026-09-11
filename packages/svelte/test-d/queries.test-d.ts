/**
 * Type-level tests for the Svelte primitives. Compiling this file IS the test:
 * positive lines must typecheck and every `@ts-expect-error` line must fail.
 *
 * Pure type checks; nothing here runs, so no component mounts.
 */

import {
  defineLive,
  defineQuery,
  preload,
  RecordId,
  type Preloaded,
  type SurrealQLAnalyzerClient,
} from "@surrealdb/analyzer-client";
import { createLive, createMutation, createQuery } from "../src/index.js";

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
    "CREATE person SET name = $name, joined = $joined": {
      result: [Array<{ id: RecordId<"person">; name: string }>];
      params: { name: string; joined: Date };
    };
  }
}

type Equal<A, B> =
  (<T>() => T extends A ? 1 : 2) extends <T>() => T extends B ? 1 : 2 ? true : false;
type Expect<T extends true> = T;

const allPeople = defineQuery("SELECT id, name, age FROM person");
const peopleOf = defineQuery("SELECT id, name FROM person WHERE team = $team");
const addPerson = defineQuery("CREATE person SET name = $name, joined = $joined");
const livePeople = defineLive("SELECT id, name, age FROM person");
const liveTeam = defineLive("SELECT id, name FROM person WHERE team = $team");

declare const db: SurrealQLAnalyzerClient;
declare const team: RecordId<"team">;
declare const enabled: boolean;

async function main() {
  // 1. A live query's rows are the JSON projection — what actually survives
  //    devalue and a React Server Component boundary.
  const people = createLive(livePeople);
  type _rows = Expect<
    Equal<typeof people.data, Array<{ id: `person:${string}`; name: string; age: number }>>
  >;
  //    …and `data` is always an array, so markup needs no `?? []`.
  people.data.map((person) => person.name.toUpperCase());

  // 2. The thunk form preserves the row type and stays reactive.
  const forTeam = createLive(() => liveTeam.with({ team }));
  type _thunk = Expect<
    Equal<typeof forTeam.data, Array<{ id: `person:${string}`; name: string }>>
  >;

  // 3. `"skip"` works in both direct and thunk position, and keeps the row type.
  const maybe = createLive(() => (enabled ? liveTeam.with({ team }) : "skip"));
  type _skip = Expect<Equal<typeof maybe.data, Array<{ id: `person:${string}`; name: string }>>>;
  createLive("skip" as const);

  // 4. A one-shot query has loading and error state, and `data` is optional
  //    because the result may be a scalar.
  const roster = createQuery(allPeople);
  type _oneShot = Expect<
    Equal<
      typeof roster.data,
      Array<{ id: `person:${string}`; name: string; age: number }> | undefined
    >
  >;
  const loading: boolean = roster.loading;
  await roster.refetch();

  // 5. `Preloaded<T>` carries the result type across the SSR boundary, and the
  //    component recovers it with NO second reference to the query text.
  const preloaded = await preload(db, livePeople);
  type _preloaded = Expect<
    Equal<
      typeof preloaded,
      Preloaded<Array<{ id: `person:${string}`; name: string; age: number }>>
    >
  >;
  const hydrated = createLive(preloaded);
  type _hydrated = Expect<
    Equal<typeof hydrated.data, Array<{ id: `person:${string}`; name: string; age: number }>>
  >;

  // 6. Mutation params are checked.
  const add = createMutation(addPerson, { invalidates: [allPeople, livePeople] });
  add.mutate({ name: "ada", joined: new Date() });
  const created = await add.mutateAsync({ name: "ada", joined: new Date() });
  type _created = Expect<
    Equal<typeof created, Array<{ id: RecordId<"person">; name: string }>>
  >;
  const pending: boolean = add.pending;

  // --- negatives: each must fail to compile --------------------------------

  // @ts-expect-error an UNBOUND live query cannot be handed to an adapter
  createLive(liveTeam);
  // @ts-expect-error nor through a thunk
  createLive(() => liveTeam);
  // @ts-expect-error a one-shot query is not a live query
  createLive(allPeople);
  // @ts-expect-error an unbound query cannot be handed to createQuery
  createQuery(peopleOf);
  // @ts-expect-error an unbound live query cannot be preloaded
  await preload(db, liveTeam);
  // @ts-expect-error missing mutation params
  add.mutate();
  // @ts-expect-error wrong mutation param type
  add.mutate({ name: 1, joined: new Date() });
  // @ts-expect-error unknown row field
  people.data[0]!.nope;
  // @ts-expect-error a live row's id is a JSON string, not a RecordId
  people.data[0]!.id.table;

  void [loading, pending, roster, forTeam, maybe, hydrated];
}
void main;
